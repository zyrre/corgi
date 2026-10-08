//! Waking each project's supervisor about its agents, and handing a supervisor
//! whose context is filling up, or whose prompt cache is about to go cold,
//! over to a fresh session.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    harness::Harness,
    herdr::{HerdrClient, ReadSource},
    inbox::{self, Inbox, Item, Kind},
    job::Job,
    model::{AgentState, DashboardAgent},
    paths::{dir_name, socket_state_file},
    session::SessionReader,
    supervisor::{self, SUPERVISOR_TOKEN},
    time::unix_now,
};

use super::{
    draft,
    form::Checkout,
    launch::{CORGI_REQUEST_TOKEN, LaunchPlan, Role, launch_agent},
    markers,
    progress::Silent,
};

/// How long a supervisor that handed over has to exit, polled this often.
const SUPERVISOR_EXIT_POLLS: usize = 60;
const SUPERVISOR_EXIT_POLL_DELAY: Duration = Duration::from_millis(250);

/// Wakes each project's supervisor when another agent of that project stops:
/// every agent whose project has a running supervisor counts, whoever started
/// it, and one the supervisor spawned counts also while the project has none, so
/// the next supervisor hears of it. Only the dashboard holding the lock for its
/// Herdr socket follows the agents, puts each stop in the supervisor's inbox and
/// types the inbox into the supervisor's box, so a supervisor hears each stop once.
/// What it saw of each agent is kept on disk, so a dashboard that starts
/// later or takes over the lock picks up where it left off. The same
/// dashboard hands supervisors over to fresh sessions.
#[derive(Debug, Default)]
pub(super) struct SupervisorWaker {
    /// The wake lock, while this dashboard holds it.
    lock: Option<fs::File>,
    /// The Corgi binary the wake messages name.
    corgi_bin: Option<PathBuf>,
    /// The supervisors' inboxes, whose base directory, when a test sets one,
    /// also holds the record of the agents followed.
    inboxes: Inboxes,
    /// The Herdr socket, which names the record of the agents followed.
    socket: Option<PathBuf>,
    /// Each agent followed, by the name its messages use.
    agents: BTreeMap<String, Followed>,
    /// Whether `agents` has taken up the record the dashboard that led
    /// before left, which happens at the first refresh this one leads.
    resumed: bool,
    /// The record as last written, so an unchanged one is not written again.
    written: Option<String>,
    /// By state directory, the items typed into a supervisor's box whose
    /// delivery could not be recorded, so they are not typed in again while
    /// recording them is retried; dropped once the inbox shows them
    /// delivered.
    unrecorded: HashMap<PathBuf, HashSet<String>>,
    /// Each project's supervisor handover, by project root.
    handovers: HashMap<String, supervisor::Handover>,
    /// Replacements running in the background, by project root, each ending
    /// with its status line or error.
    replacements: Vec<(String, Job<Result<String>>)>,
}

/// The supervisors' inboxes, a directory per project. The waker delivers
/// what is in them; the merge popup adds to one while a refresh may be
/// delivering from it in the background, which is safe because every write
/// to an inbox holds its lock.
#[derive(Debug, Clone, Default)]
pub(super) struct Inboxes {
    /// The directory holding a directory per project, in place of Corgi's
    /// state directory, so that a test never touches the developer's own
    /// state.
    base: Option<PathBuf>,
}

impl Inboxes {
    /// The state directory holding the inbox of `root`'s supervisor.
    fn dir(&self, root: &str) -> Option<PathBuf> {
        match &self.base {
            Some(base) => Some(base.join(dir_name(root).unwrap_or(supervisor::UNNAMED_PROJECT))),
            None => supervisor::state_dir(root).ok(),
        }
    }

    /// Adds `item` to the inbox of the supervisor of its project, to go out at a
    /// refresh once the supervisor is between turns.
    pub(super) fn notify(&self, item: Item) -> Result<()> {
        let dir = self
            .dir(&item.project)
            .context("Corgi has no state directory")?;
        inbox::append(&dir, item)?;
        Ok(())
    }

    /// The lines waiting for `root`'s supervisor, each as it goes out alone.
    #[cfg(test)]
    pub(super) fn pending_for(&self, root: &str) -> Vec<String> {
        let dir = self.dir(root).expect("inbox dir");
        Inbox::read(&dir)
            .undelivered_for(root)
            .into_iter()
            .map(|item| inbox::prompt_text(&[item]))
            .collect()
    }
}

/// One agent the waker follows, as kept in its record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Followed {
    /// The agent's pane: an agent of the same name in another pane is
    /// another agent.
    pane: String,
    /// Which run of the pane's state changes this is: set when the agent is
    /// first followed, and again when its `state_change_seq` goes back, as
    /// it does when Herdr restarts, so a wake's id never repeats an earlier
    /// one's.
    #[serde(default)]
    since: u64,
    /// The `state_change_seq` last seen.
    #[serde(default)]
    change: u64,
    transitions: supervisor::Transitions,
}

/// A run of state changes newer than `after`: the clock in nanoseconds.
fn incarnation(after: u64) -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| u64::try_from(time.as_nanos()).unwrap_or(u64::MAX));
    now.max(after + 1)
}

/// The report of an agent that rests, as captured then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Captured {
    pub(super) text: String,
    /// Whether it is the newest thing said in its transcript, rather than
    /// its screen.
    pub(super) transcript: bool,
}

/// A stop to add to a supervisor's inbox, found at one refresh.
struct Stop<'a> {
    agent: &'a DashboardAgent,
    name: &'a str,
    state: AgentState,
    /// Whether the agent came and finished while no dashboard led.
    appeared: bool,
    /// What the waker followed of the agent before this refresh, put back if
    /// the stop cannot be added, so it is added at the next refresh.
    before: Option<Followed>,
    /// The followed agent's run of state changes.
    since: u64,
}

/// What a handover at one refresh asks the dashboard to do for one supervisor.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HandoverAction {
    /// `None` when there is only a new baseline to keep on the pane.
    step: Option<supervisor::HandoverStep>,
    /// The session's baseline, once recorded.
    baseline: Option<u64>,
    /// The session's handover request token, kept alongside the baseline.
    token: Option<String>,
    root: String,
    name: String,
    pane_id: String,
    session: String,
    harness: Harness,
    change: u64,
}

impl SupervisorWaker {
    /// Whether this dashboard wakes supervisors, taking the lock at `path` if
    /// no other dashboard holds it. The lock goes with the process.
    pub(super) fn lead(&mut self, path: &Path) -> bool {
        if self.lock.is_none() {
            self.lock = fs::create_dir_all(path.parent().unwrap_or(path))
                .ok()
                .and_then(|()| fs::File::create(path).ok())
                .filter(|file| file.try_lock().is_ok());
        }
        self.lock.is_some()
    }

    /// Whether this dashboard wakes supervisors of `client`'s Herdr session,
    /// taking the lock if no other dashboard holds it.
    pub(super) fn leads(&mut self, client: &HerdrClient) -> bool {
        wake_lock_path(client.socket_path()).is_some_and(|lock| self.lead(&lock))
    }

    /// Whether `root`'s supervisor is handing over, so wakes for it wait for the
    /// supervisor that takes over.
    fn handing_over(&self, root: &str) -> bool {
        self.handovers
            .get(root)
            .is_some_and(supervisor::Handover::in_progress)
    }

    /// A waker that delivers from `inboxes`, picking up from the record the
    /// dashboard that led before left. A refresh that panicked lost its
    /// waker, and the next one starts again with this.
    pub(super) fn with_inboxes(inboxes: Inboxes) -> Self {
        let mut waker = Self::default();
        waker.inboxes = inboxes;
        waker
    }

    /// A waker that keeps its inboxes and record under `base`.
    #[cfg(test)]
    pub(super) fn in_dir(base: &Path) -> Self {
        Self::with_inboxes(Inboxes {
            base: Some(base.to_path_buf()),
        })
    }

    /// The inboxes this waker delivers from, for adding to them while it is
    /// away in a refresh.
    pub(super) fn inboxes(&self) -> Inboxes {
        self.inboxes.clone()
    }

    /// The state directory holding the inbox of `root`'s supervisor.
    fn inbox_dir(&self, root: &str) -> Option<PathBuf> {
        self.inboxes.dir(root)
    }

    /// The record of the agents followed: `wake/<socket>.seen.json`.
    fn record_file(&self) -> Option<PathBuf> {
        match &self.inboxes.base {
            Some(base) => Some(base.join("seen.json")),
            None => socket_state_file("wake", self.socket.as_deref()?, "seen.json"),
        }
    }

    /// The record of the agents followed that the dashboard leading before
    /// left, if there is one.
    fn load_record(&self) -> Option<BTreeMap<String, Followed>> {
        serde_json::from_str(&fs::read_to_string(self.record_file()?).ok()?).ok()
    }

    /// Writes the record of the agents followed, when it changed.
    fn save_record(&mut self) -> Result<()> {
        let Some(path) = self.record_file() else {
            return Ok(());
        };
        let text = serde_json::to_string(&self.agents)?;
        if self.written.as_deref() == Some(text.as_str()) {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, &text).with_context(|| format!("write {}", temporary.display()))?;
        fs::rename(&temporary, &path).with_context(|| format!("replace {}", path.display()))?;
        self.written = Some(text);
        Ok(())
    }

