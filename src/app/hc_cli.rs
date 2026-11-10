//! The command-line tools on herdr-core (`hc`), the Herdr app's backend:
//! `corgi supervisor` and `corgi spawn` starting agents there, and the
//! agents that `fleet`, `report`, `digest` and `tag` see.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::{
    git::trust_root,
    harness::{Harness, default_harness},
    hc::{self, HcClient, launch::Launched},
    launch::{
        CORGI_REQUEST_TOKEN, Checkout, LaunchPlan, Role, SUPERVISOR_HOME_REFUSAL,
        sanitize_agent_name, supervisor_plan,
    },
    model::DashboardAgent,
    paths::{corgi_state_dir, dir_name, is_home, project_root_digest},
    progress::Progress,
    projects::ProjectMemory,
    supervisor,
    tag::TagOptions,
    time::unix_now,
};

use super::cli::{SpawnOptions, spawn_harness};

/// The project root of `project` on hc, as its agents' `corgi.project`
/// label holds it: the primary checkout of its repository, as Git says, or
/// the directory itself outside Git, without symlinks so that labels
/// compare as text.
pub(super) fn hc_project_root(project: &Path) -> String {
    let root = trust_root(project);
    fs::canonicalize(&root)
        .unwrap_or(root)
        .to_string_lossy()
        .into_owned()
}

/// Progress on stderr, for a launch that remembers no project.
struct Said;

impl Progress for Said {
    fn report(&mut self, message: String) {
        eprintln!("{message}");
    }
}

/// `corgi spawn` on hc: starts a worker for `project` with `prompt`, unless
/// a running worker of the project has the request id, and returns the JSON
/// it prints.
pub(super) fn spawn(
    options: SpawnOptions,
    project: PathBuf,
    prompt: String,
) -> Result<serde_json::Value> {
    let client = HcClient::from_env()?;
    let root = if is_home(&project) {
        project.to_string_lossy().into_owned()
    } else {
        hc_project_root(&project)
    };
    // Held until the worker has started, so that a retry waits for it and
    // then finds it.
    let _lock = match options.request_id.as_deref() {
        Some(_) => Some(spawn_lock(&root, || {
            eprintln!("Waiting for another spawn in {root} to finish…");
        })?),
        None => None,
    };
    let agents = hc::agents::agents(&client)?;
    if let Some(id) = options.request_id.as_deref()
        && let Some(existing) = requested_agent(&agents, &root, id)
    {
        eprintln!("An agent spawned with request id {id} is already running");
        return Ok(existing);
    }
    let caller = hc::calling_terminal();
    let supervisor_harness = agents
        .iter()
        .find(|agent| agent.supervisor && Some(&agent.info.pane_id) == caller.as_ref())
        .map(|agent| agent.info.harness());
    let harness = spawn_harness(options.harness, supervisor_harness, default_harness);
    anyhow::ensure!(
        options.effort.is_empty() || harness.supports_effort(),
        "{harness} has no effort control; only codex and claude take --effort"
    );
    let base = dir_name(&project).unwrap_or("agent");
    let name = free_name(
        &client,
        options.name.as_deref().unwrap_or(base),
        options.name.is_none(),
    )?;
    let plan = LaunchPlan {
        name,
        harness,
        model: options.model,
        effort: options.effort,
        prompt,
        project,
        checkout: options.checkout,
        extra_args: options.agent_args,
        role: Role::Worker,
        request_id: options.request_id,
    };
    let launched = hc::launch::launch_worker(&client, &plan, &root, &mut Said)?;
    Ok(launched_json(&plan, &launched))
}

/// `corgi supervisor` on hc: starts the supervisor of `project` on
/// `harness`, with `task` as its first request, and returns the JSON it
/// prints.
pub(super) fn supervisor(
    options: &SpawnOptions,
    harness: Harness,
    project: &Path,
    task: String,
) -> Result<serde_json::Value> {
    let client = HcClient::from_env()?;
    let root = hc_project_root(project);
    anyhow::ensure!(!is_home(&root), SUPERVISOR_HOME_REFUSAL);
    let (exited, others) = exited_supervisor(hc::agents::agents(&client)?, &root);
    let mut plan = supervisor_plan(
        &others,
        &root,
        harness,
        options.model.clone(),
        options.effort.clone(),
        task,
    )?;
    plan.extra_args = options.agent_args.clone();
    let shell = match options.terminal.as_deref() {
        Some(terminal) => Shell::Named(terminal),
        None => Shell::New,
    };
    let (launched, _) = start_supervisor(
        &client,
        &mut plan,
        exited.as_ref(),
        options.name.as_deref(),
        shell,
    )?;
    Ok(launched_json(&plan, &launched))
}

