//! `corgi hook`: Herdr's plugin hooks as Corgi's always-on backend.
//!
//! Herdr runs `corgi hook` when its server starts (and again after a live
//! handoff), and on each of the events the plugin's manifest names: an agent
//! detected in a pane, an agent's status changed, a pane exited or closed, a
//! workspace closed. Each run is a short-lived process, and Corgi runs no
//! process of its own: the hooks do the work a dashboard did only while it
//! was open, so every Corgi window can be closed at any time.
//!
//! An event is only a trigger. Herdr 0.9 runs at most 32 plugin commands at
//! once and drops the events beyond that, starts hooks in parallel and lets
//! them finish in any order, and says little in an event (no previous
//! status, no state change counter, no session). So every hook does the
//! same thing, whatever woke it: one pass of the refresh with a
//! [`SupervisorWaker`], which reconciles from Herdr's snapshot and Corgi's
//! records (the agents followed, the handovers, the marks, the inboxes).
//! A dropped event is caught by the next one, or by the startup hook.
//!
//! One pass runs at a time, under the wake lock it shares with every open
//! dashboard. A hook first marks the state dirty, then tries the lock: one
//! that finds it taken exits at once, and the process holding it goes round
//! once more when its pass ends, since its snapshot may predate the event.
//! The flag is set before the lock is tried, so it is never set after the
//! holder last looked. A hook keeps going round while the flag is set again
//! during its pass, for at most [`Limits::rounds`], and a watchdog ends the
//! process at [`Limits::hard`] whatever it is doing, since Herdr gives a hung
//! hook no time limit and keeps its slot taken for good.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::{
    herdr::HerdrClient,
    model::AgentState,
    refresh::{self, Inputs, RefreshState},
    waker::SupervisorWaker,
};

/// What woke a hook, from the variables Herdr sets for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Event {
    pub kind: EventKind,
    pub pane_id: Option<String>,
    pub workspace_id: Option<String>,
    pub agent: Option<String>,
    /// The status an agent changed to, or ended in when it left the pane.
    pub status: Option<AgentState>,
    /// Whether the agent left the pane, for `pane.agent_detected`.
    pub released: bool,
}

/// The hooks the manifest declares, and what else may run one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum EventKind {
    /// Run by hand, with no event: a pass all the same.
    #[default]
    Manual,
    /// Herdr's server started, or took over in a live handoff.
    Startup,
    AgentDetected,
    AgentStatusChanged,
    PaneExited,
    PaneClosed,
    WorkspaceClosed,
    /// An event this version does not know: a pass all the same.
    Other(String),
}

impl EventKind {
    /// The kind of the event Herdr names `name`, as in `HERDR_PLUGIN_EVENT`
    /// (`pane.agent_detected`) or the event JSON (`pane_agent_detected`).
    fn named(name: &str) -> Self {
        match name.replace('_', ".").as_str() {
            "startup" => Self::Startup,
            "pane.agent.detected" => Self::AgentDetected,
            "pane.agent.status.changed" => Self::AgentStatusChanged,
            "pane.exited" => Self::PaneExited,
            "pane.closed" => Self::PaneClosed,
            "workspace.closed" => Self::WorkspaceClosed,
            _ => Self::Other(name.to_string()),
        }
    }

    /// The name Herdr gives the event, for the log.
    pub fn name(&self) -> &str {
        match self {
            Self::Manual => "manual",
            Self::Startup => "startup",
            Self::AgentDetected => "pane.agent_detected",
            Self::AgentStatusChanged => "pane.agent_status_changed",
            Self::PaneExited => "pane.exited",
            Self::PaneClosed => "pane.closed",
            Self::WorkspaceClosed => "workspace.closed",
            Self::Other(name) => name,
        }
    }
}

