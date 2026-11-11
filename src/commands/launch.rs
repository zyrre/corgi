//! `corgi spawn` and `corgi supervisor`: starting a worker, and a project's
//! supervisor, in herdr-core (`hc`) terminals.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::{
    git::git_output,
    harness::{EFFORT_LEVELS, Harness, default_harness, record_chosen_harness},
    hc::{self, HcClient, launch::Launched},
    launch::{
        CORGI_REQUEST_TOKEN, Checkout, LaunchPlan, Role, SUPERVISOR_HOME_REFUSAL,
        sanitize_agent_name, supervisor_harness_error, supervisor_plan,
    },
    model::Agent,
    paths::{corgi_state_dir, dir_name, is_home, project_root_digest},
    progress::Progress,
    projects::ProjectMemory,
    supervisor,
    time::unix_now,
};

use super::{existing_project_dir, project_root_of};

pub const SPAWN_USAGE: &str = "\
Usage: corgi spawn [OPTIONS] [-- AGENT_ARGS...] < task.md

Starts one agent in a herdr-core (hc) terminal of its own and prints it as
JSON. The task is read from stdin, or from --task-file, never from arguments.

Options:
  --project PATH     Project directory (default: the current directory)
  --harness KIND     Agent CLI: claude, codex, opencode, … (default: the
                     calling supervisor's own, else the last one a person
                     chose with --harness, else CORGI_DEFAULT_AGENT, else
                     the first one installed)
  --model MODEL      Model to start on (default: the harness's own setting)
  --effort LEVEL     low, medium, high, xhigh, or max (Codex and Claude Code)
  --checkout MODE    worktree (default), directory, or root: the project's
                     primary checkout. A worktree is refused, before
                     anything starts, for a project outside a Git work tree
                     (pass --project, or --checkout directory to run there
                     without one), and every mode for a directory in Corgi's
                     state directory, such as a supervisor's; the home
                     directory still starts a scratch agent
  --name NAME        Agent name (default: taken from the project)
  --task-file PATH   Read the task from PATH instead of stdin
  --request-id ID    Make a retry safe: when a running agent of the project
                     was spawned with ID, print it (with \"existing\": true)
                     and start nothing. ID is 1 to 80 ASCII letters, digits,
                     '.', '_' or '-', such as a brief id; a retry made while
                     the first spawn still runs waits for it

Arguments after -- go to the agent CLI unchanged, after the model and effort.";

/// The characters a `--request-id` may have besides ASCII letters and
/// digits. The id is stored verbatim as a label of the worker's terminal.
const REQUEST_ID_PUNCTUATION: [char; 3] = ['.', '_', '-'];

/// The longest `--request-id`, in characters.
const REQUEST_ID_MAX_CHARS: usize = 80;

/// What `corgi spawn` was asked for, before anything is resolved against
/// hc or the file system.
#[derive(Debug, Default, PartialEq, Eq)]
struct SpawnOptions {
    project: Option<String>,
    harness: Option<Harness>,
    model: String,
    effort: String,
    checkout: Checkout,
    name: Option<String>,
    task_file: Option<String>,
    request_id: Option<String>,
    agent_args: Vec<String>,
    /// The existing hc terminal `corgi supervisor --terminal` starts in.
    terminal: Option<String>,
}

/// The options of `corgi spawn` and `corgi supervisor`, which take the same
/// ones; `usage` is the command's own, for an option it does not know.
fn parse_spawn_options(args: &[String], usage: &str) -> Result<SpawnOptions> {
    let mut options = SpawnOptions::default();
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .with_context(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--project" => options.project = Some(value()?),
            "--harness" => options.harness = Some(Harness::from_typed(&value()?)),
            "--model" => options.model = value()?.trim().to_string(),
            "--effort" => options.effort = value()?.trim().to_lowercase(),
            "--checkout" => {
                let mode = value()?;
                options.checkout = Checkout::from_value(&mode).with_context(|| {
                    format!("--checkout must be worktree, directory, or root, not {mode:?}")
                })?;
            }
            "--name" => options.name = Some(value()?),
            "--task-file" => options.task_file = Some(value()?),
            "--request-id" => options.request_id = Some(valid_request_id(value()?)?),
            "--terminal" => options.terminal = Some(value()?.trim().to_string()),
            "--" => {
                options.agent_args = args.by_ref().cloned().collect();
                break;
            }
            other => bail!("unknown option {other:?}\n\n{usage}"),
        }
    }
    if !options.effort.is_empty() {
        anyhow::ensure!(
            EFFORT_LEVELS.contains(&options.effort.as_str()),
            "--effort must be one of {}",
            EFFORT_LEVELS.join(", ")
        );
        if let Some(harness) = &options.harness {
            anyhow::ensure!(
                harness.kind().is_empty() || harness.supports_effort(),
                "{harness} has no effort control; only codex and claude take --effort"
            );
        }
    }
    Ok(options)
}

/// `id` if it can be a `--request-id`: short enough to keep whole as a label
/// and in need of no quoting anywhere.
fn valid_request_id(id: String) -> Result<String> {
    anyhow::ensure!(!id.is_empty(), "--request-id must not be empty");
    anyhow::ensure!(
        id.chars().count() <= REQUEST_ID_MAX_CHARS,
        "--request-id must be at most {REQUEST_ID_MAX_CHARS} characters"
    );
    anyhow::ensure!(
        id.chars()
            .all(|c| c.is_ascii_alphanumeric() || REQUEST_ID_PUNCTUATION.contains(&c)),
        "--request-id may only have ASCII letters, digits, '.', '_' and '-', not {id:?}"
    );
    Ok(id)
}

/// `corgi spawn`: starts one worker on hc, for a supervisor agent or a person.
/// Progress goes to stderr and the started agent to stdout as one JSON
/// object.
pub fn spawn_command(args: &[String]) -> Result<()> {
    let options = parse_spawn_options(args, SPAWN_USAGE)?;
    anyhow::ensure!(
        options.terminal.is_none(),
        "--terminal is for corgi supervisor; a worker gets a terminal of its own"
    );
    let project = existing_project_dir(options.project.as_deref())?;
    // Refused before stdin is read, hc is asked anything, or a retry's
    // request id is looked up: a refused spawn starts nothing, so a retry
    // that is refused again misses no worker.
    if let Some(refusal) = spawn_project_refusal(
        &project,
        options.checkout,
        in_git_work_tree(&project),
        corgi_state_dir().as_deref(),
    ) {
        bail!(refusal);
    }
    let prompt = read_task(options.task_file.as_deref(), true)?;
    anyhow::ensure!(!prompt.is_empty(), "the task is empty\n\n{SPAWN_USAGE}");
    print_launch(&spawn_worker(options, project, prompt)?)
}

/// Why `corgi spawn` must not start a worker in `project` with `checkout`,
/// telling the caller how to fix the command, or `None` when it may. A worker
/// never belongs in Corgi's state directory `state_dir` (a supervisor that
/// ran spawn from its own would get one there), and a worktree needs a Git
/// work tree. The home directory is the scratch agent's and allowed.
fn spawn_project_refusal(
    project: &Path,
    checkout: Checkout,
    in_git: bool,
    state_dir: Option<&Path>,
) -> Option<String> {
    if is_home(project) {
        return None;
    }
    let shown = project.display();
    if state_dir.is_some_and(|state| is_within(project, state)) {
        return Some(format!(
            "{shown} is in Corgi's state directory (it looks like a supervisor's), where no \
             worker belongs with any --checkout; pass --project <project dir> rather than \
             running spawn from there"
        ));
    }
    (checkout == Checkout::Worktree && !in_git).then(|| {
        format!(
            "{shown} is not a Git repository, so it can have no worktree; pass --project \
             <project dir>, or --checkout directory to run in it without a worktree"
        )
    })
}

/// Whether `path` is `dir` or inside it, compared through symlinks when both
/// exist.
fn is_within(path: &Path, dir: &Path) -> bool {
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    canonical(path).starts_with(canonical(dir))
}

/// Whether `dir` is inside a Git work tree, as Git itself says.
fn in_git_work_tree(dir: &Path) -> bool {
    git_output(dir, &["rev-parse", "--is-inside-work-tree"]).is_ok_and(|out| out.trim() == "true")
}

/// The harness `corgi spawn` starts: the one asked for, else the calling
/// supervisor's own, else the default.
fn spawn_harness(
    requested: Option<Harness>,
    supervisor: Option<Harness>,
    default: impl FnOnce() -> Harness,
) -> Harness {
    requested.or(supervisor).unwrap_or_else(default)
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
fn spawn_worker(
    options: SpawnOptions,
    project: PathBuf,
    prompt: String,
) -> Result<serde_json::Value> {
    let client = HcClient::from_env()?;
    let root = if is_home(&project) {
        project.to_string_lossy().into_owned()
    } else {
        project_root_of(&project)
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
        .find(|agent| agent.supervisor && Some(&agent.info.address) == caller.as_ref())
        .map(|agent| agent.info.harness());
    // A harness a person asks for is the one they last chose; one a
    // supervisor (or another of Corgi's agents) asks for is not.
    let chosen = options.harness.is_some()
        && !agents
            .iter()
            .any(|agent| Some(&agent.info.address) == caller.as_ref());
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
    if chosen {
        record_chosen_harness(&plan.harness);
    }
    Ok(launched_json(&plan, &launched))
}

pub const SUPERVISOR_USAGE: &str = "\
Usage: corgi supervisor [PROJECT] [OPTIONS] [-- AGENT_ARGS...] [< task.md]

Starts the supervisor of PROJECT (default: the project containing the current
directory), the agent that directs its workers, in a kept herdr-core (hc)
shell terminal in the project's primary checkout, and prints it as JSON. A
task on stdin or in --task-file becomes its first request; without one the
supervisor greets you with the state of the project.

Options:
  --harness KIND     claude (default) or codex
  --model MODEL      Model to start on (default: the harness's own setting)
  --effort LEVEL     low, medium, high, xhigh, or max
  --name NAME        Agent name (default: <project>-supervisor)
  --task-file PATH   Read the first request from PATH
  --terminal T       Start in the existing terminal T (an address such
                     as t7, or a name) instead of a new one: a shell waiting
                     at its prompt, such as a tab open in the Herdr app. It
                     becomes the project's supervisor terminal, and the space
                     it is in the project's, where workers nest. Refused for
                     a terminal with an agent or a running command, and for
                     one Corgi already uses

Arguments after -- go to the agent CLI unchanged, after the model and effort.";

/// `corgi supervisor`: launches a project's supervisor.
pub fn supervisor_command(args: &[String]) -> Result<()> {
    let (positional, args) = match args.first() {
        Some(first) if !first.starts_with('-') => (Some(first.as_str()), &args[1..]),
        _ => (None, args),
    };
    let options = parse_spawn_options(args, SUPERVISOR_USAGE)?;
    anyhow::ensure!(
        options.request_id.is_none(),
        "--request-id is for corgi spawn; a project has one supervisor anyway"
    );
    let harness = options
        .harness
        .clone()
        .unwrap_or_else(|| Harness::SUPERVISOR_HARNESSES[0].clone());
    anyhow::ensure!(
        harness.supports_supervisor(),
        supervisor_harness_error(&harness)
    );
    let project = existing_project_dir(positional.or(options.project.as_deref()))?;
    anyhow::ensure!(!is_home(&project), SUPERVISOR_HOME_REFUSAL);
    let task = read_task(options.task_file.as_deref(), false)?;
    print_launch(&launch_supervisor(&options, harness, &project, task)?)
}

/// `corgi supervisor` on hc: starts the supervisor of `project` on
/// `harness`, with `task` as its first request, and returns the JSON it
/// prints.
fn launch_supervisor(
    options: &SpawnOptions,
    harness: Harness,
    project: &Path,
    task: String,
) -> Result<serde_json::Value> {
    let client = HcClient::from_env()?;
    let root = project_root_of(project);
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
    if options.harness.is_some() {
        record_chosen_harness(&plan.harness);
    }
    Ok(launched_json(&plan, &launched))
}

/// The supervisor of the project at `root` whose agent exited into its kept
/// shell, if there is one, and the other agents: a supervisor starts again
/// in that terminal, which keeps its name and space, and its workers keep
/// reporting.
pub(super) fn exited_supervisor(agents: Vec<Agent>, root: &str) -> (Option<Agent>, Vec<Agent>) {
    let exited = agents
        .iter()
        .find(|agent| agent.supervisor && agent.exited && agent.project_root == root)
        .cloned();
    let others = agents
        .into_iter()
        .filter(|agent| Some(&agent.info.address) != exited.as_ref().map(|e| &e.info.address))
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
    exited: Option<&Agent>,
    name: Option<&str>,
    shell: Shell,
) -> Result<(Launched, Started)> {
    let root = plan.project.to_string_lossy().into_owned();
    let corgi_bin = supervisor::corgi_bin()?;
    let named = match shell {
        Shell::Named(target) => Some(client.get(target)?),
        _ => None,
    };
    if let Some(exited) = exited {
        // One supervisor terminal per project: its own waits for it.
        anyhow::ensure!(
            named
                .as_ref()
                .is_none_or(|terminal| terminal.address == exited.info.address),
            "{root}'s supervisor terminal {} ({}) is at its shell: run corgi supervisor without \
             --terminal to start there, or close that terminal first",
            exited.info.display_name(),
            exited.info.address
        );
        plan.name = exited.info.display_name().to_string();
        eprintln!(
            "{root}'s supervisor terminal {} ({}) is at its shell: starting the supervisor there",
            plan.name, exited.info.address
        );
        let launched = hc::launch::relaunch_supervisor(
            client,
            plan,
            &corgi_bin,
            &exited.info.address,
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
fn requested_agent(agents: &[Agent], root: &str, id: &str) -> Option<serde_json::Value> {
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
        "terminal": info.address,
        "pane_id": info.address,
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
            info.address
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

/// The task for a command-line launch, from `task_file` or else stdin. A
/// launch that may go without a task reads stdin only when it is piped.
pub(super) fn read_task(task_file: Option<&str>, required: bool) -> Result<String> {
    use std::io::{IsTerminal, Read};
    let text = match task_file {
        Some(path) if path != "-" => {
            fs::read_to_string(path).with_context(|| format!("read task file {path}"))?
        }
        _ if !required && task_file.is_none() && io::stdin().is_terminal() => String::new(),
        _ => {
            let mut text = String::new();
            io::stdin()
                .read_to_string(&mut text)
                .context("read the task from stdin")?;
            text
        }
    };
    Ok(text.trim().to_string())
}

fn print_launch(started: &serde_json::Value) -> Result<()> {
    serde_json::to_writer(io::stdout(), started)?;
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corgi_corgi_refuses_the_home_directory() {
        if !crate::paths::home().is_some_and(|home| home.is_dir()) {
            return;
        }
        // Refused before stdin is read or hc is asked anything.
        let refused = supervisor_command(&["~".to_string()]).expect_err("no supervisor for ~");
        assert!(refused.to_string().contains("not a project"), "{refused}");
    }

    #[test]
    fn spawn_refuses_a_worktree_outside_git_and_any_worker_in_a_state_dir() {
        let dir = crate::test_support::ScratchDir::new("cli-spawn-refusal");
        let repo = dir.join("repo");
        let plain = dir.join("plain");
        let state = dir.join("state/corgi");
        let supervisor = state.join("supervisor/web");
        for path in [&repo, &plain, &supervisor] {
            fs::create_dir_all(path).unwrap();
        }
        let git = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(git.success());
        assert!(in_git_work_tree(&repo));
        assert!(!in_git_work_tree(&plain));

        let refusal = |project: &Path, checkout: Checkout| {
            spawn_project_refusal(project, checkout, in_git_work_tree(project), Some(&state))
        };
        // A Git repository gets a worktree, as before.
        assert_eq!(refusal(&repo, Checkout::Worktree), None);
        // A directory outside Git is refused a worktree, with the fix...
        let refused = refusal(&plain, Checkout::Worktree).expect("no worktree outside Git");
        assert!(refused.contains("is not a Git repository"), "{refused}");
        assert!(
            refused.contains("pass --project <project dir>"),
            "{refused}"
        );
        assert!(refused.contains("--checkout directory"), "{refused}");
        // ...but may be run in as is, or in its root tab.
        assert_eq!(refusal(&plain, Checkout::Directory), None);
        assert_eq!(refusal(&plain, Checkout::ProjectRoot), None);
        // A supervisor's state directory, or the state directory itself, is
        // refused with every checkout, even one that is a Git repository.
        for checkout in [
            Checkout::Worktree,
            Checkout::Directory,
            Checkout::ProjectRoot,
        ] {
            for project in [&supervisor, &state] {
                let refused = refusal(project, checkout).expect("no worker in a state dir");
                assert!(refused.contains("state directory"), "{refused}");
                assert!(
                    refused.contains("pass --project <project dir>"),
                    "{refused}"
                );
            }
            assert!(spawn_project_refusal(&repo, checkout, true, Some(&repo)).is_some());
        }
        // Without a state directory, only Git matters.
        assert_eq!(
            spawn_project_refusal(&supervisor, Checkout::Directory, false, None),
            None
        );
        // The home directory starts a scratch agent, whatever it is.
        if let Some(home) = crate::paths::home().filter(|home| home.is_dir()) {
            for checkout in [Checkout::Worktree, Checkout::Directory] {
                assert_eq!(
                    spawn_project_refusal(&home, checkout, false, Some(&home)),
                    None
                );
            }
        }
    }

    #[test]
    fn spawn_refuses_a_state_dir_before_reading_the_task_or_asking_herdr() {
        let Some(state) = corgi_state_dir().filter(|state| state.is_dir()) else {
            return;
        };
        let args = [
            "--project".to_string(),
            state.to_string_lossy().into_owned(),
        ];
        let refused = spawn_command(&args).expect_err("no worker in the state directory");
        assert!(refused.to_string().contains("state directory"), "{refused}");
    }

    #[test]
    fn a_supervisors_workers_default_to_the_supervisors_own_harness() {
        let default = || Harness::Claude;
        // A Codex supervisor starts Codex workers, a Claude supervisor Claude ones.
        assert_eq!(
            spawn_harness(None, Some(Harness::Codex), default),
            Harness::Codex
        );
        assert_eq!(
            spawn_harness(None, Some(Harness::Claude), || Harness::Codex),
            Harness::Claude
        );
        // An explicit harness wins.
        assert_eq!(
            spawn_harness(Some(Harness::Claude), Some(Harness::Codex), default),
            Harness::Claude
        );
        // A spawn not made by a supervisor keeps the default.
        assert_eq!(spawn_harness(None, None, || Harness::Codex), Harness::Codex);
    }

    #[test]
    fn spawn_options_default_to_nothing_asked_and_reject_what_cannot_be() {
        let args = |line: &str| {
            line.split_whitespace()
                .map(String::from)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            parse_spawn_options(&[], SPAWN_USAGE).unwrap(),
            SpawnOptions::default()
        );

        let options = parse_spawn_options(
            &args(
                "--project ~/repos/corgi --harness Claude --model opus --effort HIGH \
                 --checkout directory --name fix-cache --task-file brief.md \
                 --request-id 20261002-fix-cache --terminal t7 \
                 -- --append-system-prompt-file role.md --name x",
            ),
            SPAWN_USAGE,
        )
        .unwrap();
        assert_eq!(
            options,
            SpawnOptions {
                project: Some("~/repos/corgi".into()),
                harness: Some(Harness::Claude),
                model: "opus".into(),
                effort: "high".into(),
                checkout: Checkout::Directory,
                name: Some("fix-cache".into()),
                task_file: Some("brief.md".into()),
                request_id: Some("20261002-fix-cache".into()),
                agent_args: args("--append-system-prompt-file role.md --name x"),
                terminal: Some("t7".into()),
            }
        );
        assert_eq!(
            parse_spawn_options(&args("--checkout root"), SPAWN_USAGE)
                .unwrap()
                .checkout,
            Checkout::ProjectRoot
        );

        for bad in [
            "--model",
            "--checkout elsewhere",
            "--effort extreme",
            "--harness gemini --effort high",
            "--task write-it",
            "--request-id",
            "--backend hc",
            "--terminal",
        ] {
            assert!(
                parse_spawn_options(&args(bad), SPAWN_USAGE).is_err(),
                "{bad} was accepted"
            );
        }
    }

    #[test]
    fn a_request_id_is_a_short_plain_word_herdr_keeps_whole() {
        for good in [
            "20261002-w-spawn-request-id",
            "a",
            "v1.2_rc-3",
            &"x".repeat(80),
        ] {
            assert_eq!(valid_request_id(good.to_string()).unwrap(), good);
        }
        for bad in [
            "",
            " 20261002-w",
            "brief id",
            "id/with/slashes",
            "ünicode",
            "a\nb",
        ] {
            let refused = valid_request_id(bad.to_string()).expect_err(bad);
            assert!(refused.to_string().contains("--request-id"), "{refused}");
        }
        let long = "x".repeat(81);
        let refused = valid_request_id(long).expect_err("81 characters");
        assert!(refused.to_string().contains("at most 80"), "{refused}");
        // The parser applies it, and the supervisor does not take one, which
        // is refused before stdin is read or hc is asked anything.
        let args = ["--request-id".to_string(), "a b".to_string()];
        assert!(parse_spawn_options(&args, SPAWN_USAGE).is_err());
        let args = ["--request-id".to_string(), "20261002-corgi".to_string()];
        let refused = supervisor_command(&args).expect_err("no request id for a supervisor");
        assert!(refused.to_string().contains("for corgi spawn"), "{refused}");
    }
}
