//! The waker's pass over a herdr-core (`hc`), for `corgi daemon`:
//! [`SupervisorWaker::observe`], the handovers and
//! [`SupervisorWaker::deliver`], made by one long-lived process per core,
//! which holds the core's wake lock for as long as it runs and can time a
//! rest, so it asks Claude Code and Codex supervisors for their handover
//! when their prompt cache is about to go cold.
//!
//! How the core is reached ([`Core`]):
//!
//! - a supervisor is typed into only while it rests (`done` or `idle`) with
//!   its agent in front, never its shell, as a fresh look at its terminal
//!   says, and while a watch-only viewer's first paint shows its input box
//!   empty ([`crate::draft`]); hc's send is never forced, so a blocked
//!   supervisor is never answered by a wake;
//! - its handover exits the agent into the kept shell it was started from,
//!   and starts the successor there ([`crate::hc::launch::relaunch_supervisor`]),
//!   so its workers, terminals of their own, keep running;
//! - the handover's request and baseline are labels of the supervisor's
//!   terminal ([`crate::hc::HANDOVER_LABEL`], [`crate::hc::BASELINE_LABEL`]).
//!
//! Its records (`seen.json`, `handovers.json`, the lock) are named after
//! the core's socket, so each core has its own.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};

use super::{
    Capture, Captured, Delivery, HandoverAction, SUPERVISOR_EXIT_POLL_DELAY, SUPERVISOR_EXIT_POLLS,
    SupervisorWaker, successor_plan, transcript_report, wake_lock_path,
};
use crate::{
    draft,
    hc::{
        BASELINE_LABEL, HANDOVER_LABEL, HcClient, HcError,
        agents::{agent_state, shell_in_front},
        launch::relaunch_supervisor,
        pause::Holds as PauseHolds,
    },
    job::Job,
    model::{Agent, AgentState},
    progress::Silent,
    session::SessionReader,
    supervisor,
};

/// How long a send may take before it counts as having waited for a person
/// to pause typing: hc pastes within a few tenths of a second into a
/// terminal nobody types in, so a longer one may have joined the user's
/// draft, which is logged.
pub const SLOW_SEND: Duration = Duration::from_secs(1);

/// The kinds of error with which hc refuses a send before typing anything
/// (`terminal.send` in `hc schema`).
const REFUSED_BEFORE_TYPING: [&str; 8] = [
    "not_found",
    "machine_unknown",
    "finished",
    "unavailable",
    "blocked",
    "status_mismatch",
    "invalid_params",
    "config",
];

/// Whether Corgi may type into a supervisor now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ready {
    /// It rests with its agent in front and nothing in its input box.
    Yes,
    /// It works, is blocked, has its shell in front, or is gone.
    Busy,
    /// Its input box holds the user's draft.
    Drafting,
}

/// A supervisor's replacement, as the waker hands it to the core: exit the
/// agent in the terminal `address`, then start its successor there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    pub root: String,
    pub name: String,
    pub address: String,
    pub harness: crate::harness::Harness,
}

/// What the hc waker asks of the core, so its passes can be tested without
/// one. [`HcCore`] is the real one.
pub trait Core {
    /// Whether Corgi may type into `supervisor` now.
    fn ready(&mut self, supervisor: &Agent) -> Ready;
    /// Pastes `text` into the terminal `address` and presses Enter, never
    /// forced, and says how that went.
    fn send(&mut self, address: &str, text: &str) -> Delivery;
    /// The report of `agent`, which rests.
    fn report(&mut self, agent: &Agent) -> Capture;
    /// Sets `labels` on the terminal `address`; an empty value removes one.
    fn label(&mut self, address: &str, labels: &[(&str, &str)]) -> Result<()>;
    /// Starts `replacement` in the background.
    fn replace(&mut self, replacement: Replacement) -> Job<Result<String>>;
}

impl SupervisorWaker {
    /// A waker for the hc core listening on `socket`, whose records are
    /// named after that socket.
    pub fn for_core(socket: &Path) -> Self {
        let mut waker = Self::default();
        waker.socket = Some(socket.to_path_buf());
        waker.pause_file = crate::hc::pause::record_path(socket);
        waker
    }

    /// [`SupervisorWaker::for_core`] keeping its inboxes, records and lock
    /// under `base`, as [`SupervisorWaker::in_dir`] does.
    #[cfg(test)]
    pub fn for_core_in(base: &Path) -> Self {
        let mut waker = Self::in_dir(base);
        waker.socket = Some(base.join("hc.sock"));
        waker.pause_file = Some(base.join("pause.json"));
        waker
    }

