//! The dashboard's periodic refresh: Herdr's snapshot, the marks Corgi puts
//! back in it, each agent's row from its session file and screen, and the
//! supervisors' wakes.
//!
//! [`run`] is the whole of one refresh, from [`Inputs`] and the
//! [`RefreshState`] the last one handed back. The interactive dashboard runs
//! it as a background [`crate::job::Job`], so however slow Herdr, the disk
//! or the machine is, keys and frames never wait on it; the command-line
//! paths and tests run the same work in place. What a
//! refresh carries from one to the next, [`RefreshState`], goes with the job
//! and comes back with its result, so the UI thread and the job share
//! nothing that needs a lock, and applying a result is only putting its rows
//! in place. At most one refresh runs at a time: the next is due
//! [`REFRESH_INTERVAL`] after the last one finished, or at once when an
//! action asked for one, so a slow Herdr means fewer refreshes, never
//! overlapping ones.

use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    activity::{command_from_screen, message_from_screen, message_from_state},
    herdr::{HerdrClient, ReadSource, SessionSnapshot},
    model::{Activity, ActivityKind, AgentInfo, AgentState, DashboardAgent, WorkspaceInfo},
    paths::{corgi_state_dir, is_home},
    session::SessionReader,
    supervisor::{self, is_supervisor},
    time::{LocalTime, rfc3339_utc, unix_now},
};

use crate::{
    markers::{clear_stale_merge_tags, restore_markers},
    notice,
    project_main::{dashboard_pane_ids, relabel_project_mains},
    rows::{
        NO_TOOL_YET, SCRATCH_GROUP, SUPERVISOR_NAME, agent_project, agent_worktree, checkout_label,
        codex_thread_id, has_herdr_agent_session, has_real_agent_session, is_corgi_scratch_codex,
        project_group, reported_context_percent, reported_effort, reported_model, task_summary,
    },
    waker::{SupervisorWaker, wake_supervisors},
};

/// How long after one refresh finished the next is due, unless an action
/// asks for one sooner.
pub const REFRESH_INTERVAL: Duration = Duration::from_millis(850);

/// How long the status lines a refresh reports stay over the agent count.
const MARKS_HOLD: Duration = Duration::from_secs(15);
const STATE_DIR_HOLD: Duration = Duration::from_secs(60);
const WAKE_HOLD: Duration = Duration::from_secs(15);
/// Past this size the refresh log starts over, the one before kept beside it
/// with `.old` appended.
const LOG_LIMIT: u64 = 256 * 1024;
/// A refresh that takes this long is logged whatever it found; one usually
/// takes a few milliseconds.
const SLOW_REFRESH: Duration = Duration::from_millis(50);
/// Set to `all`, this environment variable has the dashboard log every
/// refresh, not only those worth reading.
const LOG_ALL_VAR: &str = "CORGI_REFRESH_LOG";

/// What a refresh carries from one to the next. Between refreshes the UI
/// thread holds it; while one runs in the background, the job does.
#[derive(Debug, Default)]
pub struct RefreshState {
    /// Reads the model and context usage each agent CLI records for itself.
    pub sessions: SessionReader,
    /// The message and tool rows shown for each pane at the last refresh, so a
    /// failed read keeps its rows rather than blanking them.
    pub cached_rows: HashMap<String, (Activity, Activity)>,
    /// Wakes supervisors about their projects' agents; only the interactive
    /// dashboard has one.
    pub waker: Option<SupervisorWaker>,
    /// The project roots whose state directories this dashboard has checked
    /// for pre-rename state left beside them, so each is warned about once.
    state_dirs_checked: HashSet<String>,
    /// What the last refresh found, as the log tells it: how many agents, or
    /// why Herdr gave none.
    found: Option<Result<usize, String>>,
}

/// Where the interactive dashboard logs its refreshes, and which.
#[derive(Debug, Clone)]
pub struct RefreshLog {
    path: PathBuf,
    /// Every refresh, rather than those worth reading.
    every: bool,
    /// How long a refresh takes to be logged whatever it found.
    pub slow: Duration,
}

/// What one refresh needs from the app besides its state, copied when it
/// starts.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub client: HerdrClient,
    /// The pane the dashboard runs in, which is never listed as an agent.
    pub dashboard_pane_id: Option<String>,
    /// The names Codex generated for its threads, by session id.
    pub codex_task_titles: HashMap<String, String>,
    /// The pane whose transcript the expanded view shows, read again with
    /// the rows.
    pub transcript_pane: Option<String>,
    /// Where to look for a notice from `corgi setup`, for the full dashboard.
    pub notice_path: Option<PathBuf>,
}

