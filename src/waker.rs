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
    inbox::{self, Inbox, Item, Kind},
    job::Job,
    launch::{CORGI_REQUEST_TOKEN, Checkout, LaunchPlan, Role},
    model::{AgentState, DashboardAgent},
    paths::{dir_name, socket_state_file},
    session::{SessionFacts, TurnState},
    supervisor,
};

pub mod hc;

/// How long a supervisor that handed over has to exit, polled this often.
const SUPERVISOR_EXIT_POLLS: usize = 60;
const SUPERVISOR_EXIT_POLL_DELAY: Duration = Duration::from_millis(250);
/// The pauses before each read again of a resting agent's transcript that
/// does not yet show how its turn ended, two seconds in all. Claude Code
/// writes its records about a tenth of a second after they happen, and Herdr
/// can see the turn end on the screen sooner.
const REPORT_REREADS: [Duration; 5] = [
    Duration::from_millis(150),
    Duration::from_millis(250),
    Duration::from_millis(350),
    Duration::from_millis(500),
    Duration::from_millis(750),
];

/// Wakes each project's supervisor when another agent of that project stops:
/// every agent whose project has a running supervisor counts, whoever started
/// it, and one the supervisor spawned counts also while the project has none, so
/// the next supervisor hears of it. Only a process holding the wake lock of its
/// Herdr socket follows the agents, puts each stop in the supervisor's inbox and
/// types the inbox into the supervisor's box, so a supervisor hears each stop
/// once. Herdr's hooks (`corgi hook`) and every open dashboard take the lock
/// in turn, for one pass each ([`SupervisorWaker::begin`] to
/// [`SupervisorWaker::end`]): what was seen of each agent and where each
/// handover stands is kept on disk, taken up at the start of a pass and
/// written at its end, so whoever comes next picks up where the last left
/// off. The same passes hand supervisors over to fresh sessions.
#[derive(Debug)]
pub struct SupervisorWaker {
    /// The wake lock, while a pass holds it.
    lock: Option<fs::File>,
    /// The wake lock to take in place of the Herdr socket's, as a test sets.
    lock_path: Option<PathBuf>,
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
    /// The record as last written or taken up, so an unchanged one is not
    /// written again.
    written: Option<String>,
    /// The handovers as last written or taken up.
    handovers_written: Option<String>,
    /// By state directory, the items typed into a supervisor's box whose
    /// delivery could not be recorded, so they are not typed in again while
    /// recording them is retried; dropped once the inbox shows them
    /// delivered.
    unrecorded: HashMap<PathBuf, HashSet<String>>,
    /// The inboxes as last read, by state directory.
    read: InboxCache,
    /// Each project's supervisor handover, by project root.
    handovers: HashMap<String, supervisor::Handover>,
    /// Replacements running in the background, by project root, each ending
    /// with its status line or error.
    replacements: Vec<(String, Job<Result<String>>)>,
    /// What `corgi pause` holds, as this pass took it up.
    paused: Paused,
    /// The file `corgi pause` keeps its pause in
    /// (`hc::pause::record_path`), and whether a pause was in force at the
    /// last pass.
    hc_pause: Option<PathBuf>,
    hc_paused: bool,
    /// The pauses before reading a report again ([`REPORT_REREADS`]).
    report_rereads: &'static [Duration],
}

/// What a pass holds back for the agents `corgi pause` stopped: the
/// terminals whose stops wake no one, and the projects whose supervisor's
/// handover record is kept while it is away.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Paused {
    panes: HashSet<String>,
    roots: HashSet<String>,
}

impl Default for SupervisorWaker {
    fn default() -> Self {
        Self {
            lock: None,
            lock_path: None,
            inboxes: Inboxes::default(),
            socket: None,
            agents: BTreeMap::new(),
            resumed: false,
            written: None,
            handovers_written: None,
            unrecorded: HashMap::new(),
            read: InboxCache::default(),
            handovers: HashMap::new(),
            replacements: Vec::new(),
            paused: Paused::default(),
            hc_pause: None,
            hc_paused: false,
            report_rereads: &REPORT_REREADS,
        }
    }
}

/// The supervisors' inboxes, a directory per project. The waker delivers
/// what is in them; the merge popup adds to one while a refresh may be
/// delivering from it in the background, which is safe because every write
/// to an inbox holds its lock.
#[derive(Debug, Clone, Default)]
pub struct Inboxes {
    /// The directory holding a directory per project, in place of Corgi's
    /// state directory, so that a test never touches the developer's own
    /// state.
    base: Option<PathBuf>,
}

