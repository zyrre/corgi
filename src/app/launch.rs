//! The launch sequence every new agent goes through: the plan, its checkout
//! or tab, starting the CLI, and the supervisor's launch plan; and the
//! dashboard's background job that runs it, with the panel that follows it.

use std::{
    env, fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use crossterm::event::{KeyCode, KeyEvent};

use crate::{
    git::trust_root,
    harness::Harness,
    herdr::{CreatedWorkspace, HerdrClient, HerdrError, StartedAgent, WorktreeSource},
    job::{Job, Update},
    model::{AgentInfo, DashboardAgent},
    paths::{dir_name, is_home},
    projects::{is_created_project, record_created_project},
    supervisor::{self, SUPERVISOR_TOKEN},
    time::unix_now,
};

use super::{
    App, DASHBOARD_PANE_LABEL,
    catalog::default_harness,
    first_prompt::prompt_started_agent,
    form::Checkout,
    markers,
    overlay::Overlay,
    progress::{JobReport, Progress},
    project_main::{
        CORGI_AGENT_WORKSPACE_ROLE, CORGI_WORKSPACE_ROLE_TOKEN, corgi_project_main,
        ensure_project_main_workspace, project_root_of,
    },
};

/// The label of a scratch agent's workspace, numbered from the second on.
const SCRATCH_LABEL: &str = "scratch";
/// Why the home directory gets no corgi.
pub(super) const SUPERVISOR_HOME_REFUSAL: &str = "The home directory is not a project, so it has no supervisor; t in the dashboard starts a scratch agent there";
const AGENT_START_ATTEMPTS: usize = 40;
const AGENT_START_RETRY_DELAY: Duration = Duration::from_millis(250);
/// How long the panel of a started agent stays up to say so, over its check
/// stamp, before it closes by itself.
const STARTED_HOLD: Duration = Duration::from_millis(900);

/// The pane token that tags a worker `corgi spawn --request-id` started
/// with that id, so a retried spawn finds the worker instead of starting a
/// second one.
pub(super) const CORGI_REQUEST_TOKEN: &str = "corgi_request";

/// Everything one new agent is started with. The dashboard form, the Omarchy
/// popup, and `corgi spawn` all fill one in, so every launch takes the same
/// checkout, start, and first-prompt sequence.
pub(super) struct LaunchPlan {
    pub(super) name: String,
    pub(super) harness: Harness,
    /// Empty leaves the model to the harness's own configuration.
    pub(super) model: String,
    /// Empty leaves the effort level to the harness's own configuration.
    pub(super) effort: String,
    pub(super) prompt: String,
    pub(super) project: PathBuf,
    /// The project directory does not exist yet and is created first.
    pub(super) new_project: bool,
    pub(super) checkout: Checkout,
    /// Arguments for the agent CLI itself, after the model and effort.
    pub(super) extra_args: Vec<String>,
    pub(super) role: Role,
    /// The `corgi spawn --request-id` the worker's pane is tagged with.
    pub(super) request_id: Option<String>,
}

/// Whom a launch starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Role {
    Worker,
    /// The project's supervisor: its pane is marked so the dashboard recognizes
    /// it wherever it goes.
    Supervisor {
        /// The pane of a supervisor that handed over and exited, where its
        /// successor starts instead of in a new or found pane.
        handover_pane: Option<String>,
    },
}

/// A started agent whose first prompt has been confirmed, and a description
/// of where it runs for the status line.
pub(super) struct LaunchedAgent {
    pub(super) agent: AgentInfo,
    pub(super) location: String,
}

/// The new-agent popup stays visible while this state is present. Progress is
/// rendered there instead of in the dashboard footer, and the popup only goes
/// away after the worker confirms that the first prompt was accepted.
#[derive(Debug, Clone)]
pub(crate) struct NewAgentLaunch {
    pub(crate) name: String,
    /// What the launch does, in order, as the panel lists them.
    pub(crate) steps: Vec<String>,
    pub(crate) state: LaunchState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchState {
    /// Running the step at index `step`, and the latest progress it
    /// reported, under the spinner on that step.
    Running { status: String, step: usize },
    /// The agent started, as `message` says, at `at` on the dashboard's
    /// motion clock. The panel says so, then closes by itself.
    Started { message: String, at: Duration },
    /// Why the launch failed. The popup stays open until Esc dismisses it.
    Failed(String),
}

impl LaunchState {
    /// The progress, the outcome or the error, whichever the popup shows.
    #[cfg(test)]
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Running { status, .. } => status,
            Self::Started { message, .. } => message,
            Self::Failed(error) => error,
        }
    }
}

/// What a launch of `plan` does, in the order [`launch_agent`] reports them:
/// its checkout or tab, starting the agent, and its first prompt when it has
/// one.
pub(super) fn launch_steps(plan: &LaunchPlan) -> Vec<String> {
    let checkout = if is_home(&plan.project) {
        "Create a scratch workspace"
    } else if matches!(
        plan.role,
        Role::Supervisor {
            handover_pane: Some(_)
        }
    ) {
        "Take over the supervisor's pane"
    } else {
        match plan.checkout {
            Checkout::Worktree => "Create a worktree",
            Checkout::Directory => "Open a tab in the project",
            Checkout::ProjectRoot => "Find the project's root tab",
        }
    };
    let mut steps = vec![
        checkout.to_string(),
        format!("Start {}{}", plan.harness, model_suffix(&plan.model)),
    ];
    if !plan.prompt.trim().is_empty() || matches!(plan.role, Role::Supervisor { .. }) {
        steps.push("Send the first prompt".into());
    }
    steps
}

impl App {
    /// The supervisor key: starts the supervisor of the selected agent's project,
    /// its repository's primary checkout, or focuses it when it already runs.
    pub(super) fn begin_supervisor(&mut self) {
        let Some(root) = self
            .selected_agent()
            .map(|agent| agent.project_root.clone())
            .filter(|root| !root.is_empty())
        else {
            self.set_status(
                "Select an agent to start the supervisor of its project",
                Some(Duration::from_secs(5)),
            );
            return;
        };
        if let Some(index) = self
            .agents
            .iter()
            .position(|agent| agent.supervisor && agent.project_root == root)
        {
            let focused = format!(
                "Focused {}'s corgi, {}",
                dir_name(&root).unwrap_or(supervisor::UNNAMED_PROJECT),
                self.agents[index].info.display_name()
            );
            self.select(index);
            self.focus_selected_saying(focused);
            return;
        }
        let saved = supervisor::state_dir(&root)
            .ok()
            .and_then(|dir| supervisor::saved_launch(&dir));
        match dashboard_supervisor_plan(self, &root, saved, default_harness) {
            Ok(plan) => self.start_new_agent(plan),
            Err(error) => self.set_status(format!("{error:#}"), Some(Duration::from_secs(8))),
        }
    }

    /// Starts the launch of `plan` in the background, the new-agent panel
    /// following its progress.
    pub(super) fn start_new_agent(&mut self, plan: LaunchPlan) {
        let name = plan.name.clone();
        let client = self.client.clone();
        let dashboard_pane_id = self.dashboard_pane_id.clone();
        let restore_focus = self.restore_focus_after_launch;
        // The form closes here; what it asked for lives on in `plan`.
        self.overlay = Overlay::Launch(NewAgentLaunch {
            state: LaunchState::Running {
                status: format!("Starting {name}…"),
                step: 0,
            },
            steps: launch_steps(&plan),
            name,
        });
        self.launch_job = Job::spawn(move |mut progress| {
            let result = launch_agent(&client, &plan, &mut progress)
                .map(|launched| format!("{} started {}", plan.name, launched.location));
            let focus_result = if restore_focus {
                restore_dashboard_focus(&client, dashboard_pane_id.as_deref())
            } else {
                Ok(())
            };
            match (result, focus_result) {
                (Err(error), _) => Err(error),
                (Ok(message), Ok(())) if restore_focus => {
                    Ok(format!("{message}; focus kept on Corgi"))
                }
                (Ok(message), Ok(())) => Ok(message),
                (Ok(_), Err(error)) => Err(error),
            }
        });
    }

