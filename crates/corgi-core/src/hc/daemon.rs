//! `corgi daemon`: what the Herdr TUI's plugin hooks do for supervisors on
//! the TUI, for supervisors on herdr-core (`hc`), with the Herdr app's window
//! open or not (`docs/herdr-app-port.md`, Design A).
//!
//! One daemon runs per core, holding a lock for its whole life. It follows
//! the core's events from its socket and, on each event that concerns one of
//! Corgi's terminals, makes the same pass as at its start and after every
//! reconnect: "trigger, then reconcile". The event decides nothing; the pass
//! reads every terminal Corgi labelled afresh and takes what it saw last time
//! from disk ([`SupervisorWaker`]'s records), so a stop it missed while it or
//! the core was down is news at its next pass. A pass:
//!
//! 1. gives each kept terminal whose program the core replaced with a shell
//!    (a core stop, a reboot) its resume line, once ([`needs_resume`]);
//! 2. adds the wakes, moves the handovers on, and types each supervisor's
//!    inbox in when it may be typed into ([`SupervisorWaker::tick_hc`]).
//!
//! It never names or makes a space of the Herdr app: the app calls a space
//! nobody named after where its first terminal is. An earlier daemon made
//! the Corgi space when `corgi app-space` had found the app's window closed
//! and left a note; this one deletes such a note when it starts
//! ([`app_space::forget_pending`]) and never talks to the app's window.
//!
//! Passes also run on a timer: every few seconds while an inbox waits for a
//! supervisor that rests (a draft in its box gives no event when it is
//! cleared) or a handover runs, else every minute, which times a resting
//! supervisor's prompt cache for its handover.
//!
//! The daemon never starts a core: it waits for the core's socket to accept
//! a connection, follows the events on that socket, and asks nothing of `hc`
//! while the socket is gone, since any `hc` command starts a stopped core
//! with the caller's environment.

use std::{
    collections::HashSet,
    fs,
    io::{Read as _, Seek, Write as _},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

use super::{
    HcClient, PID_LABEL, RESUME_LABEL, Terminal,
    agents::{corgi_terminals, dashboard_agents, shell_in_front},
    events::{EventStream, Message, concerns_corgi},
};
use crate::{
    app_space,
    model::AgentState,
    paths::{corgi_state_dir, socket_state_file},
    time::{LocalTime, unix_now},
    waker::{SupervisorWaker, hc::HcCore},
};

/// How long the daemon waits after an event for more of the same burst
/// (a turn ends with a status, a turn and a title change) before its pass.
const SETTLE: Duration = Duration::from_millis(300);
/// The timed pass while something waits that no event will announce: an
/// inbox for a resting supervisor (its box may hold a draft), a handover.
const BUSY_PASS: Duration = Duration::from_secs(3);
/// The timed pass otherwise: it times a resting supervisor's prompt cache.
const IDLE_PASS: Duration = Duration::from_secs(60);
/// How often the daemon looks for the core's socket while it is gone.
const CORE_POLL: Duration = Duration::from_secs(2);

/// What the daemon is started with.
pub struct Options {
    pub client: HcClient,
    /// The core's socket: where it waits for the core, and what names its
    /// records.
    pub socket: PathBuf,
    /// The Corgi binary the wakes' lines name and the handovers start.
    pub corgi_bin: PathBuf,
}

/// The daemon's lock for the core at `socket`, held for its whole life:
/// `daemon/<socket>.lock` in Corgi's state directory, holding its pid.
pub fn lock_path(socket: &Path) -> Option<PathBuf> {
    socket_state_file("daemon", socket, "lock")
}

/// Where a daemon run by launchd or systemd logs, in Corgi's state
/// directory.
pub fn log_path() -> Option<PathBuf> {
    corgi_state_dir().map(|dir| dir.join("daemon").join("daemon.log"))
}

/// Takes the daemon's lock for the core at `socket` and writes this
/// process's id into it. Fails, naming the holder, when another daemon of
/// the same core holds it.
pub fn take_lock(socket: &Path) -> Result<fs::File> {
    let path = lock_path(socket).context("Corgi has no state directory for the daemon's lock")?;
    take_lock_at(&path, socket)
}

/// [`take_lock`] of the lock file at `path`.
fn take_lock_at(path: &Path, socket: &Path) -> Result<fs::File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    // Not truncated before it is ours: the pid in it is the holder's.
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    if file.try_lock().is_err() {
        let mut holder = String::new();
        let _ = file.read_to_string(&mut holder);
        bail!(
            "another corgi daemon runs for the core at {} (pid {}); it holds {}",
            socket.display(),
            holder.trim(),
            path.display()
        );
    }
    file.set_len(0)?;
    file.rewind()?;
    writeln!(file, "{}", std::process::id())?;
    file.flush()?;
    Ok(file)
}

/// Whether a daemon runs for the core at `socket`, and its pid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub running: bool,
    pub pid: Option<u32>,
    pub lock: Option<PathBuf>,
}