impl Event {
    /// The event of a hook Herdr ran with `HERDR_PLUGIN_EVENT` set to `name`
    /// and `HERDR_PLUGIN_EVENT_JSON` to `json` (the startup hook has none).
    /// JSON that cannot be read leaves only the name: the event is a
    /// trigger, and the pass reads what it needs from Herdr.
    pub fn parse(name: Option<&str>, json: Option<&str>) -> Self {
        let json: Option<Value> = json
            .filter(|json| !json.trim().is_empty())
            .and_then(|json| serde_json::from_str(json).ok());
        let named = name.map(str::trim).filter(|name| !name.is_empty());
        let kind = named
            .or_else(|| json.as_ref()?["event"].as_str())
            .map_or(EventKind::Manual, EventKind::named);
        let data = json.as_ref().map(|json| &json["data"]);
        let text = |key: &str| {
            data.and_then(|data| data[key].as_str())
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        let status = ["agent_status", "final_status"]
            .into_iter()
            .find_map(|key| serde_json::from_value(data?[key].clone()).ok());
        Self {
            kind,
            pane_id: text("pane_id"),
            workspace_id: text("workspace_id"),
            agent: text("agent"),
            status,
            released: data.is_some_and(|data| data["released"] == true),
        }
    }

    /// The event of the hook running now, from Herdr's variables.
    pub fn from_env() -> Self {
        let var = |name| std::env::var(name).ok();
        Self::parse(
            var("HERDR_PLUGIN_EVENT").as_deref(),
            var("HERDR_PLUGIN_EVENT_JSON").as_deref(),
        )
    }

    /// The event as the hook's log line names it.
    pub fn describe(&self) -> String {
        let mut text = self.kind.name().to_string();
        for part in [&self.pane_id, &self.agent].into_iter().flatten() {
            text.push(' ');
            text.push_str(part);
        }
        if let Some(status) = self.status {
            text.push(' ');
            text.push_str(&status.label().to_lowercase());
        }
        if self.released {
            text.push_str(" released");
        }
        text
    }
}

/// How long a hook may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// No pass starts after this much time: what is still dirty is left to
    /// the next hook or dashboard.
    pub rounds: Duration,
    /// How long a supervisor's replacement this hook started is waited for,
    /// with the wake lock given up meanwhile, before its outcome is
    /// recorded. One that takes longer counts as failed once
    /// [`crate::supervisor::REPLACEMENT_SECS`] have passed.
    pub replacement: Duration,
    /// When the watchdog ends the process, whatever it is doing. A pass
    /// takes milliseconds and every Herdr call has its own deadline, so this
    /// is for what has none (another process's lock held for good).
    pub hard: Duration,
}

/// Set to a number of milliseconds, this variable lowers the hard limit,
/// as the tests of the limit do.
pub const LIMIT_VAR: &str = "CORGI_HOOK_LIMIT_MS";

impl Default for Limits {
    fn default() -> Self {
        Self {
            rounds: Duration::from_secs(5),
            replacement: Duration::from_secs(90),
            hard: Duration::from_secs(30),
        }
    }
}

impl Limits {
    /// The limits, with the hard limit [`LIMIT_VAR`] sets, if any.
    pub fn from_env() -> Self {
        let mut limits = Self::default();
        if let Some(millis) = std::env::var(LIMIT_VAR)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|millis| *millis > 0)
        {
            limits.hard = Duration::from_millis(millis);
            limits.rounds = limits.rounds.min(limits.hard);
        }
        limits
    }
}

/// Ends the process, through `expire`, once its deadline passes. The
/// deadline can be moved later while a replacement runs.
#[derive(Clone)]
pub struct Watchdog {
    deadline: Arc<Mutex<Instant>>,
    stopped: Arc<AtomicBool>,
}

impl Watchdog {
    /// Starts the watchdog, which calls `expire` on a thread of its own when
    /// `limit` from now has passed, unless [`Watchdog::stop`] came first.
    pub fn start(limit: Duration, expire: impl FnOnce() + Send + 'static) -> Self {
        let watchdog = Self {
            deadline: Arc::new(Mutex::new(Instant::now() + limit)),
            stopped: Arc::new(AtomicBool::new(false)),
        };
        let watching = watchdog.clone();
        thread::spawn(move || {
            loop {
                if watching.stopped.load(Ordering::Relaxed) {
                    return;
                }
                let deadline = *watching.deadline.lock().unwrap_or_else(|e| e.into_inner());
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                thread::sleep((deadline - now).min(Duration::from_millis(50)));
            }
            expire();
        });
        watchdog
    }