    pub(super) fn poll_launch(&mut self) {
        for update in self.launch_job.poll() {
            let result = match update {
                Update::Progress(JobReport::Status(message)) => {
                    if let Some(NewAgentLaunch {
                        state: LaunchState::Running { status, .. },
                        ..
                    }) = self.overlay.new_agent_launch_mut()
                    {
                        *status = message;
                    }
                    continue;
                }
                Update::Progress(JobReport::Step(index)) => {
                    if let Some(NewAgentLaunch {
                        state: LaunchState::Running { step, .. },
                        ..
                    }) = self.overlay.new_agent_launch_mut()
                    {
                        *step = index;
                    }
                    continue;
                }
                Update::Progress(JobReport::Project(root)) => {
                    self.projects.remember(&root);
                    continue;
                }
                Update::Finished(result) => result,
                // Without an outcome the popup would spin and the dashboard
                // never refresh again.
                Update::Panicked => Err(anyhow::anyhow!(
                    "the launch stopped without reporting its outcome"
                )),
            };
            match result {
                Ok(message) => {
                    self.set_status(message.clone(), Some(Duration::from_secs(8)));
                    // An animated dashboard shows the start in its panel for
                    // a moment, over the success flash; without effects there
                    // is nothing to show, and the panel closes at once.
                    let at = self.motion.now();
                    match self.overlay.new_agent_launch_mut() {
                        Some(launch) if self.motion.animates() => {
                            launch.state = LaunchState::Started { message, at };
                        }
                        Some(_) => self.overlay = Overlay::None,
                        None => {}
                    }
                }
                Err(error) => {
                    let error = format!("{error:#}");
                    self.set_status(
                        format!("New agent failed: {error}"),
                        Some(Duration::from_secs(10)),
                    );
                    if let Some(launch) = self.overlay.new_agent_launch_mut() {
                        launch.state = LaunchState::Failed(error);
                    }
                    // The failed launch keeps its popup open so the user can
                    // read the error and dismiss it with Esc.
                }
            }
            self.request_refresh();
        }
    }

    /// Closes the panel of a started agent once it has shown the start for
    /// long enough.
    pub(super) fn close_started_launch(&mut self) {
        if let Some(NewAgentLaunch {
            state: LaunchState::Started { at, .. },
            ..
        }) = self.overlay.new_agent_launch()
            && self.motion.now().saturating_sub(*at) >= STARTED_HOLD
        {
            self.overlay = Overlay::None;
        }
    }

    /// Keys while a launch runs, which take none; after it failed, when Esc
    /// dismisses the error; or once it started, when any key closes the
    /// panel early.
    pub(super) fn handle_launch_key(&mut self, key: KeyEvent) -> bool {
        match self.overlay.new_agent_launch().map(|launch| &launch.state) {
            Some(LaunchState::Failed(_)) if key.code == KeyCode::Esc => self.cancel_editor(),
            Some(LaunchState::Started { .. }) => self.overlay = Overlay::None,
            _ => {}
        }
        false
    }
}

/// The agent CLI's arguments for `plan`, before a supervisor's own.
pub(super) fn launch_args(plan: &LaunchPlan) -> Vec<String> {
    let mut args = plan.harness.launch_args(&plan.model, &plan.effort);
    args.extend(plan.extra_args.iter().cloned());
    args.extend(trusted_project_args(&plan.harness, &plan.project));
    args
}

/// `value` as a Herdr agent name: lower-case ASCII letters, digits, `-` and
/// `_`, starting with a letter and at most 32 characters long.
pub(super) fn sanitize_agent_name(value: &str) -> String {
    let mut output = String::new();
    for character in value.to_lowercase().chars() {
        if character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '_'
            || character == '-'
        {
            output.push(character);
        } else if !output.ends_with('-') {
            output.push('-');
        }
    }
    let output = output.trim_matches(['-', '_']);
    let output = if output.starts_with(|character: char| character.is_ascii_lowercase()) {
        output.to_string()
    } else {
        format!("agent-{output}")
    };
    output
        .chars()
        .take(32)
        .collect::<String>()
        .trim_end_matches(['-', '_'])
        .to_string()
}

/// A session name for a new agent in `project`: the project's directory name,
/// made unique among `agents` with a numeric suffix when it is taken.
pub(super) fn unique_agent_name(agents: &[DashboardAgent], project: &Path) -> String {
    let base = dir_name(project).unwrap_or("agent");
    let base = sanitize_agent_name(base);
    let used: Vec<&str> = agents
        .iter()
        .filter_map(|agent| agent.info.name.as_deref())
        .collect();
    if !used.contains(&base.as_str()) {
        return base;
    }
    let suffix = unix_now() % 10_000;
    sanitize_agent_name(&format!("{base}-{suffix:04}"))
}

/// Creates the agent's checkout or tab, starts its CLI, and confirms its first
/// prompt. There is no rollback: a failed step leaves what it created visible.
pub(super) fn launch_agent(
    client: &HerdrClient,
    plan: &LaunchPlan,
    progress: &mut dyn Progress,
) -> Result<LaunchedAgent> {
    let LaunchPlan {
        name,
        harness,
        model,
        effort,
        prompt,
        project,
        new_project,
        checkout,
        extra_args,
        role,
        request_id,
    } = plan;
    let scratch = is_home(project);
    anyhow::ensure!(
        !(scratch && matches!(role, Role::Supervisor { .. })),
        "{SUPERVISOR_HOME_REFUSAL}"
    );
    if *new_project && !scratch {
        create_project(project, progress)?;
    }
    let mut supervisor_root = None;
    let (pane_id, location) = if scratch {
        // The home directory is never a project: every checkout is the
        // directory itself, in a workspace of the agent's own.
        create_scratch_workspace(client, project, name, progress)?
    } else if let Role::Supervisor {
        handover_pane: Some(pane_id),
    } = role
    {
        supervisor_root = Some(project.to_string_lossy().into_owned());
        (pane_id.clone(), "in the pane it took over".to_string())
    } else {
        match checkout {
            Checkout::Worktree => {
                let (created, location) =
                    create_worktree_workspace(client, project, name, progress)?;
                (created.root_pane_id, location)
            }
            Checkout::Directory => create_directory_agent_tab(client, project, name, progress)?,
            Checkout::ProjectRoot => {
                // A supervisor is started wherever its project workspace has
                // room; a worker asked into the root tab gets that or nothing.
                let supervisor = matches!(role, Role::Supervisor { .. });
                let (pane_id, location, root) =
                    project_root_pane(client, project, name, supervisor, progress)?;
                supervisor_root = Some(root);
                (pane_id, location)
            }
        }
    };
    if let Some(id) = request_id {
        // Tagged before the agent starts, so that a retry finds this worker
        // from the moment it has a pane. Untagged, it still starts; only a
        // retry would then miss it.
        if let Err(error) =
            markers::mark_pane(client, &pane_id, name, None, &[(CORGI_REQUEST_TOKEN, id)])
        {
            progress.report(format!(
                "Could not tag {name} with request id {id}: {error:#}"
            ));
        }
    }
    progress.step(1);
    progress.report(format!(
        "Starting {harness}{}{} as {name}…",
        model_suffix(model),
        effort_suffix(harness, effort)
    ));
    let mut args = launch_args(plan);
    let prompt = if let Role::Supervisor { handover_pane } = role {
        let root = supervisor_root
            .as_deref()
            .context("a supervisor starts in its project's root tab")?;
        let installed = client.plugin_root(supervisor::PLUGIN_ID).ok().flatten();
        let corgi_bin = supervisor::corgi_bin(installed.as_deref())?;
        let prepared = supervisor::prepare(root, &corgi_bin)?;
        if let Some(warning) = &prepared.warning {
            progress.report(warning.clone());
        }
        args.extend(harness.supervisor_args(
            &prepared.role_file,
            &prepared.role,
            &prepared.state_dir,
        ));
        supervisor::save_launch(
            &prepared.state_dir,
            &supervisor::Launch {
                kind: harness.kind().to_string(),
                model: model.clone(),
                effort: effort.clone(),
                extra_args: extra_args.clone(),
            },
        )?;
        if handover_pane.is_some() {
            let archive = supervisor::handover_archive(&prepared.state_dir, unix_now());
            supervisor::takeover_prompt(root, &prepared.state_dir, &archive)
        } else {
            supervisor::first_prompt(root, &prepared.state_dir, prompt)
        }
    } else {
        prompt.clone()
    };
    let started = start_agent_when_shell_ready(client, name, harness, &args, &pane_id, progress)?;
    if prompt.trim().is_empty() {
        // A launch without a task, as a scratch agent may be, is an
        // interactive session the user types into.
        return Ok(LaunchedAgent {
            agent: started.agent,
            location,
        });
    }
    prompt_launched_agent(
        client,
        &started.agent,
        &prompt,
        name,
        matches!(role, Role::Supervisor { .. }),
        progress,
    )?;
    Ok(LaunchedAgent {
        agent: started.agent,
        location,
    })
}