/// The supervisor of the project at `root` whose agent exited into its kept
/// shell, if there is one, and the other agents: a supervisor starts again
/// in that terminal, which keeps its name and space, and its workers keep
/// reporting.
pub(super) fn exited_supervisor(
    agents: Vec<DashboardAgent>,
    root: &str,
) -> (Option<DashboardAgent>, Vec<DashboardAgent>) {
    let exited = agents
        .iter()
        .find(|agent| agent.supervisor && agent.exited && agent.project_root == root)
        .cloned();
    let others = agents
        .into_iter()
        .filter(|agent| Some(&agent.info.pane_id) != exited.as_ref().map(|e| &e.info.pane_id))
        .collect();
    (exited, others)
}

/// The existing shell terminal a supervisor may start in.
#[derive(Debug, Clone, Copy)]
pub(super) enum Shell<'a> {
    /// None: a new terminal of Corgi's.
    New,
    /// This one (an address or a name), or nothing starts.
    Named(&'a str),
    /// The first of these with a shell waiting at its prompt in the
    /// project's primary checkout, else a new terminal.
    Offered(&'a [String]),
}

/// Where [`start_supervisor`] started the supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Started {
    /// A new terminal, in the space Corgi gives the project.
    New,
    /// The project's own supervisor terminal, at its shell since its agent
    /// exited.
    Own,
    /// A shell the person had open, which is the supervisor's from now on.
    Shell,
}

impl Started {
    pub(super) fn word(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Own => "own",
            Self::Shell => "shell",
        }
    }
}

/// Starts `plan`, the supervisor of the project `plan.project`, and
/// remembers the project: in its own terminal when `exited` is that
/// terminal at its shell, else in the terminal `shell` names or offers, else
/// in a new one named `name` (default: `<project>-supervisor`). `plan.name`
/// becomes the name of the terminal it started in.
pub(super) fn start_supervisor(
    client: &HcClient,
    plan: &mut LaunchPlan,
    exited: Option<&DashboardAgent>,
    name: Option<&str>,
    shell: Shell,
) -> Result<(Launched, Started)> {
    let root = plan.project.to_string_lossy().into_owned();
    let corgi_bin = supervisor::corgi_bin(None)?;
    let named = match shell {
        Shell::Named(target) => Some(client.get(target)?),
        _ => None,
    };
    if let Some(exited) = exited {
        // One supervisor terminal per project: its own waits for it.
        anyhow::ensure!(
            named
                .as_ref()
                .is_none_or(|terminal| terminal.address == exited.info.pane_id),
            "{root}'s supervisor terminal {} ({}) is at its shell: run corgi supervisor without \
             --terminal to start there, or close that terminal first",
            exited.info.display_name(),
            exited.info.pane_id
        );
        plan.name = exited.info.display_name().to_string();
        eprintln!(
            "{root}'s supervisor terminal {} ({}) is at its shell: starting the supervisor there",
            plan.name, exited.info.pane_id
        );
        let launched = hc::launch::relaunch_supervisor(
            client,
            plan,
            &corgi_bin,
            &exited.info.pane_id,
            &mut Said,
        )?;
        remember_project(&root);
        return Ok((launched, Started::Own));
    }
    let adopt = named.or_else(|| match shell {
        Shell::Offered(terminals) => terminals.iter().find_map(|target| {
            client.find(target).ok().flatten().filter(|terminal| {
                hc::launch::adoption_refusal(terminal).is_none() && shell_is_in(terminal, &root)
            })
        }),
        _ => None,
    });
    if let Some(terminal) = adopt {
        plan.name = terminal.display_name().to_string();
        if name.is_some() {
            eprintln!(
                "--name is not used: {} keeps the name it has",
                terminal.address
            );
        }
        eprintln!(
            "Starting {root}'s supervisor in the shell of {} ({})",
            plan.name, terminal.address
        );
        let launched =
            hc::launch::adopt_terminal(client, plan, &corgi_bin, &terminal.address, &mut Said)?;
        remember_project(&root);
        return Ok((launched, Started::Shell));
    }
    let project_name = dir_name(&root).unwrap_or(supervisor::UNNAMED_PROJECT);
    plan.name = match name {
        Some(requested) => free_name(client, requested, false)?,
        None => free_name(client, &format!("{project_name}-supervisor"), false)?,
    };
    let launched = hc::launch::launch_supervisor(client, plan, &corgi_bin, &mut Said)?;
    remember_project(&root);
    Ok((launched, Started::New))
}

/// Whether the shell in front of `terminal` is in the directory `root`, a
/// project's primary checkout without symlinks.
fn shell_is_in(terminal: &hc::Terminal, root: &str) -> bool {
    terminal.foreground.as_ref().is_some_and(|foreground| {
        fs::canonicalize(&foreground.cwd).is_ok_and(|cwd| cwd == Path::new(root))
    })
}

/// Writes `root` down among the projects Corgi used, so that `corgi status`
/// still lists the project, with its supervisor's state, once no agent of it
/// runs.
fn remember_project(root: &str) {
    ProjectMemory::load().remember(root);
}