impl Inboxes {
    /// Inboxes kept under `base`, a directory per project.
    #[cfg(test)]
    pub fn in_dir(base: &Path) -> Self {
        Self {
            base: Some(base.to_path_buf()),
        }
    }

    /// The state directory holding the inbox of `root`'s supervisor.
    pub fn dir(&self, root: &str) -> Option<PathBuf> {
        match &self.base {
            Some(base) => Some(base.join(dir_name(root).unwrap_or(supervisor::UNNAMED_PROJECT))),
            None => supervisor::state_dir(root).ok(),
        }
    }

    /// Adds `item` to the inbox of the supervisor of its project, to go out at a
    /// refresh once the supervisor is between turns.
    pub fn notify(&self, item: Item) -> Result<()> {
        let dir = self
            .dir(&item.project)
            .context("Corgi has no state directory")?;
        inbox::append(&dir, item)?;
        Ok(())
    }

    /// How many of `root`'s items have not been delivered.
    pub fn undelivered(&self, root: &str) -> usize {
        self.dir(root)
            .map_or(0, |dir| Inbox::read(&dir).undelivered_for(root).len())
    }

    /// Whether `root` has had a supervisor: one was prepared in its state
    /// directory, so an item added for it is read when it next runs.
    pub fn has_supervisor(&self, root: &str) -> bool {
        self.dir(root)
            .is_some_and(|dir| dir.join(supervisor::ROLE_FILE).is_file())
    }

    /// The lines waiting for `root`'s supervisor, each as it goes out alone.
    #[cfg(test)]
    pub fn pending_for(&self, root: &str) -> Vec<String> {
        let dir = self.dir(root).expect("inbox dir");
        Inbox::read(&dir)
            .undelivered_for(root)
            .into_iter()
            .map(|item| inbox::prompt_text(&[item]))
            .collect()
    }
}

/// The inboxes as last read, by state directory, each with the stamp its
/// file had then, so that an inbox nothing wrote to since is not read and
/// parsed again at every refresh. Every write to an inbox appends to it or
/// replaces it by a rename (compaction), whoever writes, so a file of the
/// same device, inode, length, modification and change time holds what it
/// held; the stamp is taken before the read, so a write during it only
/// makes the next refresh read again.
#[derive(Debug, Default)]
struct InboxCache {
    /// Each inbox with its file's stamp, `None` when that could not be
    /// taken.
    inboxes: HashMap<PathBuf, (Option<Option<InboxStamp>>, Inbox)>,
    /// How many times an inbox was read from disk.
    #[cfg(test)]
    reads: usize,
}

/// What [`InboxCache`] compares of an inbox file: its device, inode,
/// length, and modification and change times, or `None` when it does not
/// exist.
type InboxStamp = (u64, u64, u64, i64, i64, i64, i64);