/// Mark a new or replacement supervisor before prompting, then bind any native
/// identity first revealed in the prompt acknowledgement.
fn prompt_launched_agent(
    client: &HerdrClient,
    started: &AgentInfo,
    prompt: &str,
    name: &str,
    is_supervisor: bool,
    progress: &mut dyn Progress,
) -> Result<()> {
    if is_supervisor {
        markers::mark_pane(
            client,
            &started.pane_id,
            name,
            markers::session_of(started),
            &[(SUPERVISOR_TOKEN, name)],
        )
        .with_context(|| format!("mark {name} as its project's supervisor"))?;
    }
    progress.step(2);
    progress.report(format!("Sending first prompt to {name}…"));
    prompt_started_agent(
        client,
        &started.pane_id,
        prompt,
        name,
        progress,
        |accepted| {
            // Herdr may first expose the session in its prompt acknowledgement,
            // after a naming plugin has already changed the presentation name.
            // The name marker above covers the interval before acceptance.
            if is_supervisor && let Some(session) = supervisor::native_session(accepted) {
                anyhow::ensure!(
                    accepted.pane_id == started.pane_id
                        && supervisor::native_session(started)
                            .is_none_or(|before| before == session),
                    "the supervisor's session changed while sending its first prompt"
                );
                markers::mark_pane(
                    client,
                    &accepted.pane_id,
                    accepted.name.as_deref().unwrap_or(name),
                    Some(session),
                    &[(SUPERVISOR_TOKEN, name)],
                )?;
            }
            Ok(())
        },
    )?;
    Ok(())
}

/// The model named in a launch status line, or nothing for the default.
fn model_suffix(model: &str) -> String {
    let model = model.trim();
    if model.is_empty() {
        String::new()
    } else {
        format!(" on {model}")
    }
}

/// The effort level named in a launch status line, or nothing for the
/// default or a harness that takes none.
fn effort_suffix(harness: &Harness, effort: &str) -> String {
    let effort = effort.trim();
    if harness.supports_effort() && !effort.is_empty() {
        format!(" with {effort} thinking")
    } else {
        String::new()
    }
}

/// Creates the directory of a new project and records it as one Corgi made.
/// It stays a plain directory until the project's supervisor makes it a
/// repository.
fn create_project(project: &Path, progress: &mut dyn Progress) -> Result<()> {
    progress.report(format!("Creating project {}…", project.display()));
    // The directory stays plain: the project's supervisor makes it a repository,
    // shaped by the first request, before any worker needs a worktree.
    fs::create_dir_all(project)
        .with_context(|| format!("create project directory {}", project.display()))?;
    // Remembered so Corgi can answer Claude Code's folder-trust question for
    // this project's agents, including after a restart.
    record_created_project(project)
        .with_context(|| format!("record {} as created by Corgi", project.display()))?;
    Ok(())
}

/// Creates a plain workspace rooted at `project`, labelled `label` or else
/// after the project directory.
fn create_plain_workspace(
    client: &HerdrClient,
    project: &Path,
    label: Option<&str>,
    name: &str,
    progress: &mut dyn Progress,
) -> Result<CreatedWorkspace> {
    progress.report(format!("Creating workspace for {name}…"));
    let label = label.unwrap_or_else(|| dir_name(project).unwrap_or("corgi"));
    progress.project(&project.to_string_lossy());
    client
        .create_workspace(Some(project), Some(label))
        .with_context(|| format!("create workspace at {}", project.display()))
}

/// Creates the workspace of a scratch agent in the home directory: a plain
/// one, labelled `scratch` or the first free `scratch N`, and marked as a
/// Corgi agent workspace so closing the agent closes it like any other. It
/// never becomes or looks up a project workspace, and the home directory is
/// not remembered as a project.
fn create_scratch_workspace(
    client: &HerdrClient,
    home: &Path,
    name: &str,
    progress: &mut dyn Progress,
) -> Result<(String, String)> {
    progress.report(format!("Creating a scratch workspace for {name}…"));
    let snapshot = client.snapshot().context("list workspaces")?;
    let label = scratch_label(
        snapshot
            .workspaces
            .iter()
            .map(|workspace| workspace.label.as_str()),
    );
    let created = client
        .create_workspace(Some(home), Some(&label))
        .with_context(|| format!("create workspace at {}", home.display()))?;
    markers::mark_workspace(
        client,
        &created.workspace_id,
        markers::checkout_of(&created.workspace),
        &[(CORGI_WORKSPACE_ROLE_TOKEN, CORGI_AGENT_WORKSPACE_ROLE)],
    )
    .with_context(|| format!("mark Corgi scratch workspace {}", created.workspace_id))?;
    Ok((
        created.root_pane_id,
        format!("in {label} (home directory, no project)"),
    ))
}

/// `scratch`, or the first `scratch N` no workspace among `taken` is
/// labelled.
fn scratch_label<'a>(taken: impl Iterator<Item = &'a str>) -> String {
    let taken: Vec<&str> = taken.map(str::trim).collect();
    (1..)
        .map(|number| match number {
            1 => SCRATCH_LABEL.to_string(),
            _ => format!("{SCRATCH_LABEL} {number}"),
        })
        .find(|label| !taken.contains(&label.as_str()))
        .expect("some scratch label is free")
}

/// Creates a fresh Git worktree of the repository containing `project` and
/// opens it beneath that repository's Corgi-managed main workspace.
///
/// `project` may itself be a linked worktree: Herdr resolves it to the
/// repository's primary checkout first, because it only creates worktrees from
/// there. Directories outside any Git work tree fall back to a plain workspace
/// so non-Git projects still work. Returns the workspace and a description of
/// where the agent runs for the status line.
fn create_worktree_workspace(
    client: &HerdrClient,
    project: &Path,
    name: &str,
    progress: &mut dyn Progress,
) -> Result<(CreatedWorkspace, String)> {
    progress.report(format!("Resolving the Git repository for {name}…"));
    let Some(source) = resolve_repo(client, project)? else {
        let created = create_plain_workspace(client, project, None, name, progress)?;
        return Ok((
            created,
            format!(
                "in {} (not a Git repository, no worktree)",
                project.display()
            ),
        ));
    };
    progress.project(&source.repo_root);
    let main = ensure_project_main_workspace(client, &source.repo_root, progress)?;
    progress.report(format!(
        "Creating a {} worktree for {name} under {}…",
        source.repo_name, main.label
    ));
    let created = client
        .create_worktree(&main.workspace_id)
        .with_context(|| format!("create a worktree of {}", source.repo_root))?;
    let checkout = created
        .worktree
        .as_ref()
        .map(|worktree| worktree.path.as_str())
        .or_else(|| markers::checkout_of(&created.workspace));
    markers::mark_workspace(
        client,
        &created.workspace_id,
        checkout,
        &[(CORGI_WORKSPACE_ROLE_TOKEN, CORGI_AGENT_WORKSPACE_ROLE)],
    )
    .with_context(|| format!("mark Corgi worktree workspace {}", created.workspace_id))?;
    let branch = created
        .worktree
        .as_ref()
        .and_then(|worktree| worktree.branch.clone())
        .unwrap_or_else(|| created.workspace.label.clone());
    Ok((
        created,
        format!("in {} worktree {branch}", source.repo_name),
    ))
}

