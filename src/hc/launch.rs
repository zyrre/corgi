//! Starting Corgi's agents on hc: a project's supervisor in a kept shell
//! terminal, the agent typed into it, and workers each in a terminal of its
//! own, in their own worktree.
//!
//! The supervisor runs inside a shell so that a handover can `/exit` it and
//! start its successor in the same terminal, which keeps its address, its
//! labels and the workers hc counts as its children (`docs/herdr-app-port.md`).
//! That shell is one Corgi starts ([`launch_supervisor`]), the project's own
//! again once its agent has exited ([`relaunch_supervisor`]), or one the
//! person had open, which becomes the supervisor's ([`adopt_terminal`]).
//!
//! Every agent starts through [`agent_script`], which removes the variables
//! a Claude Code session and a Herdr pane hand their children: a core
//! started from inside one passes them to every terminal, and a Claude Code
//! that inherits them saves no transcript, which Corgi reads for reports,
//! context and cache.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

use crate::{
    activity::folder_trust_dialog,
    git::{git_output, trust_root},
    harness::{CODEX_DEVELOPER_INSTRUCTIONS, Harness},
    launch::{Checkout, LaunchPlan, Role, launch_args},
    paths::{dir_name, home, is_home, project_root_digest},
    progress::Progress,
    projects::is_created_project,
    rows::SUPERVISOR_NAME,
    supervisor::{self, Prepared},
    time::unix_now,
};

use super::{
    BASELINE_LABEL, HANDOVER_LABEL, HARNESS_LABEL, HcClient, PID_LABEL, PROJECT_LABEL,
    REQUEST_LABEL, RESUME_LABEL, ROLE_LABEL, SESSION_LABEL, SPACE_LABEL, SUPERVISOR_ROLE,
    TASK_LABEL, Terminal, WORKER_ROLE, agents::shell_in_front,
};

/// The variables Corgi removes from every agent it starts on hc: the ones a
/// Claude Code session and a Herdr pane hand to the processes they start,
/// which describe that session or pane, not the user's setup. A core
/// started from inside such a session passes them to every terminal, and a
/// Claude Code that inherits `CLAUDE_CODE_CHILD_SESSION` saves no
/// transcript, which Corgi reads for reports, context and cache. Only these
/// go: a user's own settings, such as `CLAUDE_CODE_USE_BEDROCK` or a
/// limit, still reach the agent. A name ending in `*` stands for every name
/// it starts with.
pub const SESSION_VARIABLES: [&str; 10] = [
    // Claude Code's marks on the processes of one of its sessions.
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_*",
    // The pane and session of a Herdr TUI the core was started from.
    "HERDR_*",
];

/// The script every agent starts through, run by `/bin/sh -c` with the
/// agent's folder as `$1`, a file whose text is the agent's last argument
/// (or an empty word) as `$2`, and the agent's command after them. It
/// unsets [`SESSION_VARIABLES`], puts the text of the file each
/// [`ARG_FILE_PREFIX`] argument names in its place, then runs the agent in
/// the folder in its place.
pub fn agent_script() -> String {
    let names: Vec<String> = SESSION_VARIABLES
        .iter()
        .map(|name| match name.strip_suffix('*') {
            Some(prefix) => format!("{prefix}[A-Za-z0-9_]*"),
            None => name.to_string(),
        })
        .collect();
    format!(
        r#"for v in $(env | sed -nE 's/^({})=.*/\1/p'); do unset "$v"; done; cd "$1" || exit 1; p=$2; shift 2; for a do shift; case $a in {ARG_FILE_PREFIX}*) a=$(cat "${{a#{ARG_FILE_PREFIX}}}") || exit 1;; esac; set -- "$@" "$a"; done; if [ -n "$p" ]; then exec "$@" "$(cat "$p")"; fi; exec "$@""#,
        names.join("|")
    )
}

/// An argument of an agent's command that [`agent_script`] replaces with the
/// text of the file named after it, so a long argument (Codex's role as its
/// developer instructions, some 25 KB) reaches the agent without being
/// typed into a shell.
pub const ARG_FILE_PREFIX: &str = "@corgi-arg-file:";

/// Where a Codex supervisor's developer instructions are kept for
/// [`ARG_FILE_PREFIX`], in its state directory.
pub const CODEX_ROLE_ARG_FILE: &str = "codex-developer-instructions.arg";

/// The name the script runs as (its `$0`), which `ps` shows until the agent
/// replaces it.
const SCRIPT_NAME: &str = "corgi-agent";

