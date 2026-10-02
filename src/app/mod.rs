//! The dashboard: `App`, its refresh, selection, expanded transcript and
//! background readers, key dispatch and terminal loop. Each submodule adds
//! the methods of one flow to the one `App`.

use std::{
    collections::{HashMap, HashSet},
    env,
    io::{self, Stdout},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use crossterm::{
    cursor::SetCursorStyle,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{
    activity::{command_from_screen, message_from_screen, message_from_state},
    handler::{self, is_handler},
    herdr::{HerdrClient, ReadSource, SessionSnapshot},
    job::Job,
    model::{Activity, ActivityKind, AgentInfo, AgentState, DashboardAgent, WorkspaceInfo},
    motion::Motion,
    paths::is_home,
    projects::ProjectMemory,
    session::SessionReader,
    ui,
    usage::{PlanUsage, Provider, installed_providers, read_codex_thread_titles},
    usage_cache::{CachedUsage, shared_plan_usage},
};

// The overlays: the new-agent form and its model catalogs, prompting,
// closing, and merging.
mod catalog;
mod close;
mod form;
mod merge;
mod overlay;
mod prompt;
#[cfg(test)]
pub(crate) use catalog::HARNESS_DEFAULT_MODEL;
use catalog::ModelCatalogs;
pub(crate) use close::{CloseTarget, CloseWorkspaceForm};
#[cfg(test)]
pub(crate) use form::Checkout;
pub(crate) use form::{NewAgentForm, NewField};
pub(crate) use merge::{MergePhase, MergeWorktreeForm};
pub(crate) use overlay::Overlay;
pub(crate) use prompt::PromptForm;

// Row derivation, the project cards, the command-line tools, and the
// Omarchy bar endpoints.
mod bar;
mod cards;
mod cli;
mod rows;
pub use bar::{bar_action, bar_new_agent, bar_new_options, bar_stream, bar_transcript};
pub(crate) use cards::{CardMemory, is_card, project_runs};
pub use cli::{
    DIGEST_USAGE, FLEET_USAGE, HANDLER_USAGE, REPORT_USAGE, SPAWN_USAGE, digest, fleet,
    handler_command, report, spawn,
};
use rows::{
    NO_TOOL_YET, SCRATCH_GROUP, agent_project, agent_worktree, checkout_label, codex_thread_id,
    has_real_agent_session, project_group, reported_context_percent, reported_effort,
    reported_model, task_summary,
};

// The launch sequence, the project workspace Corgi owns, the record of the
// marks Corgi sets in Herdr, the handler waker and the draft check it makes
// before typing into a handler's pane.
mod draft;
mod first_prompt;
mod launch;
mod markers;
mod progress;
mod project_main;
mod waker;
pub(crate) use launch::{LaunchState, NewAgentLaunch};
use progress::JobReport;
use project_main::relabel_project_mains;
use waker::HandlerWaker;

const REFRESH_INTERVAL: Duration = Duration::from_millis(850);
/// How old a plan-usage reading may be before one Corgi process on the
/// machine fetches a new one for all of them; see [`crate::usage_cache`].
const USAGE_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
/// How often a process looks at the shared usage cache. Reading it is only a
/// file read, so a reading another process fetched shows up within seconds,
/// including the first one after a start that found no cache.
const USAGE_CACHE_CHECK_INTERVAL: Duration = Duration::from_secs(5);
/// Codex generates a thread name asynchronously after the first prompt. Keep
/// this short enough for a new row to settle quickly without spawning an app
/// server for every dashboard redraw.
const CODEX_TASK_REFRESH_INTERVAL: Duration = Duration::from_secs(3);
/// Herdr gives plugin panes their manifest title as a pane label.  It is a
/// stable identity even when the launcher moves the dashboard to a new
/// workspace, which changes its pane ID.
const DASHBOARD_PANE_LABEL: &str = "Corgi";
/// What the dashboard calls a project's handler. There is one per project, so
/// the project heading says whose it is.
const HANDLER_NAME: &str = "Project handler";

type UsageResult = (Provider, Option<CachedUsage>);

/// Latest known plan usage for one installed provider. The last good reading
/// is kept when a refresh fails so the header does not flicker to blank.
#[derive(Debug, Clone)]
pub(crate) struct UsageSlot {
    pub(crate) provider: Provider,
    pub(crate) usage: Option<PlanUsage>,
    /// When `usage` was read (Unix seconds), for the card's age label.
    pub(crate) fetched_at: Option<u64>,
    pub(crate) error: Option<String>,
}

pub(crate) struct App {
    client: HerdrClient,
    dashboard_pane_id: Option<String>,
    pub(crate) agents: Vec<DashboardAgent>,
    pub(crate) selected: usize,
    /// The form or dialog open over the agent list, if any.
    pub(crate) overlay: Overlay,
    /// The clock and state of the popups' effects. Still, without effects,
    /// until [`run`] gives the interactive dashboard animated ones.
    pub(crate) motion: Motion,
    pub(crate) status: String,
    pub(crate) compact: bool,
    connected: bool,
    pub(crate) usage: Vec<UsageSlot>,
    /// The workspaces of the last snapshot, so the project selector can offer
    /// repositories that are open in Herdr without an agent in them.
    workspaces: Vec<WorkspaceInfo>,
    /// The projects Corgi has written down for the project selector.
    projects: ProjectMemory,
    /// The projects whose cards are expanded into their workers' rows.
    pub(crate) cards: CardMemory,
    /// Whether the selected session is expanded over the whole agent list to
    /// show its transcript. This is a zoom of the list rather than a mode of
    /// its own, so prompting, merging, and closing keep working over it.
    pub(crate) expanded: bool,
    /// The selected session's latest turns, newest first, while it is
    /// expanded. Empty while the list shows every agent.
    pub(crate) transcript: std::sync::Arc<[Activity]>,
    /// Rows the expanded transcript is scrolled down by. The renderer clamps
    /// it to what the content and the viewport allow.
    pub(crate) transcript_scroll: u16,
    /// Rows the expanded transcript last drew in, so a page key moves by a
    /// screenful of the size the reader is actually looking at.
    pub(crate) transcript_page: u16,
    /// The first item the collapsed agent list was last drawn from, so the
    /// view stays put while the selection moves inside it and scrolls only
    /// when the selection passes one of its edges. The renderer clamps it.
    pub(crate) agent_list_offset: usize,
    /// When each kind of refresh last started; `None` makes it due now.
    last_refresh: Option<Instant>,
    last_usage_refresh: Option<Instant>,
    last_codex_task_refresh: Option<Instant>,
    /// The message and tool rows shown for each pane at the last refresh, so a
    /// failed read keeps its rows rather than blanking them.
    cached_rows: HashMap<String, (Activity, Activity)>,
    /// Reads the model and context usage each agent CLI records for itself.
    sessions: SessionReader,
    /// Generated task names that Codex persists for its threads, keyed by the
    /// same session identity Herdr supplies for the pane.
    codex_task_titles: HashMap<String, String>,
    restore_focus_after_launch: bool,
    launch_job: Job<Result<String>, JobReport>,
    usage_job: Job<Vec<UsageResult>>,
    codex_task_job: Job<Result<HashMap<String, String>, String>>,
    merge_job: Job<Result<(), String>, JobReport>,
    /// The model catalogs and harness configurations behind the new-agent
    /// form's selectors.
    model_catalogs: ModelCatalogs,
    /// Wakes handlers about their projects' agents; only the interactive
    /// dashboard has one.
    handler_waker: Option<HandlerWaker>,
    /// The project roots whose state directories this dashboard has checked
    /// for pre-rename state left beside them, so each is warned about once.
    state_dirs_checked: HashSet<String>,
    status_hold_until: Option<Instant>,
}

/// What an app reads from the machine it runs on. [`App::new`] reads the
/// real ones; tests pass their own, so the developer's state never leaks in.
#[derive(Debug, Default)]
pub(crate) struct AppInputs {
    /// The pane Herdr runs the dashboard in, from `HERDR_PANE_ID`.
    dashboard_pane_id: Option<String>,
    /// The provider CLIs found on `PATH`, each given a usage card.
    providers: Vec<Provider>,
    /// The projects Corgi has written down.
    projects: ProjectMemory,
    /// The projects whose cards were left expanded.
    cards: CardMemory,
}

impl App {
    fn new(client: HerdrClient, compact: bool) -> Self {
        let inputs = AppInputs {
            dashboard_pane_id: env::var("HERDR_PANE_ID").ok().filter(|id| !id.is_empty()),
            providers: installed_providers(),
            projects: ProjectMemory::load(),
            cards: CardMemory::load(),
        };
        Self::with_inputs(client, compact, inputs)
    }

    pub(crate) fn with_inputs(client: HerdrClient, compact: bool, inputs: AppInputs) -> Self {
        Self {
            client,
            restore_focus_after_launch: true,
            dashboard_pane_id: inputs.dashboard_pane_id,
            agents: Vec::new(),
            selected: 0,
            overlay: Overlay::None,
            motion: Motion::default(),
            status: "Connecting to Herdr…".into(),
            compact,
            connected: false,
            usage: inputs
                .providers
                .into_iter()
                .map(|provider| UsageSlot {
                    provider,
                    usage: None,
                    fetched_at: None,
                    error: None,
                })
                .collect(),
            workspaces: Vec::new(),
            projects: inputs.projects,
            cards: inputs.cards,
            expanded: false,
            transcript: Default::default(),
            transcript_scroll: 0,
            transcript_page: 1,
            agent_list_offset: 0,
            last_refresh: None,
            last_usage_refresh: None,
            last_codex_task_refresh: None,
            cached_rows: HashMap::new(),
            sessions: SessionReader::default(),
            codex_task_titles: HashMap::new(),
            launch_job: Job::default(),
            usage_job: Job::default(),
            codex_task_job: Job::default(),
            merge_job: Job::default(),
            model_catalogs: ModelCatalogs::default(),
            handler_waker: None,
            state_dirs_checked: HashSet::new(),
            status_hold_until: None,
        }
    }

    /// An app for the command-line and Omarchy bar paths, which draw no
    /// dashboard. It owns no dashboard pane, so an agent in the pane that ran
    /// it is listed like any other, and a launch leaves focus where it is.
    fn headless(client: HerdrClient) -> Self {
        let inputs = AppInputs {
            dashboard_pane_id: None,
            providers: installed_providers(),
            projects: ProjectMemory::load(),
            cards: CardMemory::default(),
        };
        let mut app = Self::with_inputs(client, true, inputs);
        app.restore_focus_after_launch = false;
        app
    }

    /// Shows `message` in the footer. With a `hold`, the snapshot refreshes
    /// in that time leave it in place instead of replacing it with the agent
    /// count; without one, the next refresh may replace it.
    fn set_status(&mut self, message: impl Into<String>, hold: Option<Duration>) {
        self.status = message.into();
        self.status_hold_until = hold.map(|hold| Instant::now() + hold);
    }

    /// Tells the user, once per project, when the state a project's handler
    /// had before the rename sits beside its current state directory instead
    /// of having been moved into it, so that history is not overlooked. Only
    /// the interactive dashboard, the one with a waker, checks.
    fn warn_about_old_state_dirs(&mut self) {
        if self.handler_waker.is_none() {
            return;
        }
        let roots: Vec<String> = self
            .agents
            .iter()
            .filter(|agent| agent.handler)
            .map(|agent| agent.project_root.clone())
            .filter(|root| !self.state_dirs_checked.contains(root))
            .collect();
        let mut warnings = Vec::new();
        for root in roots {
            warnings.extend(handler::state_dir_conflict(&root));
            self.state_dirs_checked.insert(root);
        }
        if !warnings.is_empty() {
            self.set_status(warnings.join("  "), Some(Duration::from_secs(60)));
        }
    }

    /// Makes the next pass of the dashboard loop take a fresh snapshot.
    fn request_refresh(&mut self) {
        self.last_refresh = None;
    }

    /// Whether the dashboard loop is due for a fresh snapshot.
    fn refresh_due(&self) -> bool {
        is_due(self.last_refresh, REFRESH_INTERVAL)
    }

    fn refresh(&mut self) {
        self.last_refresh = Some(Instant::now());
        match self.client.snapshot() {
            Ok(mut snapshot) => {
                self.connected = true;
                // The dashboard that wakes handlers is the one that puts back
                // the marks a Herdr restart lost, before anything reads them.
                if self
                    .handler_waker
                    .as_mut()
                    .is_some_and(|waker| waker.leads(&self.client))
                {
                    self.restore_markers(&mut snapshot);
                }
                relabel_project_mains(&self.client, &mut snapshot.workspaces);
                self.install_snapshot(snapshot);
                self.warn_about_old_state_dirs();
                self.wake_handlers();
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
                self.status = format!("Herdr unavailable: {error:#}");
            }
        }
    }

    fn install_snapshot(&mut self, snapshot: SessionSnapshot) {
        let workspaces: HashMap<&str, &WorkspaceInfo> = snapshot
            .workspaces
            .iter()
            .map(|workspace| (workspace.workspace_id.as_str(), workspace))
            .collect();
        let dashboard_pane_ids = self.dashboard_pane_ids(&snapshot.panes);

        let mut agents: Vec<_> = snapshot
            .agents
            .into_iter()
            // Herdr's detector can briefly identify Corgi's own terminal as
            // an agent while the pane starts. Never render the dashboard as a
            // managed agent, including after its launcher has moved it and
            // changed the original HERDR_PANE_ID.
            .filter(|info| {
                !dashboard_pane_ids.contains(info.pane_id.as_str()) && has_real_agent_session(info)
            })
            .map(|info| {
                let workspace = workspaces.get(info.workspace_id.as_str()).copied();
                self.dashboard_agent(info, workspace)
            })
            .collect();
        sort_agents(&mut agents);
        // The selection stays on its row of the list, which is a whole card
        // for a collapsed project, rather than on its index into the agents.
        let row = self.selected_stop();
        self.agents = agents;
        self.select_stop(row);
        self.forget_gone_sessions();
        self.refresh_codex_task_titles();
        self.note_open_projects(&snapshot.workspaces);
        self.workspaces = snapshot.workspaces;
        self.clamp_selection();
    }

    /// One agent's dashboard row: where it works, what it is doing, and what
    /// its session says about itself.
    fn dashboard_agent(
        &mut self,
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
        let task = task_summary(
            &info,
            workspace,
            &project,
            self.codex_task_title(&info),
            facts.task.as_deref(),
        );
        let model = facts.model.or_else(|| reported_model(&info));
        let effort = facts.effort.or_else(|| reported_effort(&info));
        let context_percent = facts
            .context_percent
            .or_else(|| reported_context_percent(&info));
        let (message, tool) = self.conversation_rows(&info, facts.message, facts.tool);
        let handler = is_handler(&info);
        let (project, task) = if handler {
            (HANDLER_NAME.to_string(), HANDLER_NAME.to_string())
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
            handler,
            scratch,
        }
    }

    /// Drops what is remembered about sessions no longer on the dashboard.
    fn forget_gone_sessions(&mut self) {
        self.sessions
            .retain(self.agents.iter().map(|agent| &agent.info));
        let live_codex_sessions: HashSet<_> = self
            .agents
            .iter()
            .filter_map(|agent| codex_thread_id(&agent.info))
            .collect();
        self.codex_task_titles
            .retain(|session_id, _| live_codex_sessions.contains(session_id.as_str()));
    }

    /// A repository whose primary workspace is open in Herdr is a project,
    /// agent or not, and stays one for the project list after it is closed.
    fn note_open_projects(&mut self, workspaces: &[WorkspaceInfo]) {
        for root in workspaces.iter().filter_map(|workspace| {
            workspace
                .worktree
                .as_ref()
                .filter(|worktree| !worktree.is_linked_worktree)
                .and_then(|_| workspace.repo_root())
        }) {
            self.projects.note(root);
        }
    }

    /// Keeps the selection on the list after agents went away, and the
    /// expanded transcript on the session now selected.
    fn clamp_selection(&mut self) {
        if self.agents.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.agents.len() - 1);
        }
        if self.expanded {
            if self.agents.is_empty() {
                // The session being read is gone; there is nothing to zoom in
                // on, so fall back to the list rather than an empty panel.
                self.expanded = false;
                self.transcript = Default::default();
                self.transcript_scroll = 0;
            } else {
                self.load_transcript();
            }
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
    fn conversation_rows(
        &mut self,
        info: &AgentInfo,
        message: Option<Activity>,
        tool: Option<Activity>,
    ) -> (Activity, Activity) {
        let previous = self.cached_rows.remove(&info.pane_id);
        let (mut message, mut tool) = (message, tool);
        let blocked = info.state == AgentState::Blocked;
        if (message.is_none() || tool.is_none() || blocked)
            && let Some(screen) = self.read_screen(info)
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

    fn read_screen(&self, info: &AgentInfo) -> Option<String> {
        self.client
            .read_agent(&info.pane_id, ReadSource::Detection, Some(64))
            .or_else(|_| {
                self.client
                    .read_agent(&info.pane_id, ReadSource::Visible, Some(64))
            })
            .ok()
            .map(|read| read.text)
    }

    fn selected_agent(&self) -> Option<&DashboardAgent> {
        self.agents.get(self.selected)
    }

    /// Moves the cursor to row `index`.
    fn select(&mut self, index: usize) {
        self.selected = index;
        // Moving the cursor while zoomed in is how the reader pages through
        // sessions, so the expanded view follows it to the newest turn.
        if self.expanded {
            self.transcript_scroll = 0;
            self.load_transcript();
        }
    }

    /// Zooms the selected session over the whole agent list, or back out.
    fn toggle_transcript(&mut self) {
        if self.expanded {
            self.expanded = false;
            self.transcript = Default::default();
            self.transcript_scroll = 0;
            self.status = "Collapsed".into();
            return;
        }
        if self.selected_agent().is_none() {
            self.status = "No agent selected".into();
            return;
        }
        self.expanded = true;
        self.transcript_scroll = 0;
        self.load_transcript();
        if self.transcript.is_empty() {
            self.status = "No transcript for this session yet".into();
        } else {
            self.status = "Expanded".into();
        }
    }

    /// Re-reads the selected session's latest turns. The reader caches by file
    /// length, so a session that has not written anything since the last
    /// refresh costs nothing here.
    fn load_transcript(&mut self) {
        let Some(agent) = self.agents.get(self.selected) else {
            self.transcript = Default::default();
            return;
        };
        self.transcript = self.sessions.transcript(&agent.info);
    }

    /// Scrolls the expanded transcript by whole screens. The renderer knows
    /// how tall the content actually is, so it clamps what this sets.
    fn scroll_transcript(&mut self, pages: i32) {
        let step = i32::from(self.transcript_page.max(1));
        let scroll = i32::from(self.transcript_scroll) + pages * step;
        self.transcript_scroll = scroll.clamp(0, i32::from(u16::MAX)) as u16;
    }

    /// Closes whichever form or dialog is open over the agent list.
    fn cancel_editor(&mut self) {
        self.overlay = Overlay::None;
        self.status = "Cancelled".into();
    }

    /// Checks the usage cache every Corgi process shares, which fetches a
    /// new reading only when the cached one is older than
    /// [`USAGE_REFRESH_INTERVAL`] and no other process is fetching it.
    fn refresh_plan_usage(&mut self) {
        if self.usage.is_empty()
            || self.usage_job.is_running()
            || !is_due(self.last_usage_refresh, USAGE_CACHE_CHECK_INTERVAL)
        {
            return;
        }
        self.last_usage_refresh = Some(Instant::now());
        let providers: Vec<Provider> = self.usage.iter().map(|slot| slot.provider).collect();
        self.usage_job = Job::spawn(move |_| {
            providers
                .into_iter()
                .map(|provider| {
                    (
                        provider,
                        shared_plan_usage(provider, USAGE_REFRESH_INTERVAL),
                    )
                })
                .collect()
        });
    }

    fn poll_plan_usage(&mut self) {
        let Some(outcome) = self.usage_job.outcome() else {
            return;
        };
        // A reader that panicked brings no results; the next interval starts
        // a new one.
        let results = outcome.unwrap_or_default();
        for (provider, cached) in results {
            let Some(slot) = self.usage.iter_mut().find(|slot| slot.provider == provider) else {
                continue;
            };
            // Nothing cached yet: another process is fetching the first
            // reading, and a later check picks it up.
            let Some(cached) = cached else {
                continue;
            };
            if let Some(usage) = cached.usage {
                slot.usage = Some(usage);
                slot.fetched_at = cached.fetched_at;
            }
            slot.error = cached.error;
        }
    }

    /// The generated name is Codex's best compact description of a task. Read
    /// every live Codex thread in one short-lived app-server session, outside
    /// the UI thread. Claude Code has no equivalent local API, so its terminal
    /// title and the common transcript fallback remain unchanged.
    fn refresh_codex_task_titles(&mut self) {
        if self.codex_task_job.is_running()
            || !is_due(self.last_codex_task_refresh, CODEX_TASK_REFRESH_INTERVAL)
        {
            return;
        }
        let thread_ids: Vec<String> = self
            .agents
            .iter()
            .filter_map(|agent| codex_thread_id(&agent.info))
            .map(str::to_string)
            .collect();
        if thread_ids.is_empty() {
            return;
        }

        self.last_codex_task_refresh = Some(Instant::now());
        self.codex_task_job = Job::spawn(move |_| {
            read_codex_thread_titles(&thread_ids).map_err(|error| format!("{error:#}"))
        });
    }

    /// Makes a freshly generated Codex title visible on the next snapshot. A
    /// failed experimental app-server read is intentionally silent: the shared
    /// transcript task remains visible and this will retry shortly.
    fn poll_codex_task_titles(&mut self) {
        // A reader that panicked brings no titles; like a failed read, it is
        // retried shortly.
        if let Some(Ok(Ok(titles))) = self.codex_task_job.outcome() {
            self.codex_task_titles.extend(titles);
            self.request_refresh();
        }
    }

    fn codex_task_title(&self, info: &AgentInfo) -> Option<&str> {
        codex_thread_id(info)
            .and_then(|session_id| self.codex_task_titles.get(session_id))
            .map(String::as_str)
    }

    fn focus_selected(&mut self) {
        self.focus_selected_saying("Focused agent pane".into());
    }

    /// Brings the selected agent's pane into view, its workspace and tab
    /// first, and says `focused` once it is.
    fn focus_selected_saying(&mut self, focused: String) {
        let Some(info) = self.selected_agent().map(|agent| agent.info.clone()) else {
            return;
        };
        match self.client.go_to_agent(&info) {
            Ok(_) => self.status = focused,
            Err(error) => self.status = format!("Focus failed: {error:#}"),
        }
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        self.motion.key_pressed();
        match self.overlay {
            Overlay::None => self.handle_normal_key(key),
            Overlay::Prompt(_) => self.handle_prompt_key(key),
            Overlay::NewAgent(_) => self.handle_new_agent_key(key),
            Overlay::Launch(_) => self.handle_launch_key(key),
            Overlay::Close(_) => self.handle_close_workspace_key(key),
            Overlay::Merge(_) => self.handle_merge_worktree_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(' ') => self.toggle_transcript(),
            // Zoomed in, Escape is the way back out to the list; only the
            // list itself answers it by closing Corgi.
            KeyCode::Esc if self.expanded => self.toggle_transcript(),
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Right => self.expand_card(),
            KeyCode::Left => self.collapse_card(),
            KeyCode::PageUp if self.expanded => self.scroll_transcript(-1),
            KeyCode::PageDown if self.expanded => self.scroll_transcript(1),
            // Page keys need a modifier on a laptop keyboard, so the plain
            // pager pair scrolls too and is what the hints name.
            KeyCode::Char('u') if self.expanded => self.scroll_transcript(-1),
            KeyCode::Char('d') if self.expanded => self.scroll_transcript(1),
            KeyCode::Home if self.expanded => self.transcript_scroll = 0,
            KeyCode::End if self.expanded => self.transcript_scroll = u16::MAX,
            KeyCode::Char('p') => self.begin_prompt(),
            KeyCode::Char('n') => self.begin_new_agent(),
            KeyCode::Char('t') => self.begin_scratch_agent(),
            KeyCode::Char('h') => self.begin_handler(),
            KeyCode::Char('f') | KeyCode::Enter => self.focus_selected(),
            KeyCode::Char('m') => self.begin_merge_worktree(),
            KeyCode::Char('x') => self.begin_close_workspace(),
            KeyCode::Char('r') => self.request_refresh(),
            _ => {}
        }
        false
    }
}

/// Keeps related worktree sessions together, each project led by its
/// handler, and the scratch sessions after every project. Within a project
/// the rest follow their live state, the ones needing attention first:
/// blocked, working, done, idle, then unknown. Agents sharing a state are
/// ordered by when they entered it, the most recent first, using Herdr's
/// `state_change_seq`: one counter across the whole Herdr session that is
/// bumped on every state change, so it compares between agents. A row
/// therefore moves only when its own agent changes state, which is rare
/// enough to be worth seeing at a glance what is blocked or busy; focus stays
/// out of the key so moving between panes never reorders the list. Equal
/// counters (older Herdr, or fixtures) fall back to name, then pane ID.
fn sort_agents(agents: &mut [DashboardAgent]) {
    agents.sort_by(|left, right| {
        left.scratch
            .cmp(&right.scratch)
            .then_with(|| {
                left.project_group
                    .to_lowercase()
                    .cmp(&right.project_group.to_lowercase())
            })
            .then_with(|| right.handler.cmp(&left.handler))
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
fn is_due(last: Option<Instant>, interval: Duration) -> bool {
    last.is_none_or(|at| at.elapsed() >= interval)
}

pub fn run() -> Result<()> {
    let compact = env::args().any(|arg| arg == "--compact");
    let client = HerdrClient::from_env().context("Corgi must run inside a Herdr plugin pane")?;
    let mut app = App::new(client, compact);
    app.handler_waker = Some(HandlerWaker::default());
    app.motion = Motion::animated();
    app.refresh();

    let mut terminal = init_terminal()?;
    let result = run_loop(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    loop {
        let frame_started = Instant::now();
        app.poll_launch();
        app.poll_plan_usage();
        app.poll_codex_task_titles();
        app.poll_model_catalogs();
        app.poll_merge();
        app.close_started_launch();
        app.refresh_plan_usage();
        // A snapshot waits for a popup to stop moving, at most half a
        // second, so the refresh's round trips never stall an animation.
        if app.refresh_due() && !app.launch_job.is_running() && !app.motion.is_moving() {
            app.refresh();
        }
        terminal.draw(|frame| ui::draw(frame, app))?;
        // Fast while a popup moves, and the usual cadence otherwise, so an
        // idle dashboard costs no more than it did before popups moved.
        // Writing a frame to the terminal takes a good part of a sixtieth of
        // a second, so while something moves that time counts towards the
        // frame; at rest the loop waits its whole cadence after drawing, as
        // it always has.
        let wait = app
            .motion
            .frame_interval(app.overlay.spins(), app.overlay.blinks());
        let wait = if app.motion.is_moving() {
            wait.saturating_sub(frame_started.elapsed())
        } else {
            wait
        };
        if event::poll(wait)?
            && let Event::Key(key) = event::read()?
            && app.handle_key(key)
        {
            return Ok(());
        }
    }
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // A steady caret, because Corgi blinks it itself: shown and hidden on
    // its own clock, and on at once under the user's typing.
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        SetCursorStyle::SteadyBar
    )?;
    Terminal::new(CrosstermBackend::new(stdout)).map_err(Into::into)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        SetCursorStyle::DefaultUserShape
    )?;
    terminal.show_cursor()?;
    Ok(())
}

#[cfg(test)]
mod test_helpers;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crossterm::event::{KeyCode, KeyModifiers};

    use crate::{
        app::{
            launch::sanitize_agent_name,
            project_main::{
                CORGI_PROJECT_MAIN_ROLE, CORGI_PROJECT_MAIN_TAB_TOKEN, CORGI_WORKSPACE_ROLE_TOKEN,
            },
        },
        handler::CORGI_HANDLER_TOKEN,
        herdr::{PaneInfo, SessionSnapshot},
        model::{
            AgentInfo, AgentSession, AgentState, DashboardAgent, WorkspaceInfo,
            WorkspaceWorktreeInfo,
        },
        test_support::test_app,
    };

    use super::{
        test_helpers::{poll_until, press},
        *,
    };

    fn sortable_agent(
        pane_id: &str,
        name: &str,
        group: &str,
        state: AgentState,
        handler: bool,
        scratch: bool,
    ) -> DashboardAgent {
        DashboardAgent {
            info: AgentInfo {
                pane_id: pane_id.into(),
                name: Some(name.into()),
                state,
                ..AgentInfo::default()
            },
            project_group: group.into(),
            handler,
            scratch,
            ..DashboardAgent::default()
        }
    }

    fn sorted_panes(mut agents: Vec<DashboardAgent>) -> Vec<String> {
        sort_agents(&mut agents);
        agents.into_iter().map(|agent| agent.info.pane_id).collect()
    }

    #[test]
    fn agents_in_a_project_follow_the_handler_by_state_then_name() {
        let agents = vec![
            sortable_agent("unknown", "a", "corgi", AgentState::Unknown, false, false),
            sortable_agent("idle", "a", "corgi", AgentState::Idle, false, false),
            sortable_agent("done", "a", "corgi", AgentState::Done, false, false),
            sortable_agent("working-b", "B", "corgi", AgentState::Working, false, false),
            sortable_agent("working-a", "a", "corgi", AgentState::Working, false, false),
            sortable_agent("blocked", "z", "corgi", AgentState::Blocked, false, false),
            sortable_agent("handler", "z", "corgi", AgentState::Idle, true, false),
        ];
        assert_eq!(
            sorted_panes(agents),
            [
                "handler",
                "blocked",
                "working-a",
                "working-b",
                "done",
                "idle",
                "unknown"
            ]
        );
    }

    fn changed_at(mut agent: DashboardAgent, change: u64) -> DashboardAgent {
        agent.info.state_change_seq = change;
        agent
    }

    #[test]
    fn agents_sharing_a_state_put_the_latest_change_first() {
        let agents = vec![
            changed_at(
                sortable_agent(
                    "working-old",
                    "a",
                    "corgi",
                    AgentState::Working,
                    false,
                    false,
                ),
                10,
            ),
            changed_at(
                sortable_agent("idle-new", "a", "corgi", AgentState::Idle, false, false),
                40,
            ),
            changed_at(
                sortable_agent(
                    "working-new",
                    "z",
                    "corgi",
                    AgentState::Working,
                    false,
                    false,
                ),
                30,
            ),
            changed_at(
                sortable_agent("idle-old", "z", "corgi", AgentState::Idle, false, false),
                20,
            ),
            changed_at(
                sortable_agent("handler", "m", "corgi", AgentState::Idle, true, false),
                5,
            ),
            changed_at(
                sortable_agent("blocked", "m", "corgi", AgentState::Blocked, false, false),
                1,
            ),
        ];
        assert_eq!(
            sorted_panes(agents),
            [
                "handler",
                "blocked",
                "working-new",
                "working-old",
                "idle-new",
                "idle-old"
            ]
        );
    }

    #[test]
    fn a_state_change_moves_only_the_agent_that_changed() {
        let before = vec![
            changed_at(
                sortable_agent("a", "a", "corgi", AgentState::Idle, false, false),
                3,
            ),
            changed_at(
                sortable_agent("b", "b", "corgi", AgentState::Idle, false, false),
                2,
            ),
            changed_at(
                sortable_agent("c", "c", "corgi", AgentState::Idle, false, false),
                1,
            ),
        ];
        assert_eq!(sorted_panes(before.clone()), ["a", "b", "c"]);

        // A refresh where "a", already the latest, changed again reorders
        // nothing; once "c" changes, it alone moves to the top.
        let mut after = before;
        after[0].info.state_change_seq = 5;
        assert_eq!(sorted_panes(after.clone()), ["a", "b", "c"]);
        after[2].info.state_change_seq = 6;
        assert_eq!(sorted_panes(after), ["c", "a", "b"]);
    }

    #[test]
    fn agents_that_changed_together_fall_back_to_name_then_pane() {
        let agents = vec![
            changed_at(
                sortable_agent("p2", "Same", "corgi", AgentState::Done, false, false),
                7,
            ),
            changed_at(
                sortable_agent("p1", "same", "corgi", AgentState::Done, false, false),
                7,
            ),
            changed_at(
                sortable_agent("beta", "beta", "corgi", AgentState::Done, false, false),
                7,
            ),
            changed_at(
                sortable_agent("Alpha", "Alpha", "corgi", AgentState::Done, false, false),
                7,
            ),
        ];
        assert_eq!(sorted_panes(agents), ["Alpha", "beta", "p1", "p2"]);
    }

    #[test]
    fn state_orders_within_each_group_without_mixing_groups() {
        let agents = vec![
            sortable_agent("scratch-idle", "a", "~", AgentState::Idle, false, true),
            sortable_agent(
                "scratch-blocked",
                "b",
                "~",
                AgentState::Blocked,
                false,
                true,
            ),
            sortable_agent(
                "zebra-blocked",
                "a",
                "zebra",
                AgentState::Blocked,
                false,
                false,
            ),
            sortable_agent("alpha-idle", "a", "alpha", AgentState::Idle, false, false),
            sortable_agent(
                "alpha-working",
                "b",
                "alpha",
                AgentState::Working,
                false,
                false,
            ),
        ];
        assert_eq!(
            sorted_panes(agents),
            [
                "alpha-working",
                "alpha-idle",
                "zebra-blocked",
                "scratch-blocked",
                "scratch-idle"
            ]
        );
    }

    #[test]
    fn agent_names_are_herdr_safe() {
        assert_eq!(sanitize_agent_name("Corgi Dashboard!"), "corgi-dashboard");
        assert_eq!(sanitize_agent_name("42 things"), "agent-42-things");
        assert_eq!(sanitize_agent_name("---"), "agent");
    }

    #[test]
    fn space_zooms_the_selected_session_and_escape_zooms_back_out() {
        let mut app = test_app();
        app.agents = vec![
            DashboardAgent {
                info: AgentInfo {
                    pane_id: "p1".into(),
                    ..AgentInfo::default()
                },
                ..DashboardAgent::default()
            },
            DashboardAgent {
                info: AgentInfo {
                    pane_id: "p2".into(),
                    ..AgentInfo::default()
                },
                ..DashboardAgent::default()
            },
        ];

        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
        assert!(app.expanded);

        // Paging moves by the screenful the renderer last drew, from either
        // the page keys or the plain pair a laptop keyboard reaches without
        // a modifier.
        app.transcript_page = 10;
        press(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        assert_eq!(app.transcript_scroll, 10);
        press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
        assert_eq!(app.transcript_scroll, 20);
        press(&mut app, KeyCode::Char('u'), KeyModifiers::NONE);
        press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(app.transcript_scroll, 0);
        press(&mut app, KeyCode::End, KeyModifiers::NONE);
        assert_eq!(app.transcript_scroll, u16::MAX);
        press(&mut app, KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(app.transcript_scroll, 0);

        // Moving the cursor shows the next session from its newest turn.
        app.transcript_scroll = 4;
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.selected, 1);
        assert_eq!(app.transcript_scroll, 0);

        // Zoomed in, Escape goes back to the list instead of closing Corgi.
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Esc)));
        assert!(!app.expanded);
        assert!(app.transcript.is_empty());
        // Collapsed, the scroll keys belong to nothing and stay silent.
        press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
        assert_eq!(app.transcript_scroll, 0);
        // From the list Escape closes Corgi, as it always did.
        assert!(app.handle_key(KeyEvent::from(KeyCode::Esc)));
    }

    #[test]
    fn space_with_nothing_selected_says_so_instead_of_zooming_into_nothing() {
        let mut app = test_app();

        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);

        assert!(!app.expanded);
        assert_eq!(app.status, "No agent selected");
    }

    #[test]
    fn snapshot_hides_dashboard_when_its_pane_id_changed() {
        let mut app = test_app();
        // The launcher opens the pane as w0:p1, then moves it into Corgi's
        // workspace where Herdr renumbers it. The environment still contains
        // the first ID, so the live plugin-pane label is the reliable match.
        app.dashboard_pane_id = Some("w0:p1".into());

        app.install_snapshot(SessionSnapshot {
            panes: vec![PaneInfo {
                pane_id: "w9:p1".into(),
                label: Some("Corgi".into()),
                ..PaneInfo::default()
            }],
            agents: vec![
                AgentInfo {
                    pane_id: "w9:p1".into(),
                    agent: Some("codex".into()),
                    ..AgentInfo::default()
                },
                AgentInfo {
                    pane_id: "w2:p1".into(),
                    agent: Some("codex".into()),
                    name: Some("real-agent".into()),
                    agent_session: Some(AgentSession {
                        value: "real-session".into(),
                        ..AgentSession::default()
                    }),
                    ..AgentInfo::default()
                },
            ],
            ..SessionSnapshot::default()
        });

        assert_eq!(app.agents.len(), 1);
        assert_eq!(app.agents[0].info.pane_id, "w2:p1");
    }

    #[test]
    fn snapshot_hides_detector_only_panes_without_a_session_identity() {
        let mut app = test_app();

        app.install_snapshot(SessionSnapshot {
            agents: vec![
                AgentInfo {
                    pane_id: "w-preview:p1".into(),
                    workspace_id: "w-preview".into(),
                    agent: Some("codex".into()),
                    state: AgentState::Idle,
                    ..AgentInfo::default()
                },
                AgentInfo {
                    pane_id: "w-real:p1".into(),
                    workspace_id: "w-real".into(),
                    agent: Some("codex".into()),
                    state: AgentState::Idle,
                    agent_session: Some(AgentSession {
                        value: "real-session".into(),
                        ..AgentSession::default()
                    }),
                    ..AgentInfo::default()
                },
            ],
            ..SessionSnapshot::default()
        });

        assert_eq!(app.agents.len(), 1);
        assert_eq!(app.agents[0].info.pane_id, "w-real:p1");
        assert!(has_real_agent_session(&app.agents[0].info));
    }

    #[test]
    fn snapshot_groups_projects_and_orders_their_sessions_by_state() {
        let mut app = test_app();
        let worktree = |repo_name: &str, checkout: &str| WorkspaceInfo {
            worktree: Some(WorkspaceWorktreeInfo {
                repo_name: repo_name.into(),
                repo_root: format!("/projects/{repo_name}"),
                checkout_path: checkout.into(),
                is_linked_worktree: true,
            }),
            ..WorkspaceInfo::default()
        };
        let mut corgi_a = worktree("corgi", "/worktrees/corgi/alpha");
        corgi_a.workspace_id = "w1".into();
        let mut corgi_z = worktree("corgi", "/worktrees/corgi/zebra");
        corgi_z.workspace_id = "w2".into();
        let mut weather = worktree("weather-cottage", "/worktrees/weather-cottage/beta");
        weather.workspace_id = "w3".into();

        app.install_snapshot(SessionSnapshot {
            workspaces: vec![corgi_z, weather, corgi_a],
            agents: vec![
                AgentInfo {
                    pane_id: "p-zebra".into(),
                    workspace_id: "w2".into(),
                    cwd: Some("/worktrees/corgi/zebra".into()),
                    name: Some("Zebra".into()),
                    state: AgentState::Blocked,
                    agent_session: Some(AgentSession {
                        value: "zebra-session".into(),
                        ..AgentSession::default()
                    }),
                    ..AgentInfo::default()
                },
                AgentInfo {
                    pane_id: "p-weather".into(),
                    workspace_id: "w3".into(),
                    cwd: Some("/worktrees/weather-cottage/beta".into()),
                    name: Some("Beta".into()),
                    state: AgentState::Working,
                    agent_session: Some(AgentSession {
                        value: "beta-session".into(),
                        ..AgentSession::default()
                    }),
                    ..AgentInfo::default()
                },
                AgentInfo {
                    pane_id: "p-alpha".into(),
                    workspace_id: "w1".into(),
                    cwd: Some("/worktrees/corgi/alpha".into()),
                    name: Some("Alpha".into()),
                    state: AgentState::Done,
                    agent_session: Some(AgentSession {
                        value: "alpha-session".into(),
                        ..AgentSession::default()
                    }),
                    ..AgentInfo::default()
                },
            ],
            ..SessionSnapshot::default()
        });

        assert_eq!(
            app.agents
                .iter()
                .map(|agent| (agent.project_group.as_str(), agent.info.display_name()))
                .collect::<Vec<_>>(),
            vec![
                ("corgi", "Zebra"),
                ("corgi", "Alpha"),
                ("weather-cottage", "Beta"),
            ]
        );
    }

    #[test]
    fn agents_in_the_home_directory_are_grouped_under_scratch_after_every_project() {
        let Some(home) = crate::paths::home() else {
            return;
        };
        let home = home.to_string_lossy().into_owned();
        let mut app = test_app();
        let agent = |pane_id: &str, workspace_id: &str, cwd: &str, name: &str| AgentInfo {
            pane_id: pane_id.into(),
            workspace_id: workspace_id.into(),
            cwd: Some(cwd.into()),
            name: Some(name.into()),
            agent_session: Some(AgentSession {
                value: format!("{name}-session"),
                ..AgentSession::default()
            }),
            ..AgentInfo::default()
        };
        let workspace = |workspace_id: &str, label: &str| WorkspaceInfo {
            workspace_id: workspace_id.into(),
            label: label.into(),
            ..WorkspaceInfo::default()
        };
        app.install_snapshot(SessionSnapshot {
            workspaces: vec![
                workspace("w1", "scratch"),
                workspace("w2", "zoo"),
                workspace("w3", "scratch 2"),
            ],
            agents: vec![
                agent("w1:p1", "w1", &home, "alpha"),
                agent("w2:p1", "w2", "/projects/zoo", "zoo"),
                agent("w3:p1", "w3", &format!("{home}/"), "beta"),
            ],
            ..SessionSnapshot::default()
        });

        assert_eq!(
            app.agents
                .iter()
                .map(|agent| (
                    agent.project_group.as_str(),
                    agent.project.as_str(),
                    agent.scratch
                ))
                .collect::<Vec<_>>(),
            vec![
                ("zoo", "zoo", false),
                ("Scratch", "scratch", true),
                ("Scratch", "scratch 2", true),
            ]
        );
        // The home directory is offered as no project.
        assert!(
            crate::projects::known_projects(&app.agents, &app.workspaces, &app.projects)
                .iter()
                .all(|project| project.root != home)
        );
    }

    #[test]
    fn the_marked_handler_leads_its_project_wherever_its_pane_is() {
        let mut app = test_app();
        let main = WorkspaceInfo {
            workspace_id: "w-main".into(),
            label: "corgi handler".into(),
            tokens: BTreeMap::from([
                (
                    CORGI_WORKSPACE_ROLE_TOKEN.into(),
                    CORGI_PROJECT_MAIN_ROLE.into(),
                ),
                (CORGI_PROJECT_MAIN_TAB_TOKEN.into(), "w-main:t1".into()),
            ]),
            worktree: Some(WorkspaceWorktreeInfo {
                repo_name: "corgi".into(),
                repo_root: "/projects/corgi".into(),
                checkout_path: "/projects/corgi".into(),
                is_linked_worktree: false,
            }),
        };
        let worker = WorkspaceInfo {
            workspace_id: "w-worker".into(),
            worktree: Some(WorkspaceWorktreeInfo {
                repo_name: "corgi".into(),
                repo_root: "/projects/corgi".into(),
                checkout_path: "/worktrees/corgi/alpha".into(),
                is_linked_worktree: true,
            }),
            ..WorkspaceInfo::default()
        };
        let agent =
            |pane: &str, workspace: &str, tab: &str, cwd: &str, name: &str, marker: &str| {
                AgentInfo {
                    pane_id: pane.into(),
                    workspace_id: workspace.into(),
                    tab_id: tab.into(),
                    cwd: Some(cwd.into()),
                    name: Some(name.into()),
                    // A busy handler with a task title of its own still leads.
                    state: AgentState::Working,
                    title: Some("Reviewing the ledger".into()),
                    tokens: if marker.is_empty() {
                        BTreeMap::new()
                    } else {
                        BTreeMap::from([(CORGI_HANDLER_TOKEN.into(), marker.into())])
                    },
                    agent_session: Some(AgentSession {
                        value: format!("{pane}-session"),
                        ..AgentSession::default()
                    }),
                    ..AgentInfo::default()
                }
            };

        app.install_snapshot(SessionSnapshot {
            workspaces: vec![worker, main],
            agents: vec![
                agent(
                    "p-alpha",
                    "w-worker",
                    "w-worker:t1",
                    "/worktrees/corgi/alpha",
                    "alpha",
                    "",
                ),
                // A worker in the root tab is not the handler...
                agent(
                    "p-root",
                    "w-main",
                    "w-main:t1",
                    "/projects/corgi",
                    "aardvark",
                    "",
                ),
                // ...nor is a later agent in a pane that once held one.
                agent(
                    "p-reused",
                    "w-main",
                    "w-main:t3",
                    "/projects/corgi",
                    "beaver",
                    "handler-old",
                ),
                // The marked handler, moved out of the root tab.
                agent(
                    "p-handler",
                    "w-main",
                    "w-main:t2",
                    "/projects/corgi",
                    "start-your-handler-session-for-m",
                    "session:p-handler-session",
                ),
            ],
            ..SessionSnapshot::default()
        });

        assert_eq!(
            app.agents
                .iter()
                .map(|agent| (
                    agent.info.pane_id.as_str(),
                    agent.handler,
                    agent.project.as_str(),
                    agent.task.as_str(),
                ))
                .collect::<Vec<_>>(),
            vec![
                ("p-handler", true, "Project handler", "Project handler"),
                ("p-root", false, "corgi", "Reviewing the ledger"),
                ("p-alpha", false, "corgi/alpha", "Reviewing the ledger"),
                ("p-reused", false, "corgi", "Reviewing the ledger"),
            ]
        );
        assert!(
            super::launch::handler_plan(
                &app,
                "/projects/corgi",
                crate::harness::Harness::Codex,
                String::new(),
                String::new(),
                String::new()
            )
            .is_err(),
            "a renamed Project handler prevents a duplicate launch"
        );
    }

    #[test]
    fn a_new_dashboard_and_a_requested_refresh_are_due_at_once() {
        let mut app = test_app();
        assert!(app.refresh_due());
        app.last_refresh = Some(Instant::now());
        assert!(!app.refresh_due());
        app.request_refresh();
        assert!(app.refresh_due());
        // Usage and Codex titles start due as well, whatever the clock's age.
        assert!(super::is_due(
            app.last_usage_refresh,
            super::USAGE_CACHE_CHECK_INTERVAL
        ));
        assert!(super::is_due(
            app.last_codex_task_refresh,
            super::CODEX_TASK_REFRESH_INTERVAL
        ));
    }

    #[test]
    fn a_held_status_survives_refreshes_and_a_plain_one_does_not() {
        let mut app = test_app();
        app.set_status("Prompt sent", Some(Duration::from_secs(5)));
        assert_eq!(app.status, "Prompt sent");
        assert!(
            app.status_hold_until
                .is_some_and(|until| until > Instant::now())
        );
        app.set_status("Collapsed", None);
        assert!(app.status_hold_until.is_none());
    }

    #[test]
    fn a_worker_that_panics_does_not_stall_the_dashboard() {
        let mut app = test_app();
        app.usage_job = Job::spawn(|_| panic!("usage reader failed"));
        app.codex_task_job = Job::spawn(|_| panic!("title reader failed"));
        poll_until(&mut app, App::poll_plan_usage, |app| {
            !app.usage_job.is_running()
        });
        poll_until(&mut app, App::poll_codex_task_titles, |app| {
            !app.codex_task_job.is_running()
        });

        app.overlay = Overlay::Launch(NewAgentLaunch {
            name: "corgi-worker".into(),
            state: LaunchState::Running {
                status: "Starting corgi-worker…".into(),
                step: 0,
            },
            steps: Vec::new(),
        });
        app.launch_job = Job::spawn(|progress| {
            progress.send(JobReport::Status("Sending first prompt…".into()));
            panic!("launch failed");
        });
        app.last_refresh = Some(Instant::now());
        poll_until(&mut app, App::poll_launch, |app| {
            !app.launch_job.is_running()
        });
        assert!(app.refresh_due());
        assert!(
            app.overlay.new_agent_launch().is_some_and(
                |launch| matches!(&launch.state, LaunchState::Failed(error) if error.contains("stopped without reporting"))
            )
        );

        app.overlay = Overlay::merge(MergeWorktreeForm {
            label: "corgi".into(),
            workspace_id: "w-worker".into(),
            project_root: "/repos/corgi".into(),
            worktree_checkout: "/worktrees/corgi/quiet-owl".into(),
            source_branch: "worktree/quiet-owl".into(),
            target_branch: "main".into(),
            task: String::new(),
            commits: Vec::new(),
            phase: MergePhase::Running(1),
        });
        app.merge_job = Job::spawn(|_| panic!("merge failed"));
        poll_until(&mut app, App::poll_merge, |app| !app.merge_job.is_running());
        assert!(matches!(
            app.overlay.merge_worktree_form().map(|form| &form.phase),
            Some(MergePhase::Failed(_))
        ));
    }
}