/// Starts a shared-checkout agent in an independent tab of its project's main
/// workspace. The shared-directory option deliberately does not make a Git
/// worktree, but it still must never replace or close the project main.
fn create_directory_agent_tab(
    client: &HerdrClient,
    project: &Path,
    name: &str,
    progress: &mut dyn Progress,
) -> Result<(String, String)> {
    progress.report(format!("Resolving the Git repository for {name}…"));
    let Some(source) = resolve_repo(client, project)? else {
        let created = create_plain_workspace(client, project, None, name, progress)?;
        return Ok((
            created.root_pane_id,
            format!(
                "in {} (not a Git repository, shared directory)",
                project.display()
            ),
        ));
    };
    progress.project(&source.repo_root);
    let main = ensure_project_main_workspace(client, &source.repo_root, progress)?;
    progress.report(format!(
        "Creating a shared-checkout tab for {name} in {}…",
        main.label
    ));
    let tab = client
        .create_tab(&main.workspace_id, Some(project), Some(name))
        .with_context(|| format!("create a shared-checkout tab in {}", main.label))?;
    Ok((
        tab.root_pane_id,
        format!("in {} (shared checkout)", main.label),
    ))
}

/// Returns the single shell pane of the project main's root tab, creating the
/// project workspace when the repository has none yet. When an agent already
/// runs there or the tab was split, nothing the user put there is replaced:
/// with `open_tab` the agent gets a new tab of the same workspace, and
/// without it the launch is refused.
fn project_root_pane(
    client: &HerdrClient,
    project: &Path,
    name: &str,
    open_tab: bool,
    progress: &mut dyn Progress,
) -> Result<(String, String, String)> {
    progress.report(format!("Resolving the project directory for {name}…"));
    // A directory outside Git still gets a project workspace, found again by
    // its recorded directory once the supervisor has made it a repository.
    let root = project_root_of(client, project)?;
    progress.project(&root);
    let main = if open_tab {
        corgi_project_main(client, &root, progress)?
    } else {
        ensure_project_main_workspace(client, &root, progress)?
    };
    anyhow::ensure!(
        !main.root_tab_id.is_empty(),
        "project workspace {} does not record its root tab",
        main.label
    );
    let snapshot = client.snapshot().context("inspect the project root tab")?;
    let panes: Vec<_> = snapshot
        .panes
        .iter()
        .filter(|pane| pane.tab_id == main.root_tab_id)
        .collect();
    let occupied = match panes.as_slice() {
        [pane] => snapshot
            .agents
            .iter()
            .find(|agent| agent.pane_id == pane.pane_id)
            .map(|agent| {
                format!(
                    "the {} project tab already runs {}",
                    main.label,
                    agent
                        .name
                        .as_deref()
                        .or(agent.agent.as_deref())
                        .unwrap_or("an agent")
                )
            }),
        panes => Some(format!(
            "the {} project tab has {} panes; the root tab must hold one shell",
            main.label,
            panes.len()
        )),
    };
    let Some(occupied) = occupied else {
        return Ok((
            panes[0].pane_id.clone(),
            format!("in the {} project tab", main.label),
            main.project_root,
        ));
    };
    if !open_tab {
        bail!(occupied);
    }
    progress.report(format!("{occupied}; opening a new tab for {name}…"));
    let tab = client
        .create_tab(
            &main.workspace_id,
            Some(Path::new(&main.project_root)),
            Some(name),
        )
        .with_context(|| format!("create a tab for {name} in {}", main.label))?;
    Ok((
        tab.root_pane_id,
        format!("in a new tab of {} ({occupied})", main.label),
        main.project_root,
    ))
}

/// The repository containing `project`, as Herdr reports it, or `None` when
/// `project` is outside any Git work tree.
pub(super) fn resolve_repo(client: &HerdrClient, project: &Path) -> Result<Option<WorktreeSource>> {
    match client.worktree_source(project) {
        Ok(source) if !source.repo_root.is_empty() => Ok(Some(source)),
        Ok(_) => bail!(
            "Herdr did not report a repository root for {}",
            project.display()
        ),
        Err(error) if HerdrError::is_not_git_worktree(&error) => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("resolve the Git repository for {}", project.display())),
    }
}

/// Starts an agent in a freshly created pane.
///
/// Herdr answers `agent_pane_busy` until the new pane's shell has reported
/// itself as available, which takes longer than the round trip from
/// `workspace.create` or `worktree.create`. Retry briefly so the launch does not
/// fail on that race; any other error is reported at once.
fn start_agent_when_shell_ready(
    client: &HerdrClient,
    name: &str,
    harness: &Harness,
    args: &[String],
    pane_id: &str,
    progress: &mut dyn Progress,
) -> Result<StartedAgent> {
    for attempt in 1..=AGENT_START_ATTEMPTS {
        match client.start_agent(name, harness.kind(), args, pane_id) {
            Ok(started) => return Ok(started),
            Err(error)
                if HerdrError::is_pane_not_ready(&error) && attempt < AGENT_START_ATTEMPTS =>
            {
                progress.report(format!(
                    "Waiting for the new shell before starting {name}… ({attempt}/{AGENT_START_ATTEMPTS})"
                ));
                thread::sleep(AGENT_START_RETRY_DELAY);
            }
            Err(error) => {
                return Err(error).with_context(|| format!("start {harness} in {pane_id}"));
            }
        }
    }
    unreachable!("the start retry loop always returns on its final attempt")
}

/// The arguments that answer `harness`'s folder-trust question for an agent
/// in `project` on its command line, when Corgi created that project, so the
/// CLI never asks. Nothing is written to the harness's configuration. Codex,
/// like Claude Code, keys trust on a worktree's main checkout.
fn trusted_project_args(harness: &Harness, project: &Path) -> Vec<String> {
    let root = trust_root(project);
    match fs::canonicalize(&root) {
        Ok(root) if is_created_project(&root) => harness.trusted_project_args(&root),
        _ => Vec::new(),
    }
}

/// Returns focus to the Corgi pane after a new agent has been started.
///
/// `HERDR_PANE_ID` can be stale: pane IDs are workspace-scoped and the launcher
/// moves Corgi into its own workspace right after opening it. Fall back to the
/// pane labelled "Corgi" whose cwd is this plugin root.
fn restore_dashboard_focus(client: &HerdrClient, pane_id: Option<&str>) -> Result<()> {
    let snapshot = client.snapshot().context("look up the Corgi pane")?;
    let plugin_root = env::current_dir().ok();
    let own_pane = snapshot
        .panes
        .iter()
        .find(|pane| Some(pane.pane_id.as_str()) == pane_id)
        .or_else(|| {
            snapshot.panes.iter().find(|pane| {
                pane.label.as_deref() == Some(DASHBOARD_PANE_LABEL)
                    && match (&plugin_root, pane.cwd.as_deref()) {
                        (Some(root), Some(cwd)) => Path::new(cwd) == root,
                        _ => false,
                    }
            })
        });
    match own_pane {
        Some(pane) => client
            .go_to_pane(pane)
            .context("restore focus to the Corgi dashboard"),
        None => Ok(()),
    }
}

/// The harness of the supervisor in `pane_id`, the `HERDR_PANE_ID` Herdr gives
/// every process in a pane, if that pane holds one: the kind Herdr detected
/// running there.
pub(super) fn calling_supervisor_harness(app: &App, pane_id: Option<&str>) -> Option<Harness> {
    let pane_id = pane_id?;
    app.agents
        .iter()
        .find(|agent| agent.supervisor && agent.info.pane_id == pane_id)
        .and_then(|agent| agent.info.agent.as_deref())
        .map(Harness::from_typed)
        .filter(|harness| !harness.kind().is_empty())
}