/// Whether a daemon holds the lock of the core at `socket`. Trying the lock
/// holds it for that moment only.
pub fn running(socket: &Path) -> Running {
    running_at(lock_path(socket))
}

/// [`running`] by the lock file `lock`.
fn running_at(lock: Option<PathBuf>) -> Running {
    let file = lock.as_deref().and_then(|path| fs::File::open(path).ok());
    let Some(mut file) = file else {
        return Running {
            running: false,
            pid: None,
            lock,
        };
    };
    let held = match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(_) => true,
    };
    let mut text = String::new();
    let _ = file.read_to_string(&mut text);
    Running {
        running: held,
        pid: held.then(|| text.trim().parse().ok()).flatten(),
        lock,
    }
}

/// One line of the daemon's log, on stderr, with the local date and time.
fn log(line: &str) {
    eprintln!(
        "{} corgi daemon: {line}",
        log_stamp(LocalTime::at(unix_now()))
    );
}

/// `at` as the log shows it: `YYYY-MM-DD HH:MM:SS`, on the local clock.
fn log_stamp(at: LocalTime) -> String {
    let local = at.secs as i64 + at.offset;
    let stamp = crate::time::utc_stamp(u64::try_from(local).unwrap_or_default());
    // `utc_stamp` writes `YYYYMMDD-HHMMSS`.
    match (
        stamp.get(0..4),
        stamp.get(4..6),
        stamp.get(6..8),
        stamp.get(9..11),
        stamp.get(11..13),
        stamp.get(13..15),
    ) {
        (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(s)) => {
            format!("{y}-{mo}-{d} {h}:{mi}:{s}")
        }
        _ => stamp,
    }
}

/// Whether the core at `socket` accepts connections now.
fn core_up(socket: &Path) -> bool {
    UnixStream::connect(socket).is_ok()
}

/// What the event reader hands the daemon's loop.
enum Feed {
    Message(Message),
    /// The stream ended: the core stopped or restarted.
    Ended(Option<String>),
}

/// Runs the daemon for good: waits for the core, follows it, makes its
/// passes, and starts over whenever the core goes. Returns only when the
/// lock is another daemon's.
pub fn run(options: Options) -> Result<()> {
    let _lock = take_lock(&options.socket)?;
    let mut waker = SupervisorWaker::for_hc(&options.socket);
    anyhow::ensure!(
        waker.lead_hc(),
        "another process holds the wake lock of the core at {}",
        options.socket.display()
    );
    log(&format!(
        "started (pid {}) for the core at {}, naming {}",
        std::process::id(),
        options.socket.display(),
        options.corgi_bin.display()
    ));
    if app_space::forget_pending() {
        log("forgot a pending Corgi space: only corgi app-space, run by hand, makes one now");
    }
    let mut daemon = Daemon {
        core: HcCore::new(options.client.clone(), options.corgi_bin.clone()),
        options,
        waker,
        known: HashSet::new(),
        resumed: HashSet::new(),
        last_said: Vec::new(),
        busy: false,
        last_pass: Instant::now(),
    };
    loop {
        daemon.wait_for_core();
        let (stream, _) = match EventStream::connect(&daemon.options.socket) {
            Ok(connected) => connected,
            Err(error) => {
                log(&format!("could not follow the core's events: {error:#}"));
                thread::sleep(CORE_POLL);
                continue;
            }
        };
        log("following the core's events");
        let feed = follow(stream);
        // Whatever happened while the daemon or the core was away.
        daemon.pass();
        loop {
            // Timed from the last pass: heartbeats and the events of other
            // terminals come more often than a timed pass is due.
            let every = if daemon.busy { BUSY_PASS } else { IDLE_PASS };
            let wait = every.saturating_sub(daemon.last_pass.elapsed());
            let ended = match feed.recv_timeout(wait) {
                Ok(Feed::Message(message)) if concerns_corgi(&message, &daemon.known) => {
                    let ended = settle(&feed);
                    daemon.pass();
                    ended
                }
                Ok(Feed::Message(_)) => None,
                Ok(Feed::Ended(error)) => Some(error),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    daemon.pass();
                    None
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => Some(None),
            };
            if let Some(error) = ended {
                match error {
                    Some(error) => log(&format!("the core's events stopped: {error}")),
                    None => log("the core's events ended (the core stopped or restarted)"),
                }
                break;
            }
        }
        thread::sleep(Duration::from_millis(500));
    }
}

/// Reads `stream` on a thread of its own, handing each message to the
/// returned channel and, last, how it ended.
fn follow(mut stream: EventStream) -> mpsc::Receiver<Feed> {
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        loop {
            match stream.next_message() {
                Ok(Some(message)) => {
                    if send.send(Feed::Message(message)).is_err() {
                        return;
                    }
                }
                Ok(None) => {
                    let _ = send.send(Feed::Ended(None));
                    return;
                }
                Err(error) => {
                    let _ = send.send(Feed::Ended(Some(format!("{error:#}"))));
                    return;
                }
            }
        }
    });
    receive
}