    /// Records the agents' states from one refresh, adding an item to the
    /// supervisor's inbox for each one that stopped, with the report `report`
    /// captures for one that rests. At the first refresh this dashboard
    /// leads, it takes up the record the one before left, so what changed
    /// while none led is news, and an agent the supervisor spawned that appeared
    /// and finished meanwhile is reported once, if it is done or has a
    /// report from its transcript. A stop that cannot be added is tried
    /// again at the next refresh. Returns the error of an inbox or the
    /// record that could not be written.
    fn observe(
        &mut self,
        agents: &[DashboardAgent],
        corgi_bin: &Path,
        mut report: impl FnMut(&DashboardAgent) -> Option<Captured>,
    ) -> Option<String> {
        let supervisors: HashSet<&str> = agents
            .iter()
            .filter(|agent| agent.supervisor)
            .map(|agent| agent.project_root.as_str())
            .collect();
        let record = if self.resumed {
            None
        } else {
            self.resumed = true;
            Some(self.load_record())
        };
        let mut seen = HashSet::new();
        let mut stops = Vec::new();
        // A scratch agent belongs to no project, so no supervisor hears of it.
        for agent in agents
            .iter()
            .filter(|agent| !agent.supervisor && !agent.scratch)
        {
            let name = agent.info.name.as_deref().unwrap_or(&agent.info.pane_id);
            seen.insert(name.to_string());
            let (state, change, pane) = (
                agent.info.state,
                agent.info.state_change_seq,
                &agent.info.pane_id,
            );
            let followed = self
                .agents
                .get(name)
                .or_else(|| record.as_ref()?.as_ref()?.get(name))
                .filter(|followed| followed.pane == *pane)
                .cloned();
            // Known to no dashboard, though one led before.
            let appeared = followed.is_none() && matches!(record, Some(Some(_)));
            let mut next = followed.clone().unwrap_or_else(|| Followed {
                pane: pane.clone(),
                since: incarnation(0),
                change,
                transitions: supervisor::Transitions::default(),
            });
            if change < next.change {
                next.since = incarnation(next.since);
            }
            next.change = change;
            let mut woke = next.transitions.observe(state, change);
            let appeared =
                appeared && own(agent) && matches!(state, AgentState::Done | AgentState::Idle);
            if appeared {
                woke = Some(state);
            }
            let since = next.since;
            self.agents.insert(name.to_string(), next);
            let root = agent.project_root.as_str();
            if let Some(state) = woke
                && (supervisors.contains(root) || self.handing_over(root) || own(agent))
            {
                stops.push(Stop {
                    agent,
                    name,
                    state,
                    appeared,
                    before: followed,
                    since,
                });
            }
        }
        self.agents.retain(|name, _| seen.contains(name));
        let mut error = None;
        for stop in stops {
            let resting = matches!(stop.state, AgentState::Done | AgentState::Idle);
            let report = resting.then(|| report(stop.agent)).flatten();
            // One that only appeared needs something to show for it: its
            // screen may be one that never started.
            if stop.appeared
                && !(stop.state == AgentState::Done
                    || report.as_ref().is_some_and(|report| report.transcript))
            {
                continue;
            }
            let item = wake_item(&stop, report.map(|report| report.text), corgi_bin);
            if let Err(failed) = self.notify(item) {
                let name = stop.name;
                error = Some(format!(
                    "Could not add {name}'s wake to the inbox: {failed:#}"
                ));
                match stop.before {
                    Some(before) => {
                        self.agents.insert(name.to_string(), before);
                    }
                    // Unknown before: left out of the record, and found
                    // again as having appeared when the record is taken up
                    // at the next refresh.
                    None => {
                        self.agents.remove(name);
                        self.resumed = false;
                    }
                }
            }
        }
        // The items go first: a record saved before them could lose them,
        // while a wake added again after a crash is the same item.
        if let Err(failed) = self.save_record() {
            error = Some(format!("Could not save the agents followed: {failed:#}"));
        }
        error
    }

    /// Adds `item` to the inbox of the supervisor of its project, to go out at a
    /// refresh once the supervisor is between turns.
    pub(super) fn notify(&self, item: Item) -> Result<()> {
        self.inboxes.notify(item)
    }

    /// The lines waiting for `root`'s supervisor, each as it goes out alone.
    #[cfg(test)]
    pub(super) fn pending_for(&self, root: &str) -> Vec<String> {
        self.inboxes.pending_for(root)
    }

    /// Advances each supervisor's handover by one refresh, with `percent` as
    /// the threshold and `idle_secs` giving each harness's idle time, and
    /// returns what to do about it. `note_since(root, at)` says whether
    /// `root`'s supervisor wrote its note since `at`, and `drafting(supervisor)`
    /// whether the user is writing in its input box, which holds a due
    /// request, or the replacement, back for a later refresh.
    fn hand_over(
        &mut self,
        agents: &[DashboardAgent],
        percent: u8,
        idle_secs: impl Fn(&Harness) -> Option<u64>,
        now: u64,
        mut note_since: impl FnMut(&str, u64) -> bool,
        mut drafting: impl FnMut(&DashboardAgent) -> bool,
    ) -> Vec<HandoverAction> {
        let busy: HashSet<&str> = agents
            .iter()
            .filter(|agent| {
                !agent.supervisor
                    && matches!(agent.info.state, AgentState::Working | AgentState::Blocked)
            })
            .map(|agent| agent.project_root.as_str())
            .collect();
        let mut actions = Vec::new();
        // The home directory is never a project, so nothing there is
        // handed over as its supervisor.
        for agent in agents
            .iter()
            .filter(|agent| agent.supervisor && !agent.scratch)
        {
            let (Some(name), Some(session)) = (
                agent.info.name.as_deref(),
                supervisor::native_session(&agent.info),
            ) else {
                continue;
            };
            let root = agent.project_root.as_str();
            let token = |name| agent.info.tokens.get(name).map(String::as_str);
            let harness = agent.info.harness();
            let sighting = supervisor::Sighting {
                session,
                state: agent.info.state,
                change: agent.info.state_change_seq,
                context_percent: agent.context_percent,
                context_tokens: agent.context_tokens,
                workers_busy: busy.contains(root),
                token: token(supervisor::HANDOVER_TOKEN),
                baseline_token: token(supervisor::BASELINE_TOKEN),
            };
            let limits = supervisor::Thresholds {
                percent,
                idle_secs: idle_secs(&harness),
            };
            let handover = self.handovers.entry(root.to_string()).or_default();
            let step = handover.observe(
                sighting,
                limits,
                now,
                |at| note_since(root, at),
                || !drafting(agent),
            );
            let baseline = handover.baseline();
            let unsaved = baseline.is_some_and(|baseline| {
                sighting.baseline_token != Some(format!("{baseline} {session}").as_str())
            });
            if step.is_some() || unsaved {
                actions.push(HandoverAction {
                    step,
                    baseline,
                    token: sighting.token.map(str::to_string),
                    root: root.to_string(),
                    name: name.to_string(),
                    pane_id: agent.info.pane_id.clone(),
                    session: session.to_string(),
                    harness,
                    change: agent.info.state_change_seq,
                });
            }
        }
        // A project whose supervisor is gone keeps its record only while the
        // replacement that removed it runs.
        let live: HashSet<&str> = agents
            .iter()
            .filter(|agent| agent.supervisor)
            .map(|agent| agent.project_root.as_str())
            .collect();
        self.handovers
            .retain(|root, handover| live.contains(root.as_str()) || handover.in_progress());
        actions
    }

    /// Records the end of `root`'s replacement.
    fn replaced(&mut self, root: &str, succeeded: bool, agents: &[DashboardAgent]) {
        let change = agents
            .iter()
            .find(|agent| agent.supervisor && agent.project_root == root)
            .map_or(0, |agent| agent.info.state_change_seq);
        if let Some(handover) = self.handovers.get_mut(root) {
            handover.replaced(succeeded, change);
        }
    }