/// Whether a new agent in `project` is its supervisor: the first agent of a new
/// project, or of a project none of whose workspaces has an agent session.
/// The home directory is never a project, so an agent there never is.
pub(super) fn starts_supervisor(
    new_project: bool,
    project: &str,
    agents: &[DashboardAgent],
) -> bool {
    let project = project.trim().trim_end_matches('/');
    !project.is_empty()
        && !is_home(project)
        && (new_project
            || !agents
                .iter()
                .any(|agent| agent.project_root.trim_end_matches('/') == project))
}

/// Why a supervisor cannot start on `harness`.
pub(super) fn supervisor_harness_error(harness: &Harness) -> String {
    format!(
        "A supervisor runs on {}, not {harness}",
        Harness::supervisor_kinds()
    )
}

/// The launch the dashboard's corgi key starts for `root`: the harness,
/// model, effort and arguments its supervisor was last launched with, `saved`,
/// else the new-agent form's preset harness (or the first a supervisor runs on)
/// with the harness's own defaults. It has no task, so the supervisor greets
/// with the state of the project.
pub(super) fn dashboard_supervisor_plan(
    app: &App,
    root: &str,
    saved: Option<supervisor::Launch>,
    preset: impl FnOnce() -> Harness,
) -> Result<LaunchPlan> {
    let saved = saved.filter(|launch| Harness::from_kind(&launch.kind).supports_supervisor());
    let (harness, model, effort, extra_args) = match saved {
        Some(launch) => (
            Harness::from_kind(&launch.kind),
            launch.model,
            launch.effort,
            launch.extra_args,
        ),
        None => {
            let harness = Some(preset())
                .filter(Harness::supports_supervisor)
                .unwrap_or_else(|| Harness::SUPERVISOR_HARNESSES[0].clone());
            (harness, String::new(), String::new(), Vec::new())
        }
    };
    let mut plan = supervisor_plan(app, root, harness, model, effort, String::new())?;
    plan.extra_args = extra_args;
    Ok(plan)
}