impl Inputs {
    /// The inputs of a refresh by a process that owns no dashboard pane and
    /// shows no transcript, such as a Herdr hook or a frontend's data
    /// source: every agent on `client` is listed.
    pub fn new(client: HerdrClient) -> Self {
        Self {
            client,
            dashboard_pane_id: None,
            codex_task_titles: HashMap::new(),
            transcript_pane: None,
            notice_path: None,
        }
    }
}

/// A refresh's result and the state it hands back.
#[derive(Debug)]
pub struct Refreshed {
    pub state: RefreshState,
    /// The new rows, or why Herdr gave none.
    pub view: Result<View, String>,
    /// A notice from `corgi setup` this refresh took, to show in a popup.
    pub notice: Option<String>,
}

/// The rows one refresh found, ready to put in place.
#[derive(Debug, Default)]
pub struct View {
    /// The agents' rows, in the dashboard's order ([`sort_agents`]).
    pub agents: Vec<DashboardAgent>,
    pub workspaces: Vec<WorkspaceInfo>,
    /// The expanded view's pane and its newest turns.
    pub transcript: Option<(String, Arc<[Activity]>)>,
    /// The status line to show, and how long it stays.
    pub status: Option<(String, Duration)>,
}

/// How long each step of one refresh took, for the log.
#[derive(Debug, Default)]
struct Timings {
    herdr: Duration,
    marks: Duration,
    rows: Duration,
    wake: Duration,
}

/// The refresh as a whole: everything slow, run on whichever thread calls
/// it. `log` is where a refresh worth reading is logged, if anywhere.
pub fn run(inputs: &Inputs, mut state: RefreshState, log: Option<&RefreshLog>) -> Refreshed {
    let started = Instant::now();
    let mut timings = Timings::default();
    let view = state.refresh(inputs, &mut timings);
    let found = match &view {
        Ok(view) => Ok(view.agents.len()),
        Err(error) => Err(error.clone()),
    };
    if let Some(log) = log {
        log.refresh(started.elapsed(), &timings, &found, state.found.as_ref());
    }
    state.found = Some(found);
    let notice = inputs
        .notice_path
        .as_deref()
        .and_then(|path| notice::take(path, unix_now()));
    Refreshed {
        state,
        view,
        notice,
    }
}

impl RefreshState {
    /// A state for a dashboard that wakes supervisors when `waker` is given,
    /// as a new dashboard's or one whose last refresh panicked and took its
    /// state with it.
    pub fn new(waker: Option<SupervisorWaker>) -> Self {
        Self {
            waker,
            ..Self::default()
        }
    }

    fn refresh(&mut self, inputs: &Inputs, timings: &mut Timings) -> Result<View, String> {
        let client = &inputs.client;
        let step = Instant::now();
        let snapshot = client.snapshot();
        timings.herdr = step.elapsed();
        let mut snapshot = snapshot.map_err(|error| format!("{error:#}"))?;

        let step = Instant::now();
        let mut status = None;
        // The dashboard that wakes supervisors is the one that puts back the
        // marks a Herdr restart lost, before anything reads them, and that
        // clears the merge tags their agents worked past.
        if self.waker.as_mut().is_some_and(|waker| waker.leads(client)) {
            status = restore_markers(client, &mut snapshot).map(|status| (status, MARKS_HOLD));
            clear_stale_merge_tags(client, &mut snapshot);
        }
        relabel_project_mains(client, &mut snapshot.workspaces);
        timings.marks = step.elapsed();

        let step = Instant::now();
        let mut view = self.view(inputs, snapshot);
        timings.rows = step.elapsed();

        let step = Instant::now();
        if let Some(warning) = self.warn_about_old_state_dirs(&view.agents) {
            status = Some((warning, STATE_DIR_HOLD));
        }
        if let Some(waker) = self.waker.as_mut()
            && let Some(woke) = wake_supervisors(waker, client, &mut self.sessions, &view.agents)
        {
            status = Some((woke, WAKE_HOLD));
        }
        timings.wake = step.elapsed();
        view.status = status;
        Ok(view)
    }