    /// Moves the deadline to `at`, if that is later.
    pub fn extend_to(&self, at: Instant) {
        let mut deadline = self.deadline.lock().unwrap_or_else(|e| e.into_inner());
        *deadline = (*deadline).max(at);
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

/// How a hook ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Another process held the wake lock: the state is marked dirty for it.
    Deferred,
    /// This many passes ran, and nothing was left dirty.
    Done { passes: usize },
    /// This many passes ran, and the state was dirty again when the time for
    /// rounds was up: the next hook or dashboard takes it from there.
    OutOfTime { passes: usize },
    /// The dirty flag could not be set, nor the lock taken (no state
    /// directory, or none writable): nothing ran.
    NoState,
}

/// What [`reconcile`] needs of the waker, as a trait so the lock and flag
/// protocol is tested without Herdr.
pub trait Reconciler {
    /// Marks the state dirty; whether that could be done.
    fn mark_dirty(&mut self) -> bool;
    fn is_dirty(&self) -> bool;
    /// Takes the wake lock and keeps it until [`Reconciler::release`];
    /// whether it got it.
    fn hold(&mut self) -> bool;
    fn release(&mut self);
    /// One pass, holding the lock. Returns whether the state was marked
    /// dirty during it.
    fn pass(&mut self) -> bool;
    /// Whether a replacement a pass started still runs.
    fn replacing(&self) -> bool;
}

/// The hook's protocol: mark the state dirty, take the lock or leave the
/// work to its holder, and go round while the state is dirty, within
/// `limits`. A replacement a pass started is waited for with the lock given
/// up, its deadline moved through `extend`, and then a pass records it.
pub fn reconcile(
    reconciler: &mut impl Reconciler,
    limits: Limits,
    mut extend: impl FnMut(Instant),
) -> Outcome {
    let started = Instant::now();
    let marked = reconciler.mark_dirty();
    let mut passes = 0;
    // A pass is owed for the event, and after a replacement for recording
    // it, however long the rounds have taken.
    let mut owed = true;
    loop {
        if !reconciler.hold() {
            // The holder goes round again for the flag set before trying;
            // without a flag, nobody would.
            return if marked {
                Outcome::Deferred
            } else {
                Outcome::NoState
            };
        }
        let mut again = true;
        while again && (owed || started.elapsed() < limits.rounds) {
            again = reconciler.pass();
            owed = false;
            passes += 1;
        }
        reconciler.release();
        if reconciler.replacing() {
            let until = Instant::now() + limits.replacement;
            extend(until + limits.hard);
            while reconciler.replacing() && Instant::now() < until {
                thread::sleep(Duration::from_millis(100));
            }
            reconciler.mark_dirty();
            owed = true;
            continue;
        }
        // Set after the last look under the lock, by a hook that then found
        // the lock taken: this process goes round for it.
        if !reconciler.is_dirty() {
            return Outcome::Done { passes };
        }
        if started.elapsed() >= limits.rounds {
            return Outcome::OutOfTime { passes };
        }
    }
}

/// A hook's passes: the refresh with a waker that leaves the idle
/// prompt-cache handover to dashboards.
pub struct HookPasses {
    inputs: Inputs,
    state: RefreshState,
    /// The status lines the passes reported, for the hook's log.
    pub statuses: Vec<String>,
    /// Why Herdr gave no snapshot, at the last pass, if it gave none.
    pub error: Option<String>,
}

impl HookPasses {
    /// The passes of a hook on `client`'s Herdr session.
    pub fn new(client: HerdrClient) -> Self {
        Self::with_waker(client, SupervisorWaker::default())
    }

    /// The passes with `waker`, as tests give one that keeps its files in a
    /// scratch directory.
    pub fn with_waker(client: HerdrClient, waker: SupervisorWaker) -> Self {
        Self {
            inputs: Inputs::new(client),
            state: RefreshState::new(Some(waker.without_idle_handover())),
            statuses: Vec::new(),
            error: None,
        }
    }

    fn waker(&mut self) -> &mut SupervisorWaker {
        self.state
            .waker
            .get_or_insert_with(|| SupervisorWaker::default().without_idle_handover())
    }
}

impl Reconciler for HookPasses {
    fn mark_dirty(&mut self) -> bool {
        let client = self.inputs.client.clone();
        self.waker().mark_dirty(&client)
    }

    fn is_dirty(&self) -> bool {
        self.state
            .waker
            .as_ref()
            .is_some_and(SupervisorWaker::is_dirty)
    }

    fn hold(&mut self) -> bool {
        let client = self.inputs.client.clone();
        self.waker().hold(&client)
    }

    fn release(&mut self) {
        self.waker().release();
    }

    fn pass(&mut self) -> bool {
        let state = std::mem::take(&mut self.state);
        let refreshed = refresh::run(&self.inputs, state, None);
        self.state = refreshed.state;
        match refreshed.view {
            Ok(view) => {
                self.error = None;
                self.statuses.extend(view.status.map(|(status, _)| status));
            }
            Err(error) => self.error = Some(error),
        }
        refreshed.again
    }

    fn replacing(&self) -> bool {
        self.state
            .waker
            .as_ref()
            .is_some_and(SupervisorWaker::replacing)
    }
}

/// Runs the hook woken by `event` on `client`'s Herdr session, within
/// `limits`, and returns the line for Herdr's plugin log. The watchdog is
/// the caller's, so that a test can run this without ending its process.
pub fn run(client: HerdrClient, event: &Event, limits: Limits, watchdog: &Watchdog) -> String {
    let started = Instant::now();
    let mut passes = HookPasses::new(client);
    let outcome = reconcile(&mut passes, limits, |at| watchdog.extend_to(at));
    log_line(event, &outcome, &passes, started.elapsed())
}

/// The hook's line for Herdr's plugin log: what woke it, how it ended, and
/// what its passes reported.
pub fn log_line(event: &Event, outcome: &Outcome, passes: &HookPasses, took: Duration) -> String {
    let ended = match outcome {
        Outcome::Deferred => "deferred to the pass holding the wake lock".to_string(),
        Outcome::Done { passes } => format!("{passes} pass{}", plural(*passes)),
        Outcome::OutOfTime { passes } => {
            format!(
                "{passes} pass{}, out of time with more to do",
                plural(*passes)
            )
        }
        Outcome::NoState => "no state directory to keep the wake lock in".to_string(),
    };
    let mut line = format!(
        "corgi hook {}: {ended} in {} ms",
        event.describe(),
        took.as_millis()
    );
    if let Some(error) = &passes.error {
        line.push_str(&format!("; Herdr unavailable: {error}"));
    }
    for status in &passes.statuses {
        line.push_str("; ");
        line.push_str(status);
    }
    line
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "es" }
}

#[cfg(test)]
mod tests;