/// What `corgi spawn` and `corgi supervisor` print of an agent they started:
/// the terminal's address is `terminal` (and `pane_id`, for a script that
/// reads that).
pub(super) fn launched_json(plan: &LaunchPlan, launched: &Launched) -> serde_json::Value {
    serde_json::json!({
        "name": launched.name,
        "terminal": launched.terminal.address,
        "pane_id": launched.terminal.address,
        "cwd": launched.cwd,
        "kind": plan.harness.kind(),
        "model": plan.model,
        "effort": plan.effort,
        "checkout": launched.checkout.value(),
        "supervisor": matches!(plan.role, Role::Supervisor { .. }),
        "location": launched.location,
        "session": launched.session,
        "backend": "hc",
    })
}

/// The running agent of the project at `root` on hc that a spawn with
/// request id `id` started, as the JSON a fresh spawn prints, marked
/// `"existing"`.
fn requested_agent(agents: &[DashboardAgent], root: &str, id: &str) -> Option<serde_json::Value> {
    let agent = agents.iter().find(|agent| {
        agent.project_root == root
            && agent
                .info
                .tokens
                .get(CORGI_REQUEST_TOKEN)
                .map(String::as_str)
                == Some(id)
    })?;
    let info = &agent.info;
    let checkout = if agent.worktree_checkout.is_some() {
        Checkout::Worktree
    } else if info.cwd() == root {
        Checkout::ProjectRoot
    } else {
        Checkout::Directory
    };
    Some(serde_json::json!({
        "name": info.display_name(),
        "terminal": info.pane_id,
        "pane_id": info.pane_id,
        "cwd": info.cwd,
        "kind": info.kind(),
        "model": agent.model.as_deref().unwrap_or_default(),
        "effort": agent.effort.as_deref().unwrap_or_default(),
        "checkout": checkout.value(),
        "supervisor": agent.supervisor,
        "location": format!(
            "already running in {} ({}), hc terminal {}",
            info.cwd(),
            info.state.label().to_lowercase(),
            info.pane_id
        ),
        "existing": true,
        "backend": "hc",
    }))
}

/// `requested` as an hc terminal name no live terminal has (hc keeps names
/// unique among live terminals, Corgi's or not). With `numbered`, a taken
/// name gets a number instead of being refused.
fn free_name(client: &HcClient, requested: &str, numbered: bool) -> Result<String> {
    let name = sanitize_agent_name(requested);
    let taken = |name: &str| -> Result<bool> {
        Ok(client
            .find(name)?
            .is_some_and(|terminal| terminal.is_live()))
    };
    if !taken(&name)? {
        return Ok(name);
    }
    anyhow::ensure!(numbered, "a terminal named {name} is already running");
    let numbered = sanitize_agent_name(&format!("{name}-{:04}", unix_now() % 10_000));
    anyhow::ensure!(
        !taken(&numbered)?,
        "a terminal named {numbered} is already running"
    );
    Ok(numbered)
}

/// The lock that keeps a spawn with a request id in the project at `root`
/// from racing another, in Corgi's state directory. `waiting` is called
/// once if another spawn holds it.
fn spawn_lock(root: &str, waiting: impl FnOnce()) -> Result<fs::File> {
    let dir = corgi_state_dir()
        .context("no state directory for the spawn lock")?
        .join("hc");
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let digest = project_root_digest(root);
    let path = dir.join(format!(
        "spawn-{}.lock",
        digest.trim_start_matches("fnv1a64:")
    ));
    let lock = fs::File::create(&path).with_context(|| format!("create {}", path.display()))?;
    if lock.try_lock().is_err() {
        waiting();
        lock.lock()
            .with_context(|| format!("lock {}", path.display()))?;
    }
    Ok(lock)
}

/// Corgi's agents on hc, and the client that read them.
pub(super) struct Seen {
    pub(super) client: HcClient,
    pub(super) agents: Vec<DashboardAgent>,
}

/// What a command run here sees of hc: inside one of its terminals, that
/// terminal's core; anywhere else, the core that already runs, since asking
/// a stopped one would start it.
pub(super) fn seen() -> Result<Seen> {
    anyhow::ensure!(
        hc::in_terminal() || hc::core_running(),
        "herdr-core (hc) is not running; run this inside one of its terminals, or start it"
    );
    let client = HcClient::from_env()?;
    let agents = hc::agents::agents(&client)?;
    Ok(Seen { client, agents })
}

impl Seen {
    /// The running agent named or addressed `target`.
    pub(super) fn find(&self, target: &str) -> Option<&DashboardAgent> {
        self.agents.iter().find(|agent| {
            agent.info.name.as_deref() == Some(target) || agent.info.pane_id == target
        })
    }

    /// `corgi tag` on the agent `options` names, of the project at `root`.
    pub(super) fn tag(&self, root: &str, options: &TagOptions) -> Result<String> {
        hc::agents::tag(&self.client, &self.agents, root, options)
    }
}