    /// Takes the core's wake lock for as long as this waker lives, as the
    /// daemon does, and takes up what the waker before it left on disk: the
    /// agents it followed and the handovers. Returns whether it got the
    /// lock; another process holding it wakes this core's supervisors.
    pub fn lead(&mut self) -> bool {
        let Some(path) = self
            .lock_path
            .clone()
            .or_else(|| wake_lock_path(self.socket.as_deref()?))
        else {
            return false;
        };
        if !self.take_lock(&path) {
            return false;
        }
        self.take_up();
        true
    }

    /// One pass over `agents`, the core's agents as they are now: adds a
    /// wake to the inbox of each project's supervisor for each of its agents
    /// that stopped, moves each supervisor's handover on, and types each
    /// supervisor's inbox in when it may be typed into. `corgi_bin` is the
    /// binary the wakes' lines name, and `now` the time in Unix seconds.
    /// Returns what happened worth logging.
    pub fn tick(
        &mut self,
        core: &mut dyn Core,
        agents: &[Agent],
        corgi_bin: &Path,
        now: u64,
    ) -> Vec<String> {
        let mut said = Vec::new();
        if !self.leading() {
            return said;
        }
        let core = RefCell::new(core);
        // What `corgi pause` holds: the paused agents' stops wake no one,
        // and while any agent is paused nothing is typed into any
        // supervisor and no handover moves on (`hc::pause`).
        let holds = self
            .pause_file
            .as_deref()
            .map(PauseHolds::read)
            .unwrap_or_default();
        self.paused = super::Paused {
            panes: holds.panes(agents),
            roots: holds.supervisor_roots(),
        };
        said.extend(self.observe(agents, corgi_bin, |agent| core.borrow_mut().report(agent)));
        // What this pass followed is recorded: an agent seen back after
        // Resume is taken up from here, and its mark goes.
        if let Some(path) = &self.pause_file
            && !holds.is_empty()
        {
            said.extend(
                crate::hc::pause::settle(path, agents, now)
                    .into_iter()
                    .map(|name| format!("{name} is back after Resume")),
            );
        }
        self.pause_in_force = holds.in_force;
        // The replacements that ended since the last pass.
        let mut finished = Vec::new();
        self.replacements.retain_mut(|(root, replacement)| {
            let Some(outcome) = replacement.outcome() else {
                return true;
            };
            let result =
                outcome.unwrap_or_else(|_| Err(anyhow::anyhow!("the replacement panicked")));
            finished.push((std::mem::take(root), result));
            false
        });
        for (root, result) in finished {
            self.replaced(&root, result.is_ok(), agents);
            said.push(
                result
                    .unwrap_or_else(|error| format!("The supervisor's handover failed: {error:#}")),
            );
        }
        if holds.in_force {
            said.push(
                "Agents are paused: no supervisor is typed into and no handover moves on until \
                 Resume"
                    .to_string(),
            );
            if let Err(error) = self.save_handovers() {
                said.push(format!(
                    "Could not save the supervisors' handovers: {error:#}"
                ));
            }
            return said;
        }
        // One look per supervisor and pass: a handover that waits for a
        // draft and the inbox ask the same.
        let mut looked: HashMap<String, Ready> = HashMap::new();
        let mut ready = |agent: &Agent| -> Ready {
            *looked
                .entry(agent.info.address.clone())
                .or_insert_with(|| core.borrow_mut().ready(agent))
        };
        let percent = supervisor::handover_percent();
        let inboxes = self.inboxes.clone();
        let actions = self.hand_over(
            agents,
            percent,
            supervisor::Cache::of,
            now,
            |root, at| {
                inboxes
                    .dir(root)
                    .is_some_and(|dir| supervisor::note_written_since(&dir, at))
            },
            |agent| ready(agent) != Ready::Yes,
        );
        for action in actions {
            let name = action.name.clone();
            let Some(step) = action.step else {
                keep_tokens(&mut **core.borrow_mut(), &action, action.token.as_deref());
                continue;
            };
            match step {
                supervisor::HandoverStep::Ask(at, trigger) => {
                    let request = self.inbox_dir(&action.root).map(|dir| {
                        supervisor::handover_request(corgi_bin, &action.root, &dir, trigger)
                    });
                    let sent = match &request {
                        Some(request) => core.borrow_mut().send(&action.address, request),
                        None => Delivery::Refused,
                    };
                    if sent == Delivery::Sent {
                        let asked = format!("{at} {}", action.session);
                        keep_tokens(&mut **core.borrow_mut(), &action, Some(&asked));
                        said.push(format!("Asked {name} for its handover note ({trigger:?})"));
                    } else {
                        if let Some(handover) = self.handovers.get_mut(&action.root) {
                            handover.unsent(action.change);
                        }
                        said.push(format!("Could not ask {name} for its handover note"));
                    }
                }
                supervisor::HandoverStep::Replace => {
                    said.push(format!("Replacing {name} with a fresh session…"));
                    let root = action.root.clone();
                    let job = core.borrow_mut().replace(Replacement {
                        root: action.root.clone(),
                        name: action.name.clone(),
                        address: action.address.clone(),
                        harness: action.harness.clone(),
                    });
                    self.replacements.push((root, job));
                }
                supervisor::HandoverStep::NoNote => said.push(format!(
                    "{name} wrote no handover note: it keeps its session, and is asked again \
                     after a later turn"
                )),
            }
        }
        let addresses: HashMap<String, String> = agents
            .iter()
            .filter(|agent| agent.supervisor)
            .filter_map(|agent| Some((agent.info.name.clone()?, agent.info.address.clone())))
            .collect();
        let mut held = Vec::new();
        let undelivered = self.deliver(
            agents,
            |agent| match ready(agent) {
                Ready::Yes => false,
                Ready::Drafting => {
                    held.push(agent.info.display_name().to_string());
                    true
                }
                Ready::Busy => true,
            },
            |name, text| match addresses.get(name) {
                Some(address) => {
                    let sent = core.borrow_mut().send(address, text);
                    if sent == Delivery::Sent {
                        said.push(format!("Delivered the inbox to {name} ({address})"));
                    }
                    sent
                }
                None => Delivery::Refused,
            },
        );
        said.extend(undelivered);
        said.extend(
            held.into_iter()
                .map(|name| format!("{name}'s input box holds a draft: its inbox waits")),
        );
        if let Err(error) = self.save_handovers() {
            said.push(format!(
                "Could not save the supervisors' handovers: {error:#}"
            ));
        }
        said
    }