impl InboxCache {
    /// The inbox in the state directory `dir`, as [`Inbox::read`] reads
    /// it, read again only when its file changed since the last time.
    fn read(&mut self, dir: &Path) -> &Inbox {
        use std::os::unix::fs::MetadataExt;
        let stamp = match fs::symlink_metadata(dir.join(inbox::INBOX_FILE)) {
            Ok(meta) => Some(Some((
                meta.dev(),
                meta.ino(),
                meta.len(),
                meta.mtime(),
                meta.mtime_nsec(),
                meta.ctime(),
                meta.ctime_nsec(),
            ))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(None),
            // Not known to be unchanged, so it is read every time.
            Err(_) => None,
        };
        let unchanged = stamp.is_some()
            && self
                .inboxes
                .get(dir)
                .is_some_and(|(seen, _)| *seen == stamp);
        if !unchanged {
            #[cfg(test)]
            {
                self.reads += 1;
            }
            self.inboxes
                .insert(dir.to_path_buf(), (stamp, Inbox::read(dir)));
        }
        &self.inboxes[dir].1
    }

    /// Forgets the inboxes not in `dirs`, those of projects without a
    /// running supervisor.
    fn retain(&mut self, dirs: &HashSet<PathBuf>) {
        self.inboxes.retain(|dir, _| dirs.contains(dir));
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
pub struct Captured {
    pub text: String,
    /// Whether it is the newest thing said in its transcript, rather than
    /// its screen.
    pub transcript: bool,
}

/// What reading a resting agent's report found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capture {
    /// The report to quote, or none.
    Found(Option<Captured>),
    /// The transcript does not yet show how the agent's turn ended, so its
    /// newest reply may be an older turn's: read it again.
    Behind,
}

impl From<Option<Captured>> for Capture {
    fn from(report: Option<Captured>) -> Self {
        Self::Found(report)
    }
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
    fn take_lock(&mut self, path: &Path) -> bool {
        if self.lock.is_none() {
            self.lock = fs::create_dir_all(path.parent().unwrap_or(path))
                .ok()
                .and_then(|()| fs::File::create(path).ok())
                .filter(|file| file.try_lock().is_ok());
        }
        self.lock.is_some()
    }

    /// Whether this waker holds the wake lock.
    pub fn leading(&self) -> bool {
        self.lock.is_some()
    }

    /// Whether a supervisor's replacement this waker started still runs.
    pub fn replacing(&self) -> bool {
        self.replacements
            .iter()
            .any(|(_, replacement)| replacement.is_working())
    }

    /// Takes up what the last pass left on disk: the agents followed and the
    /// handovers. A file that is missing or cannot be read leaves what this
    /// waker has, so a first pass starts from nothing and a damaged file
    /// costs only what it held.
    fn take_up(&mut self) {
        if let Some(text) = self
            .record_file()
            .and_then(|path| fs::read_to_string(path).ok())
            && let Ok(agents) = serde_json::from_str(&text)
        {
            self.agents = agents;
            self.written = Some(text);
        }
        if let Some(text) = self
            .handovers_file()
            .and_then(|path| fs::read_to_string(path).ok())
            && let Ok(handovers) = serde_json::from_str(&text)
        {
            self.handovers = handovers;
            self.handovers_written = Some(text);
        }
    }

    /// Where the handovers are kept: `wake/<socket>.handovers.json`.
    fn handovers_file(&self) -> Option<PathBuf> {
        match &self.inboxes.base {
            Some(base) => Some(base.join("handovers.json")),
            None => socket_state_file("wake", self.socket.as_deref()?, "handovers.json"),
        }
    }

    /// Writes the handovers, when they changed.
    fn save_handovers(&mut self) -> Result<()> {
        let Some(path) = self.handovers_file() else {
            return Ok(());
        };
        let sorted: BTreeMap<&String, &supervisor::Handover> = self.handovers.iter().collect();
        let text = serde_json::to_string(&sorted)?;
        if self.handovers_written.as_deref() == Some(text.as_str()) {
            return Ok(());
        }
        replace_file(&path, &text)?;
        self.handovers_written = Some(text);
        Ok(())
    }

    /// Whether `root`'s supervisor is handing over, so wakes for it wait for the
    /// supervisor that takes over.
    fn handing_over(&self, root: &str) -> bool {
        self.handovers
            .get(root)
            .is_some_and(supervisor::Handover::in_progress)
    }

    /// A waker that keeps its inboxes, its record, its handovers, its
    /// dirty flag and its wake lock under `base`, and reads a report again
    /// as often as any other but without pausing.
    #[cfg(test)]
    pub fn in_dir(base: &Path) -> Self {
        let mut waker = Self::default();
        waker.inboxes = Inboxes::in_dir(base);
        waker.lock_path = Some(base.join("wake.lock"));
        waker.report_rereads = &[Duration::ZERO; REPORT_REREADS.len()];
        waker
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
        replace_file(&path, &text)?;
        self.written = Some(text);
        Ok(())
    }

    /// Records the agents' states from one refresh, adding an item to the
    /// supervisor's inbox for each one that stopped, with the report `report`
    /// captures for one that rests. A report whose transcript is behind is
    /// read again, for the agents behind alone, after each of the pauses of
    /// `report_rereads`, and left out if it is behind still: the wake then
    /// names the command that prints it, rather than quote an older turn's
    /// reply. At the first refresh this dashboard
    /// leads, it takes up the record the one before left, so what changed
    /// while none led is news, and an agent the supervisor spawned that appeared
    /// and finished meanwhile is reported once, if it is done or has a
    /// report from its transcript. A stop that cannot be added is tried
    /// again at the next refresh. Returns the error of an inbox or the
    /// record that could not be written.
    pub fn observe<R: Into<Capture>>(
        &mut self,
        agents: &[DashboardAgent],
        corgi_bin: &Path,
        mut report: impl FnMut(&DashboardAgent) -> R,
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
                && !self.paused.panes.contains(pane)
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
        let mut captures: Vec<Capture> = stops
            .iter()
            .map(|stop| match stop.state {
                AgentState::Done | AgentState::Idle => report(stop.agent).into(),
                _ => Capture::Found(None),
            })
            .collect();
        for pause in self.report_rereads {
            if !captures.contains(&Capture::Behind) {
                break;
            }
            thread::sleep(*pause);
            for (stop, capture) in stops.iter().zip(&mut captures) {
                if *capture == Capture::Behind {
                    *capture = report(stop.agent).into();
                }
            }
        }
        let mut error = None;
        for (stop, capture) in stops.into_iter().zip(captures) {
            let report = match capture {
                Capture::Found(report) => report,
                Capture::Behind => None,
            };
            // One that only appeared needs something to show for it: its
            // screen may be one that never started.
            if stop.appeared
                && !(stop.state == AgentState::Done
                    || report.as_ref().is_some_and(|report| report.transcript))
            {
                continue;
            }
            let item = wake_item(&stop, report, corgi_bin);
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
    pub fn notify(&self, item: Item) -> Result<()> {
        self.inboxes.notify(item)
    }

    /// The lines waiting for `root`'s supervisor, each as it goes out alone.
    #[cfg(test)]
    pub fn pending_for(&self, root: &str) -> Vec<String> {
        self.inboxes.pending_for(root)
    }

    /// Advances each supervisor's handover by one refresh, with `percent` as
    /// the threshold (and its harness's token threshold, if it has one) and
    /// `cache` giving what is known of each harness's
    /// prompt cache (a harness without an idle time has none to wait for), and
    /// returns what to do about it. `note_since(root, at)` says whether
    /// `root`'s supervisor wrote its note since `at`, and `drafting(supervisor)`
    /// whether the user is writing in its input box, which holds a due
    /// request, or the replacement, back for a later refresh.
    fn hand_over(
        &mut self,
        agents: &[DashboardAgent],
        percent: u8,
        cache: impl Fn(&Harness) -> supervisor::Cache,
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
                tokens: supervisor::handover_tokens(&harness),
                cache: cache(&harness),
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
        let away = &self.paused.roots;
        self.handovers.retain(|root, handover| {
            live.contains(root.as_str()) || handover.in_progress() || away.contains(root)
        });
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
    pub fn deliver<D: Into<Delivery>>(
        &mut self,
        agents: &[DashboardAgent],
        mut drafting: impl FnMut(&DashboardAgent) -> bool,
        mut prompt: impl FnMut(&str, &str) -> D,
    ) -> Option<String> {
        let mut error = None;
        let mut roots = HashSet::new();
        let mut dirs = HashSet::new();
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
            dirs.insert(dir.clone());
            let inbox = self.read.read(&dir);
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
            {
                continue;
            }
            let ids: Vec<String> = pending.iter().map(|item| item.id.clone()).collect();
            match prompt(supervisor, &inbox::prompt_text(&pending)).into() {
                Delivery::Sent => {}
                // Nothing was typed, or it may have been: either way the
                // items stay undelivered, and go out at a later pass.
                Delivery::Refused | Delivery::Unknown => continue,
            }
            if let Err(failed) = inbox::mark_delivered(&dir, &ids, supervisor) {
                unrecorded.extend(ids);
                error = Some(format!(
                    "Could not record the delivery to {supervisor}: {failed:#}"
                ));
            }
        }
        self.read.retain(&dirs);
        error
    }
}

/// How typing an inbox into a supervisor's box went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Herdr typed it in.
    Sent,
    /// Herdr refused before typing anything: the agent was gone, not the
    /// pane's foreground process, blocked on a question, or ambiguous. The
    /// items may go out at a later pass.
    Refused,
    /// It may or may not have been typed (the connection failed, Herdr
    /// timed out or failed while writing): the items are quarantined
    /// rather than risk typing them twice.
    Unknown,
}

/// `true` is a prompt typed in, and `false` one that may have been: the
/// tests' prompts, which say only whether they went.
#[cfg(test)]
impl From<bool> for Delivery {
    fn from(sent: bool) -> Self {
        if sent { Self::Sent } else { Self::Unknown }
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
fn wake_item(stop: &Stop<'_>, report: Option<Captured>, corgi_bin: &Path) -> Item {
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
        source: "daemon".into(),
        kind: Kind::Wake,
        agent: Some(name.to_string()),
        state: Some(label),
        pane: Some(pane.clone()),
        change: Some(change),
        key: name.to_string(),
        text: supervisor::wake_message(corgi_bin, name, *state),
        own: own(agent),
        request: agent.info.tokens.get(CORGI_REQUEST_TOKEN).cloned(),
        said: report.as_ref().is_some_and(|report| report.transcript),
        report: report.map(|report| report.text),
        ..Item::default()
    }
}

/// The report a transcript's `facts` give, if they give one: the newest
/// reply, when it is the newest turn's. A transcript whose newest turn shows
/// no reply yet is behind, and one whose turn ended without one has no
/// report; neither falls back to the screen, which would show the turn
/// before as readily.
fn transcript_report(facts: &SessionFacts) -> Option<Capture> {
    match facts.turn {
        Some(TurnState::Open) => Some(Capture::Behind),
        Some(TurnState::Silent) => Some(Capture::Found(None)),
        Some(TurnState::Answered) | None => facts
            .report
            .as_ref()
            .filter(|entry| !entry.text.trim().is_empty())
            .map(|entry| {
                Capture::Found(Some(Captured {
                    text: entry.text.clone(),
                    transcript: true,
                }))
            }),
    }
}

/// Gives up the wake lock at once. Closing the file alone may not: a child
/// process spawned on another thread at that moment briefly holds a copy of
/// the descriptor, and with it the lock, so the next process would find it
/// still taken.
impl Drop for SupervisorWaker {
    fn drop(&mut self) {
        if let Some(lock) = self.lock.take() {
            let _ = lock.unlock();
        }
    }
}

/// Replaces the file at `path` with `text` whole, through a temporary file
/// and a rename, so a reader never sees half of it.
fn replace_file(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, text).with_context(|| format!("write {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
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
        checkout: Checkout::ProjectRoot,
        extra_args: launch.extra_args,
        role: Role::Supervisor {
            handover_pane: Some(action.pane_id.clone()),
        },
        request_id: None,
    }
}

/// The lock that makes one dashboard per Herdr socket the one that wakes
/// supervisors, named after the socket so that dashboards of other Herdr
/// sessions wake their own.
fn wake_lock_path(socket: &Path) -> Option<PathBuf> {
    socket_state_file("wake", socket, "lock")
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use crate::{
        launch::launch_args,
        model::{AgentInfo, AgentState, DashboardAgent},
        session::SessionReader,
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
    fn a_waker_taking_over_the_wake_lock_resends_nothing_delivered() {
        use AgentState::{Done, Idle, Working};
        let scratch = ScratchDir::new("waker-takeover");
        let mut leader = SupervisorWaker::in_dir(&scratch);
        let mut other = SupervisorWaker::in_dir(&scratch);
        assert!(leader.lead_hc());
        assert!(!other.lead_hc());
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
        assert!(other.lead_hc());
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

    /// `w-radar`, a worker of `/repos/weather` on `harness` whose session
    /// `s-radar` has its transcript at the returned path under `home`.
    fn transcribed_worker(
        home: &Path,
        harness: &str,
        state: AgentState,
        change: u64,
    ) -> (DashboardAgent, PathBuf) {
        let mut agent = weather_worker("w-radar", state, change, true);
        agent.info.agent = Some(harness.into());
        agent.info.cwd = Some("/repos/weather".into());
        agent.info.agent_session = Some(crate::model::AgentSession {
            value: "s-radar".into(),
            ..Default::default()
        });
        let dir = match harness {
            "claude" => home.join(".claude/projects/-repos-weather"),
            _ => home.join(".codex/sessions/2026/10/30"),
        };
        fs::create_dir_all(&dir).unwrap();
        let file = match harness {
            "claude" => "s-radar.jsonl",
            _ => "rollout-2026-10-30T12-00-00-s-radar.jsonl",
        };
        (agent, dir.join(file))
    }

    /// A turn that answered "Waiting for discovery.", then a `[supervisor]`
    /// prompt whose turn ran a command, as `harness` writes them.
    fn earlier_turns(harness: &str) -> Vec<&'static str> {
        match harness {
            "claude" => vec![
                r#"{"type":"user","message":{"content":"Run discovery."}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Waiting for discovery."}]}}"#,
                r#"{"type":"system","subtype":"turn_duration","durationMs":5}"#,
                r#"{"type":"user","message":{"content":"[supervisor] Noted. Your plan?"}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#,
                r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"a b"}]}}"#,
            ],
            _ => vec![
                r#"{"type":"event_msg","payload":{"type":"task_started"}}"#,
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"Run discovery."}}"#,
                r#"{"type":"event_msg","payload":{"type":"agent_message","message":"Waiting for discovery."}}"#,
                r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#,
                r#"{"type":"event_msg","payload":{"type":"task_started"}}"#,
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"[supervisor] Noted. Your plan?"}}"#,
                r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"ls\"]}"}}"#,
                r#"{"type":"response_item","payload":{"type":"function_call_output","output":"a b"}}"#,
            ],
        }
    }

    /// The reply "My plan: disable it.", as `harness` writes it.
    fn plan_reply(harness: &str) -> &'static str {
        match harness {
            "claude" => {
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"My plan: disable it."}]}}"#
            }
            _ => {
                r#"{"type":"event_msg","payload":{"type":"agent_message","message":"My plan: disable it."}}"#
            }
        }
    }

    /// The record that ends a turn, as `harness` writes it.
    fn turn_end(harness: &str) -> &'static str {
        match harness {
            "claude" => r#"{"type":"system","subtype":"turn_duration","durationMs":5}"#,
            _ => r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#,
        }
    }

    fn write_lines(path: &Path, lines: &[&str]) {
        fs::write(
            path,
            lines
                .iter()
                .map(|line| format!("{line}\n"))
                .collect::<String>(),
        )
        .unwrap();
    }

    /// One refresh whose reports `sessions` reads from the transcripts, as
    /// the dashboard and the hooks read them, with `late` added to `path`
    /// just after the first read, as a CLI writes its records a moment after
    /// Herdr sees its turn end. Returns what was typed into the supervisor's
    /// box and how often a report was read.
    fn transcript_refresh(
        waker: &mut SupervisorWaker,
        agents: &[DashboardAgent],
        sessions: &mut SessionReader,
        path: &Path,
        late: &[&str],
    ) -> (Vec<String>, usize) {
        let mut reads = 0;
        let read = |agent: &DashboardAgent| {
            reads += 1;
            let capture =
                transcript_report(&sessions.facts(&agent.info)).unwrap_or(Capture::Found(None));
            if reads == 1 && !late.is_empty() {
                let mut lines = fs::read_to_string(path).unwrap();
                lines.extend(late.iter().map(|line| format!("{line}\n")));
                fs::write(path, lines).unwrap();
            }
            capture
        };
        assert_eq!(waker.observe(agents, Path::new("/opt/corgi"), read), None);
        let mut sent = Vec::new();
        waker.deliver(
            agents,
            |_| false,
            |supervisor, text| {
                sent.push(format!("{supervisor}: {text}"));
                true
            },
        );
        (sent, reads)
    }

    const PLAN_QUOTED: &str = "supervisor-weather: [Corgi] w-radar is done. Its report follows, \
        quoted, so you need not run report for it:\n> My plan: disable it.\n[Corgi] End of \
        w-radar's report.";
    const NOTHING_QUOTED: &str =
        "supervisor-weather: [Corgi] w-radar is done. Run: /opt/corgi report w-radar";

    /// Waits for the turn that ended to show in `harness`'s transcript: the
    /// wake gets `turn`'s report when the transcript has gained `late` by the
    /// next read. Returns what was typed into the supervisor's box and how
    /// often the report was read.
    fn wake_after(harness: &str, turn: &[&str], late: &[&str]) -> (Vec<String>, usize) {
        let scratch = ScratchDir::new("waker-transcript");
        let home = scratch.join("home");
        let mut waker = SupervisorWaker::in_dir(&scratch.join("state"));
        let mut sessions = SessionReader::in_home(&home);
        let (working, path) = transcribed_worker(&home, harness, AgentState::Working, 1);
        let (done, _) = transcribed_worker(&home, harness, AgentState::Done, 2);
        let supervisor = weather_supervisor(AgentState::Idle);
        write_lines(&path, turn);
        transcript_refresh(
            &mut waker,
            &[supervisor.clone(), working],
            &mut sessions,
            &path,
            &[],
        );
        transcript_refresh(&mut waker, &[supervisor, done], &mut sessions, &path, late)
    }

    #[test]
    fn a_wake_quotes_the_reply_the_transcript_gains_after_the_stop_is_seen() {
        for harness in ["claude", "codex"] {
            let (sent, reads) = wake_after(
                harness,
                &earlier_turns(harness),
                &[plan_reply(harness), turn_end(harness)],
            );
            // Not the turn before's "Waiting for discovery.".
            assert_eq!(sent, [PLAN_QUOTED], "{harness}");
            assert_eq!(reads, 2, "{harness}");
        }
    }

    #[test]
    fn a_transcript_still_behind_after_the_rereads_wakes_without_a_quote() {
        for harness in ["claude", "codex"] {
            let (sent, reads) = wake_after(harness, &earlier_turns(harness), &[]);
            assert_eq!(sent, [NOTHING_QUOTED], "{harness}");
            assert_eq!(reads, 1 + REPORT_REREADS.len(), "{harness}");
        }
    }

    #[test]
    fn a_turn_of_tool_calls_alone_quotes_no_older_reply_and_waits_for_nothing() {
        for harness in ["claude", "codex"] {
            let mut turn = earlier_turns(harness);
            turn.push(turn_end(harness));
            let (sent, reads) = wake_after(harness, &turn, &[]);
            assert_eq!(sent, [NOTHING_QUOTED], "{harness}");
            assert_eq!(reads, 1, "{harness}");
        }
    }

    #[test]
    fn a_transcript_that_already_holds_the_reply_is_quoted_at_the_first_read() {
        for harness in ["claude", "codex"] {
            let mut turn = earlier_turns(harness);
            turn.push(plan_reply(harness));
            let (sent, reads) = wake_after(harness, &turn, &[]);
            assert_eq!(sent, [PLAN_QUOTED], "{harness}");
            assert_eq!(reads, 1, "{harness}");
        }
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
            inbox
                .newest_report("w-radar")
                .unwrap()
                .try_report_text(&dir)
                .unwrap(),
            Some(long.clone())
        );
        // It is kept for good too, named by its time and request id.
        let kept: Vec<_> = fs::read_dir(dir.join(inbox::KEPT_REPORTS_DIR))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(kept.len(), 1);
        assert!(kept[0].ends_with("-20261003-w-radar.md"), "{kept:?}");
        let text = fs::read_to_string(dir.join(inbox::KEPT_REPORTS_DIR).join(&kept[0])).unwrap();
        assert!(text.starts_with("# Report of w-radar (done), "));
        assert!(text.contains("\nRequest: 20261003-w-radar\nProject: /repos/weather\n\nxxx"));
        assert!(text.ends_with(&format!("{long}\n")));
    }

    #[test]
    fn the_report_of_an_agent_the_user_started_is_not_kept_for_good() {
        use AgentState::{Done, Idle, Working};
        let scratch = ScratchDir::new("waker-users-report");
        let mut waker = SupervisorWaker::in_dir(&scratch);
        let agents = |state, change| {
            [
                weather_supervisor(Idle),
                weather_worker("users-claude", state, change, false),
            ]
        };
        wake_refresh(&mut waker, &agents(Working, 1), None, false);
        wake_refresh(&mut waker, &agents(Done, 2), Some("my own session"), false);
        let dir = scratch.join("weather");
        assert!(Inbox::read(&dir).newest_report("users-claude").is_some());
        assert!(!dir.join(inbox::KEPT_REPORTS_DIR).exists());
    }

    #[test]
    fn an_unchanged_inbox_is_not_read_again_and_any_write_to_it_is_picked_up() {
        use AgentState::{Idle, Working};
        let scratch = ScratchDir::new("waker-inbox-cache");
        let dir = scratch.join("weather");
        let note = |text: &str| Item {
            project: "/repos/weather".into(),
            source: "notify".into(),
            kind: Kind::Note,
            text: text.into(),
            ..Item::default()
        };
        let mut waker = SupervisorWaker::in_dir(&scratch);
        let refresh = |waker: &mut SupervisorWaker, state| {
            let sent = wake_refresh(waker, &[weather_supervisor(state)], None, false);
            (sent, waker.read.reads)
        };
        // No inbox yet; then one that only waits while its supervisor works.
        assert_eq!(refresh(&mut waker, Working), (vec![], 1));
        assert_eq!(refresh(&mut waker, Working), (vec![], 1));
        let first = inbox::append(&dir, note("[Corgi] first")).unwrap().unwrap();
        assert_eq!(refresh(&mut waker, Working), (vec![], 2));
        assert_eq!(refresh(&mut waker, Working), (vec![], 2));

        // Another process takes it (`corgi inbox --take`): nothing goes out.
        inbox::mark_delivered(&dir, std::slice::from_ref(&first.id), "corgi inbox --take").unwrap();
        assert_eq!(refresh(&mut waker, Idle), (vec![], 3));

        // An append goes out, and recording it is read back once.
        inbox::append(&dir, note("[Corgi] second")).unwrap();
        assert_eq!(
            refresh(&mut waker, Idle),
            (vec!["supervisor-weather: [Corgi] second".to_string()], 4)
        );
        assert_eq!(refresh(&mut waker, Idle), (vec![], 5));
        assert_eq!(refresh(&mut waker, Idle), (vec![], 5));

        // A compaction replaces the file by a rename, here with one of the
        // same length holding a new undelivered item in place of the old.
        let path = dir.join(inbox::INBOX_FILE);
        let text = fs::read_to_string(&path).unwrap();
        let compacted = text.replace("[Corgi] second", "[Corgi] thirds");
        assert_eq!(compacted.len(), text.len());
        // Its record of delivery names another id, so it is undelivered.
        let record = compacted.rfind("\"delivered\":[\"").unwrap() + 14;
        let compacted = format!("{}x{}", &compacted[..record], &compacted[record + 1..]);
        let temporary = dir.join("compacted");
        fs::write(&temporary, &compacted).unwrap();
        fs::rename(&temporary, &path).unwrap();
        assert_eq!(
            refresh(&mut waker, Idle),
            (vec!["supervisor-weather: [Corgi] thirds".to_string()], 6)
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
        let actions = waker.hand_over(
            &agents(Done),
            1,
            |_| supervisor::Cache::Expires(0),
            1_000,
            |_, _| true,
            |_| false,
        );
        assert!(actions.is_empty());
        assert!(waker.handovers.is_empty());
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
                    |_| supervisor::Cache::Expires(0),
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
                    .hand_over(
                        agents,
                        50,
                        |_| supervisor::Cache::Expires(0),
                        1_000,
                        |_, _| note,
                        |_| false,
                    )
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
                    .hand_over(
                        &agents,
                        50,
                        |_| supervisor::Cache::Expires(0),
                        1_000,
                        |_, _| false,
                        |_| false,
                    )
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
    fn only_a_claude_supervisor_is_handed_over_at_200k_tokens_under_half_its_window() {
        let scratch = ScratchDir::new("waker");
        let asked = |kind: &str| {
            let mut supervisor = project_agent(
                "supervisor-weather",
                "/repos/weather",
                true,
                AgentState::Idle,
            );
            supervisor.info.agent = Some(kind.into());
            supervisor.info.agent_session = Some(crate::model::AgentSession {
                value: "s1".into(),
                ..Default::default()
            });
            supervisor.context_percent = Some(20);
            supervisor.context_tokens = Some(200_000);
            SupervisorWaker::in_dir(&scratch)
                .hand_over(
                    std::slice::from_ref(&supervisor),
                    50,
                    |_| supervisor::Cache::Expires(0),
                    7,
                    |_, _| false,
                    |_| false,
                )
                .into_iter()
                .filter_map(|action| action.step)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            asked("claude"),
            [supervisor::HandoverStep::Ask(
                7,
                supervisor::Trigger::Tokens(200_000)
            )]
        );
        assert_eq!(asked("codex"), []);
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
                    |harness| match harness {
                        Harness::Codex => supervisor::Cache::Expires(60),
                        _ => supervisor::Cache::Untimed,
                    },
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
    fn one_waker_per_core_wakes_supervisors() {
        let scratch = ScratchDir::new("waker");
        let mut first = SupervisorWaker::in_dir(&scratch);
        let mut second = SupervisorWaker::in_dir(&scratch);
        assert!(first.lead_hc());
        assert!(!second.lead_hc());
        assert!(first.lead_hc());
        // When the leader goes, another waker takes over.
        drop(first);
        assert!(second.lead_hc());
        assert_eq!(
            wake_lock_path(Path::new("/tmp/hc.sock"))
                .and_then(|path| path.file_name().map(|name| name.to_owned())),
            Some("_tmp_hc_sock.lock".into())
        );
    }

    #[test]
    fn a_paused_supervisors_project_keeps_its_handover_while_it_is_away() {
        let mut waker = SupervisorWaker::default();
        waker
            .handovers
            .insert("/repos/weather".into(), supervisor::Handover::default());
        waker
            .handovers
            .insert("/repos/gone".into(), supervisor::Handover::default());
        waker.paused.roots.insert("/repos/weather".into());
        waker.hand_over(
            &[],
            50,
            |_| supervisor::Cache::Unknown,
            1_000,
            |_, _| false,
            |_| false,
        );
        assert!(waker.handovers.contains_key("/repos/weather"));
        assert!(!waker.handovers.contains_key("/repos/gone"));
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
