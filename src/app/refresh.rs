//! The dashboard's periodic refresh: Herdr's snapshot, the marks Corgi puts
//! back in it, each agent's row from its session file and screen, and the
//! supervisors' wakes.
//!
//! The interactive dashboard runs it as a background [`Job`], so however
//! slow Herdr, the disk or the machine is, keys and frames never wait on it;
//! the command-line paths and tests run the same work in place. What a
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
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use crate::{
    activity::{command_from_screen, message_from_screen, message_from_state},
    herdr::{HerdrClient, ReadSource, SessionSnapshot},
    job::Job,
    model::{Activity, ActivityKind, AgentInfo, AgentState, DashboardAgent, WorkspaceInfo},
    paths::{corgi_state_dir, is_home},
    session::SessionReader,
    supervisor::{self, is_supervisor},
    time::{rfc3339_utc, unix_now},
};

use super::{
    App, REFRESH_INTERVAL, SUPERVISOR_NAME, is_due,
    markers::{clear_stale_merge_tags, restore_markers},
    project_main::{dashboard_pane_ids, relabel_project_mains},
    rows::{
        NO_TOOL_YET, SCRATCH_GROUP, agent_project, agent_worktree, checkout_label, codex_thread_id,
        has_herdr_agent_session, has_real_agent_session, is_corgi_scratch_codex, project_group,
        reported_context_percent, reported_effort, reported_model, task_summary,
    },
    sort_agents,
    waker::{SupervisorWaker, wake_supervisors},
};

/// How long quitting waits for a refresh in flight, so that a wake being
/// typed into a supervisor's box is not cut off.
pub(super) const QUIT_WAIT: Duration = Duration::from_secs(2);
/// How long the status lines a refresh reports stay over the agent count.
const MARKS_HOLD: Duration = Duration::from_secs(15);
const STATE_DIR_HOLD: Duration = Duration::from_secs(60);
const WAKE_HOLD: Duration = Duration::from_secs(15);
/// Past this size the refresh log starts over, the one before kept beside it
/// with `.old` appended.
const LOG_LIMIT: u64 = 256 * 1024;

/// What a refresh carries from one to the next. Between refreshes the UI
/// thread holds it; while one runs in the background, the job does.
#[derive(Debug, Default)]
pub(super) struct RefreshState {
    /// Reads the model and context usage each agent CLI records for itself.
    pub(super) sessions: SessionReader,
    /// The message and tool rows shown for each pane at the last refresh, so a
    /// failed read keeps its rows rather than blanking them.
    pub(super) cached_rows: HashMap<String, (Activity, Activity)>,
    /// Wakes supervisors about their projects' agents; only the interactive
    /// dashboard has one.
    pub(super) waker: Option<SupervisorWaker>,
    /// The project roots whose state directories this dashboard has checked
    /// for pre-rename state left beside them, so each is warned about once.
    state_dirs_checked: HashSet<String>,
}

/// What one refresh needs from the app besides its state, copied when it
/// starts.
#[derive(Debug, Clone)]
struct Inputs {
    client: HerdrClient,
    dashboard_pane_id: Option<String>,
    codex_task_titles: HashMap<String, String>,
    /// The pane whose transcript the expanded view shows, read again with
    /// the rows.
    transcript_pane: Option<String>,
}

/// A refresh's result and the state it hands back.
#[derive(Debug)]
pub(super) struct Refreshed {
    state: RefreshState,
    /// The new rows, or why Herdr gave none.
    view: Result<View, String>,
}

/// The rows one refresh found, ready to put in place.
#[derive(Debug, Default)]
pub(super) struct View {
    agents: Vec<DashboardAgent>,
    workspaces: Vec<WorkspaceInfo>,
    /// The expanded view's pane and its newest turns.
    transcript: Option<(String, Arc<[Activity]>)>,
    /// The status line to show, and how long it stays.
    status: Option<(String, Duration)>,
}

/// How long each step of one refresh took, for the log.
#[derive(Debug, Default)]
struct Timings {
    herdr: Duration,
    marks: Duration,
    rows: Duration,
    wake: Duration,
}

/// The refresh as a whole: everything slow, run on whichever thread calls it.
fn run(inputs: &Inputs, mut state: RefreshState, log: Option<&Path>) -> Refreshed {
    let started = Instant::now();
    let mut timings = Timings::default();
    let view = state.refresh(inputs, &mut timings);
    if let Some(log) = log {
        log_refresh(log, started.elapsed(), &timings, &view);
    }
    Refreshed { state, view }
}