    /// Whether this waker has a handover's replacement running, or a
    /// request waiting for its answer: the daemon then looks again soon,
    /// rather than at its next timed pass.
    pub fn handing_over_any(&self) -> bool {
        self.replacing()
            || self
                .handovers
                .values()
                .any(supervisor::Handover::in_progress)
    }

    /// Whether a pause was in force at the last pass: nothing is typed
    /// into a supervisor then, so an inbox that waits calls for no early
    /// pass.
    pub fn pause_in_force(&self) -> bool {
        self.pause_in_force
    }

    /// Whether `root`'s supervisor has items no delivery took yet.
    pub fn undelivered_for(&self, root: &str) -> usize {
        self.inboxes.undelivered(root)
    }
}

/// Keeps a supervisor's handover request (`handover`, if it has one) and
/// baseline on its terminal, where a daemon that starts again finds them.
fn keep_tokens(core: &mut dyn Core, action: &HandoverAction, handover: Option<&str>) {
    let baseline = action
        .baseline
        .map(|baseline| format!("{baseline} {}", action.session));
    let mut labels: Vec<(&str, &str)> = Vec::new();
    if let Some(handover) = handover {
        labels.push((HANDOVER_LABEL, handover));
    }
    if let Some(baseline) = &baseline {
        labels.push((BASELINE_LABEL, baseline));
    }
    if !labels.is_empty() {
        let _ = core.label(&action.address, &labels);
    }
}

/// The core as `hc` reaches it.
pub struct HcCore {
    client: HcClient,
    sessions: SessionReader,
    corgi_bin: PathBuf,
    /// What the calls did worth logging, for the daemon to take.
    pub notes: Vec<String>,
}

impl HcCore {
    pub fn new(client: HcClient, corgi_bin: PathBuf) -> Self {
        Self {
            client,
            sessions: SessionReader::default(),
            corgi_bin,
            notes: Vec::new(),
        }
    }
}