/// The harnesses Corgi starts on hc.
const HC_HARNESSES: [Harness; 3] = [Harness::Claude, Harness::Codex, Harness::OpenCode];

/// Where a supervisor's first prompt is written for the script to pass on,
/// in its state directory: the typed command line stays one short line.
pub const FIRST_PROMPT_FILE: &str = "launch-prompt.txt";

/// How long a new shell may take to draw its prompt before the supervisor's
/// command is typed into it anyway.
const SHELL_READY: Duration = Duration::from_secs(15);
/// How long the agent typed into the supervisor's shell may take to replace
/// it.
const AGENT_STARTED: Duration = Duration::from_secs(20);
/// How long an agent may take to show Claude Code's folder-trust question,
/// or report a status that says it asks none.
const TRUST_WAIT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(250);
/// How long a key in the folder-trust question may take to show, before it
/// is taken for lost; and how many keys Corgi presses there in all.
const TRUST_SETTLE: Duration = Duration::from_secs(2);
const TRUST_ATTEMPTS: usize = 6;

/// The worktrees Corgi makes go where Herdr makes its own:
/// `~/.herdr/worktrees/<repo>/worktree-<name>`, on branch `worktree/<name>`.
const WORKTREES_DIR: &str = ".herdr/worktrees";
const ADJECTIVES: [&str; 16] = [
    "brave", "calm", "clear", "eager", "gentle", "green", "happy", "keen", "lucky", "merry",
    "quiet", "silver", "swift", "sunny", "warm", "wise",
];
const NOUNS: [&str; 16] = [
    "brook", "cedar", "dune", "field", "forest", "harbor", "hill", "lake", "meadow", "otter",
    "pine", "river", "stone", "valley", "willow", "wren",
];

/// An agent started on hc.
#[derive(Debug, Clone)]
pub struct Launched {
    pub terminal: Terminal,
    pub name: String,
    /// The folder the agent works in.
    pub cwd: PathBuf,
    pub checkout: Checkout,
    /// Where it runs, for the caller to show.
    pub location: String,
    /// The harness's session id, when Corgi chose it.
    pub session: Option<String>,
}

/// Why `harness` cannot run on hc, if it cannot.
pub fn harness_refusal(harness: &Harness) -> Option<String> {
    (!HC_HARNESSES.contains(harness))
        .then(|| format!("on hc, Corgi starts claude, codex or opencode, not {harness}"))
}

/// Starts `plan`, a project's supervisor (`plan.project` is the project's
/// primary checkout), in a kept shell terminal named `plan.name` whose
/// commands call `corgi_bin`, and types its agent's command line into it.
pub fn launch_supervisor(
    client: &HcClient,
    plan: &LaunchPlan,
    corgi_bin: &Path,
    progress: &mut dyn Progress,
) -> Result<Launched> {
    start_supervisor(client, plan, corgi_bin, Place::New, progress)
}

/// Starts `plan`, a project's supervisor, in the kept shell terminal
/// `address` whose agent has exited (the user's `/exit`, or a handover's),
/// as [`launch_supervisor`] would in a new one: the terminal keeps its
/// address, its name, its space, and the workers that report to the
/// project. Its session labels are replaced before the agent starts. A
/// `plan` whose supervisor takes over starts with the takeover prompt,
/// which has the new session read the note its predecessor left.
pub fn relaunch_supervisor(
    client: &HcClient,
    plan: &LaunchPlan,
    corgi_bin: &Path,
    address: &str,
    progress: &mut dyn Progress,
) -> Result<Launched> {
    start_supervisor(client, plan, corgi_bin, Place::Again(address), progress)
}

/// Starts `plan`, a project's supervisor, in the shell terminal `address`
/// that Corgi did not start: one open in the Herdr app, or any other of hc's.
/// The terminal becomes the project's supervisor terminal, as one
/// [`launch_supervisor`] starts is: it gets Corgi's labels, which is how
/// `fleet`, `status`, spawn, merge and the daemon find a supervisor, and is
/// kept, so the daemon can resume it. It stays in the space it is in, which
/// from then on is the project's ([`current_project_space`]); one in no
/// space of the app gets the project's own ([`project_space`]). Refused
/// ([`adoption_refusal`]) unless a shell waits at its prompt there.
pub fn adopt_terminal(
    client: &HcClient,
    plan: &LaunchPlan,
    corgi_bin: &Path,
    address: &str,
    progress: &mut dyn Progress,
) -> Result<Launched> {
    start_supervisor(client, plan, corgi_bin, Place::Adopt(address), progress)
}