    /// The rows of `snapshot`, in the dashboard's order, with the expanded
    /// view's transcript read again.
    pub fn view(&mut self, inputs: &Inputs, snapshot: SessionSnapshot) -> View {
        let workspaces: HashMap<&str, &WorkspaceInfo> = snapshot
            .workspaces
            .iter()
            .map(|workspace| (workspace.workspace_id.as_str(), workspace))
            .collect();
        let dashboard_pane_ids =
            dashboard_pane_ids(inputs.dashboard_pane_id.as_deref(), &snapshot.panes);
        let client = &inputs.client;

        let mut agents: Vec<_> = snapshot
            .agents
            .into_iter()
            // Herdr's detector can briefly identify Corgi's own terminal as
            // an agent while the pane starts. Never render the dashboard as a
            // managed agent, including after its launcher has moved it and
            // changed the original HERDR_PANE_ID.
            .filter(|info| {
                !dashboard_pane_ids.contains(info.pane_id.as_str())
                    && (has_real_agent_session(info)
                        || (is_corgi_scratch_codex(
                            info,
                            workspaces.get(info.workspace_id.as_str()).copied(),
                        ) && client
                            .has_foreground_codex(&info.pane_id, info.cwd())
                            .unwrap_or(false)))
            })
            .map(|info| {
                let workspace = workspaces.get(info.workspace_id.as_str()).copied();
                self.dashboard_agent(inputs, info, workspace)
            })
            .collect();
        sort_agents(&mut agents);
        // Drops what is remembered about sessions no longer on the dashboard.
        self.sessions.retain(agents.iter().map(|agent| &agent.info));
        let transcript = inputs.transcript_pane.as_ref().and_then(|pane| {
            let agent = agents.iter().find(|agent| agent.info.pane_id == *pane)?;
            Some((pane.clone(), self.sessions.transcript(&agent.info)))
        });
        View {
            agents,
            workspaces: snapshot.workspaces,
            transcript,
            status: None,
        }
    }

    /// One agent's dashboard row: where it works, what it is doing, and what
    /// its session says about itself.
    fn dashboard_agent(
        &mut self,
        inputs: &Inputs,
        info: AgentInfo,
        workspace: Option<&WorkspaceInfo>,
    ) -> DashboardAgent {
        let project = agent_project(&info, workspace);
        let worktree = agent_worktree(&info, workspace);
        let project_root = worktree
            .map(|worktree| worktree.repo_root.as_str())
            .filter(|root| !root.is_empty())
            .unwrap_or_else(|| info.cwd())
            .to_string();
        let scratch = is_home(&project_root);
        let project_group = if scratch {
            SCRATCH_GROUP.to_string()
        } else {
            project_group(&info, workspace, &project)
        };
        let worktree_checkout = worktree
            .filter(|worktree| worktree.is_linked_worktree)
            .map(|worktree| worktree.checkout_path.clone());
        let worktree_label = worktree_checkout
            .as_deref()
            .and_then(checkout_label)
            .map(str::to_string);
        // The agent CLI's own session file is authoritative; a status-line
        // bridge that reports pane metadata is only a fallback for sessions
        // whose file cannot be read.
        let facts = self.sessions.facts(&info);
        let codex_task_title = codex_thread_id(&info)
            .and_then(|session_id| inputs.codex_task_titles.get(session_id))
            .map(String::as_str);
        let task = task_summary(
            &info,
            workspace,
            &project,
            codex_task_title,
            facts.task.as_deref(),
        );
        let model = facts.model.or_else(|| reported_model(&info));
        let effort = facts.effort.or_else(|| reported_effort(&info));
        let context_percent = facts
            .context_percent
            .or_else(|| reported_context_percent(&info));
        let (message, tool) =
            self.conversation_rows(&inputs.client, &info, facts.message, facts.tool);
        let supervisor = is_supervisor(&info);
        let (project, task) = if supervisor {
            (SUPERVISOR_NAME.to_string(), SUPERVISOR_NAME.to_string())
        } else {
            (project, task)
        };
        DashboardAgent {
            info,
            project_group,
            project,
            project_root,
            worktree_checkout,
            worktree_label,
            task,
            model,
            effort,
            context_percent,
            context_tokens: facts.context_tokens,
            cache: facts.cache,
            last_activity: facts.last_activity.map(LocalTime::at),
            message,
            tool,
            supervisor,
            scratch,
        }
    }

