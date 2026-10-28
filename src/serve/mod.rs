//! `corgi serve`: the dashboard as data, for a frontend that draws its own
//! window. It speaks the protocol of [`corgi_core::protocol`] as JSON lines
//! over stdin and stdout, so a desktop GUI runs it locally or as
//! `ssh HOST corgi serve`; `docs/serve-protocol.md` describes it.
//!
//! It runs the dashboard's refresh pipeline, contending for the same
//! per-socket wake lock as every dashboard, so the GUI, the TUI and the
//! Omarchy widget run in any combination. One thread owns the state: it
//! reads the frontend's lines and the refresh's results from one channel,
//! answers the quick commands itself, and hands those that wait on Herdr,
//! Git or a harness to a thread of their own, which writes the reply. Only
//! protocol lines go to stdout; anything else goes to stderr.

mod commands;
mod state;
#[cfg(test)]
mod tests;

use std::{
    collections::HashMap,
    io::{self, BufRead, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use corgi_core::{
    catalog::ModelCatalogs,
    harness::Harness,
    herdr::HerdrClient,
    merge::MergeWorktreeForm,
    model::{DashboardAgent, WorkspaceInfo},
    paths::home,
    projects::ProjectMemory,
    protocol::{
        Hello, HerdrStatus, PROTOCOL_VERSION, PaletteSet, Reply, ServerMessage, Snapshot, State,
        Transcript, UsageCard,
    },
    refresh::{self, Inputs, RefreshLog, RefreshState, Refreshed},
    rows::codex_thread_id,
    time::unix_now,
    usage::{Provider, installed_providers, read_codex_thread_titles},
    usage_cache::{CachedUsage, shared_plan_usage},
    waker::{Inboxes, SupervisorWaker},
};

pub const SERVE_USAGE: &str = "\
Usage: corgi serve [--socket PATH | --session NAME]

Speaks Corgi's frontend protocol, JSON lines over stdin and stdout, for a
desktop GUI that runs it locally or as ssh HOST corgi serve: a hello with the
protocol version, a snapshot of what the dashboard shows, then a delta
whenever a refresh changes it, and a reply to every command for the
dashboard's actions. docs/serve-protocol.md describes every message. It runs
the dashboard's refresh, waking supervisors like a dashboard does, writes only
protocol lines to stdout and anything else to stderr, and exits when stdin
closes.

The Herdr socket is PATH, else the named session's, else $HERDR_SOCKET_PATH,
else the default session's (~/.config/herdr/herdr.sock).";

/// How often the shared plan-usage cache is checked; it fetches a new
/// reading only when the cached one is older than [`USAGE_REFRESH_INTERVAL`].
const USAGE_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const USAGE_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const CODEX_TITLE_INTERVAL: Duration = Duration::from_secs(3);
/// The longest the loop sleeps without looking at its timers.
const TICK: Duration = Duration::from_millis(250);

/// `corgi serve`.
pub fn serve_command(args: &[String]) -> Result<()> {
    let socket = socket_path(args)?;
    let client = HerdrClient::from_socket_path(&socket);
    let options = Options::live(client);
    let (events, receiver) = mpsc::channel();
    read_lines(io::stdin(), events.clone());
    let output = Output::new(Box::new(io::stdout()));
    Server::new(options, output, events, receiver).run();
    Ok(())
}

/// The Herdr socket `args` name, else the one Herdr gave this pane, else the
/// default session's.
fn socket_path(args: &[String]) -> Result<PathBuf> {
    let mut args = args.iter();
    let socket = match (args.next().map(String::as_str), args.next()) {
        (Some("--socket"), Some(path)) => PathBuf::from(path),
        (Some("--session"), Some(name)) => {
            herdr_dir()?.join("sessions").join(name).join("herdr.sock")
        }
        (Some(other), _) => bail!("unknown option {other:?}\n\n{SERVE_USAGE}"),
        (None, _) => match std::env::var_os("HERDR_SOCKET_PATH").filter(|path| !path.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => herdr_dir()?.join("herdr.sock"),
        },
    };
    if let Some(extra) = args.next() {
        bail!("unexpected argument {extra:?}\n\n{SERVE_USAGE}");
    }
    Ok(socket)
}

fn herdr_dir() -> Result<PathBuf> {
    Ok(home().context("HOME is not set")?.join(".config/herdr"))
}

/// Everything a server takes from its surroundings, so a test can keep it
/// off the developer's own state, network and harnesses.
pub(crate) struct Options {
    pub(crate) client: HerdrClient,
    /// Contends for the wake lock and wakes supervisors when it holds it.
    pub(crate) waker: Option<SupervisorWaker>,
    /// Where merges, conflicts and answers go, and where each project's
    /// supervisor keeps its memory.
    pub(crate) inboxes: Inboxes,
    pub(crate) providers: Vec<Provider>,
    /// Asks Codex for its threads' titles, as the dashboard does.
    pub(crate) codex_titles: bool,
    /// The project list the new-agent form offers; `None` reads Corgi's
    /// own each time, and remembers the projects agents start in.
    pub(crate) projects: Option<ProjectMemory>,
    /// The harnesses installed, for the form's harness list.
    pub(crate) installed: Vec<Harness>,
    /// Asks a harness for its models, rather than offering the fallback.
    pub(crate) discover_models: bool,
    /// Where the selection goes for `corgi edit`.
    pub(crate) selection_file: Option<PathBuf>,
    pub(crate) palette: fn() -> PaletteSet,
    pub(crate) log: Option<RefreshLog>,
    pub(crate) refresh_interval: Duration,
}

impl Options {
    /// A server on the user's own Corgi, as `corgi serve` runs.
    fn live(client: HerdrClient) -> Self {
        let waker = SupervisorWaker::default();
        let selection_file = crate::app::selection_file(client.socket_path(), std::process::id());
        Self {
            inboxes: waker.inboxes(),
            waker: Some(waker),
            providers: installed_providers(),
            codex_titles: true,
            projects: None,
            installed: Harness::installed(),
            discover_models: true,
            selection_file,
            palette: state::palette_set,
            log: RefreshLog::from_env(),
            refresh_interval: refresh::REFRESH_INTERVAL,
            client,
        }
    }
}

/// Where protocol lines go: stdout, shared by the loop and the threads
/// that answer slow commands, one whole line at a time.
#[derive(Clone)]
pub(crate) struct Output(Arc<Mutex<Box<dyn Write + Send>>>);

impl Output {
    pub(crate) fn new(writer: Box<dyn Write + Send>) -> Self {
        Self(Arc::new(Mutex::new(writer)))
    }

    /// Writes `message` as one line. Fails when the frontend has gone.
    pub(crate) fn send(&self, message: &ServerMessage) -> io::Result<()> {
        let mut line = serde_json::to_vec(message).map_err(io::Error::other)?;
        line.push(b'\n');
        let mut writer = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        writer.write_all(&line)?;
        writer.flush()
    }

    pub(crate) fn reply(&self, reply: Reply) {
        if let Err(error) = self.send(&ServerMessage::Reply(reply)) {
            eprintln!("corgi serve: cannot write a reply: {error}");
        }
    }
}

/// What wakes the loop.
pub(crate) enum Event {
    Line(String),
    InputClosed,
    Refreshed(Box<Refreshed>),
    /// The refresh panicked and took its state with it.
    RefreshPanicked,
    Usage(Vec<(Provider, Option<CachedUsage>)>),
    CodexTitles(HashMap<String, String>),
    /// A command changed Herdr or Git: refresh now.
    Changed,
}

/// Sends each line of `input` as an [`Event::Line`], then
/// [`Event::InputClosed`].
pub(crate) fn read_lines(input: impl io::Read + Send + 'static, events: Sender<Event>) {
    thread::spawn(move || {
        for line in io::BufReader::new(input).lines() {
            let Ok(line) = line else { break };
            if events.send(Event::Line(line)).is_err() {
                return;
            }
        }
        let _ = events.send(Event::InputClosed);
    });
}

/// What the commands share with the threads that run them.
#[derive(Clone)]
pub(crate) struct Shared {
    pub(crate) client: HerdrClient,
    pub(crate) inboxes: Inboxes,
    pub(crate) output: Output,
    pub(crate) events: Sender<Event>,
    /// The merges confirmed or under way, by agent pane.
    pub(crate) merges: Arc<Mutex<HashMap<String, MergeWorktreeForm>>>,
    pub(crate) catalogs: Arc<Mutex<ModelCatalogs>>,
    /// Commands still running on threads of their own.
    pub(crate) running: Arc<AtomicUsize>,
}

pub(crate) struct Server {
    options: Options,
    shared: Shared,
    events: Receiver<Event>,
    /// The refresh's state, `None` while a refresh has it.
    refresh_state: Option<RefreshState>,
    /// The refresh wakes supervisors when it holds the wake lock.
    wakes: bool,
    last_refresh: Option<Instant>,
    refresh_requested: bool,
    agents: Vec<DashboardAgent>,
    workspaces: Vec<WorkspaceInfo>,
    herdr: HerdrStatus,
    status: Option<(String, Instant)>,
    projects: Vec<corgi_core::protocol::Project>,
    usage: Vec<UsageCard>,
    last_usage_check: Option<Instant>,
    usage_running: bool,
    codex_titles: HashMap<String, String>,
    last_codex_check: Option<Instant>,
    codex_running: bool,
    watch: Option<String>,
    transcript: Option<Transcript>,
    selection: Option<String>,
    palette: PaletteSet,
    /// The state the frontend has, and its number.
    sent: Option<State>,
    seq: u64,
    input_closed: bool,
    output_gone: bool,
}

impl Server {
    pub(crate) fn new(
        mut options: Options,
        output: Output,
        events_sender: Sender<Event>,
        events: Receiver<Event>,
    ) -> Self {
        let wakes = options.waker.is_some();
        let refresh_state = RefreshState::new(options.waker.take());
        let usage = options
            .providers
            .iter()
            .map(|provider| UsageCard {
                provider: *provider,
                title: provider.card_title().to_string(),
                usage: None,
                fetched_at: None,
                error: None,
            })
            .collect();
        let palette = (options.palette)();
        Self {
            shared: Shared {
                client: options.client.clone(),
                inboxes: options.inboxes.clone(),
                output,
                events: events_sender,
                merges: Arc::default(),
                catalogs: Arc::default(),
                running: Arc::default(),
            },
            options,
            events,
            refresh_state: Some(refresh_state),
            wakes,
            last_refresh: None,
            refresh_requested: false,
            agents: Vec::new(),
            workspaces: Vec::new(),
            herdr: HerdrStatus::default(),
            status: None,
            projects: Vec::new(),
            usage,
            last_usage_check: None,
            usage_running: false,
            codex_titles: HashMap::new(),
            last_codex_check: None,
            codex_running: false,
            watch: None,
            transcript: None,
            selection: None,
            palette,
            sent: None,
            seq: 0,
            input_closed: false,
            output_gone: false,
        }
    }

    /// Says hello, sends the first snapshot from a refresh made in place,
    /// then serves until stdin closes and no command is left running, or
    /// stdout is gone.
    pub(crate) fn run(mut self) {
        let hello = ServerMessage::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            corgi: env!("CARGO_PKG_VERSION").to_string(),
            socket: self.options.client.socket_path().display().to_string(),
            pid: std::process::id(),
            commands: corgi_core::protocol::Command::catalog(),
        });
        self.send(&hello);
        let state = self.refresh_state.take().unwrap_or_default();
        let refreshed = refresh::run(&self.refresh_inputs(), state, self.options.log.as_ref());
        self.apply_refresh(refreshed);
        self.publish();
        while !self.output_gone {
            if self.input_closed && self.shared.running.load(Ordering::SeqCst) == 0 {
                break;
            }
            self.start_due_work();
            match self.events.recv_timeout(TICK) {
                Ok(event) => self.handle(event),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            // Take whatever else has arrived before publishing once.
            while let Ok(event) = self.events.try_recv() {
                self.handle(event);
            }
            self.publish();
        }
        if let Some(path) = &self.options.selection_file {
            let _ = std::fs::remove_file(path);
        }
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Line(line) => self.command(&line),
            Event::InputClosed => self.input_closed = true,
            Event::Refreshed(refreshed) => self.apply_refresh(*refreshed),
            Event::RefreshPanicked => {
                eprintln!("corgi serve: a refresh panicked; starting over");
                self.last_refresh = Some(Instant::now());
                let waker = self
                    .wakes
                    .then(|| SupervisorWaker::with_inboxes(self.shared.inboxes.clone()));
                self.refresh_state = Some(RefreshState::new(waker));
            }
            Event::Usage(readings) => {
                self.usage_running = false;
                for (provider, cached) in readings {
                    let Some(card) = self.usage.iter_mut().find(|card| card.provider == provider)
                    else {
                        continue;
                    };
                    // Nothing yet: another process fetches the first reading.
                    let Some(cached) = cached else { continue };
                    if let Some(usage) = cached.usage {
                        card.usage = Some(usage);
                        card.fetched_at = cached.fetched_at;
                    }
                    card.error = cached.error;
                }
            }
            Event::CodexTitles(titles) => {
                self.codex_running = false;
                if !titles.is_empty() {
                    self.codex_titles.extend(titles);
                    self.refresh_requested = true;
                }
            }
            Event::Changed => self.refresh_requested = true,
        }
    }

    /// Starts the refresh, the usage check and the Codex titles when each
    /// is due, each on a thread of its own.
    fn start_due_work(&mut self) {
        let due = self.refresh_requested
            || self
                .last_refresh
                .is_none_or(|at| at.elapsed() >= self.options.refresh_interval);
        if due && let Some(state) = self.refresh_state.take() {
            self.refresh_requested = false;
            let inputs = self.refresh_inputs();
            let log = self.options.log.clone();
            let events = self.shared.events.clone();
            thread::spawn(move || {
                let refreshed = catch_unwind(AssertUnwindSafe(|| {
                    refresh::run(&inputs, state, log.as_ref())
                }));
                let _ = events.send(match refreshed {
                    Ok(refreshed) => Event::Refreshed(Box::new(refreshed)),
                    Err(_) => Event::RefreshPanicked,
                });
            });
        }
        if !self.usage_running
            && self
                .last_usage_check
                .is_none_or(|at| at.elapsed() >= USAGE_CHECK_INTERVAL)
        {
            self.last_usage_check = Some(Instant::now());
            // The Omarchy theme can change while the GUI is open.
            self.palette = (self.options.palette)();
            if !self.options.providers.is_empty() {
                self.usage_running = true;
                let providers = self.options.providers.clone();
                let events = self.shared.events.clone();
                thread::spawn(move || {
                    let readings = providers
                        .into_iter()
                        .map(|provider| {
                            (
                                provider,
                                shared_plan_usage(provider, USAGE_REFRESH_INTERVAL),
                            )
                        })
                        .collect();
                    let _ = events.send(Event::Usage(readings));
                });
            }
        }
        if self.options.codex_titles
            && !self.codex_running
            && self
                .last_codex_check
                .is_none_or(|at| at.elapsed() >= CODEX_TITLE_INTERVAL)
        {
            let threads: Vec<String> = self
                .agents
                .iter()
                .filter_map(|agent| codex_thread_id(&agent.info))
                .map(str::to_string)
                .collect();
            if !threads.is_empty() {
                self.last_codex_check = Some(Instant::now());
                self.codex_running = true;
                let events = self.shared.events.clone();
                thread::spawn(move || {
                    // A failed read is retried at the next interval.
                    let titles = read_codex_thread_titles(&threads).unwrap_or_default();
                    let _ = events.send(Event::CodexTitles(titles));
                });
            }
        }
        if self
            .status
            .as_ref()
            .is_some_and(|(_, until)| Instant::now() >= *until)
        {
            self.status = None;
        }
    }

    fn refresh_inputs(&self) -> Inputs {
        let mut inputs = Inputs::new(self.options.client.clone());
        inputs.codex_task_titles = self.codex_titles.clone();
        inputs.transcript_pane = self.watch.clone();
        inputs
    }

    /// Puts a refresh's rows in place. While Herdr is away the last rows stay,
    /// for the frontend to show as they were.
    fn apply_refresh(&mut self, refreshed: Refreshed) {
        self.refresh_state = Some(refreshed.state);
        self.last_refresh = Some(Instant::now());
        // A hook found the wake lock taken by this refresh and left the work
        // to it: the next pass goes now rather than after the interval.
        if refreshed.again {
            self.refresh_requested = true;
        }
        match refreshed.view {
            Ok(view) => {
                self.herdr = HerdrStatus {
                    connected: true,
                    error: None,
                };
                self.agents = view.agents;
                self.workspaces = view.workspaces;
                self.projects = state::projects(&self.agents, &self.shared.inboxes);
                if let Some((status, hold)) = view.status {
                    self.status = Some((status, Instant::now() + hold));
                }
                let live: Vec<&str> = self
                    .agents
                    .iter()
                    .filter_map(|agent| codex_thread_id(&agent.info))
                    .collect();
                self.codex_titles
                    .retain(|thread, _| live.contains(&thread.as_str()));
                self.transcript = match (&self.watch, view.transcript) {
                    (Some(watch), Some((pane_id, entries))) if *watch == pane_id => {
                        Some(Transcript {
                            pane_id,
                            entries: state::transcript_entries(&entries),
                        })
                    }
                    // The watched agent is gone.
                    _ => None,
                };
            }
            Err(error) => {
                self.herdr = HerdrStatus {
                    connected: false,
                    error: Some(error),
                };
            }
        }
    }

    /// Sends what changed since the frontend's state, or the whole state the
    /// first time.
    fn publish(&mut self) {
        let state = self.state();
        let message = match &self.sent {
            None => ServerMessage::Snapshot(Box::new(Snapshot {
                seq: self.seq,
                now: unix_now(),
                state: state.clone(),
            })),
            Some(sent) => {
                let changes = state::changes(sent, &state);
                if changes.is_empty() {
                    return;
                }
                self.seq += 1;
                ServerMessage::Delta(corgi_core::protocol::Delta {
                    seq: self.seq,
                    now: unix_now(),
                    changes,
                })
            }
        };
        self.send(&message);
        self.sent = Some(state);
    }

    /// Sends the whole state again, as the next in sequence.
    fn resync(&mut self) {
        if self.sent.is_some() {
            self.seq += 1;
        }
        self.sent = None;
        self.publish();
    }

    fn state(&self) -> State {
        State {
            herdr: self.herdr.clone(),
            status: self.status.as_ref().map(|(status, _)| status.clone()),
            usage: self.usage.clone(),
            projects: self.projects.clone(),
            agents: self.agents.iter().map(state::agent_row).collect(),
            transcript: self.transcript.clone(),
            selection: self.selection.clone(),
            palette: self.palette.clone(),
        }
    }

    fn send(&mut self, message: &ServerMessage) {
        if let Err(error) = self.shared.output.send(message) {
            eprintln!("corgi serve: the frontend has gone: {error}");
            self.output_gone = true;
        }
    }

    /// The agent `pane_id` names among the last refresh's rows.
    fn agent(&self, pane_id: &str) -> Result<&DashboardAgent> {
        self.agents
            .iter()
            .find(|agent| agent.info.pane_id == pane_id)
            .with_context(|| format!("no agent in pane {pane_id}"))
    }

    /// Writes the selection where `corgi edit` looks for it.
    fn share_selection(&self) {
        let Some(path) = &self.options.selection_file else {
            return;
        };
        let _ = crate::app::write_selection(path, self.selection.as_deref().unwrap_or(""));
    }
}