/// Where a supervisor starts.
#[derive(Debug, Clone, Copy)]
pub(super) enum Place<'a> {
    /// A new kept shell terminal.
    New,
    /// The project's own supervisor terminal, whose agent has exited.
    Again(&'a str),
    /// A shell terminal Corgi did not start.
    Adopt(&'a str),
}

/// Why the terminal cannot become a supervisor's ([`adopt_terminal`]), if
/// it cannot: only a live terminal of this machine with a shell at its
/// prompt can, so neither an agent nor a running command is ever typed
/// into, and never one Corgi already runs an agent in.
pub fn adoption_refusal(terminal: &Terminal) -> Option<String> {
    let address = &terminal.address;
    if address.contains('/') {
        return Some(format!(
            "{address} is on another machine; Corgi starts its agents on this one"
        ));
    }
    if !terminal.is_live() {
        return Some(format!("{address} has ended"));
    }
    if let Some(role) = terminal.label(ROLE_LABEL) {
        return Some(format!(
            "{address} is already Corgi's {role} terminal of {}",
            terminal.label(PROJECT_LABEL).unwrap_or("a project")
        ));
    }
    match &terminal.foreground {
        None => Some(format!(
            "hc does not say what runs in {address}, so nothing is typed into it"
        )),
        Some(_) if shell_in_front(terminal) => None,
        Some(foreground) => Some(format!(
            "{address} runs {}, not a shell waiting at its prompt",
            foreground.name
        )),
    }
}

fn start_supervisor(
    client: &HcClient,
    plan: &LaunchPlan,
    corgi_bin: &Path,
    place: Place,
    progress: &mut dyn Progress,
) -> Result<Launched> {
    let prepare = |root: &str| supervisor::prepare(root, corgi_bin);
    start_supervisor_with(client, plan, place, &prepare, progress)
}

/// [`start_supervisor`] with the supervisor's state directory made ready by
/// `prepare`, which a test points at a directory of its own.
pub(super) fn start_supervisor_with(
    client: &HcClient,
    plan: &LaunchPlan,
    place: Place,
    prepare: &dyn Fn(&str) -> Result<Prepared>,
    progress: &mut dyn Progress,
) -> Result<Launched> {
    if let Some(refusal) = harness_refusal(&plan.harness) {
        bail!(refusal);
    }
    anyhow::ensure!(
        plan.harness.supports_supervisor(),
        crate::launch::supervisor_harness_error(&plan.harness)
    );
    let existing = match place {
        Place::Again(address) => {
            let terminal = client.get(address)?;
            anyhow::ensure!(
                terminal.is_live() && shell_in_front(&terminal),
                "{address} has no shell in front for a supervisor to start in (its agent still \
                 runs, or the terminal ended)"
            );
            Some(terminal)
        }
        Place::Adopt(address) => {
            let terminal = client.get(address)?;
            if let Some(refusal) = adoption_refusal(&terminal) {
                bail!(refusal);
            }
            Some(terminal)
        }
        Place::New => None,
    };
    let root = plan.project.to_string_lossy().into_owned();
    let prepared = prepare(&root)?;
    if let Some(warning) = &prepared.warning {
        progress.report(warning.clone());
    }
    supervisor::save_launch(
        &prepared.state_dir,
        &supervisor::Launch {
            kind: plan.harness.kind().to_string(),
            model: plan.model.clone(),
            effort: plan.effort.clone(),
            extra_args: plan.extra_args.clone(),
        },
    )?;
    let prompt_file = prepared.state_dir.join(FIRST_PROMPT_FILE);
    let prompt = match &plan.role {
        Role::Supervisor { takes_over: true } => {
            let archive = supervisor::handover_archive(&prepared.state_dir, unix_now());
            supervisor::takeover_prompt(&root, &prepared.state_dir, &archive)
        }
        _ => supervisor::first_prompt(&root, &prepared.state_dir, &plan.prompt),
    };
    fs::write(&prompt_file, &prompt).with_context(|| format!("write {}", prompt_file.display()))?;
    let role_args = supervisor_role_args(&plan.harness, &prepared)?;
    let session = session_id_for(&plan.harness)?;
    let command = harness_command(plan, session.as_deref(), &role_args, Start::Fresh);
    let line = shell_line(&agent_argv(&plan.project, Some(&prompt_file), &command));
    let resume = session.as_ref().map(|session| {
        let resume = harness_command(plan, None, &role_args, Start::Resume(session));
        shell_line(&agent_argv(&plan.project, None, &resume))
    });

    let address = match &existing {
        Some(terminal) => {
            let address = terminal.address.clone();
            progress.report(format!("Taking over {} ({address})…", plan.name));
            // The new session's labels, and none of the old one's.
            let pid = terminal.pid.map(|pid| pid.to_string()).unwrap_or_default();
            let mut labels = vec![
                (HARNESS_LABEL, plan.harness.kind()),
                (SESSION_LABEL, session.as_deref().unwrap_or_default()),
                (RESUME_LABEL, resume.as_deref().unwrap_or_default()),
                (HANDOVER_LABEL, ""),
                (BASELINE_LABEL, ""),
                (PID_LABEL, &pid),
            ];
            // A terminal that becomes the supervisor's gets what one Corgi
            // starts has from its first moment. Its space stays the one it
            // is in; only one outside the app gets the project's own.
            let space = project_space(&root);
            if matches!(place, Place::Adopt(_)) {
                labels.extend([
                    (PROJECT_LABEL, root.as_str()),
                    (ROLE_LABEL, SUPERVISOR_ROLE),
                    (TASK_LABEL, SUPERVISOR_NAME),
                ]);
                if terminal.label(SPACE_LABEL).is_none() {
                    labels.push((SPACE_LABEL, &space));
                }
            }
            client.label(&address, &labels)?;
            // Kept, so that a core stop or a reboot leaves a shell in its
            // place, which the daemon gives the resume line.
            if !terminal.keep {
                client.keep(&address)?;
            }
            address
        }
        None => {
            let space = project_space(&root);
            let mut labels = agent_labels(plan, &root, SUPERVISOR_NAME, &space, session.as_deref());
            if let Some(resume) = &resume {
                labels.push((RESUME_LABEL.into(), resume.clone()));
            }
            progress.report(format!("Starting a kept shell terminal {}…", plan.name));
            let started = client.run(&super::RunSpec {
                name: plan.name.clone(),
                cwd: plan.project.clone(),
                labels,
                keep: true,
                argv: Vec::new(),
            })?;
            let address = started.address.clone();
            wait_for_shell(client, &address);
            mark_pid(client, &address);
            address
        }
    };
    progress.report(format!(
        "Starting {} in {address} as {}…",
        plan.harness, plan.name
    ));
    if let Err(error) = client.send(&address, &line) {
        // Nothing was typed: a terminal that was to become the supervisor's
        // is the person's own again, not a supervisor's with no agent.
        if matches!(place, Place::Adopt(_)) {
            disown(client, &address);
        }
        return Err(error.context(format!("type the supervisor's command into {address}")));
    }
    let terminal = wait_for_agent(client, &address)?;
    // Only Claude Code asks on screen; Codex takes its answer on the
    // command line.
    if plan.harness == Harness::Claude {
        answer_folder_trust(
            client,
            &address,
            &plan.project,
            &plan.name,
            progress,
            &created,
        );
    }
    Ok(Launched {
        terminal,
        name: plan.name.clone(),
        cwd: plan.project.clone(),
        checkout: Checkout::ProjectRoot,
        location: match place {
            Place::Adopt(_) => {
                format!("in hc terminal {address}, the shell that was open in {root}")
            }
            _ => format!("in hc terminal {address}, a kept shell in {root}"),
        },
        session,
    })
}

/// Takes Corgi's labels off `address` again, a terminal [`adopt_terminal`]
/// labelled and then could type nothing into. Its space label is the app's
/// and stays.
fn disown(client: &HcClient, address: &str) {
    let labels = [
        PROJECT_LABEL,
        ROLE_LABEL,
        TASK_LABEL,
        HARNESS_LABEL,
        SESSION_LABEL,
        RESUME_LABEL,
        PID_LABEL,
    ]
    .map(|label| (label, ""));
    let _ = client.label(address, &labels);
}

/// The arguments that make `harness`'s session the supervisor prepared in
/// `prepared`, for a command line typed into a shell: Codex's developer
/// instructions, the whole role inline, are written to
/// [`CODEX_ROLE_ARG_FILE`] and passed through [`ARG_FILE_PREFIX`].
///
/// Codex has no file form of `developer_instructions`. Its
/// `model_instructions_file` would replace Codex's own base instructions,
/// which a supervisor still needs, rather than add to them; a project's
/// `.codex/config.toml` would land in the user's repository and apply to
/// every Codex session there; and `CODEX_HOME` would replace the user's
/// whole Codex home, login included. So the instructions stay an argument,
/// and only the typed line names a file.
pub fn supervisor_role_args(harness: &Harness, prepared: &Prepared) -> Result<Vec<String>> {
    let mut args =
        harness.supervisor_args(&prepared.role_file, &prepared.role, &prepared.state_dir);
    if *harness == Harness::Codex {
        let file = prepared.state_dir.join(CODEX_ROLE_ARG_FILE);
        for arg in &mut args {
            if arg.starts_with(CODEX_DEVELOPER_INSTRUCTIONS) {
                write_private(&file, arg)?;
                *arg = format!("{ARG_FILE_PREFIX}{}", file.display());
            }
        }
    }
    Ok(args)
}

/// Writes `text` to `path`, readable by its owner alone.
fn write_private(path: &Path, text: &str) -> Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("write {}", path.display()))?;
    file.write_all(text.as_bytes())
        .with_context(|| format!("write {}", path.display()))
}