    /// The two rows under an agent's status line: the newest thing said in the
    /// session, and the tool call it is running or last ran.
    ///
    /// The transcript is the source of both. The terminal fills in whatever it
    /// lacks, and is also asked while the agent is blocked, because permission
    /// prompts and other questions the CLI itself asks never reach the
    /// transcript. When nothing new can be read, the rows from the previous
    /// refresh stay, and before anything was ever read the state speaks.
    pub fn conversation_rows(
        &mut self,
        client: &HerdrClient,
        info: &AgentInfo,
        message: Option<Activity>,
        tool: Option<Activity>,
    ) -> (Activity, Activity) {
        // Without an identity, a reused pane cannot safely inherit the
        // previous occupant's conversation when a terminal read fails. Corgi's
        // own marks can outlive the agent in a pane, so only Herdr's counts.
        let previous = self
            .cached_rows
            .remove(&info.pane_id)
            .filter(|_| has_herdr_agent_session(info));
        let (mut message, mut tool) = (message, tool);
        let blocked = info.state == AgentState::Blocked;
        if (message.is_none() || tool.is_none() || blocked)
            && let Some(screen) = read_screen(client, info)
        {
            let asked = message_from_screen(info.kind(), &screen);
            if blocked
                && asked
                    .as_ref()
                    .is_some_and(|asked| asked.kind == ActivityKind::Question)
            {
                message = asked;
            } else {
                message = message.or(asked);
            }
            tool = tool.or_else(|| command_from_screen(&screen));
        }
        let (previous_message, previous_tool) = previous.unzip();
        let message = message
            .or(previous_message)
            .unwrap_or_else(|| message_from_state(info.state));
        let tool = tool.or(previous_tool).unwrap_or_else(|| Activity {
            kind: ActivityKind::Ready,
            text: NO_TOOL_YET.into(),
        });
        self.cached_rows
            .insert(info.pane_id.clone(), (message.clone(), tool.clone()));
        (message, tool)
    }

    /// Tells the user, once per project, when the state a project's supervisor
    /// had before the rename sits beside its current state directory instead
    /// of having been moved into it, so that history is not overlooked. Only
    /// the interactive dashboard, the one with a waker, checks.
    fn warn_about_old_state_dirs(&mut self, agents: &[DashboardAgent]) -> Option<String> {
        self.waker.as_ref()?;
        let roots: Vec<String> = agents
            .iter()
            .filter(|agent| agent.supervisor)
            .map(|agent| agent.project_root.clone())
            .filter(|root| !self.state_dirs_checked.contains(root))
            .collect();
        let mut warnings = Vec::new();
        for root in roots {
            warnings.extend(supervisor::state_dir_conflict(&root));
            self.state_dirs_checked.insert(root);
        }
        (!warnings.is_empty()).then(|| warnings.join("  "))
    }
}

fn read_screen(client: &HerdrClient, info: &AgentInfo) -> Option<String> {
    client
        .read_agent(&info.pane_id, ReadSource::Detection, Some(64))
        .or_else(|_| client.read_agent(&info.pane_id, ReadSource::Visible, Some(64)))
        .ok()
        .map(|read| read.text)
}

impl RefreshLog {
    /// The interactive dashboard's: `refresh.log` in Corgi's state
    /// directory, with every refresh when [`LOG_ALL_VAR`] says so.
    pub fn from_env() -> Option<Self> {
        let every = std::env::var_os(LOG_ALL_VAR).is_some_and(|value| value == "all");
        corgi_state_dir().map(|dir| Self::new(dir.join("refresh.log"), every))
    }

    pub fn new(path: PathBuf, every: bool) -> Self {
        Self {
            path,
            every,
            slow: SLOW_REFRESH,
        }
    }

    /// Logs a refresh worth reading: one that took `slow` ([`SLOW_REFRESH`])
    /// or more, or that found something else than the one before, `before`:
    /// another number of agents, Herdr gone or back, or another error. The line
    /// says when it ended, the process, how long it took in all and in each
    /// step, and what it found. It is written from the refresh's own
    /// thread, and a failure to write it is ignored.
    fn refresh(
        &self,
        took: Duration,
        timings: &Timings,
        found: &Result<usize, String>,
        before: Option<&Result<usize, String>>,
    ) {
        if !self.every && took < self.slow && before == Some(found) {
            return;
        }
        let found = match found {
            Ok(agents) => format!("{agents} agents"),
            Err(error) => format!("Herdr unavailable: {error}"),
        };
        let millis = |duration: Duration| duration.as_millis();
        self.append(&format!(
            "refresh {} ms (herdr {}, marks {}, rows {}, wake {}): {found}",
            millis(took),
            millis(timings.herdr),
            millis(timings.marks),
            millis(timings.rows),
            millis(timings.wake),
        ));
    }