    /// Types each project's undelivered inbox items into its supervisor's box,
    /// one prompt with the newest item of each key, once the supervisor is
    /// between turns, and records them delivered. A supervisor that is working or
    /// blocked keeps them until a later refresh, so they arrive after its
    /// turn, and so does one whose input box holds the user's draft, as
    /// `drafting` says, and one handing over, for its successor. Items for
    /// a project without a supervisor wait in its inbox, and each supervisor gets only
    /// its own project's, though projects of the same directory name share
    /// an inbox. Returns the error of a delivery that could not be recorded.
    fn deliver(
        &mut self,
        agents: &[DashboardAgent],
        mut drafting: impl FnMut(&DashboardAgent) -> bool,
        mut prompt: impl FnMut(&str, &str) -> bool,
    ) -> Option<String> {
        let mut error = None;
        let mut roots = HashSet::new();
        for agent in agents
            .iter()
            .filter(|agent| agent.supervisor && !agent.scratch)
        {
            let root = agent.project_root.as_str();
            let Some(supervisor) = agent.info.name.as_deref() else {
                continue;
            };
            if !roots.insert(root) || self.handing_over(root) {
                continue;
            }
            let Some(dir) = self.inbox_dir(root) else {
                continue;
            };
            let inbox = Inbox::read(&dir);
            let undelivered = inbox.undelivered_for(root);
            let unrecorded = self.unrecorded.entry(dir.clone()).or_default();
            unrecorded.retain(|id| undelivered.iter().any(|item| item.id == *id));
            if !unrecorded.is_empty() {
                let ids: Vec<String> = unrecorded.iter().cloned().collect();
                let _ = inbox::mark_delivered(&dir, &ids, supervisor);
            }
            let pending: Vec<&Item> = undelivered
                .into_iter()
                .filter(|item| !unrecorded.contains(&item.id))
                .collect();
            if pending.is_empty()
                || matches!(agent.info.state, AgentState::Working | AgentState::Blocked)
                || drafting(agent)
                || !prompt(supervisor, &inbox::prompt_text(&pending))
            {
                continue;
            }
            let ids: Vec<String> = pending.iter().map(|item| item.id.clone()).collect();
            if let Err(failed) = inbox::mark_delivered(&dir, &ids, supervisor) {
                unrecorded.extend(ids);
                error = Some(format!(
                    "Could not record the delivery to {supervisor}: {failed:#}"
                ));
            }
        }
        error
    }

    /// Takes one refresh's agents: tells each project's supervisor about its
    /// agents that stopped since the last one, and hands supervisors whose
    /// handover is due over to fresh sessions, when this dashboard is
    /// the one that wakes supervisors. Returns the status line to show, if any.
    fn tick(
        &mut self,
        client: &HerdrClient,
        agents: &[DashboardAgent],
        report: impl FnMut(&DashboardAgent) -> Option<Captured>,
    ) -> Option<String> {
        if !self.leads(client) {
            return None;
        }
        self.socket
            .get_or_insert_with(|| client.socket_path().to_path_buf());
        let corgi_bin = self.corgi_bin.get_or_insert_with(|| {
            let installed = client.plugin_root(supervisor::PLUGIN_ID).ok().flatten();
            supervisor::corgi_bin(installed.as_deref()).unwrap_or_else(|_| PathBuf::from("corgi"))
        });
        let corgi_bin = corgi_bin.clone();
        let mut status = self.observe(agents, &corgi_bin, report);
        let mut finished = Vec::new();
        self.replacements.retain_mut(|(root, replacement)| {
            let Some(outcome) = replacement.outcome() else {
                return true;
            };
            // A panicking replacement still reports, or the handover would
            // stay in progress for good.
            let result =
                outcome.unwrap_or_else(|_| Err(anyhow::anyhow!("the replacement panicked")));
            finished.push((std::mem::take(root), result));
            false
        });
        for (root, result) in finished {
            self.replaced(&root, result.is_ok(), agents);
            status =
                Some(result.unwrap_or_else(|error| {
                    format!("The supervisor's handover failed: {error:#}")
                }));
        }
        let percent = supervisor::handover_percent();
        let actions = self.hand_over(
            agents,
            percent,
            supervisor::handover_idle_secs,
            unix_now(),
            |root, at| {
                supervisor::state_dir(root)
                    .is_ok_and(|dir| supervisor::note_written_since(&dir, at))
            },
            |supervisor| holds_draft(client, supervisor),
        );
        for action in actions {
            let name = &action.name;
            let Some(step) = action.step else {
                report_tokens(client, &action, action.token.as_deref());
                continue;
            };
            match step {
                supervisor::HandoverStep::Ask(at, trigger) => {
                    let asked = supervisor::state_dir(&action.root).and_then(|dir| {
                        client.prompt_agent(name, &supervisor::handover_request(&dir, trigger))
                    });
                    status = Some(match asked {
                        Ok(_) => {
                            let asked = format!("{at} {}", action.session);
                            report_tokens(client, &action, Some(&asked));
                            match trigger {
                                supervisor::Trigger::Full(percent) => format!(
                                    "{name}'s context is past {percent}%: asked for its handover note"
                                ),
                                supervisor::Trigger::Idle(minutes) => format!(
                                    "{name} has been idle {minutes} min, its prompt cache about to expire: asked for its handover note"
                                ),
                                supervisor::Trigger::Update => format!(
                                    "{name} worked after writing its handover note: asked to bring it up to date"
                                ),
                            }
                        }
                        Err(error) => {
                            if let Some(handover) = self.handovers.get_mut(&action.root) {
                                handover.unsent(action.change);
                            }
                            format!("Could not ask {name} for its handover note: {error:#}")
                        }
                    });
                }
                supervisor::HandoverStep::Replace => {
                    status = Some(format!("Replacing {name} with a fresh session…"));
                    let client = client.clone();
                    let root = action.root.clone();
                    let replacement = Job::spawn(move |_| replace_supervisor(&client, &action));
                    self.replacements.push((root, replacement));
                }
                supervisor::HandoverStep::NoNote => {
                    status = Some(format!(
                        "{name} wrote no handover note: it keeps its session, and is asked again after a later turn"
                    ));
                }
            }
        }
        let undelivered = self.deliver(
            agents,
            |supervisor| holds_draft(client, supervisor),
            |supervisor, text| client.prompt_agent(supervisor, text).is_ok(),
        );
        status.or(undelivered)
    }
}

/// Whether `agent` is one a supervisor spawned: it carries the spawn's request id.
fn own(agent: &DashboardAgent) -> bool {
    agent.info.tokens.contains_key(CORGI_REQUEST_TOKEN)
}

/// The inbox item for `stop`, with the agent's `report`. Its line names the
/// command that prints the report; [`inbox::prompt_text`] decides whether
/// the report goes out with it instead. Its id names the agent's pane, run of
/// state changes and state change, so the same stop is never added twice and
/// a later one never taken for it.
fn wake_item(stop: &Stop<'_>, report: Option<String>, corgi_bin: &Path) -> Item {
    let Stop {
        agent, name, state, ..
    } = stop;
    let (pane, change) = (&agent.info.pane_id, agent.info.state_change_seq);
    let label = state.label().to_lowercase();
    Item {
        id: inbox::file_safe(&format!(
            "wake-{name}-{pane}-{}-{change}-{label}",
            stop.since
        )),
        project: agent.project_root.clone(),
        source: "dashboard".into(),
        kind: Kind::Wake,
        agent: Some(name.to_string()),
        state: Some(label),
        pane: Some(pane.clone()),
        change: Some(change),
        key: name.to_string(),
        text: supervisor::wake_message(corgi_bin, name, *state),
        own: own(agent),
        report,
        ..Item::default()
    }
}

/// The report of `agent`, as `corgi report` prints it: the newest thing it
/// said, from its transcript, or its screen when its harness writes none.
fn captured_report(
    sessions: &mut SessionReader,
    client: &HerdrClient,
    agent: &DashboardAgent,
) -> Option<Captured> {
    let said = sessions.facts(&agent.info).report.map(|entry| Captured {
        text: entry.text,
        transcript: true,
    });
    said.or_else(|| {
        let read = client
            .read_agent(&agent.info.pane_id, ReadSource::RecentUnwrapped, Some(120))
            .ok()?;
        Some(Captured {
            text: read.text.trim_end().to_string(),
            transcript: false,
        })
    })
    .filter(|report| !report.text.trim().is_empty())
}

/// Whether the user is writing in `supervisor`'s input box, so that nothing is
/// typed into their draft. A screen that cannot be read, or a harness whose
/// box Corgi does not know, counts as no draft: the text goes out as it did
/// before this check, rather than waiting for good.
fn holds_draft(client: &HerdrClient, supervisor: &DashboardAgent) -> bool {
    let screen = client
        .read_agent_styled(&supervisor.info.pane_id)
        .map(|read| read.text);
    screen_holds_draft(screen, &supervisor.info.harness())
}

/// Whether `screen`, as read from a `harness` supervisor's pane, shows a draft.
fn screen_holds_draft(screen: Result<String>, harness: &Harness) -> bool {
    screen
        .ok()
        .and_then(|screen| draft::holds_draft(harness, &screen))
        .unwrap_or(false)
}