/// Labels `address` with the process id of its program as it is now
/// ([`PID_LABEL`]), so that a program the core later replaces with a shell
/// is told apart from it. A terminal hc cannot describe keeps no label.
pub fn mark_pid(client: &HcClient, address: &str) {
    if let Ok(Some(pid)) = client.find(address).map(|t| t.and_then(|t| t.pid)) {
        let _ = client.label(address, &[(PID_LABEL, &pid.to_string())]);
    }
}

/// Starts `plan`, a worker of the project whose primary checkout is `root`,
/// in a terminal of its own: in a new worktree, the project directory, or
/// the primary checkout, as `plan.checkout` says, or in the home directory
/// for a scratch agent.
pub fn launch_worker(
    client: &HcClient,
    plan: &LaunchPlan,
    root: &str,
    progress: &mut dyn Progress,
) -> Result<Launched> {
    if let Some(refusal) = harness_refusal(&plan.harness) {
        bail!(refusal);
    }
    let (cwd, checkout, location) = if is_home(&plan.project) {
        (
            plan.project.clone(),
            Checkout::Directory,
            "in the home directory (no project)".to_string(),
        )
    } else {
        match plan.checkout {
            Checkout::Worktree => {
                progress.report(format!("Creating a worktree for {}…", plan.name));
                let (path, branch) = create_worktree(Path::new(root))?;
                let repo = dir_name(root).unwrap_or(supervisor::UNNAMED_PROJECT);
                (
                    path,
                    Checkout::Worktree,
                    format!("in {repo} worktree {branch}"),
                )
            }
            Checkout::Directory => (
                plan.project.clone(),
                Checkout::Directory,
                format!("in {} (shared checkout)", plan.project.display()),
            ),
            Checkout::ProjectRoot => (
                PathBuf::from(root),
                Checkout::ProjectRoot,
                format!("in the primary checkout {root}"),
            ),
        }
    };
    let session = session_id_for(&plan.harness)?;
    let command = harness_command(plan, session.as_deref(), &[], Start::Task(&plan.prompt));
    let space = worker_space(
        &current_project_space(client, root),
        checkout,
        &space_nonce()?,
    );
    let mut labels = agent_labels(
        plan,
        root,
        &task_title(&plan.prompt),
        &space,
        session.as_deref(),
    );
    if let Some(id) = &plan.request_id {
        labels.push((REQUEST_LABEL.into(), id.clone()));
    }
    if let Some(session) = &session {
        let resume = harness_command(plan, None, &[], Start::Resume(session));
        labels.push((
            RESUME_LABEL.into(),
            shell_line(&agent_argv(&cwd, None, &resume)),
        ));
    }
    progress.report(format!("Starting {} as {}…", plan.harness, plan.name));
    // Kept, so that a core stop or a reboot leaves a shell in its place,
    // which the daemon gives the resume line.
    let terminal = client.run(&super::RunSpec {
        name: plan.name.clone(),
        cwd: cwd.clone(),
        labels,
        keep: true,
        argv: agent_argv(&cwd, None, &command),
    })?;
    mark_pid(client, &terminal.address);
    // Only Claude Code asks on screen; Codex takes its answer on the
    // command line.
    if plan.harness == Harness::Claude {
        answer_folder_trust(
            client,
            &terminal.address,
            &cwd,
            &plan.name,
            progress,
            &created,
        );
    }
    let location = format!("{location}, hc terminal {}", terminal.address);
    Ok(Launched {
        terminal,
        name: plan.name.clone(),
        cwd,
        checkout,
        location,
        session,
    })
}