/// The launch of `root`'s corgi on `harness`, refused while one is already
/// running.
pub(super) fn supervisor_plan(
    app: &App,
    root: &str,
    harness: Harness,
    model: String,
    effort: String,
    task: String,
) -> Result<LaunchPlan> {
    anyhow::ensure!(!is_home(root), "{SUPERVISOR_HOME_REFUSAL}");
    if let Some(running) = app
        .agents
        .iter()
        .find(|agent| agent.supervisor && agent.project_root == root)
    {
        bail!(
            "{}'s corgi is already running as {}",
            dir_name(root).unwrap_or(supervisor::UNNAMED_PROJECT),
            running.info.display_name()
        );
    }
    let name = sanitize_agent_name(&format!(
        "corgi-{}",
        dir_name(root).unwrap_or(supervisor::UNNAMED_PROJECT)
    ));
    anyhow::ensure!(
        !app.agents
            .iter()
            .any(|agent| agent.info.name.as_deref() == Some(name.as_str())),
        "an agent named {name} is already running"
    );
    Ok(LaunchPlan {
        name,
        harness,
        model,
        effort,
        prompt: task,
        project: PathBuf::from(root),
        new_project: false,
        checkout: Checkout::ProjectRoot,
        extra_args: Vec::new(),
        role: Role::Supervisor {
            handover_pane: None,
        },
        request_id: None,
    })
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, path::Path, sync::mpsc};

    use anyhow::anyhow;
    use crossterm::event::{KeyCode, KeyModifiers};
    use serde_json::{Value, json};

    use crate::{
        app::{
            progress::Silent,
            project_main::{
                CORGI_PROJECT_MAIN_ROLE, CORGI_PROJECT_MAIN_TAB_TOKEN, CORGI_PROJECT_ROOT_TOKEN,
                project_main_workspace,
            },
            test_helpers::{poll_until, press},
        },
        herdr::HerdrClient,
        model::{AgentInfo, DashboardAgent, WorkspaceInfo},
        test_support::{answer, fake_herdr, test_app},
    };

    use super::*;

    #[test]
    fn only_a_new_project_or_one_without_sessions_starts_its_supervisor() {
        let agent = |root: &str, supervisor: bool| DashboardAgent {
            project_root: root.into(),
            supervisor,
            ..DashboardAgent::default()
        };
        let agents = [agent("/repos/corgi", true), agent("/repos/weather", false)];

        // A project with a supervisor, or with only workers, gets a worker.
        assert!(!starts_supervisor(false, "/repos/corgi", &agents));
        assert!(!starts_supervisor(false, "/repos/weather/", &agents));
        // A project nobody works in, or a new one, starts its corgi.
        assert!(starts_supervisor(false, "/repos/copy", &agents));
        assert!(starts_supervisor(true, "/repos/brand-new", &agents));
        assert!(!starts_supervisor(false, "  ", &agents));
    }

    #[test]
    fn the_home_directory_never_starts_a_supervisor() {
        let Some(home) = crate::paths::home() else {
            return;
        };
        let home = home.to_string_lossy().into_owned();
        // No agent works there, and it is not even new: still no corgi.
        assert!(!starts_supervisor(false, &home, &[]));
        assert!(!starts_supervisor(true, &format!("{home}/"), &[]));
        assert!(!starts_supervisor(false, "~", &[]));
        // Its subdirectories are projects like any other.
        assert!(starts_supervisor(
            false,
            &format!("{home}/repos/corgi"),
            &[]
        ));

        let refused = supervisor_plan(
            &test_app(),
            &home,
            Harness::Claude,
            "".into(),
            "".into(),
            "".into(),
        )
        .err()
        .expect("the home directory has no supervisor");
        assert!(refused.to_string().contains("not a project"), "{refused}");
    }

    #[test]
    fn scratch_workspaces_are_numbered_from_the_second() {
        assert_eq!(scratch_label(["corgi corgi"].into_iter()), "scratch");
        assert_eq!(scratch_label(["scratch"].into_iter()), "scratch 2");
        assert_eq!(
            scratch_label(["scratch", "scratch 3", "scratch 2"].into_iter()),
            "scratch 4"
        );
    }

    #[test]
    fn new_and_replacement_supervisors_are_marked_before_the_prompt_and_bound_after_renaming() {
        for session_known in [false, true] {
            let (socket, server) = fake_herdr("corgi-first-prompt", move |listener| {
                let mut marker = "session:previous-occupant".to_string();
                for method in [
                    "pane.report_metadata",
                    "agent.prompt",
                    "pane.report_metadata",
                    "agent.read",
                ] {
                    answer(&listener, |request| {
                        assert_eq!(request["method"], method);
                        let result = match method {
                            "pane.report_metadata" => {
                                marker = request["params"]["tokens"][SUPERVISOR_TOKEN]
                                    .as_str()
                                    .unwrap()
                                    .to_string();
                                json!({"type": "ok"})
                            }
                            "agent.prompt" => {
                                assert_eq!(
                                    marker,
                                    if session_known {
                                        "session:launch-session"
                                    } else {
                                        "corgi-m-ta-sverige"
                                    }
                                );
                                json!({"type": "agent_prompted", "agent": {
                                    "pane_id": "w9:p1", "workspace_id": "w9", "tab_id": "w9:t1",
                                    "name": "start-your-corgi-session-for-m",
                                    "agent_session": {"value": "launch-session"}
                                }})
                            }
                            _ => {
                                assert_eq!(marker, "session:launch-session");
                                json!({"type": "pane_read", "read": {"text": "Start your supervisor session"}})
                            }
                        };
                        json!({"result": result})
                    });
                }
            });
            let client = HerdrClient::from_socket_path(&socket);
            let started = AgentInfo {
                pane_id: "w9:p1".into(),
                name: Some("corgi-m-ta-sverige".into()),
                agent_session: session_known.then(|| crate::model::AgentSession {
                    value: "launch-session".into(),
                    ..Default::default()
                }),
                ..Default::default()
            };
            prompt_launched_agent(
                &client,
                &started,
                "Start your supervisor session",
                "corgi-m-ta-sverige",
                true,
                &mut Silent,
            )
            .unwrap();
            server.join().unwrap();
            fs::remove_file(socket).unwrap();
        }
    }

    /// Launches a scratch agent with `request_id` against a fake Herdr that
    /// answers only the requests such a launch makes, in order, and returns
    /// the launch and the parameters of each request. Any other request,
    /// such as worktree.list, project-main metadata or agent.prompt, fails
    /// the test.
    fn scratch_launch(home: &Path, request_id: Option<&str>) -> (LaunchedAgent, Vec<Value>) {
        let expected_cwd = home.to_string_lossy().into_owned();
        let mut methods = vec![
            "session.snapshot",
            "workspace.create",
            "workspace.report_metadata",
        ];
        if request_id.is_some() {
            methods.push("pane.report_metadata");
        }
        methods.push("agent.start");
        let (socket_path, server) = fake_herdr("scratch-launch", move |listener| {
            let mut requests = Vec::new();
            for method in methods {
                answer(&listener, |request| {
                    assert_eq!(request["method"], method);
                    requests.push(request["params"].clone());
                    match method {
                        "session.snapshot" => json!({ "result": {
                            "type": "session_snapshot",
                            "snapshot": { "workspaces": [
                                { "workspace_id": "w1", "label": "scratch" }
                            ]}
                        }}),
                        "workspace.create" => json!({ "result": {
                            "type": "workspace_created",
                            "workspace": { "workspace_id": "w9", "label": "scratch 2" },
                            "tab": { "tab_id": "w9:t1" },
                            "root_pane": { "pane_id": "w9:p1" }
                        }}),
                        "workspace.report_metadata" => json!({ "result": {
                            "type": "workspace_metadata_updated"
                        }}),
                        "pane.report_metadata" => json!({ "result": {
                            "type": "pane_metadata_updated"
                        }}),
                        _ => json!({ "result": {
                            "type": "agent_started",
                            "agent": {
                                "agent": "claude", "name": "scratch-agent",
                                "pane_id": "w9:p1", "workspace_id": "w9", "tab_id": "w9:t1",
                                "cwd": expected_cwd
                            },
                            "argv": ["claude"]
                        }}),
                    }
                });
            }
            requests
        });
        let client = HerdrClient::from_socket_path(&socket_path);
        let plan = LaunchPlan {
            name: "scratch-agent".into(),
            harness: Harness::Claude,
            model: String::new(),
            effort: String::new(),
            prompt: String::new(),
            project: home.to_path_buf(),
            new_project: false,
            // The form's default, which the home directory overrides.
            checkout: Checkout::Worktree,
            extra_args: Vec::new(),
            role: Role::Worker,
            request_id: request_id.map(str::to_string),
        };
        let launched = launch_agent(&client, &plan, &mut Silent).expect("scratch launch");
        let requests = server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");
        (launched, requests)
    }

    #[test]
    fn a_scratch_launch_makes_a_plain_agent_workspace_and_sends_no_first_prompt() {
        let Some(home) = crate::paths::home().filter(|home| home.is_dir()) else {
            return;
        };
        let (launched, requests) = scratch_launch(&home, None);
        assert_eq!(launched.agent.pane_id, "w9:p1");
        assert!(
            launched.location.contains("scratch 2"),
            "{}",
            launched.location
        );
        assert_eq!(requests[1]["cwd"], home.to_string_lossy().as_ref());
        assert_eq!(requests[1]["label"], "scratch 2");
        assert_eq!(requests[2]["workspace_id"], "w9");
        assert_eq!(
            requests[2]["tokens"],
            json!({ "corgi_workspace_role": "agent-workspace" })
        );
        assert_eq!(requests[3]["pane_id"], "w9:p1");
    }

    #[test]
    fn a_spawn_with_a_request_id_tags_its_pane_before_the_agent_starts() {
        let Some(home) = crate::paths::home().filter(|home| home.is_dir()) else {
            return;
        };
        let (launched, requests) = scratch_launch(&home, Some("20261002-w-retry"));
        assert_eq!(launched.agent.pane_id, "w9:p1");
        assert_eq!(requests[3]["pane_id"], "w9:p1");
        // Corgi's other pane tokens are cleared, as every pane mark does.
        assert_eq!(
            requests[3]["tokens"][CORGI_REQUEST_TOKEN],
            "20261002-w-retry"
        );
        assert_eq!(requests[3]["tokens"][SUPERVISOR_TOKEN], Value::Null);
        assert_eq!(requests[4]["pane_id"], "w9:p1", "then agent.start");
    }

    #[test]
    fn a_supervisor_launch_is_named_after_its_project_and_refused_while_one_runs() {
        let mut app = test_app();
        let plan = supervisor_plan(
            &app,
            "/repos/weather",
            Harness::Codex,
            "gpt-5-codex".into(),
            "high".into(),
            "Hi".into(),
        )
        .expect("no supervisor runs yet");
        assert_eq!(plan.name, "corgi-weather");
        assert_eq!(plan.harness, Harness::Codex);
        assert_eq!(plan.model, "gpt-5-codex");
        assert_eq!(plan.effort, "high");
        assert_eq!(plan.checkout, Checkout::ProjectRoot);
        assert!(matches!(plan.role, Role::Supervisor { .. }));

        app.agents = vec![DashboardAgent {
            info: AgentInfo {
                name: Some("corgi-weather".into()),
                ..AgentInfo::default()
            },
            project_root: "/repos/weather".into(),
            supervisor: true,
            ..DashboardAgent::default()
        }];
        let refused = supervisor_plan(
            &app,
            "/repos/weather",
            Harness::Claude,
            "".into(),
            "".into(),
            "".into(),
        )
        .err()
        .expect("a supervisor already runs");
        assert!(
            refused
                .to_string()
                .contains("already running as corgi-weather")
        );
        // Another project's supervisor is no reason to refuse.
        assert!(
            supervisor_plan(
                &app,
                "/repos/corgi",
                Harness::Claude,
                "".into(),
                "".into(),
                "".into()
            )
            .is_ok()
        );
    }

    #[test]
    fn a_spawn_is_a_supervisors_only_from_that_supervisors_pane() {
        let mut app = test_app();
        let agent = |pane_id: &str, name: &str, kind: &str, supervisor: bool| DashboardAgent {
            info: AgentInfo {
                pane_id: pane_id.into(),
                name: Some(name.into()),
                agent: Some(kind.into()),
                ..AgentInfo::default()
            },
            supervisor,
            ..DashboardAgent::default()
        };
        app.agents = vec![
            agent("w1:p1", "corgi-weather", "codex", true),
            agent("w2:p1", "w-forecast", "claude", false),
            // A supervisor whose harness Herdr has not detected names none.
            agent("w3:p1", "corgi-corgi", " ", true),
        ];
        assert_eq!(
            calling_supervisor_harness(&app, Some("w1:p1")),
            Some(Harness::Codex)
        );
        assert!(calling_supervisor_harness(&app, Some("w2:p1")).is_none());
        assert!(calling_supervisor_harness(&app, Some("w3:p1")).is_none());
        assert!(calling_supervisor_harness(&app, Some("w9:p1")).is_none());
        assert!(calling_supervisor_harness(&app, None).is_none());
    }

    #[test]
    fn codex_is_told_a_corgi_created_project_is_trusted_as_one_table() {
        // A dotted `-c projects."…".trust_level` key would be split at the
        // dot in `jane.doe`.
        assert_eq!(
            Harness::Codex.trusted_project_args(Path::new("/Users/jane.doe/repos/weather")),
            [
                "-c",
                r#"projects={"/Users/jane.doe/repos/weather"={trust_level="trusted"}}"#
            ]
        );
        // A project Corgi did not create gets no override.
        let elsewhere =
            std::env::temp_dir().join(format!("corgi-not-created-{}", std::process::id()));
        fs::create_dir_all(&elsewhere).expect("create directory");
        assert!(trusted_project_args(&Harness::Codex, &elsewhere).is_empty());
        fs::remove_dir_all(&elsewhere).ok();
    }

    #[test]
    fn a_plain_directory_gets_a_project_workspace_found_by_its_directory() {
        let (socket_path, server) = fake_herdr("plain-project", move |listener| {
            for method in [
                "worktree.list",
                "session.snapshot",
                "workspace.create",
                "workspace.report_metadata",
                "session.snapshot",
            ] {
                answer(&listener, |request| {
                    assert_eq!(request["method"], method);
                    match method {
                        "worktree.list" => json!({
                            "error": { "code": "not_git_worktree", "message": "not a Git work tree" }
                        }),
                        "workspace.create" => {
                            assert_eq!(request["params"]["cwd"], "/tmp/corgi-plain-project");
                            assert_eq!(request["params"]["label"], "corgi-plain-project corgi");
                            json!({ "result": {
                                "type": "workspace_created",
                                "workspace": { "workspace_id": "w9", "label": "corgi-plain-project corgi" },
                                "tab": { "tab_id": "w9:t1" },
                                "root_pane": { "pane_id": "w9:p1" }
                            }})
                        }
                        "workspace.report_metadata" => {
                            let tokens = &request["params"]["tokens"];
                            assert_eq!(tokens["corgi_workspace_role"], "project-main");
                            assert_eq!(tokens["corgi_project_main_tab"], "w9:t1");
                            assert_eq!(tokens["corgi_project_root"], "/tmp/corgi-plain-project");
                            json!({ "result": { "type": "workspace_metadata_updated" } })
                        }
                        // No workspace exists yet, and the new one's root tab
                        // holds a single shell.
                        _ => json!({ "result": { "type": "session_snapshot", "snapshot": {
                            "panes": [{ "pane_id": "w9:p1", "workspace_id": "w9", "tab_id": "w9:t1" }]
                        }}}),
                    }
                });
            }
        });

        let client = HerdrClient::from_socket_path(&socket_path);
        let (pane_id, location, root) = project_root_pane(
            &client,
            Path::new("/tmp/corgi-plain-project"),
            "corgi-corgi-plain-project",
            true,
            &mut Silent,
        )
        .expect("start in the plain project's root tab");
        assert_eq!(pane_id, "w9:p1");
        assert_eq!(root, "/tmp/corgi-plain-project");
        assert!(location.contains("corgi-plain-project corgi"));
        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");

        // Once the supervisor has made the directory a repository, Herdr still
        // reports no Git details for that workspace; the recorded directory
        // is what finds it for the first worktree.
        let workspace = WorkspaceInfo {
            workspace_id: "w9".into(),
            label: "corgi-plain-project corgi".into(),
            tokens: BTreeMap::from([
                (
                    CORGI_WORKSPACE_ROLE_TOKEN.into(),
                    CORGI_PROJECT_MAIN_ROLE.into(),
                ),
                (CORGI_PROJECT_MAIN_TAB_TOKEN.into(), "w9:t1".into()),
                (
                    CORGI_PROJECT_ROOT_TOKEN.into(),
                    "/tmp/corgi-plain-project".into(),
                ),
            ]),
            worktree: None,
        };
        let main = project_main_workspace(&[workspace], &[], "/tmp/corgi-plain-project")
            .expect("found by its recorded directory");
        assert_eq!(main.workspace_id, "w9");
        assert_eq!(main.root_tab_id, "w9:t1");
    }

    /// A fake Herdr for a launch into the root tab of the existing project
    /// workspace of `/tmp/corgi-root-reuse`, a plain directory, whose root
    /// tab holds one shell, running `agent` if one is named. It answers the
    /// methods in `methods` in order and hands back each request.
    fn root_tab_herdr(
        label: &str,
        agent: Option<&'static str>,
        methods: &'static [&'static str],
    ) -> (
        std::path::PathBuf,
        std::thread::JoinHandle<Vec<serde_json::Value>>,
    ) {
        fake_herdr(label, move |listener| {
            let mut requests = Vec::new();
            for method in methods {
                answer(&listener, |request| {
                    assert_eq!(request["method"], *method);
                    requests.push(request.clone());
                    match *method {
                        "worktree.list" => json!({
                            "error": { "code": "not_git_worktree", "message": "not a Git work tree" }
                        }),
                        "tab.create" => json!({ "result": {
                            "type": "tab_created",
                            "tab": { "tab_id": "w9:t2" },
                            "root_pane": { "pane_id": "w9:p2" }
                        }}),
                        _ => {
                            let agents: Vec<_> = agent
                                .into_iter()
                                .map(|name| {
                                    json!({
                                        "pane_id": "w9:p1", "workspace_id": "w9",
                                        "tab_id": "w9:t1", "name": name
                                    })
                                })
                                .collect();
                            json!({ "result": { "type": "session_snapshot", "snapshot": {
                                "workspaces": [{
                                    "workspace_id": "w9",
                                    "label": "corgi-root-reuse corgi",
                                    "tokens": {
                                        "corgi_workspace_role": "project-main",
                                        "corgi_project_main_tab": "w9:t1",
                                        "corgi_project_root": "/tmp/corgi-root-reuse"
                                    }
                                }],
                                "panes": [{ "pane_id": "w9:p1", "workspace_id": "w9", "tab_id": "w9:t1" }],
                                "agents": agents
                            }}})
                        }
                    }
                });
            }
            requests
        })
    }

    #[test]
    fn a_supervisor_reuses_the_agentless_root_tab_of_its_project_workspace() {
        let (socket_path, server) = root_tab_herdr(
            "root-reuse",
            None,
            &["worktree.list", "session.snapshot", "session.snapshot"],
        );
        let client = HerdrClient::from_socket_path(&socket_path);
        let (pane_id, location, root) = project_root_pane(
            &client,
            Path::new("/tmp/corgi-root-reuse"),
            "corgi-corgi-root-reuse",
            true,
            &mut Silent,
        )
        .expect("start in the existing root tab");
        // No workspace or tab was created: the fake answers nothing else.
        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");
        assert_eq!(pane_id, "w9:p1");
        assert_eq!(root, "/tmp/corgi-root-reuse");
        assert_eq!(location, "in the corgi-root-reuse corgi project tab");
    }

    #[test]
    fn a_supervisor_opens_a_new_tab_beside_a_root_tab_that_runs_an_agent() {
        let (socket_path, server) = root_tab_herdr(
            "root-busy",
            Some("mine"),
            &[
                "worktree.list",
                "session.snapshot",
                "session.snapshot",
                "tab.create",
            ],
        );
        let client = HerdrClient::from_socket_path(&socket_path);
        let (pane_id, location, root) = project_root_pane(
            &client,
            Path::new("/tmp/corgi-root-reuse"),
            "corgi-corgi-root-reuse",
            true,
            &mut Silent,
        )
        .expect("start in a new tab");
        let requests = server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");
        assert_eq!(pane_id, "w9:p2");
        assert_eq!(root, "/tmp/corgi-root-reuse");
        assert!(location.contains("already runs mine"), "{location}");
        let tab = &requests[3]["params"];
        assert_eq!(tab["workspace_id"], "w9");
        assert_eq!(tab["cwd"], "/tmp/corgi-root-reuse");
        assert_eq!(tab["label"], "corgi-corgi-root-reuse");
        assert_eq!(tab["focus"], false);
    }

    #[test]
    fn a_worker_asked_into_a_busy_root_tab_is_refused() {
        let (socket_path, server) = root_tab_herdr(
            "root-refused",
            Some("mine"),
            &["worktree.list", "session.snapshot", "session.snapshot"],
        );
        let client = HerdrClient::from_socket_path(&socket_path);
        let refused = project_root_pane(
            &client,
            Path::new("/tmp/corgi-root-reuse"),
            "corgi-root-reuse",
            false,
            &mut Silent,
        )
        .expect_err("the root tab is taken");
        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");
        assert!(
            refused.to_string().contains("already runs mine"),
            "{refused}"
        );
    }

    #[test]
    fn the_supervisor_key_launches_as_last_time_else_on_the_form_presets() {
        let app = test_app();
        let root = "/repos/weather";
        // Nothing saved: the form's preset harness and its own defaults.
        let plan = dashboard_supervisor_plan(&app, root, None, || Harness::Codex).expect("plan");
        assert_eq!(plan.harness, Harness::Codex);
        assert_eq!((plan.model.as_str(), plan.effort.as_str()), ("", ""));
        assert_eq!(plan.prompt, "");
        assert_eq!(plan.checkout, Checkout::ProjectRoot);
        assert_eq!(plan.project, Path::new(root));
        assert!(matches!(
            plan.role,
            Role::Supervisor {
                handover_pane: None
            }
        ));
        // A preset no supervisor runs on falls back to the first that does.
        let plan = dashboard_supervisor_plan(&app, root, None, || Harness::OpenCode).expect("plan");
        assert_eq!(plan.harness, Harness::SUPERVISOR_HARNESSES[0]);
        // The last launch wins over the preset.
        let saved = supervisor::Launch {
            kind: "codex".into(),
            model: "gpt-5-codex".into(),
            effort: "high".into(),
            extra_args: vec!["--search".into()],
        };
        let plan =
            dashboard_supervisor_plan(&app, root, Some(saved), || Harness::Claude).expect("plan");
        assert_eq!(plan.harness, Harness::Codex);
        assert_eq!(plan.model, "gpt-5-codex");
        assert_eq!(plan.effort, "high");
        assert_eq!(plan.extra_args, ["--search"]);
    }

    #[test]
    fn the_supervisor_key_starts_the_selected_projects_supervisor_or_focuses_the_running_one() {
        // A name no developer's own supervisor state directory has.
        let root = "/repos/corgi-key-test";
        let worker = DashboardAgent {
            info: AgentInfo {
                name: Some("corgi-key-test".into()),
                pane_id: "w1:p1".into(),
                ..AgentInfo::default()
            },
            project_root: root.into(),
            ..DashboardAgent::default()
        };

        let mut app = test_app();
        press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.status.contains("Select an agent"), "{}", app.status);

        // Only a worker runs: the key starts the launch at once, no form.
        app.agents = vec![worker.clone()];
        press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);
        assert!(
            app.overlay
                .new_agent_launch()
                .is_some_and(|launch| launch.name == "corgi-corgi-key-test"),
            "{:?}",
            app.overlay.new_agent_launch()
        );
        poll_until(&mut app, App::poll_launch, |app| {
            !app.launch_job.is_running()
        });

        // With the supervisor running, the key selects and focuses it, walking
        // to its workspace and tab first as Enter does, and starts nothing.
        let (socket_path, server) = fake_herdr("corgi-key-focus", |listener| {
            let mut requests = Vec::new();
            for (method, reply) in [
                ("workspace.focus", "workspace_info"),
                ("tab.focus", "tab_info"),
                ("agent.focus", "agent_info"),
            ] {
                answer(&listener, |request| {
                    assert_eq!(request["method"], method);
                    requests.push(request["params"].clone());
                    json!({ "result": {
                        "type": reply,
                        "agent": {
                            "pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t3"
                        }
                    }})
                });
            }
            requests
        });
        let mut app = test_app();
        app.client = HerdrClient::from_socket_path(&socket_path);
        app.agents = vec![
            DashboardAgent {
                info: AgentInfo {
                    name: Some("corgi-corgi-key-test".into()),
                    pane_id: "w2:p1".into(),
                    workspace_id: "w2".into(),
                    tab_id: "w2:t3".into(),
                    ..AgentInfo::default()
                },
                project_root: root.into(),
                supervisor: true,
                ..DashboardAgent::default()
            },
            worker,
        ];
        app.selected = 1;
        press(&mut app, KeyCode::Char('c'), KeyModifiers::NONE);
        let requests = server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");
        assert_eq!(
            requests,
            [
                json!({ "workspace_id": "w2" }),
                json!({ "tab_id": "w2:t3" }),
                json!({ "target": "w2:p1" }),
            ]
        );
        assert!(matches!(app.overlay, Overlay::None));
        assert!(!app.launch_job.is_running());
        assert_eq!(app.selected, 0);
        assert_eq!(
            app.status,
            "Focused corgi-key-test's corgi, corgi-corgi-key-test"
        );
    }

    #[test]
    fn new_agent_popup_stays_open_until_launch_finishes() {
        let mut app = test_app();
        app.overlay = Overlay::Launch(NewAgentLaunch {
            name: "corgi-worker".into(),
            state: LaunchState::Running {
                status: "Creating workspace for corgi-worker…".into(),
                step: 0,
            },
            steps: Vec::new(),
        });
        let (step, steps) = mpsc::channel::<String>();
        app.launch_job = Job::spawn(move |progress| {
            for message in steps {
                progress.send(JobReport::Status(message));
            }
            Ok("corgi-worker started".into())
        });

        step.send("Sending first prompt…".into())
            .expect("send progress");
        poll_until(&mut app, App::poll_launch, |app| {
            app.overlay
                .new_agent_launch()
                .is_some_and(|launch| launch.state.text() == "Sending first prompt…")
        });
        assert!(matches!(app.overlay, Overlay::Launch(_)));

        drop(step);
        poll_until(&mut app, App::poll_launch, |app| {
            !app.launch_job.is_running()
        });
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.overlay.new_agent_launch().is_none());
    }

    #[test]
    fn an_animated_dashboard_shows_a_started_agent_for_a_moment_then_closes() {
        let mut app = test_app();
        app.motion = crate::motion::Motion::manual();
        app.motion.set_time(Duration::from_secs(10));
        app.overlay = Overlay::Launch(NewAgentLaunch {
            name: "corgi-worker".into(),
            steps: vec!["Create a worktree".into(), "Start claude".into()],
            state: LaunchState::Running {
                status: "Starting corgi-worker…".into(),
                step: 0,
            },
        });
        app.launch_job = Job::spawn(|progress| {
            progress.send(JobReport::Step(1));
            Ok("corgi-worker started in worktree calm-river".into())
        });
        poll_until(&mut app, App::poll_launch, |app| {
            !app.launch_job.is_running()
        });
        assert!(matches!(
            app.overlay.new_agent_launch().map(|launch| &launch.state),
            Some(LaunchState::Started { message, .. }) if message.contains("calm-river")
        ));
        assert_eq!(app.overlay.outcome(), Some(crate::motion::Outcome::Success));

        // It stays up over the whole stamp, then closes by itself.
        assert!(crate::motion::STAMP_LENGTH < STARTED_HOLD);
        app.motion
            .set_time(Duration::from_secs(10) + STARTED_HOLD / 2);
        app.close_started_launch();
        assert!(matches!(app.overlay, Overlay::Launch(_)));
        app.motion.set_time(Duration::from_secs(10) + STARTED_HOLD);
        app.close_started_launch();
        assert!(matches!(app.overlay, Overlay::None));

        // A key closes it sooner.
        app.overlay = Overlay::Launch(NewAgentLaunch {
            name: "corgi-worker".into(),
            steps: Vec::new(),
            state: LaunchState::Started {
                message: "started".into(),
                at: Duration::from_secs(10),
            },
        });
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn a_failed_new_agent_keeps_its_popup_open_for_dismissal() {
        let mut app = test_app();
        app.overlay = Overlay::Launch(NewAgentLaunch {
            name: "corgi-worker".into(),
            state: LaunchState::Running {
                status: "Starting corgi-worker…".into(),
                step: 0,
            },
            steps: Vec::new(),
        });
        app.launch_job = Job::spawn(|_| Err(anyhow!("approval required")));

        poll_until(&mut app, App::poll_launch, |app| {
            !app.launch_job.is_running()
        });
        assert!(matches!(app.overlay, Overlay::Launch(_)));
        assert!(
            app.overlay
                .new_agent_launch()
                .is_some_and(|launch| matches!(launch.state, LaunchState::Failed(_)))
        );

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.overlay.new_agent_launch().is_none());
    }
}