/// Keeps a supervisor's handover tokens on its pane: its `handover` request,
/// if it has one, and its baseline. The marker goes with them, so the supervisor
/// keeps it whether Herdr merges tokens or replaces them, and all of them go
/// into Corgi's record of its marks.
fn report_tokens(client: &HerdrClient, action: &HandoverAction, handover: Option<&str>) {
    let baseline = action
        .baseline
        .map(|baseline| format!("{baseline} {}", action.session));
    let tokens: Vec<(&str, &str)> = [
        Some((SUPERVISOR_TOKEN, action.name.as_str())),
        handover.map(|handover| (supervisor::HANDOVER_TOKEN, handover)),
        baseline
            .as_deref()
            .map(|baseline| (supervisor::BASELINE_TOKEN, baseline)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let _ = markers::mark_pane(
        client,
        &action.pane_id,
        &action.name,
        Some(&action.session),
        &tokens,
    );
}

/// Gives up the wake lock at once. Closing the file alone may not: a child
/// process spawned on another thread at that moment briefly holds a copy of
/// the descriptor, and with it the lock, so the next dashboard would find
/// it still taken.
impl Drop for SupervisorWaker {
    fn drop(&mut self) {
        if let Some(lock) = &self.lock {
            let _ = lock.unlock();
        }
    }
}

/// The launch of the supervisor that takes over from the one in `action`,
/// started the way `launch` records.
fn successor_plan(action: &HandoverAction, launch: supervisor::Launch) -> LaunchPlan {
    LaunchPlan {
        name: action.name.clone(),
        harness: Harness::from_kind(&launch.kind),
        model: launch.model,
        effort: launch.effort,
        prompt: String::new(),
        project: PathBuf::from(&action.root),
        new_project: false,
        checkout: Checkout::ProjectRoot,
        extra_args: launch.extra_args,
        role: Role::Supervisor {
            handover_pane: Some(action.pane_id.clone()),
        },
        request_id: None,
    }
}

/// Replaces the supervisor that handed over in `action` with a fresh session
/// in the same pane: the old one exits through its harness's own command,
/// which leaves its transcript on disk and its pane at the shell, and the new
/// one starts there under the same name, marked as the supervisor, the way it
/// was launched, and with a first prompt that takes up the note.
fn replace_supervisor(client: &HerdrClient, action: &HandoverAction) -> Result<String> {
    let HandoverAction {
        root,
        name,
        pane_id,
        harness,
        ..
    } = action;
    client
        .prompt_agent(name, harness.exit_command())
        .with_context(|| format!("ask {name} to exit"))?;
    let exited = (0..SUPERVISOR_EXIT_POLLS).any(|_| {
        thread::sleep(SUPERVISOR_EXIT_POLL_DELAY);
        client.snapshot().is_ok_and(|snapshot| {
            !snapshot
                .agents
                .iter()
                .any(|agent| agent.pane_id == *pane_id && agent.name.as_deref() == Some(name))
        })
    });
    anyhow::ensure!(
        exited,
        "{name} did not exit; it keeps running, with its handover note in place"
    );
    let plan = successor_plan(
        action,
        supervisor::launch_of(&supervisor::state_dir(root)?, harness),
    );
    launch_agent(client, &plan, &mut Silent)
        .with_context(|| format!("{name} exited, but its successor did not start"))?;
    Ok(format!("{name} handed over to a fresh session"))
}

/// The lock that makes one dashboard per Herdr socket the one that wakes
/// supervisors, named after the socket so that dashboards of other Herdr
/// sessions wake their own.
fn wake_lock_path(socket: &Path) -> Option<PathBuf> {
    socket_state_file("wake", socket, "lock")
}

/// Whether a dashboard holds the wake lock of the Herdr session at
/// `socket`, so the supervisors' inboxes are being delivered. Trying the lock
/// holds it only for that moment, and a lock nobody ever took is free.
pub(super) fn wake_lock_held(socket: &Path) -> bool {
    let Some(file) = wake_lock_path(socket).and_then(|path| fs::File::open(path).ok()) else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(_) => true,
    }
}

/// Tells each project's supervisor about its `agents` that stopped since the
/// last refresh, and hands supervisors whose handover is due over to fresh
/// sessions, when this dashboard is the one that wakes supervisors. Reports
/// are read with `sessions`. Returns the status line to show, if any.
pub(super) fn wake_supervisors(
    waker: &mut SupervisorWaker,
    client: &HerdrClient,
    sessions: &mut SessionReader,
    agents: &[DashboardAgent],
) -> Option<String> {
    let report = |agent: &DashboardAgent| captured_report(sessions, client, agent);
    waker.tick(client, agents, report)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use crate::{
        app::launch::launch_args,
        model::{AgentInfo, AgentState, DashboardAgent},
    };

    use super::*;
    use crate::test_support::ScratchDir;

    fn project_agent(
        name: &str,
        root: &str,
        supervisor: bool,
        state: AgentState,
    ) -> DashboardAgent {
        DashboardAgent {
            info: AgentInfo {
                pane_id: format!("{name}:p1"),
                name: Some(name.into()),
                state,
                ..AgentInfo::default()
            },
            project_root: root.into(),
            supervisor,
            ..DashboardAgent::default()
        }
    }

    #[test]
    fn the_dashboard_wakes_a_supervisor_once_per_stop_of_its_projects_agents() {
        let scratch = ScratchDir::new("waker");
        use AgentState::{Done, Idle, Working};
        let corgi_bin = Path::new("/opt/corgi");
        let mut waker = SupervisorWaker::in_dir(&scratch);
        let sent = std::cell::RefCell::new(Vec::new());
        let refresh = |waker: &mut SupervisorWaker, agents: &[DashboardAgent]| {
            waker.observe(agents, corgi_bin, |_| None);
            waker.deliver(
                agents,
                |_| false,
                |supervisor, text| {
                    sent.borrow_mut().push(format!("{supervisor}: {text}"));
                    true
                },
            );
        };
        let agents = |supervisor: AgentState, worker: AgentState, other: AgentState| {
            vec![
                project_agent("supervisor-weather", "/repos/weather", true, supervisor),
                project_agent("w-forecast", "/repos/weather", false, worker),
                project_agent("w-elsewhere", "/repos/corgi", false, other),
            ]
        };
        // The first refresh is a baseline: nothing is news yet.
        refresh(&mut waker, &agents(Idle, Done, Done));
        refresh(&mut waker, &agents(Working, Working, Working));
        // The worker stops while its supervisor is mid-turn: the message waits.
        refresh(&mut waker, &agents(Working, Done, Done));
        refresh(&mut waker, &agents(Working, Done, Done));
        refresh(&mut waker, &agents(Done, Done, Done));
        refresh(&mut waker, &agents(Idle, Idle, Idle));
        assert_eq!(
            *sent.borrow(),
            ["supervisor-weather: [Corgi] w-forecast is done. Run: /opt/corgi report w-forecast"]
        );
        // The supervisor's own turns, and another project's agents, wake nobody.
        refresh(&mut waker, &agents(Working, Idle, Working));
        refresh(&mut waker, &agents(Done, Idle, Done));
        assert_eq!(sent.borrow().len(), 1);
    }

    /// A worker of `/repos/weather` in `state` after `change` state changes,
    /// spawned by the supervisor (with a request id) when `own`.
    fn weather_worker(name: &str, state: AgentState, change: u64, own: bool) -> DashboardAgent {
        let mut agent = project_agent(name, "/repos/weather", false, state);
        agent.info.state_change_seq = change;
        if own {
            agent
                .info
                .tokens
                .insert(CORGI_REQUEST_TOKEN.into(), format!("20261003-{name}"));
        }
        agent
    }

    /// One refresh of the dashboard that wakes supervisors, with `report` as
    /// every resting agent's report and the supervisor's box holding a draft
    /// when `drafting`. Returns what was typed into the supervisor's box.
    fn wake_refresh(
        waker: &mut SupervisorWaker,
        agents: &[DashboardAgent],
        report: Option<&str>,
        drafting: bool,
    ) -> Vec<String> {
        let mut sent = Vec::new();
        assert_eq!(
            waker.observe(agents, Path::new("/opt/corgi"), |_| report.map(said)),
            None
        );
        waker.deliver(
            agents,
            |_| drafting,
            |supervisor, text| {
                sent.push(format!("{supervisor}: {text}"));
                true
            },
        );
        sent
    }

    /// `text` as a report from a transcript.
    fn said(text: &str) -> Captured {
        Captured {
            text: text.into(),
            transcript: true,
        }
    }

    fn weather_supervisor(state: AgentState) -> DashboardAgent {
        project_agent("supervisor-weather", "/repos/weather", true, state)
    }

    #[test]
    fn a_worker_herdr_gave_no_session_is_listed_and_wakes_its_supervisor() {
        use crate::{
            app::{cli::fleet_rows, rows::STATUS_LINE_SESSION_TOKEN},
            herdr::{PaneInfo, SessionSnapshot},
            model::AgentSession,
            supervisor::{SUPERVISOR_TOKEN, marker},
            test_support::test_app,
        };
        use AgentState::{Done, Working};

        let scratch = ScratchDir::new("waker-no-session");
        let agent = |name: &str, state: AgentState, tokens: &[(&str, &str)]| AgentInfo {
            pane_id: format!("{name}:p1"),
            name: Some(name.into()),
            agent: Some("codex".into()),
            cwd: Some("/repos/weather".into()),
            state,
            tokens: tokens
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            ..AgentInfo::default()
        };
        let snapshot = |state: AgentState| SessionSnapshot {
            panes: vec![PaneInfo {
                pane_id: "corgi:p1".into(),
                label: Some("Corgi".into()),
                ..PaneInfo::default()
            }],
            agents: vec![
                AgentInfo {
                    agent_session: Some(AgentSession {
                        value: "supervisor-session".into(),
                        ..AgentSession::default()
                    }),
                    ..agent(
                        "supervisor-weather",
                        AgentState::Idle,
                        &[(SUPERVISOR_TOKEN, &marker("", Some("supervisor-session")))],
                    )
                },
                agent(
                    "w-request",
                    state,
                    &[(CORGI_REQUEST_TOKEN, "20261008-w-request")],
                ),
                agent(
                    "w-bridge",
                    state,
                    &[(STATUS_LINE_SESSION_TOKEN, "f44d6b29-bridge")],
                ),
                // Herdr's detector taking a terminal for an agent.
                agent("detected", state, &[]),
                agent("blank", state, &[(STATUS_LINE_SESSION_TOKEN, " ")]),
                // Corgi's own dashboard, whatever marks its pane carries.
                AgentInfo {
                    pane_id: "corgi:p1".into(),
                    ..agent(
                        "dashboard",
                        state,
                        &[(CORGI_REQUEST_TOKEN, "20261008-dashboard")],
                    )
                },
            ],
            ..SessionSnapshot::default()
        };
        let mut app = test_app();
        let mut waker = SupervisorWaker::in_dir(&scratch);

        app.install_snapshot(snapshot(Working));
        let names: Vec<_> = app
            .agents
            .iter()
            .map(|agent| agent.info.display_name())
            .collect();
        assert_eq!(names.len(), 3, "{names:?}");
        for name in ["supervisor-weather", "w-request", "w-bridge"] {
            assert!(names.contains(&name), "{name} missing from {names:?}");
        }
        let fleet = fleet_rows(&app, "/repos/weather");
        for name in ["w-request", "w-bridge"] {
            assert!(
                fleet
                    .iter()
                    .any(|row| row.starts_with(&format!("{name}\tworker\t"))),
                "{name} missing from {fleet:?}"
            );
        }
        assert!(wake_refresh(&mut waker, &app.agents, None, false).is_empty());

        app.install_snapshot(snapshot(Done));
        assert_eq!(
            wake_refresh(&mut waker, &app.agents, None, false),
            [
                "supervisor-weather: [Corgi] w-bridge is done. Run: /opt/corgi report w-bridge\n\
                 [Corgi] w-request is done. Run: /opt/corgi report w-request"
            ]
        );
    }

    #[test]
    fn a_stop_while_no_dashboard_ran_is_reported_once_when_one_starts_again() {
        use AgentState::{Done, Idle, Working};
        let scratch = ScratchDir::new("waker-restart");
        let report = "### Report\n- Result: done";
        let mut first = SupervisorWaker::in_dir(&scratch);
        let working = [
            weather_supervisor(Idle),
            weather_worker("w-forecast", Working, 4, true),
        ];
        assert!(wake_refresh(&mut first, &working, None, false).is_empty());
        drop(first);
        // The worker finishes while no dashboard runs.
        let done = [
            weather_supervisor(Idle),
            weather_worker("w-forecast", Done, 6, true),
        ];
        let mut second = SupervisorWaker::in_dir(&scratch);
        assert_eq!(
            wake_refresh(&mut second, &done, Some(report), false),
            [
                "supervisor-weather: [Corgi] w-forecast is done. Its report follows, quoted, so you \
                 need not run report for it:\n> ### Report\n> - Result: done\n\
                 [Corgi] End of w-forecast's report."
            ]
        );
        assert!(wake_refresh(&mut second, &done, Some(report), false).is_empty());
        drop(second);
        // A third dashboard finds nothing new and nothing undelivered.
        let mut third = SupervisorWaker::in_dir(&scratch);
        assert!(wake_refresh(&mut third, &done, Some(report), false).is_empty());
    }

    #[test]
    fn a_wake_held_back_by_a_draft_when_the_dashboard_closed_goes_out_once_after() {
        use AgentState::{Done, Idle, Working};
        let scratch = ScratchDir::new("waker-draft-restart");
        let mut first = SupervisorWaker::in_dir(&scratch);
        let agents = |state, change| {
            [
                weather_supervisor(Idle),
                weather_worker("w-radar", state, change, false),
            ]
        };
        assert!(wake_refresh(&mut first, &agents(Working, 1), None, true).is_empty());
        assert!(wake_refresh(&mut first, &agents(Done, 2), Some("all done"), true).is_empty());
        drop(first);
        let mut second = SupervisorWaker::in_dir(&scratch);
        // Still the user's draft: nothing yet.
        assert!(wake_refresh(&mut second, &agents(Done, 2), None, true).is_empty());
        // Not the supervisor's own worker: the line points at the report.
        assert_eq!(
            wake_refresh(&mut second, &agents(Done, 2), None, false),
            ["supervisor-weather: [Corgi] w-radar is done. Run: /opt/corgi report w-radar"]
        );
        assert!(wake_refresh(&mut second, &agents(Done, 2), None, false).is_empty());
        let inbox = Inbox::read(&scratch.join("weather"));
        assert_eq!(
            inbox.newest_report("w-radar").unwrap().report.as_deref(),
            Some("all done")
        );
    }

    #[test]
    fn a_dashboard_taking_over_the_wake_lock_resends_nothing_delivered() {
        use AgentState::{Done, Idle, Working};
        let scratch = ScratchDir::new("waker-takeover");
        let lock = scratch.join("wake.lock");
        let mut leader = SupervisorWaker::in_dir(&scratch);
        let mut other = SupervisorWaker::in_dir(&scratch);
        assert!(leader.lead(&lock));
        assert!(!other.lead(&lock));
        let agents = |state, change| {
            [
                weather_supervisor(Idle),
                weather_worker("w-radar", state, change, false),
            ]
        };
        wake_refresh(&mut leader, &agents(Working, 1), None, false);
        assert_eq!(
            wake_refresh(&mut leader, &agents(Done, 2), None, false).len(),
            1
        );
        drop(leader);
        assert!(other.lead(&lock));
        assert!(wake_refresh(&mut other, &agents(Done, 2), None, false).is_empty());
        // It goes on from the leader's record: the next stop is news.
        wake_refresh(&mut other, &agents(Working, 3), None, false);
        assert_eq!(
            wake_refresh(&mut other, &agents(Done, 4), None, false).len(),
            1
        );
    }

    #[test]
    fn an_own_worker_that_came_and_finished_while_no_dashboard_ran_is_reported_once() {
        use AgentState::{Done, Idle, Working};
        let scratch = ScratchDir::new("waker-appeared");
        let mut first = SupervisorWaker::in_dir(&scratch);
        wake_refresh(
            &mut first,
            &[
                weather_supervisor(Idle),
                weather_worker("w-radar", Working, 1, true),
            ],
            None,
            false,
        );
        drop(first);
        let agents = [
            weather_supervisor(Idle),
            weather_worker("w-radar", Working, 1, true),
            weather_worker("w-new", Done, 5, true),
            weather_worker("w-users", Done, 5, false),
            weather_worker("w-silent", Idle, 5, true),
            weather_worker("w-unstarted", Idle, 5, true),
            weather_worker("w-screen", Done, 5, true),
        ];
        let mut second = SupervisorWaker::in_dir(&scratch);
        // Only a screen for the last two: one never started, one is done.
        let mut report = |agent: &DashboardAgent| match agent.info.name.as_deref() {
            Some("w-silent") => None,
            Some("w-unstarted" | "w-screen") => Some(Captured {
                text: "$ claude".into(),
                transcript: false,
            }),
            _ => Some(said("short report")),
        };
        assert_eq!(
            second.observe(&agents, Path::new("/opt/corgi"), &mut report),
            None
        );
        assert_eq!(
            second.pending_for("/repos/weather"),
            [
                "[Corgi] w-new is done. Its report follows, quoted, so you need not run report \
                 for it:\n> short report\n[Corgi] End of w-new's report."
                    .to_string(),
                "[Corgi] w-screen is done. Its report follows, quoted, so you need not run \
                 report for it:\n> $ claude\n[Corgi] End of w-screen's report."
                    .to_string()
            ]
        );
        // Without any record, as at a first start, nothing found resting is news.
        let fresh = ScratchDir::new("waker-appeared-fresh");
        let mut third = SupervisorWaker::in_dir(&fresh);
        assert_eq!(
            third.observe(&agents, Path::new("/opt/corgi"), &mut report),
            None
        );
        assert!(third.pending_for("/repos/weather").is_empty());
    }

    #[test]
    fn an_own_workers_stop_while_no_supervisor_runs_waits_for_the_next_supervisor() {
        use AgentState::{Done, Idle, Working};
        let scratch = ScratchDir::new("waker-no-corgi");
        let mut waker = SupervisorWaker::in_dir(&scratch);
        let workers = |state, change| {
            [
                weather_worker("w-radar", state, change, true),
                weather_worker("w-users", state, change, false),
            ]
        };
        wake_refresh(&mut waker, &workers(Working, 1), None, false);
        assert!(
            wake_refresh(&mut waker, &workers(Done, 2), Some("radar report"), false).is_empty()
        );
        // A supervisor starts: it hears of the worker it spawned, not the user's.
        let mut agents = workers(Done, 2).to_vec();
        agents.push(weather_supervisor(Idle));
        assert_eq!(
            wake_refresh(&mut waker, &agents, None, false),
            [
                "supervisor-weather: [Corgi] w-radar is done. Its report follows, quoted, so you need \
              not run report for it:\n> radar report\n[Corgi] End of w-radar's report."
            ]
        );
    }

    #[test]
    fn a_long_report_stays_in_the_inbox_and_the_wake_points_to_it() {
        use AgentState::{Done, Idle, Working};
        let scratch = ScratchDir::new("waker-long-report");
        let long = "x".repeat(inbox::INLINE_REPORT_MAX + 10);
        let mut waker = SupervisorWaker::in_dir(&scratch);
        let agents = |state, change| {
            [
                weather_supervisor(Idle),
                weather_worker("w-radar", state, change, true),
            ]
        };
        wake_refresh(&mut waker, &agents(Working, 1), None, false);
        assert_eq!(
            wake_refresh(&mut waker, &agents(Done, 2), Some(&long), false),
            ["supervisor-weather: [Corgi] w-radar is done. Run: /opt/corgi report w-radar"]
        );
        let dir = scratch.join("weather");
        let inbox = Inbox::read(&dir);
        assert_eq!(
            inbox.newest_report("w-radar").unwrap().report_text(&dir),
            Some(long)
        );
    }

    #[test]
    fn a_notified_item_is_delivered_with_the_wakes() {
        use AgentState::Idle;
        let scratch = ScratchDir::new("waker-notify");
        let mut waker = SupervisorWaker::in_dir(&scratch);
        inbox::append(
            &scratch.join("weather"),
            Item {
                project: "/repos/weather".into(),
                source: "notify".into(),
                kind: Kind::Note,
                text: "[Corgi] The nightly build failed.".into(),
                ..Item::default()
            },
        )
        .unwrap();
        // While the supervisor works, it waits.
        assert!(
            wake_refresh(
                &mut waker,
                &[weather_supervisor(AgentState::Working)],
                None,
                false
            )
            .is_empty()
        );
        assert_eq!(
            wake_refresh(&mut waker, &[weather_supervisor(Idle)], None, false),
            ["supervisor-weather: [Corgi] The nightly build failed."]
        );
        assert!(wake_refresh(&mut waker, &[weather_supervisor(Idle)], None, false).is_empty());
    }

    #[test]
    fn a_renamed_supervisor_receives_wakes_and_keeps_its_handover_session() {
        let scratch = ScratchDir::new("waker");
        use AgentState::{Done, Idle, Working};
        let root = "/repos/weather";
        let mut owner = project_agent("supervisor-weather", root, false, Idle);
        owner.info.agent_session = Some(crate::model::AgentSession {
            value: "s1".into(),
            ..Default::default()
        });
        owner
            .info
            .tokens
            .insert(SUPERVISOR_TOKEN.into(), supervisor::marker("", Some("s1")));
        owner.supervisor = supervisor::is_supervisor(&owner.info);
        let mut waker = SupervisorWaker::in_dir(&scratch);
        waker.observe(
            &[owner.clone(), project_agent("worker", root, false, Working)],
            Path::new("/opt/corgi"),
            |_| None,
        );
        owner.info.name = Some("renamed-task".into());
        owner.supervisor = supervisor::is_supervisor(&owner.info);
        let agents = [owner.clone(), project_agent("worker", root, false, Done)];
        waker.observe(&agents, Path::new("/opt/corgi"), |_| None);
        let mut recipients = Vec::new();
        waker.deliver(
            &agents,
            |_| false,
            |name, _| {
                recipients.push(name.to_string());
                true
            },
        );
        assert_eq!(recipients, ["renamed-task"]);

        owner.context_percent = Some(80);
        let actions = waker.hand_over(&[owner], 50, |_| None, 100, |_, _| false, |_| false);
        assert_eq!(actions.len(), 1);
        let action = &actions[0];
        assert_eq!(action.name, "renamed-task");
        assert_eq!(action.session, "s1");
        assert_eq!(
            action.step,
            Some(supervisor::HandoverStep::Ask(
                100,
                supervisor::Trigger::Full(50)
            ))
        );
        let (socket, server) = crate::test_support::fake_herdr("renamed-handover", |listener| {
            crate::test_support::answer(&listener, |request| {
                assert_eq!(request["method"], "pane.report_metadata");
                assert_eq!(request["params"]["tokens"][SUPERVISOR_TOKEN], "session:s1");
                assert_eq!(
                    request["params"]["tokens"][supervisor::HANDOVER_TOKEN],
                    "100 s1"
                );
                serde_json::json!({"result": {"type": "ok"}})
            });
        });
        report_tokens(
            &HerdrClient::from_socket_path(&socket),
            action,
            Some("100 s1"),
        );
        server.join().unwrap();
        fs::remove_file(socket).unwrap();
    }

    #[test]
    fn a_scratch_agent_wakes_no_supervisor_and_none_is_handed_over_in_home() {
        let dir = ScratchDir::new("waker");
        use AgentState::{Done, Working};
        let scratch = |name: &str, supervisor: bool, state: AgentState| DashboardAgent {
            scratch: true,
            info: AgentInfo {
                agent_session: Some(crate::model::AgentSession {
                    value: format!("{name}-session"),
                    ..Default::default()
                }),
                ..project_agent(name, "/home/me", supervisor, state).info
            },
            context_percent: Some(99),
            ..project_agent(name, "/home/me", supervisor, state)
        };
        let mut waker = SupervisorWaker::in_dir(&dir);
        // Even beside a supervisor someone started in home before it was
        // refused there, a scratch agent that stops is nobody's news.
        let agents = |state| {
            [
                scratch("corgi-me", true, Done),
                scratch("scratch", false, state),
            ]
        };
        waker.observe(&agents(Done), Path::new("/opt/corgi"), |_| None);
        waker.observe(&agents(Working), Path::new("/opt/corgi"), |_| None);
        waker.observe(&agents(Done), Path::new("/opt/corgi"), |_| None);
        assert!(waker.pending_for("/home/me").is_empty());
        let actions = waker.hand_over(&agents(Done), 1, |_| Some(0), 1_000, |_, _| true, |_| false);
        assert!(actions.is_empty());
        assert!(waker.handovers.is_empty());
    }

    #[test]
    fn a_wake_waits_while_the_user_writes_in_the_supervisors_input_box() {
        use AgentState::{Done, Working};
        let screen = |name: &str| {
            fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("src/app/fixtures/input_box")
                    .join(name),
            )
            .map_err(anyhow::Error::from)
        };
        for (harness, draft, empty) in [
            ("claude", "claude_draft.ansi", "claude_suggestion.ansi"),
            ("claude", "claude_pasted_text.ansi", "claude_empty.ansi"),
            (
                "codex",
                "codex_multiline_draft.ansi",
                "codex_placeholder.ansi",
            ),
        ] {
            let corgi_bin = Path::new("/opt/corgi");
            let scratch = ScratchDir::new("waker");
            let mut waker = SupervisorWaker::in_dir(&scratch);
            let mut sent = Vec::new();
            let mut refresh = |worker, shown: Result<String>| {
                let mut supervisor =
                    project_agent("supervisor-weather", "/repos/weather", true, Done);
                supervisor.info.agent = Some(harness.into());
                let agents = [
                    supervisor,
                    project_agent("w-forecast", "/repos/weather", false, worker),
                ];
                waker.observe(&agents, corgi_bin, |_| None);
                let mut shown = Some(shown);
                waker.deliver(
                    &agents,
                    |supervisor| {
                        let screen = shown.take().expect("one read per refresh");
                        screen_holds_draft(screen, &supervisor.info.harness())
                    },
                    |_, text| {
                        sent.push(text.to_string());
                        true
                    },
                );
                sent.len()
            };
            assert_eq!(refresh(Working, screen(draft)), 0);
            // The worker stops while the user writes: the wake waits.
            assert_eq!(refresh(Done, screen(draft)), 0, "{draft}");
            assert_eq!(refresh(Done, screen(draft)), 0, "{draft}");
            // The box is empty, or shows only what the harness suggests.
            assert_eq!(refresh(Done, screen(empty)), 1, "{empty}");
            assert_eq!(refresh(Done, screen(empty)), 1, "sent once");
            assert_eq!(
                sent,
                ["[Corgi] w-forecast is done. Run: /opt/corgi report w-forecast"]
            );
        }
    }

    #[test]
    fn a_screen_that_cannot_be_read_or_understood_holds_nothing_back() {
        let unreadable = Err(anyhow::anyhow!("agent_not_found"));
        assert!(!screen_holds_draft(unreadable, &Harness::Claude));
        let draft = "────\n❯ fix it\n────".to_string();
        assert!(screen_holds_draft(Ok(draft.clone()), &Harness::Claude));
        assert!(!screen_holds_draft(Ok(draft), &Harness::Gemini));
        assert!(!screen_holds_draft(Ok("$ ".into()), &Harness::Codex));
    }

    #[test]
    fn a_handover_request_waits_while_the_user_writes_in_the_supervisors_input_box() {
        let scratch = ScratchDir::new("waker");
        use crate::supervisor::{HandoverStep::Ask, Trigger::Full};
        let mut supervisor = project_agent(
            "supervisor-weather",
            "/repos/weather",
            true,
            AgentState::Idle,
        );
        supervisor.info.agent_session = Some(crate::model::AgentSession {
            value: "s1".into(),
            ..Default::default()
        });
        supervisor.context_percent = Some(70);
        let mut waker = SupervisorWaker::in_dir(&scratch);
        let mut steps = |now, drafting: bool| {
            waker
                .hand_over(
                    std::slice::from_ref(&supervisor),
                    50,
                    |_| None,
                    now,
                    |_, _| false,
                    |_| drafting,
                )
                .into_iter()
                .filter_map(|action| action.step)
                .collect::<Vec<_>>()
        };
        assert_eq!(steps(1, true), []);
        assert_eq!(steps(2, true), []);
        assert_eq!(steps(3, false), [Ask(3, Full(50))]);
    }

    #[test]
    fn the_replacement_waits_while_the_user_writes_in_the_supervisors_input_box() {
        let scratch = ScratchDir::new("waker");
        use crate::supervisor::{HandoverStep::Replace, Trigger::Full};
        use AgentState::{Done, Idle, Working};
        let screen = |name: &str| {
            fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("src/app/fixtures/input_box")
                    .join(name),
            )
            .map_err(anyhow::Error::from)
        };
        let supervisor = |state, change| {
            let mut agent = project_agent("supervisor-weather", "/repos/weather", true, state);
            agent.info.agent = Some("claude".into());
            agent.info.state_change_seq = change;
            agent.info.agent_session = Some(crate::model::AgentSession {
                value: "s1".into(),
                ..Default::default()
            });
            agent.context_percent = Some(70);
            agent
        };
        // The screen each refresh shows; `None` means it is never read.
        let run = |screens: Vec<(AgentState, u64, u64, Option<Result<String>>)>| {
            let mut waker = SupervisorWaker::in_dir(&scratch);
            let mut steps = Vec::new();
            for (state, change, now, mut shown) in screens {
                let agents = [supervisor(state, change)];
                steps.push(
                    waker
                        .hand_over(
                            &agents,
                            50,
                            |_| None,
                            now,
                            |_, _| true,
                            |supervisor| {
                                let screen = shown.take().expect("read only when due");
                                screen_holds_draft(screen, &supervisor.info.harness())
                            },
                        )
                        .into_iter()
                        .find_map(|action| action.step),
                );
            }
            steps
        };
        let empty = || Some(screen("claude_empty.ansi"));
        let draft = || Some(screen("claude_draft.ansi"));
        let late = 1_000;
        assert_eq!(
            run(vec![
                (Idle, 1, 10, empty()),
                (Working, 2, 11, None),
                // The note is there, but so is the user's draft, long past
                // the two minutes a request has to be taken up.
                (Done, 3, 12, draft()),
                (Done, 3, late, draft()),
                (Done, 3, late + 1, Some(screen("claude_suggestion.ansi"))),
            ]),
            [
                Some(crate::supervisor::HandoverStep::Ask(10, Full(50))),
                None,
                None,
                None,
                Some(Replace)
            ]
        );
        // A screen that cannot be read does not hold the replacement back.
        let unreadable = || Some(Err(anyhow::anyhow!("agent_not_found")));
        assert_eq!(
            run(vec![
                (Idle, 1, 10, empty()),
                (Working, 2, 11, None),
                (Done, 3, 12, unreadable()),
            ])[2],
            Some(Replace)
        );
    }

    #[test]
    fn wakes_a_handing_over_supervisor_did_not_get_reach_the_supervisor_that_takes_over() {
        let scratch = ScratchDir::new("waker");
        use crate::supervisor::{
            HandoverStep::{Ask, Replace},
            Trigger::Full,
        };
        use AgentState::{Done, Idle, Working};
        let corgi_bin = Path::new("/opt/corgi");
        let root = "/repos/weather";
        let mut waker = SupervisorWaker::in_dir(&scratch);
        let mut sent = Vec::new();
        let mut steps = Vec::new();
        let supervisor = |session: &str, state, change, percent| DashboardAgent {
            info: AgentInfo {
                state_change_seq: change,
                agent_session: Some(crate::model::AgentSession {
                    value: session.into(),
                    ..Default::default()
                }),
                ..project_agent("supervisor-weather", root, true, state).info
            },
            context_percent: Some(percent),
            ..project_agent("supervisor-weather", root, true, state)
        };
        let worker = |name: &str, state, change| DashboardAgent {
            info: AgentInfo {
                state_change_seq: change,
                ..project_agent(name, root, false, state).info
            },
            ..project_agent(name, root, false, state)
        };
        let mut refresh = |waker: &mut SupervisorWaker, agents: &[DashboardAgent], note: bool| {
            waker.observe(agents, corgi_bin, |_| None);
            steps.extend(
                waker
                    .hand_over(agents, 50, |_| None, 1_000, |_, _| note, |_| false)
                    .into_iter()
                    .filter_map(|action| Some((action.name, action.step?))),
            );
            waker.deliver(
                agents,
                |_| false,
                |supervisor, text| {
                    sent.push(format!("{supervisor}: {text}"));
                    true
                },
            );
        };
        let forecast = |state, change| worker("w-forecast", state, change);
        refresh(
            &mut waker,
            &[supervisor("s1", Idle, 1, 70), forecast(Working, 1)],
            false,
        );
        // The handover turn runs, and a worker stops meanwhile.
        refresh(
            &mut waker,
            &[supervisor("s1", Working, 2, 71), forecast(Done, 2)],
            false,
        );
        // The turn ends with a note: the supervisor is idle, but the wake waits.
        refresh(
            &mut waker,
            &[supervisor("s1", Done, 3, 72), forecast(Done, 2)],
            true,
        );
        // The old supervisor has exited, and another worker stops in the gap.
        refresh(
            &mut waker,
            &[forecast(Done, 2), worker("w-radar", Working, 1)],
            true,
        );
        refresh(
            &mut waker,
            &[forecast(Done, 2), worker("w-radar", Done, 2)],
            true,
        );
        // The successor is at its first prompt when the replacement reports.
        let successor = [
            supervisor("s2", Working, 1, 3),
            forecast(Done, 2),
            worker("w-radar", Done, 2),
        ];
        refresh(&mut waker, &successor, false);
        waker.replaced(root, true, &successor);
        refresh(&mut waker, &successor, false);
        let rested = [
            supervisor("s2", Done, 2, 4),
            forecast(Done, 2),
            worker("w-radar", Done, 2),
        ];
        refresh(&mut waker, &rested, false);
        // One prompt, to the successor, once its first turn is over.
        assert_eq!(
            steps,
            [
                ("supervisor-weather".to_string(), Ask(1_000, Full(50))),
                ("supervisor-weather".to_string(), Replace)
            ]
        );
        assert_eq!(
            sent,
            [
                "supervisor-weather: [Corgi] w-forecast is done. Run: /opt/corgi report w-forecast\n\
              [Corgi] w-radar is done. Run: /opt/corgi report w-radar"
            ]
        );
    }

    #[test]
    fn a_supervisor_whose_turn_ends_without_a_note_keeps_its_session_and_gets_its_wakes() {
        let scratch = ScratchDir::new("waker");
        use crate::supervisor::{
            HandoverStep::{Ask, NoNote},
            Trigger::Full,
        };
        use AgentState::{Done, Idle, Working};
        let corgi_bin = Path::new("/opt/corgi");
        let root = "/repos/weather";
        let mut waker = SupervisorWaker::in_dir(&scratch);
        let mut sent = Vec::new();
        let mut steps = Vec::new();
        let agents = |state, change, worker| {
            let mut supervisor = project_agent("supervisor-weather", root, true, state);
            supervisor.info.state_change_seq = change;
            supervisor.info.agent_session = Some(crate::model::AgentSession {
                value: "s1".into(),
                ..Default::default()
            });
            supervisor.context_percent = Some(80);
            let mut forecast = project_agent("w-forecast", root, false, worker);
            forecast.info.state_change_seq = change;
            [supervisor, forecast]
        };
        for (state, change, worker) in [(Idle, 1, Working), (Working, 2, Done), (Done, 3, Done)] {
            let agents = agents(state, change, worker);
            waker.observe(&agents, corgi_bin, |_| None);
            steps.extend(
                waker
                    .hand_over(&agents, 50, |_| None, 1_000, |_, _| false, |_| false)
                    .into_iter()
                    .filter_map(|action| action.step),
            );
            waker.deliver(
                &agents,
                |_| false,
                |supervisor, text| {
                    sent.push(format!("{supervisor}: {text}"));
                    true
                },
            );
        }
        assert_eq!(steps, [Ask(1_000, Full(50)), NoNote]);
        assert_eq!(
            sent,
            ["supervisor-weather: [Corgi] w-forecast is done. Run: /opt/corgi report w-forecast"]
        );
    }

    #[test]
    fn an_idle_supervisor_is_handed_over_from_the_baseline_its_pane_keeps() {
        let scratch = ScratchDir::new("waker");
        use crate::supervisor::{HandoverStep::Ask, Trigger::Idle};
        use AgentState::{Done, Idle as Resting, Working};
        let root = "/repos/weather";
        let supervisor = |state, change, tokens, baseline: Option<&str>| {
            let mut agent = project_agent("supervisor-weather", root, true, state);
            agent.info.agent = Some("codex".into());
            agent.info.state_change_seq = change;
            agent.info.agent_session = Some(crate::model::AgentSession {
                value: "s1".into(),
                ..Default::default()
            });
            if let Some(baseline) = baseline {
                agent
                    .info
                    .tokens
                    .insert(supervisor::BASELINE_TOKEN.into(), baseline.into());
            }
            agent.context_percent = Some(10);
            agent.context_tokens = Some(tokens);
            agent
        };
        let mut waker = SupervisorWaker::in_dir(&scratch);
        let mut refresh = |agents: &[DashboardAgent], now| {
            waker
                .hand_over(
                    agents,
                    50,
                    |harness| (*harness == Harness::Codex).then_some(60),
                    now,
                    |_, _| true,
                    |_| false,
                )
                .into_iter()
                .map(|action| (action.step, action.baseline))
                .collect::<Vec<_>>()
        };
        // The first turn ends: its baseline goes on the pane, once.
        assert_eq!(
            refresh(&[supervisor(Done, 2, 30_000, None)], 0),
            [(None, Some(30_000))]
        );
        assert_eq!(
            refresh(&[supervisor(Done, 2, 30_000, Some("30000 s1"))], 1),
            []
        );
        // A later turn doubles it; a worker of this project, then only one
        // of another project's, is at work while it rests.
        let grown = |state| supervisor(state, 4, 60_000, Some("30000 s1"));
        let worker = |root| project_agent("w-forecast", root, false, Working);
        assert_eq!(refresh(&[grown(Done), worker(root)], 10), []);
        assert_eq!(refresh(&[grown(Done), worker(root)], 70), []);
        assert_eq!(
            refresh(&[grown(Resting), worker("/repos/corgi")], 71),
            [(Some(Ask(71, Idle(1))), Some(30_000))]
        );
    }

    #[test]
    fn a_codex_supervisor_is_succeeded_in_its_pane_on_the_model_and_effort_it_was_launched_with() {
        let action = HandoverAction {
            step: Some(crate::supervisor::HandoverStep::Replace),
            baseline: None,
            token: None,
            root: "/repos/weather".into(),
            name: "supervisor-weather".into(),
            pane_id: "w9:p1".into(),
            session: "019a-thread".into(),
            harness: Harness::Codex,
            change: 12,
        };
        let plan = successor_plan(
            &action,
            crate::supervisor::Launch {
                kind: "codex".into(),
                model: "gpt-6-astra".into(),
                effort: "high".into(),
                extra_args: vec!["--search".into()],
            },
        );
        assert_eq!(plan.name, "supervisor-weather");
        assert_eq!(
            plan.role,
            Role::Supervisor {
                handover_pane: Some("w9:p1".into())
            }
        );
        assert_eq!(plan.project, Path::new("/repos/weather"));
        assert!(
            plan.prompt.is_empty(),
            "the takeover prompt replaces any task"
        );
        let args = launch_args(&plan);
        assert_eq!(
            &args[..5],
            [
                "--model",
                "gpt-6-astra",
                "-c",
                "model_reasoning_effort=high",
                "--search"
            ]
        );
        assert_eq!(action.harness.exit_command(), "/quit");
        // A Claude supervisor launched on its harness's defaults stays on them.
        let claude = successor_plan(
            &HandoverAction {
                harness: Harness::Claude,
                ..action
            },
            crate::supervisor::Launch {
                kind: "claude".into(),
                ..crate::supervisor::Launch::default()
            },
        );
        assert!(launch_args(&claude).is_empty());
    }

    #[test]
    fn one_dashboard_per_herdr_socket_wakes_supervisors() {
        let scratch = ScratchDir::new("waker");
        let lock = std::env::temp_dir()
            .join(format!("corgi-wake-test-{}", std::process::id()))
            .join("herdr.lock");
        let mut first = SupervisorWaker::in_dir(&scratch);
        let mut second = SupervisorWaker::in_dir(&scratch);
        assert!(first.lead(&lock));
        assert!(!second.lead(&lock));
        assert!(first.lead(&lock));
        // When the leader goes, another dashboard takes over.
        drop(first);
        assert!(second.lead(&lock));
        fs::remove_dir_all(lock.parent().expect("lock dir")).ok();
        assert_eq!(
            wake_lock_path(Path::new("/tmp/herdr.sock"))
                .and_then(|path| path.file_name().map(|name| name.to_owned())),
            Some("_tmp_herdr_sock.lock".into())
        );
    }

    // ---- Adversarial review tests (throwaway; not for the branch) ----

    /// Herdr persists each pane's id and agent name in session.json and
    /// restores them after a server restart, but not `state_change_seq`, which
    /// counts from the start again. The dashboard (a Herdr plugin pane) restarts
    /// with it. The worker's next stop gets the same wake id as an earlier one
    /// still in the inbox, so `inbox::append` drops it as already recorded.
    #[test]
    fn a_stop_after_herdr_restarts_and_counts_state_changes_again_wakes() {
        use AgentState::{Done, Idle, Working};
        let scratch = ScratchDir::new("review-seq-reset");
        let agents = |state, change| {
            [
                weather_supervisor(Idle),
                weather_worker("w-radar", state, change, false),
            ]
        };
        let mut before = SupervisorWaker::in_dir(&scratch);
        wake_refresh(&mut before, &agents(Idle, 1), None, false);
        wake_refresh(&mut before, &agents(Working, 2), None, false);
        assert_eq!(
            wake_refresh(&mut before, &agents(Done, 3), None, false).len(),
            1
        );
        drop(before);
        // Herdr restarts: same pane id and name, seq counted from 1 again.
        let mut after = SupervisorWaker::in_dir(&scratch);
        wake_refresh(&mut after, &agents(Idle, 1), None, false);
        // The supervisor steers the worker; it works and stops again.
        wake_refresh(&mut after, &agents(Working, 2), None, false);
        assert_eq!(
            wake_refresh(&mut after, &agents(Done, 3), None, false),
            ["supervisor-weather: [Corgi] w-radar is done. Run: /opt/corgi report w-radar"],
            "the second stop never reaches the supervisor"
        );
    }

    /// A wake whose append fails still advances the saved record, so the
    /// stop is never added once the inbox is writable again.
    #[test]
    fn a_wake_that_could_not_be_added_is_added_once_the_inbox_is_writable() {
        use AgentState::{Done, Idle, Working};
        let scratch = ScratchDir::new("review-append-fails");
        let agents = |state, change| {
            [
                weather_supervisor(Idle),
                weather_worker("w-radar", state, change, false),
            ]
        };
        let mut waker = SupervisorWaker::in_dir(&scratch);
        wake_refresh(&mut waker, &agents(Working, 1), None, false);
        // The state directory cannot be created (stands in for a full disk,
        // a permission problem, a failed migration...).
        fs::write(scratch.join("weather"), "not a directory").unwrap();
        assert!(
            waker
                .observe(&agents(Done, 2), Path::new("/opt/corgi"), |_| None)
                .is_some()
        );
        fs::remove_file(scratch.join("weather")).unwrap();
        assert_eq!(
            wake_refresh(&mut waker, &agents(Done, 2), None, false),
            ["supervisor-weather: [Corgi] w-radar is done. Run: /opt/corgi report w-radar"],
            "the stop is lost for good"
        );
    }

    /// Two projects with the same directory name share one state directory,
    /// so one inbox; delivery types every item of that inbox into whichever
    /// supervisor comes first, and records them delivered. On main the queue was
    /// keyed by the full project root.
    #[test]
    fn a_wake_reaches_the_supervisor_of_its_own_project_when_two_share_a_name() {
        use AgentState::{Done, Idle, Working};
        let scratch = ScratchDir::new("review-same-basename");
        let agents = |state, change| {
            let mut worker = project_agent("w-oss", "/oss/web", false, state);
            worker.info.state_change_seq = change;
            [
                project_agent("corgi-work-web", "/work/web", true, Idle),
                project_agent("corgi-oss-web", "/oss/web", true, Idle),
                worker,
            ]
        };
        let mut waker = SupervisorWaker::in_dir(&scratch);
        wake_refresh(&mut waker, &agents(Working, 1), None, false);
        assert_eq!(
            wake_refresh(&mut waker, &agents(Done, 2), None, false),
            ["corgi-oss-web: [Corgi] w-oss is done. Run: /opt/corgi report w-oss"]
        );
    }

    /// Nothing bounds the one prompt typed into the supervisor's box: every own
    /// worker's report up to 4 KB each goes inline. Eight workers stopping
    /// during one long supervisor turn (or one dashboard downtime) make a ~33 KB
    /// agent.prompt. `corgi inbox` caps itself at 16 KB of reports.
    #[test]
    fn the_typed_prompt_stays_within_a_budget() {
        use AgentState::{Done, Idle, Working};
        let scratch = ScratchDir::new("review-prompt-size");
        let report = "r".repeat(inbox::INLINE_REPORT_MAX);
        let agents = |supervisor, state, change| {
            let mut agents = vec![weather_supervisor(supervisor)];
            for n in 0..8 {
                agents.push(weather_worker(&format!("w-{n}"), state, change, true));
            }
            agents
        };
        let mut waker = SupervisorWaker::in_dir(&scratch);
        wake_refresh(&mut waker, &agents(Working, Working, 1), None, false);
        assert!(
            wake_refresh(&mut waker, &agents(Working, Done, 2), Some(&report), false).is_empty()
        );
        let sent = wake_refresh(&mut waker, &agents(Idle, Done, 2), Some(&report), false);
        assert_eq!(sent.len(), 1);
        assert!(
            sent[0].len() <= 16 * 1024,
            "one agent.prompt of {} bytes",
            sent[0].len()
        );
    }
}