/// The labels every agent Corgi starts on hc carries.
fn agent_labels(
    plan: &LaunchPlan,
    root: &str,
    task: &str,
    space: &str,
    session: Option<&str>,
) -> Vec<(String, String)> {
    let role = match plan.role {
        Role::Supervisor { .. } => SUPERVISOR_ROLE,
        Role::Worker => WORKER_ROLE,
    };
    let mut labels = vec![
        (PROJECT_LABEL.to_string(), root.to_string()),
        (ROLE_LABEL.to_string(), role.to_string()),
        (TASK_LABEL.to_string(), task.to_string()),
        (HARNESS_LABEL.to_string(), plan.harness.kind().to_string()),
        (SPACE_LABEL.to_string(), space.to_string()),
    ];
    if let Some(session) = session {
        labels.push((SESSION_LABEL.to_string(), session.to_string()));
    }
    labels
}

/// How a harness command starts its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start<'a> {
    /// A new session whose first prompt the script passes on from its file.
    Fresh,
    /// A new session with this first prompt.
    Task(&'a str),
    /// The session with this id again.
    Resume(&'a str),
}

/// The command that runs `plan`'s harness on hc: its CLI by full path (a
/// core started by launchd has a bare `PATH`), the session id Corgi chose
/// for it (`session`, Claude Code's `--session-id`), the plan's model,
/// effort and arguments, `role_args`, and the first prompt or the resumed
/// session. Codex always gets `--no-daemon`, or its commands would run
/// outside the terminal.
pub fn harness_command(
    plan: &LaunchPlan,
    session: Option<&str>,
    role_args: &[String],
    start: Start,
) -> Vec<String> {
    let harness = &plan.harness;
    let mut command = vec![harness.command().to_string_lossy().into_owned()];
    if *harness == Harness::Codex {
        command.push("--no-daemon".into());
        if let Start::Resume(id) = start {
            command.extend(["resume".into(), id.to_string()]);
        }
    }
    if *harness == Harness::Claude {
        match (start, session) {
            (Start::Resume(id), _) => command.extend(["--resume".into(), id.to_string()]),
            (_, Some(id)) => command.extend(["--session-id".into(), id.to_string()]),
            (_, None) => {}
        }
    }
    if *harness == Harness::OpenCode
        && let Start::Resume(id) = start
    {
        command.extend(["--session".into(), id.to_string()]);
    }
    command.extend(launch_args(plan));
    command.extend(role_args.iter().cloned());
    match start {
        // OpenCode takes its first message as an option; the others as
        // their one argument, after `--` so that no option before it (such
        // as Claude Code's `--add-dir`, which takes several) swallows it.
        Start::Fresh | Start::Task(_) if *harness == Harness::OpenCode => {
            command.push("--prompt".into());
        }
        Start::Fresh | Start::Task(_) => command.push("--".into()),
        Start::Resume(_) => {}
    }
    if let Start::Task(prompt) = start {
        command.push(prompt.to_string());
    }
    command
}

/// The words that give a resumed session of `harness` its next prompt,
/// appended to its resume command ([`Start::Resume`]): after `--`, as a new
/// session's first prompt is, for Claude Code (`--resume <id> -- <prompt>`)
/// and Codex (`resume <id> -- <prompt>`). `None` for OpenCode, whose
/// `--prompt` is submitted only from its home screen, not in the session
/// `--session` opens (1.18.33), and for any other harness: the prompt is
/// typed once it is in front instead.
pub fn resume_prompt_args(harness: &Harness, prompt: &str) -> Option<[String; 2]> {
    matches!(harness, Harness::Claude | Harness::Codex).then(|| ["--".into(), prompt.into()])
}

/// The argv that runs `command` through [`agent_script`] in `dir`, with the
/// text of `prompt_file` as its last argument when there is one.
pub fn agent_argv(dir: &Path, prompt_file: Option<&Path>, command: &[String]) -> Vec<String> {
    let mut argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        agent_script(),
        SCRIPT_NAME.to_string(),
        dir.to_string_lossy().into_owned(),
        prompt_file
            .map(|file| file.to_string_lossy().into_owned())
            .unwrap_or_default(),
    ];
    argv.extend(command.iter().cloned());
    argv
}

