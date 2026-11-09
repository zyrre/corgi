//! The command-line tools on herdr-core (`hc`), the Herdr app's backend:
//! `corgi supervisor` and `corgi spawn` starting agents there, and the
//! agents the commands that only read (`fleet`, `report`, `digest`) and
//! `tag` see, from either backend. Which backend a command uses is
//! [`corgi_core::backend`]'s rule.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::{
    harness::Harness,
    herdr::HerdrClient,
    model::DashboardAgent,
    paths::{corgi_state_dir, dir_name, is_home},
    supervisor,
    tag::{TagOptions, tag_agent},
};
use corgi_core::{
    backend::{Backend, Caller, reachable_herdr},
    git::trust_root,
    hc::{self, HcClient, launch::Launched},
    launch::{
        CORGI_REQUEST_TOKEN, Checkout, LaunchPlan, Role, SUPERVISOR_HOME_REFUSAL,
        sanitize_agent_name, supervisor_plan,
    },
    progress::Progress,
    project_main::{project_root_digest, project_root_of},
    time::unix_now,
};

use super::{
    App,
    catalog::default_harness,
    cli::{SpawnOptions, connected_app, refreshed_app, spawn_harness},
};

/// Where `corgi supervisor` and `corgi spawn` start their agent: as
/// `requested` by `--backend`, else where the caller runs, else on hc
/// unless a Herdr TUI is running.
pub(super) fn launch_backend(requested: Option<Backend>) -> Backend {
    Caller::from_env().launch_backend(requested, || reachable_herdr().is_some())
}

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
    // then finds it, as on the Herdr TUI.
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
        new_project: false,
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
    let agents = hc::agents::agents(&client)?;
    if let Some(exited) = agents
        .iter()
        .find(|agent| agent.supervisor && agent.exited && agent.project_root == root)
    {
        anyhow::bail!(
            "{root}'s supervisor terminal {} ({}) is still there, its agent exited; resume \
             it there, or end it with hc kill {} first",
            exited.info.display_name(),
            exited.info.pane_id,
            exited.info.pane_id
        );
    }
    let mut plan = supervisor_plan(
        &agents,
        &root,
        harness,
        options.model.clone(),
        options.effort.clone(),
        task,
    )?;
    let project_name = dir_name(&root).unwrap_or(supervisor::UNNAMED_PROJECT);
    plan.name = match options.name.as_deref() {
        Some(requested) => free_name(&client, requested, false)?,
        None => free_name(&client, &format!("{project_name}-supervisor"), false)?,
    };
    plan.extra_args = options.agent_args.clone();
    let corgi_bin = supervisor::corgi_bin(None)?;
    let launched = hc::launch::launch_supervisor(&client, &plan, &corgi_bin, &mut Said)?;
    Ok(launched_json(&plan, &launched))
}

/// What `corgi spawn` and `corgi supervisor` print of an agent started on
/// hc: the fields they print on the Herdr TUI, with the terminal's address
/// as `terminal` (and as `pane_id`, for a script that reads that).
fn launched_json(plan: &LaunchPlan, launched: &Launched) -> serde_json::Value {
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
        "backend": Backend::Hc.name(),
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
        "backend": Backend::Hc.name(),
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

/// The agents a command sees: the Herdr TUI's and hc's, each where the
/// caller can reach it.
pub(super) struct Seen {
    pub(super) herdr: Option<(HerdrClient, App)>,
    pub(super) hc: Option<(HcClient, Vec<DashboardAgent>)>,
}

/// What a command run here sees: inside a Herdr TUI pane, that TUI alone,
/// exactly as before hc; inside an hc terminal, hc alone; anywhere else,
/// whichever of the two runs (hc only when its core already runs, since
/// asking it would start one).
pub(super) fn seen() -> Result<Seen> {
    match Caller::from_env() {
        Caller::Herdr => Ok(Seen {
            herdr: Some(connected_app()?),
            hc: None,
        }),
        Caller::Hc => {
            let client = HcClient::from_env()?;
            let agents = hc::agents::agents(&client)?;
            Ok(Seen {
                herdr: None,
                hc: Some((client, agents)),
            })
        }
        Caller::Outside => {
            let herdr = reachable_herdr()
                .and_then(|client| refreshed_app(client.clone()).ok().map(|app| (client, app)));
            let hc = hc::core_running()
                .then(HcClient::locate)
                .flatten()
                .and_then(|client| {
                    hc::agents::agents(&client)
                        .ok()
                        .map(|agents| (client, agents))
                });
            anyhow::ensure!(
                herdr.is_some() || hc.is_some(),
                "neither a Herdr TUI nor herdr-core (hc) is running; run this inside one, or \
                 start one"
            );
            Ok(Seen { herdr, hc })
        }
    }
}

impl Seen {
    /// The root of `project`: as the Herdr TUI reports it when it is seen,
    /// else as hc's labels hold it.
    pub(super) fn root(&self, project: &Path) -> Result<String> {
        match &self.herdr {
            Some((client, _)) => project_root_of(client, project),
            None => Ok(hc_project_root(project)),
        }
    }

    /// Every agent seen, the Herdr TUI's first.
    pub(super) fn agents(&self) -> Vec<DashboardAgent> {
        let herdr = self.herdr.iter().flat_map(|(_, app)| app.agents.iter());
        let hc = self.hc.iter().flat_map(|(_, agents)| agents.iter());
        herdr.chain(hc).cloned().collect()
    }

    /// The running agent named or addressed `target`, and how to read its
    /// screen: the Herdr TUI's first.
    pub(super) fn find(&self, target: &str) -> Option<(DashboardAgent, Screen<'_>)> {
        let named = |agent: &&DashboardAgent| {
            agent.info.name.as_deref() == Some(target) || agent.info.pane_id == target
        };
        if let Some((client, app)) = &self.herdr
            && let Some(agent) = app.agents.iter().find(named)
        {
            return Some((agent.clone(), Screen::Herdr(client)));
        }
        let (client, agents) = self.hc.as_ref()?;
        let agent = agents.iter().find(named)?;
        Some((agent.clone(), Screen::Hc(client)))
    }

    /// `corgi tag` on whichever backend runs the agent: the Herdr TUI when
    /// it is seen and has the agent (or hc is not seen), else hc.
    pub(super) fn tag(&self, root: &str, options: &TagOptions) -> Result<String> {
        let in_project = |agents: &[DashboardAgent]| {
            agents.iter().any(|agent| {
                agent.project_root == root
                    && (agent.info.name.as_deref() == Some(options.name.as_str())
                        || agent.info.pane_id == options.name)
            })
        };
        match (&self.herdr, &self.hc) {
            (Some((client, app)), None) => tag_agent(client, &app.agents, root, options),
            (Some((client, app)), Some(_)) if in_project(&app.agents) => {
                tag_agent(client, &app.agents, root, options)
            }
            (_, Some((client, agents))) => hc::agents::tag(client, agents, root, options),
            (None, None) => anyhow::bail!("no Herdr TUI or hc to tag {} in", options.name),
        }
    }
}

/// How to read a running agent's screen.
pub(super) enum Screen<'a> {
    Herdr(&'a HerdrClient),
    Hc(&'a HcClient),
}

impl Screen<'_> {
    /// The newest `lines` lines of the screen of the agent in `pane`.
    pub(super) fn read(&self, pane: &str, lines: u32) -> Result<String> {
        match self {
            Self::Herdr(client) => Ok(client
                .read_agent(pane, crate::herdr::ReadSource::RecentUnwrapped, Some(lines))?
                .text),
            Self::Hc(client) => client.read(pane, lines),
        }
    }
}