impl Core for HcCore {
    fn ready(&mut self, supervisor: &Agent) -> Ready {
        let address = &supervisor.info.address;
        let terminal = match self.client.find(address) {
            Ok(Some(terminal)) if terminal.is_live() => terminal,
            _ => return Ready::Busy,
        };
        if !resting(&terminal) {
            return Ready::Busy;
        }
        match self
            .client
            .observe_paint(address, terminal.cols, terminal.rows)
        {
            Ok(paint) => match draft::holds_draft(&supervisor.info.harness(), &paint) {
                Some(true) => Ready::Drafting,
                Some(false) => Ready::Yes,
                // A box Corgi cannot find holds nothing back.
                None => {
                    self.notes.push(format!(
                        "Found no input box on {address}'s screen; typing anyway"
                    ));
                    Ready::Yes
                }
            },
            // A screen that cannot be read holds nothing back, rather
            // than the inbox waiting for good.
            Err(error) => {
                self.notes.push(format!(
                    "Could not read {address}'s screen for a draft ({error:#}); typing anyway"
                ));
                Ready::Yes
            }
        }
    }

    fn send(&mut self, address: &str, text: &str) -> Delivery {
        let started = Instant::now();
        let result = self.client.send(address, text);
        let took = started.elapsed();
        if result.is_ok() && took > SLOW_SEND {
            self.notes.push(format!(
                "The send to {address} took {:.1}s: hc waited for someone to stop typing, so it \
                 may have joined a draft there",
                took.as_secs_f64()
            ));
        }
        delivery_of(&result)
    }

    fn report(&mut self, agent: &Agent) -> Capture {
        if let Some(capture) = transcript_report(&self.sessions.facts(&agent.info)) {
            return capture;
        }
        self.client
            .read(&agent.info.address, 120)
            .ok()
            .map(|text| Captured {
                text: text.trim_end().to_string(),
                transcript: false,
            })
            .filter(|report| !report.text.trim().is_empty())
            .into()
    }

    fn label(&mut self, address: &str, labels: &[(&str, &str)]) -> Result<()> {
        self.client.label(address, labels).map(drop)
    }

    fn replace(&mut self, replacement: Replacement) -> Job<Result<String>> {
        let client = self.client.clone();
        let corgi_bin = self.corgi_bin.clone();
        Job::spawn(move || replace_supervisor(&client, &replacement, &corgi_bin))
    }
}

/// How the send that returned `result` went: hc refused it before typing
/// anything, or it may have typed.
pub fn delivery_of<T>(result: &Result<T>) -> Delivery {
    match result {
        Ok(_) => Delivery::Sent,
        Err(error) => match error.downcast_ref::<HcError>() {
            Some(error) if REFUSED_BEFORE_TYPING.contains(&error.kind.as_str()) => {
                Delivery::Refused
            }
            _ => Delivery::Unknown,
        },
    }
}

/// Whether the agent in `terminal` rests (`done` or `idle`) with itself in
/// front, not the shell it may have exited into.
pub fn resting(terminal: &crate::hc::Terminal) -> bool {
    !shell_in_front(terminal)
        && matches!(agent_state(terminal), AgentState::Done | AgentState::Idle)
}

/// Replaces the supervisor in `replacement` with a fresh session in the same
/// terminal: its harness's exit command, a wait for the kept shell to come
/// to the front, then the launch that takes up the handover note there.
fn replace_supervisor(
    client: &HcClient,
    replacement: &Replacement,
    corgi_bin: &Path,
) -> Result<String> {
    let Replacement {
        root,
        name,
        address,
        harness,
    } = replacement;
    client
        .send(address, harness.exit_command())
        .with_context(|| format!("ask {name} to exit"))?;
    let exited = (0..SUPERVISOR_EXIT_POLLS).any(|_| {
        thread::sleep(SUPERVISOR_EXIT_POLL_DELAY);
        client
            .find(address)
            .is_ok_and(|terminal| terminal.is_some_and(|terminal| shell_in_front(&terminal)))
    });
    anyhow::ensure!(
        exited,
        "{name} did not exit; it keeps running, with its handover note in place"
    );
    let action = HandoverAction {
        step: None,
        baseline: None,
        token: None,
        root: root.clone(),
        name: name.clone(),
        address: address.clone(),
        session: String::new(),
        harness: harness.clone(),
        change: 0,
    };
    let plan = successor_plan(
        &action,
        supervisor::launch_of(&supervisor::state_dir(root)?, harness),
    );
    relaunch_supervisor(client, &plan, corgi_bin, address, &mut Silent)
        .with_context(|| format!("{name} exited, but its successor did not start"))?;
    Ok(format!(
        "{name} handed over to a fresh session in {address}"
    ))
}

#[cfg(test)]
mod tests;