/// `argv` as one command line for a shell that quotes as POSIX `sh` does
/// (zsh, bash, fish and nu read single quotes alike).
pub fn shell_line(argv: &[String]) -> String {
    argv.iter()
        .map(|word| shell_quote(word))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// A first line of `prompt` short enough to be a task title: its first
/// non-empty line, without Markdown heading marks, at most 80 characters.
pub fn task_title(prompt: &str) -> String {
    let line = prompt
        .lines()
        .map(|line| line.trim().trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let mut title: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        title.pop();
        title.push('…');
    }
    title
}

/// The Herdr app's space for a worker that works in `checkout`, whoever
/// spawned it, of the project whose space is `project`
/// ([`current_project_space`]). A worker in a worktree gets a new space of
/// its own: the project's with `nonce` after it. The app makes a space for
/// an id it does not know, unnamed, so it is called after where the worker's
/// terminal is, and its sidebar nests a space whose first terminal is in a
/// linked worktree under the space of the repository's main checkout, the
/// supervisor's, by `git rev-parse --git-common-dir`, whoever made either
/// space and whatever they are called. A worker in the primary checkout or
/// another directory is a tab of the project's space: a space of its own
/// there would not nest.
pub fn worker_space(project: &str, checkout: Checkout, nonce: &str) -> String {
    match checkout {
        Checkout::Worktree => format!("{project}-{nonce}"),
        Checkout::ProjectRoot | Checkout::Directory => project.to_string(),
    }
}

/// The Herdr app's space of the project at `root` as it is now: the space
/// its supervisor's terminal is in, which is one the person had open when
/// the supervisor was started in a shell of theirs ([`adopt_terminal`]),
/// else the one Corgi gives a project ([`project_space`]), where a
/// supervisor started later will be.
pub fn current_project_space(client: &HcClient, root: &str) -> String {
    client
        .list(&[(ROLE_LABEL, SUPERVISOR_ROLE), (PROJECT_LABEL, root)])
        .ok()
        .and_then(|terminals| {
            terminals
                .iter()
                // hc filtered by the labels; a fake of it in a test does not.
                .filter(|terminal| {
                    terminal.label(ROLE_LABEL) == Some(SUPERVISOR_ROLE)
                        && terminal.label(PROJECT_LABEL) == Some(root)
                })
                .find_map(|terminal| terminal.label(SPACE_LABEL).map(str::to_string))
        })
        .unwrap_or_else(|| project_space(root))
}

/// Eight random hexadecimal digits for [`worker_space`].
fn space_nonce() -> Result<String> {
    let mut bytes = [0u8; 4];
    fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .context("read /dev/urandom for a space id")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The Herdr app's space Corgi gives the project at `root`, for a
/// supervisor it starts in a terminal of its own: `corgi-` and its directory
/// name, with a few characters of a hash of the whole path so that two
/// projects of the same name get two spaces. The supervisor's terminal is on
/// the primary checkout, so the app calls the space after the repository;
/// Corgi never names it.
pub fn project_space(root: &str) -> String {
    let name =
        crate::launch::sanitize_agent_name(dir_name(root).unwrap_or(supervisor::UNNAMED_PROJECT));
    let digest = project_root_digest(root);
    let digest = digest.trim_start_matches("fnv1a64:");
    format!("corgi-{name}-{}", &digest[..6])
}

/// A session id for a harness that lets Corgi choose it (Claude Code), so
/// it can be labelled before the agent starts and resumed by it.
fn session_id_for(harness: &Harness) -> Result<Option<String>> {
    match harness {
        Harness::Claude => new_session_id().map(Some),
        _ => Ok(None),
    }
}

/// A random version 4 UUID.
pub fn new_session_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .context("read /dev/urandom for a session id")?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// Creates a worktree of the repository whose primary checkout is `root`,
/// from its `HEAD`: returns its path and branch.
pub fn create_worktree(root: &Path) -> Result<(PathBuf, String)> {
    let home = home().context("HOME is not set, so there is no ~/.herdr/worktrees")?;
    create_worktree_in(&home.join(WORKTREES_DIR), root)
}

/// [`create_worktree`] under `base` in place of `~/.herdr/worktrees`.
pub fn create_worktree_in(base: &Path, root: &Path) -> Result<(PathBuf, String)> {
    let repo = dir_name(root).unwrap_or(supervisor::UNNAMED_PROJECT);
    let dir = base.join(repo);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    for _ in 0..16 {
        let slug = worktree_slug()?;
        let path = dir.join(format!("worktree-{slug}"));
        let branch = format!("worktree/{slug}");
        let taken = git_output(
            root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok();
        if path.exists() || taken {
            continue;
        }
        git_output(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &branch,
                &path.to_string_lossy(),
                "HEAD",
            ],
        )
        .with_context(|| format!("create a worktree of {}", root.display()))?;
        return Ok((path, branch));
    }
    bail!("found no free worktree name under {}", dir.display())
}

/// A name like Herdr gives its worktrees: `calm-otter-1f2e`.
fn worktree_slug() -> Result<String> {
    let mut bytes = [0u8; 4];
    fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .context("read /dev/urandom for a worktree name")?;
    Ok(format!(
        "{}-{}-{:02x}{:02x}",
        ADJECTIVES[usize::from(bytes[0]) % ADJECTIVES.len()],
        NOUNS[usize::from(bytes[1]) % NOUNS.len()],
        bytes[2],
        bytes[3]
    ))
}

/// Waits, up to [`SHELL_READY`], for the new shell in `address` to draw its
/// prompt, so that the command typed next is not read before the shell has
/// set up its line editor. Gives up silently: the shell then reads the
/// command when it gets to it.
fn wait_for_shell(client: &HcClient, address: &str) {
    let deadline = Instant::now() + SHELL_READY;
    while Instant::now() < deadline {
        if client
            .read(address, 0)
            .is_ok_and(|screen| !screen.trim().is_empty())
        {
            // A prompt drawn in pieces (a theme's second line) settles.
            thread::sleep(Duration::from_millis(500));
            return;
        }
        thread::sleep(POLL);
    }
}

/// Waits, up to [`AGENT_STARTED`], for the agent typed into `address` to
/// replace its shell, and returns the terminal then.
fn wait_for_agent(client: &HcClient, address: &str) -> Result<Terminal> {
    let deadline = Instant::now() + AGENT_STARTED;
    loop {
        let terminal = client.get(address)?;
        anyhow::ensure!(
            terminal.is_live(),
            "{address} ended before its agent started"
        );
        let started = terminal.foreground.is_some() && !shell_in_front(&terminal);
        if started || terminal.status.is_some() {
            return Ok(terminal);
        }
        if Instant::now() >= deadline {
            bail!(
                "the agent typed into {address} did not start within {}s; see hc read {address}",
                AGENT_STARTED.as_secs()
            );
        }
        thread::sleep(POLL);
    }
}

/// Answers Claude Code's folder-trust question in `address` with Yes, and
/// does nothing else: only when exactly that
/// question is on screen, about `cwd`, in a project Corgi created. Waits up
/// to [`TRUST_WAIT`] for the question, and stops at once when the agent
/// reports a status (it asks nothing then). A question left for the user
/// is said so. `created` tells whether Corgi created the project a folder
/// asked about is in.
pub(super) fn answer_folder_trust(
    client: &HcClient,
    address: &str,
    cwd: &Path,
    name: &str,
    progress: &mut dyn Progress,
    created: &dyn Fn(&Path) -> bool,
) {
    let dialog = || {
        client
            .read(address, 0)
            .ok()
            .and_then(|screen| folder_trust_dialog(&screen))
    };
    let deadline = Instant::now() + TRUST_WAIT;
    let question = loop {
        if let Some(question) = dialog() {
            break question;
        }
        let reported = client
            .get(address)
            .map(|terminal| terminal.status.is_some() || !terminal.is_live())
            .unwrap_or(true);
        if reported || Instant::now() >= deadline {
            return;
        }
        thread::sleep(POLL);
    };
    let asked = PathBuf::from(&question.path);
    if !same_directory(cwd, &asked) || !created(&asked) {
        progress.report(format!(
            "{name} asks whether to trust {}; answer it in the Herdr app or with hc attach {address}",
            asked.display()
        ));
        return;
    }
    progress.report(format!(
        "Trusting {} for {name}: Corgi created this project…",
        asked.display()
    ));
    // A key pressed while the dialog is first drawn can be lost, so Enter
    // is pressed only once Yes shows highlighted, and a key that changed
    // nothing on screen after a while is pressed again.
    let mut current = question;
    for _ in 0..TRUST_ATTEMPTS {
        let key = if current.yes_selected {
            "enter"
        } else {
            "down"
        };
        if client.keys(address, &[key]).is_err() {
            return;
        }
        let deadline = Instant::now() + TRUST_SETTLE;
        loop {
            thread::sleep(POLL);
            match dialog() {
                // Answered: the question is gone.
                None => return,
                // Only ever the same folder's question is answered.
                Some(now) if now.path != current.path => return,
                Some(now) if now.yes_selected != current.yes_selected => {
                    current = now;
                    break;
                }
                Some(now) if Instant::now() >= deadline => {
                    current = now;
                    break;
                }
                Some(_) => {}
            }
        }
    }
    progress.report(format!(
        "{name} still asks whether to trust {}; answer it in the Herdr app or with hc attach {address}",
        asked.display()
    ));
}

/// Whether Corgi created the project `folder` is in.
fn created(folder: &Path) -> bool {
    is_created_project(&trust_root(folder))
}

fn same_directory(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}