    /// Appends `line` to the log, after when and the process. A log past
    /// [`LOG_LIMIT`] is moved aside first, so it never grows without bound.
    pub fn append(&self, line: &str) {
        let log = &self.path;
        if fs::metadata(log).is_ok_and(|meta| meta.len() > LOG_LIMIT) {
            let mut old = log.as_os_str().to_owned();
            old.push(".old");
            let _ = fs::rename(log, old);
        }
        if let Some(dir) = log.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let line = format!(
            "{} pid {} {line}\n",
            rfc3339_utc(unix_now()),
            std::process::id(),
        );
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log) {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

/// Keeps related worktree sessions together, each project led by its
/// supervisor, and the scratch sessions after every project. Within a project
/// the rest follow their live state, the ones needing attention first:
/// blocked, working, done, idle, then unknown. Agents sharing a state are
/// ordered by when they entered it, the most recent first, using Herdr's
/// `state_change_seq`: one counter across the whole Herdr session that is
/// bumped on every state change, so it compares between agents. A row
/// therefore moves only when its own agent changes state, which is rare
/// enough to be worth seeing at a glance what is blocked or busy; focus stays
/// out of the key so moving between panes never reorders the list. Equal
/// counters (older Herdr, or fixtures) fall back to name, then pane ID.
pub fn sort_agents(agents: &mut [DashboardAgent]) {
    agents.sort_by(|left, right| {
        left.scratch
            .cmp(&right.scratch)
            .then_with(|| {
                left.project_group
                    .to_lowercase()
                    .cmp(&right.project_group.to_lowercase())
            })
            .then_with(|| right.supervisor.cmp(&left.supervisor))
            .then_with(|| state_rank(left.info.state).cmp(&state_rank(right.info.state)))
            .then_with(|| right.info.state_change_seq.cmp(&left.info.state_change_seq))
            .then_with(|| {
                left.info
                    .display_name()
                    .to_lowercase()
                    .cmp(&right.info.display_name().to_lowercase())
            })
            .then_with(|| left.info.pane_id.cmp(&right.info.pane_id))
    });
}

/// Position of a state within a project, the most urgent first.
fn state_rank(state: AgentState) -> u8 {
    match state {
        AgentState::Blocked => 0,
        AgentState::Working => 1,
        AgentState::Done => 2,
        AgentState::Idle => 3,
        AgentState::Unknown => 4,
    }
}

/// Whether something last done at `last` is due again after `interval`.
/// `None` means it never ran, or was asked for, so it is due now.
pub fn is_due(last: Option<Instant>, interval: Duration) -> bool {
    last.is_none_or(|at| at.elapsed() >= interval)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use crate::test_support::{ScratchDir, UNUSED_SOCKET, answer, fake_herdr};

    use super::*;

    #[test]
    fn a_refresh_runs_from_a_client_alone() {
        let (socket_path, server) = fake_herdr("refresh-core", |listener| {
            answer(&listener, |request| {
                assert_eq!(request["method"], "session.snapshot");
                json!({"result": {"type": "session_snapshot", "snapshot": {
                    "workspaces": [{"workspace_id": "w1", "label": "weather"}],
                    // Herdr's detector alone, with no session: not listed.
                    "agents": [{
                        "agent": "codex", "agent_status": "working",
                        "pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1"
                    }]
                }}})
            });
        });
        let inputs = Inputs::new(HerdrClient::from_socket_path(&socket_path));
        let refreshed = run(&inputs, RefreshState::default(), None);
        server.join().expect("fake server panicked");
        std::fs::remove_file(&socket_path).expect("remove fake socket");
        let view = refreshed.view.expect("a view");
        assert!(view.agents.is_empty());
        assert_eq!(view.workspaces[0].workspace_id, "w1");

        // Herdr gone: the reason, and the state to hand to the next one.
        let gone = Inputs::new(HerdrClient::from_socket_path(UNUSED_SOCKET));
        let refreshed = run(&gone, refreshed.state, None);
        assert!(refreshed.view.is_err());
    }

    #[test]
    fn a_slow_refresh_is_logged_and_every_one_when_asked() {
        let scratch = ScratchDir::new("refresh-log-slow");
        let timings = Timings::default();
        let read = |name: &str| std::fs::read_to_string(scratch.join(name)).unwrap_or_default();

        let quiet = RefreshLog::new(scratch.join("quiet.log"), false);
        let before = Ok(3);
        quiet.refresh(Duration::from_millis(4), &timings, &Ok(3), Some(&before));
        assert_eq!(read("quiet.log"), "");
        quiet.refresh(SLOW_REFRESH, &timings, &Ok(3), Some(&before));
        let log = read("quiet.log");
        assert!(
            log.contains(&format!(" refresh {} ms ", SLOW_REFRESH.as_millis())),
            "{log}"
        );
        assert_eq!(log.lines().count(), 1, "{log}");

        let every = RefreshLog::new(scratch.join("every.log"), true);
        for _ in 0..3 {
            every.refresh(Duration::from_millis(4), &timings, &Ok(3), Some(&before));
        }
        assert_eq!(read("every.log").lines().count(), 3);
    }
}