impl RefreshState {
    /// A state for a dashboard that wakes supervisors when `waker` is given,
    /// as a new dashboard's or one whose last refresh panicked and took its
    /// state with it.
    pub(super) fn new(waker: Option<SupervisorWaker>) -> Self {
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
    fn view(&mut self, inputs: &Inputs, snapshot: SessionSnapshot) -> View {
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
    pub(super) fn conversation_rows(
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

/// Where the interactive dashboard logs how long each refresh took:
/// `refresh.log` in Corgi's state directory.
pub(super) fn refresh_log_path() -> Option<PathBuf> {
    corgi_state_dir().map(|dir| dir.join("refresh.log"))
}

/// Appends one line about a refresh to `log`: when it ended, the process,
/// how long it took in all and in each step, and what it found. A log past
/// [`LOG_LIMIT`] is moved aside first, so it never grows without bound. It is
/// written from the refresh's own thread, and a failure to write it is
/// ignored.
fn log_refresh(log: &Path, took: Duration, timings: &Timings, view: &Result<View, String>) {
    if fs::metadata(log).is_ok_and(|meta| meta.len() > LOG_LIMIT) {
        let mut old = log.as_os_str().to_owned();
        old.push(".old");
        let _ = fs::rename(log, old);
    }
    if let Some(dir) = log.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let found = match view {
        Ok(view) => format!("{} agents", view.agents.len()),
        Err(error) => format!("Herdr unavailable: {error}"),
    };
    let millis = |duration: Duration| duration.as_millis();
    let line = format!(
        "{} pid {} refresh {} ms (herdr {}, marks {}, rows {}, wake {}): {found}\n",
        rfc3339_utc(unix_now()),
        std::process::id(),
        millis(took),
        millis(timings.herdr),
        millis(timings.marks),
        millis(timings.rows),
        millis(timings.wake),
    );
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log) {
        let _ = file.write_all(line.as_bytes());
    }
}

impl App {
    /// Makes the next pass of the dashboard loop take a fresh snapshot: at
    /// once, or as soon as the refresh running now has finished, since that
    /// one may have started before what asked for this.
    pub(super) fn request_refresh(&mut self) {
        self.refresh_requested = true;
    }

    /// Whether the dashboard loop is due for a fresh snapshot: none is
    /// running, and one was asked for or the last finished long enough ago.
    pub(super) fn refresh_due(&self) -> bool {
        !self.refresh_job.is_running()
            && (self.refresh_requested || is_due(self.last_refresh, REFRESH_INTERVAL))
    }

    /// Whether a refresh is running in the background.
    #[cfg(test)]
    pub(super) fn refreshing(&self) -> bool {
        self.refresh_job.is_running()
    }

    fn refresh_inputs(&self) -> Inputs {
        Inputs {
            client: self.client.clone(),
            dashboard_pane_id: self.dashboard_pane_id.clone(),
            codex_task_titles: self.codex_task_titles.clone(),
            transcript_pane: self
                .selected_agent()
                .filter(|_| self.expanded)
                .map(|agent| agent.info.pane_id.clone()),
        }
    }

    /// The refresh state, to hand to a refresh. One a panicked refresh took
    /// with it is started again from nothing, as a new dashboard's.
    fn take_refresh_state(&mut self) -> RefreshState {
        self.refresh_state.take().unwrap_or_else(|| {
            RefreshState::new(self.inboxes.clone().map(SupervisorWaker::with_inboxes))
        })
    }

    /// The refresh state while no refresh has it, for the work done in place.
    pub(super) fn refresh_state(&mut self) -> &mut RefreshState {
        self.wait_for_refresh(None);
        if self.refresh_state.is_none() {
            let state = self.take_refresh_state();
            self.refresh_state = Some(state);
        }
        self.refresh_state.as_mut().expect("a refresh state")
    }

    /// Starts a refresh in the background when one is due. None starts while
    /// a launch runs, so the snapshot never catches a launch halfway.
    pub(super) fn start_refresh(&mut self) {
        if !self.refresh_due() || self.launch_job.is_running() {
            return;
        }
        self.refresh_requested = false;
        let inputs = self.refresh_inputs();
        let state = self.take_refresh_state();
        let log = self.refresh_log.clone();
        self.refresh_job = Job::spawn(move |_| run(&inputs, state, log.as_deref()));
    }

    /// Puts a finished background refresh in place, if one has finished.
    pub(super) fn poll_refresh(&mut self) {
        match self.refresh_job.outcome() {
            None => {}
            Some(Ok(refreshed)) => self.apply_refresh(refreshed),
            // Its state went with it; the next refresh starts as usual, from
            // a new one.
            Some(Err(_)) => {
                self.last_refresh = Some(Instant::now());
                self.set_status("The refresh failed; trying again", None);
            }
        }
    }

    /// Waits for a refresh running in the background and puts it in place,
    /// for at most `limit`, or for as long as it takes without one.
    pub(super) fn wait_for_refresh(&mut self, limit: Option<Duration>) {
        let started = Instant::now();
        while self.refresh_job.is_running() {
            if limit.is_some_and(|limit| started.elapsed() >= limit) {
                return;
            }
            self.poll_refresh();
            if self.refresh_job.is_running() {
                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// Refreshes in place, on this thread, as the command-line paths and the
    /// Omarchy bar do, which draw no dashboard and have nothing to keep
    /// responsive.
    pub(crate) fn refresh(&mut self) {
        self.wait_for_refresh(None);
        self.refresh_requested = false;
        let inputs = self.refresh_inputs();
        let state = self.take_refresh_state();
        let refreshed = run(&inputs, state, None);
        self.apply_refresh(refreshed);
    }

    /// Takes a refresh's state back and puts its rows in place.
    fn apply_refresh(&mut self, refreshed: Refreshed) {
        let Refreshed { state, view } = refreshed;
        self.refresh_state = Some(state);
        self.last_refresh = Some(Instant::now());
        match view {
            Ok(mut view) => {
                self.connected = true;
                let status = view.status.take();
                self.install_view(view);
                if let Some((status, hold)) = status {
                    self.set_status(status, Some(hold));
                }
                let status_held = self
                    .status_hold_until
                    .is_some_and(|deadline| Instant::now() < deadline);
                if !status_held && !self.status.starts_with("Starting ") {
                    self.status_hold_until = None;
                    self.status = format!(
                        "{} agent{}",
                        self.agents.len(),
                        if self.agents.len() == 1 { "" } else { "s" }
                    );
                }
            }
            Err(error) => {
                self.connected = false;
                self.status = format!("Herdr unavailable: {error}");
            }
        }
    }

    /// Puts the rows of `snapshot` in place, as a refresh would, without
    /// the marks, the wakes or the snapshot's round trip.
    #[cfg(test)]
    pub(super) fn install_snapshot(&mut self, snapshot: SessionSnapshot) {
        let inputs = self.refresh_inputs();
        let view = self.refresh_state().view(&inputs, snapshot);
        self.install_view(view);
    }

    /// Puts one refresh's rows in place. Nothing here reads a file or asks
    /// Herdr: it runs on the UI thread.
    fn install_view(&mut self, view: View) {
        // The selection stays on its row of the list, which is a whole card
        // for a collapsed project, rather than on its index into the agents.
        let row = self.selected_stop();
        self.agents = view.agents;
        self.select_stop(row);
        self.forget_gone_sessions();
        self.refresh_codex_task_titles();
        self.note_open_projects(&view.workspaces);
        self.workspaces = view.workspaces;
        self.clamp_selection(view.transcript);
    }

    /// Keeps the selection on the list after agents went away, and the
    /// expanded transcript on the session now selected: the one the refresh
    /// read, when it read the selected session's, or else the one already
    /// shown, when that is the selected session's. Otherwise the transcript
    /// is left empty for the refresh asked for here to read.
    fn clamp_selection(&mut self, transcript: Option<(String, Arc<[Activity]>)>) {
        if self.agents.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.agents.len() - 1);
        }
        if !self.expanded {
            return;
        }
        let Some(selected) = self
            .selected_agent()
            .map(|agent| agent.info.pane_id.clone())
        else {
            // The session being read is gone; there is nothing to zoom in
            // on, so fall back to the list rather than an empty panel.
            self.expanded = false;
            self.transcript = Default::default();
            self.transcript_pane = None;
            self.transcript_scroll = 0;
            return;
        };
        match transcript {
            Some((pane, entries)) if pane == selected => {
                self.transcript = entries;
                self.transcript_pane = Some(pane);
            }
            _ if self.transcript_pane.as_ref() == Some(&selected) => {}
            _ => {
                self.transcript = Default::default();
                self.transcript_pane = None;
                self.request_refresh();
            }
        }
    }
}

#[cfg(test)]
mod tests;
