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
//!    inbox in when it may be typed into ([`SupervisorWaker::tick_hc`]);
//! 3. names the Herdr app's space of each project Corgi made one for, when a
//!    window is open and nobody named it ([`SpaceNamer`]).
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
    collections::{BTreeMap, HashSet},
    fs,
    io::{Read as _, Seek, Write as _},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::{
    HcClient, PID_LABEL, PROJECT_LABEL, RESUME_LABEL, SPACE_LABEL, Terminal,
    agents::{corgi_terminals, dashboard_agents, shell_in_front},
    events::{EventStream, Message, concerns_corgi},
    launch::project_space,
};
use crate::{
    harness::find_executable,
    model::AgentState,
    paths::{corgi_state_dir, dir_name, socket_state_file},
    supervisor,
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
/// How often, at most, the daemon asks the Herdr app's window for its spaces.
const SPACE_LOOK: Duration = Duration::from_secs(60);

/// What the daemon is started with.
pub struct Options {
    pub client: HcClient,
    /// The core's socket: where it waits for the core, and what names its
    /// records.
    pub socket: PathBuf,
    /// The Corgi binary the wakes' lines name and the handovers start.
    pub corgi_bin: PathBuf,
    /// Whether to name the Herdr app's spaces: only for the core the app's
    /// window shows, the user's default one.
    pub name_spaces: bool,
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
    let mut daemon = Daemon {
        core: HcCore::new(options.client.clone(), options.corgi_bin.clone()),
        options,
        waker,
        known: HashSet::new(),
        resumed: HashSet::new(),
        spaces: SpaceNamer::default(),
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
    spaces: SpaceNamer,
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

    /// One pass: resumes, wakes, handovers, deliveries, space names.
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
        if self.options.name_spaces {
            said.extend(self.spaces.name(&terminals));
        }
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

/// Names the Herdr app's space of each project Corgi made one for
/// (`corgi-<project>-<hash>`, which the app creates unnamed when it first
/// sees the label) after the project, once, through the open window: never
/// a space the user named, and never when no window is open, which it does
/// not open.
#[derive(Debug, Default)]
pub struct SpaceNamer {
    /// The spaces it named or found named, which it leaves alone after.
    done: HashSet<String>,
    last_look: Option<Instant>,
}

impl SpaceNamer {
    fn name(&mut self, terminals: &[Terminal]) -> Vec<String> {
        let wanted: BTreeMap<String, String> = project_spaces(terminals)
            .into_iter()
            .filter(|(space, _)| !self.done.contains(space))
            .collect();
        if wanted.is_empty() || self.last_look.is_some_and(|at| at.elapsed() < SPACE_LOOK) {
            return Vec::new();
        }
        self.last_look = Some(Instant::now());
        let Some(app) = herdr_app() else {
            return Vec::new();
        };
        if !app_window_open() {
            return Vec::new();
        }
        let listed = Command::new(&app).arg("ls").output();
        let spaces = match listed {
            Ok(output) if output.status.success() => app_spaces(&output.stdout),
            _ => return Vec::new(),
        };
        let mut said = Vec::new();
        for (id, project) in wanted {
            let Some(named) = spaces.get(&id) else {
                continue;
            };
            self.done.insert(id.clone());
            if *named {
                continue;
            }
            let renamed = Command::new(&app)
                .args(["do", "space.rename", "--id", &id, "--name", &project])
                .output();
            said.push(match renamed {
                Ok(output) if output.status.success() => {
                    format!("named the Herdr app's space {id} {project}")
                }
                _ => format!("could not name the Herdr app's space {id}"),
            });
        }
        said
    }
}

/// The spaces Corgi made for projects among `terminals`' labels, with the
/// project's name: those whose [`SPACE_LABEL`] is the one
/// [`project_space`] gives the terminal's project. A space a terminal took
/// from its caller or its supervisor is someone else's to name.
pub fn project_spaces(terminals: &[Terminal]) -> BTreeMap<String, String> {
    terminals
        .iter()
        .filter_map(|terminal| {
            let root = terminal.label(PROJECT_LABEL)?;
            let space = terminal.label(SPACE_LABEL)?;
            (space == project_space(root)).then(|| {
                (
                    space.to_string(),
                    dir_name(root)
                        .unwrap_or(supervisor::UNNAMED_PROJECT)
                        .to_string(),
                )
            })
        })
        .collect()
}

/// The spaces `herdr-app ls` printed, by id, with whether someone named
/// each: every JSON object in it with a string `id` and a boolean `named`,
/// as the app describes a space. Anything else gives none.
pub fn app_spaces(listed: &[u8]) -> BTreeMap<String, bool> {
    fn walk(value: &Value, found: &mut BTreeMap<String, bool>) {
        match value {
            Value::Object(object) => {
                if let (Some(Value::String(id)), Some(Value::Bool(named))) =
                    (object.get("id"), object.get("named"))
                {
                    found.insert(id.clone(), *named);
                }
                object.values().for_each(|value| walk(value, found));
            }
            Value::Array(values) => values.iter().for_each(|value| walk(value, found)),
            _ => {}
        }
    }
    let mut found = BTreeMap::new();
    if let Ok(value) = serde_json::from_slice::<Value>(listed) {
        walk(&value, &mut found);
    }
    found
}

/// The Herdr app's command-line tool: `HERDR_APP_BIN`, else `herdr-app`.
fn herdr_app() -> Option<PathBuf> {
    std::env::var_os("HERDR_APP_BIN")
        .filter(|bin| !bin.is_empty())
        .map(PathBuf::from)
        .or_else(|| find_executable("herdr-app"))
}

/// Whether a Herdr app window listens on its socket, found without asking
/// the app (`herdr-app` with no window open says so and does nothing, but a
/// look at the socket costs no process).
fn app_window_open() -> bool {
    let home = std::env::var_os("HERDR_APP_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::paths::home().map(|home| home.join(".config/herdr-app")));
    home.is_some_and(|home| UnixStream::connect(home.join("herdr-app.sock")).is_ok())
}

/// Whether the daemon for the core of this environment should name the
/// Herdr app's spaces: only for the user's default core, the one the app's
/// window shows. `session` is `HC_SESSION`, `socket` `HC_SOCKET`.
pub fn names_spaces(session: Option<&str>, socket: Option<&str>) -> bool {
    socket.is_none_or(str::is_empty)
        && session.is_none_or(|session| session.is_empty() || session == "default")
}

#[cfg(test)]
mod tests;