/// Lets a burst of events pass: waits until none came for [`SETTLE`], two
/// seconds at most. Returns how the stream ended, if it ended meanwhile.
fn settle(feed: &mpsc::Receiver<Feed>) -> Option<Option<String>> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match feed.recv_timeout(SETTLE) {
            Ok(Feed::Message(_)) => continue,
            Ok(Feed::Ended(error)) => return Some(error),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Some(None),
            Err(mpsc::RecvTimeoutError::Timeout) => return None,
        }
    }
    None
}

struct Daemon {
    options: Options,
    core: HcCore,
    waker: SupervisorWaker,
    /// The addresses of Corgi's terminals at the last pass, whose events
    /// call for a pass.
    known: HashSet<String>,
    /// The terminals given their resume line, with the program's pid then.
    resumed: HashSet<(String, u64)>,
    /// The last pass's lines, so a state that lasts is logged once.
    last_said: Vec<String>,
    /// Whether something waits that no event will announce.
    busy: bool,
    /// When the last pass began.
    last_pass: Instant,
}

impl Daemon {
    /// Waits until the core's socket accepts connections.
    fn wait_for_core(&self) {
        if core_up(&self.options.socket) {
            return;
        }
        log(&format!(
            "waiting for the core at {} (the daemon starts none)",
            self.options.socket.display()
        ));
        while !core_up(&self.options.socket) {
            thread::sleep(CORE_POLL);
        }
    }

    /// One pass: resumes, wakes, handovers, deliveries.
    fn pass(&mut self) {
        self.last_pass = Instant::now();
        // A core that went since the stream last spoke is not started by
        // asking hc about it.
        if !core_up(&self.options.socket) {
            return;
        }
        let terminals = match corgi_terminals(&self.options.client) {
            Ok(terminals) => terminals,
            Err(error) => {
                self.say(vec![format!("could not list Corgi's terminals: {error:#}")]);
                return;
            }
        };
        self.known = terminals.iter().map(|t| t.address.clone()).collect();
        let mut said = self.resume(&terminals);
        let agents = dashboard_agents(&terminals, None);
        said.extend(self.waker.tick_hc(
            &mut self.core,
            &agents,
            &self.options.corgi_bin,
            unix_now(),
        ));
        said.append(&mut self.core.notes);
        self.busy = self.waker.handing_over_any()
            || agents.iter().any(|agent| {
                agent.supervisor
                    && matches!(agent.info.state, AgentState::Done | AgentState::Idle)
                    && self.waker.undelivered_for(&agent.project_root) > 0
            });
        self.say(said);
    }

    /// Types the resume line into each kept terminal of `terminals` whose
    /// program the core replaced with a shell, once per program.
    fn resume(&mut self, terminals: &[Terminal]) -> Vec<String> {
        let mut said = Vec::new();
        for terminal in terminals {
            let Some(line) = needs_resume(terminal) else {
                continue;
            };
            let Some(pid) = terminal.pid else {
                continue;
            };
            let key = (terminal.address.clone(), pid);
            if self.resumed.contains(&key) {
                continue;
            }
            // A shell that has not drawn its prompt yet reads the line when
            // it gets to it, but one still starting may drop it.
            let drawn = self
                .options
                .client
                .read(&terminal.address, 0)
                .is_ok_and(|screen| !screen.trim().is_empty());
            if !drawn {
                continue;
            }
            self.resumed.insert(key);
            let name = terminal.display_name();
            match self.options.client.send(&terminal.address, line) {
                Ok(_) => {
                    let _ = self
                        .options
                        .client
                        .label(&terminal.address, &[(PID_LABEL, &pid.to_string())]);
                    said.push(format!(
                        "resumed {name} ({}) after its program was lost",
                        terminal.address
                    ));
                }
                Err(error) => said.push(format!(
                    "could not resume {name} ({}): {error:#}",
                    terminal.address
                )),
            }
        }
        said
    }

    /// Logs `said`, leaving out what the last pass said too.
    fn say(&mut self, said: Vec<String>) {
        for line in &said {
            if !self.last_said.contains(line) {
                log(line);
            }
        }
        self.last_said = said;
    }
}

/// The resume line to type into `terminal`, if the core replaced the
/// program Corgi started in it with a shell (it lost it: a core stop, a
/// reboot), which its [`PID_LABEL`] tells: a supervisor's shell, or a
/// worker's agent, of another pid. A supervisor whose agent exited into its
/// own shell keeps its pid, and waits for `corgi supervisor`.
pub fn needs_resume(terminal: &Terminal) -> Option<&str> {
    let resume = terminal.label(RESUME_LABEL)?;
    let marked = terminal.label(PID_LABEL)?;
    let pid = terminal.pid?;
    (terminal.is_live() && shell_in_front(terminal) && marked != pid.to_string()).then_some(resume)
}

#[cfg(test)]
mod tests;
