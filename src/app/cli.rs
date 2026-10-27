//! The command-line tools: `corgi spawn`, `corgi supervisor`, `corgi fleet`,
//! `corgi digest`, `corgi report`, `corgi notify`, `corgi inbox`,
//! `corgi tag` and `corgi role`.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::{
    digest,
    git::{git_current_branch, git_output, trust_root},
    harness::Harness,
    herdr::{HerdrClient, METADATA_VALUE_MAX_CHARS, ReadSource},
    inbox::{self, Inbox, Item, Kind},
    model::{PromptCache, PromptCacheKind, cache_countdown, compact_tokens},
    paths::{corgi_state_dir, expand_home, is_home},
    session::SessionReader,
    supervisor::{OLD_WAKE_PREFIX, WAKE_PREFIX},
    time::unix_now,
};

use super::{
    App,
    catalog::{EFFORT_LEVELS, default_harness},
    form::Checkout,
    launch::{
        CORGI_REQUEST_TOKEN, LaunchPlan, Role, SUPERVISOR_HOME_REFUSAL, calling_supervisor_harness,
        launch_agent, sanitize_agent_name, supervisor_harness_error, supervisor_plan,
        unique_agent_name,
    },
    markers,
    progress::Stderr,
    project_main::{CORGI_PROJECT_MAIN_TAB_TOKEN, project_root_of},
};

pub const SPAWN_USAGE: &str = "\
Usage: corgi spawn [OPTIONS] [-- AGENT_ARGS...] < task.md

Starts one agent the way the dashboard's new-agent form does and prints it as
JSON. The task is read from stdin, or from --task-file, never from arguments.

Options:
  --project PATH     Project directory (default: the current directory)
  --harness KIND     Agent CLI, as Herdr names it (default: the calling
                     supervisor's own, else as in the form)
  --model MODEL      Model to start on (default: the harness's own setting)
  --effort LEVEL     low, medium, high, xhigh, or max (Codex and Claude Code)
  --checkout MODE    worktree (default), directory, or root: the project
                     workspace's own root tab in the primary checkout. A
                     worktree is refused, before anything starts, for a
                     project outside a Git work tree (pass --project, or
                     --checkout directory to run there without one), and
                     every mode for a directory in Corgi's state directory,
                     such as a supervisor's; the home directory still starts
                     a scratch agent
  --name NAME        Agent name (default: taken from the project)
  --task-file PATH   Read the task from PATH instead of stdin
  --request-id ID    Make a retry safe: when a running agent of the project
                     was spawned with ID, print it (with \"existing\": true)
                     and start nothing. ID is 1 to 80 ASCII letters, digits,
                     '.', '_' or '-', such as a brief id; a retry made while
                     the first spawn still runs waits for it

Arguments after -- go to the agent CLI unchanged, after the model and effort.";

/// The characters a `--request-id` may have besides ASCII letters and
/// digits. The id is stored verbatim as a Herdr metadata token value.
const REQUEST_ID_PUNCTUATION: [char; 3] = ['.', '_', '-'];

/// What `corgi spawn` was asked for, before anything is resolved against
/// Herdr or the file system.
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
}

fn parse_spawn_options(args: &[String]) -> Result<SpawnOptions> {
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
            "--" => {
                options.agent_args = args.by_ref().cloned().collect();
                break;
            }
            other => bail!("unknown option {other:?}\n\n{SPAWN_USAGE}"),
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

/// `id` if it can be a `--request-id`: Herdr keeps it whole as a token value
/// and it needs no quoting anywhere.
fn valid_request_id(id: String) -> Result<String> {
    anyhow::ensure!(!id.is_empty(), "--request-id must not be empty");
    anyhow::ensure!(
        id.chars().count() <= METADATA_VALUE_MAX_CHARS,
        "--request-id must be at most {METADATA_VALUE_MAX_CHARS} characters"
    );
    anyhow::ensure!(
        id.chars()
            .all(|c| c.is_ascii_alphanumeric() || REQUEST_ID_PUNCTUATION.contains(&c)),
        "--request-id may only have ASCII letters, digits, '.', '_' and '-', not {id:?}"
    );
    Ok(id)
}

/// `corgi spawn`: starts one agent through the same launch as the new-agent
/// form, for a caller without a dashboard, such as a supervisor agent. It runs
/// inside Herdr and uses the injected socket. Progress goes to stderr and the
/// started agent to stdout as one JSON object.
pub fn spawn(args: &[String]) -> Result<()> {
    let options = parse_spawn_options(args)?;
    let project = existing_project_dir(options.project.as_deref())?;
    // Refused before stdin is read, Herdr is asked anything, or a retry's
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

    let client = HerdrClient::from_env()?;
    // With a request id, the project's spawn lock is held until this spawn
    // has started its worker, so that a retry waits for it and then finds
    // that worker, before any checkout is made.
    let request = match options.request_id.as_deref() {
        Some(id) => {
            let root = project_root_of(&client, &project)?;
            let lock = markers::spawn_lock(&client, &root, || {
                eprintln!("Waiting for another spawn in {root} to finish…");
            })?;
            Some((id, root, lock))
        }
        None => None,
    };
    let app = refreshed_app(client.clone())?;
    if let Some((id, root, _lock)) = &request
        && let Some(existing) = requested_agent(&app, root, id)
    {
        eprintln!("An agent spawned with request id {id} is already running");
        return print_launch(&existing);
    }
    let pane_id = env::var("HERDR_PANE_ID").ok();
    if let Some(warning) = calling_supervisor_state_conflict(&app, pane_id.as_deref()) {
        eprintln!("warning: {warning}");
    }
    let supervisor_harness = calling_supervisor_harness(&app, pane_id.as_deref());
    let harness = spawn_harness(options.harness, supervisor_harness, default_harness);
    anyhow::ensure!(
        options.effort.is_empty() || harness.supports_effort(),
        "{harness} has no effort control; only codex and claude take --effort"
    );
    let name = match options.name.as_deref() {
        Some(requested) => free_agent_name(&client, requested)?,
        None => unique_agent_name(&app.agents, &project),
    };
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
        request_id: options.request_id.clone(),
    };
    print_launch(&run_launch(client, app, plan)?)
}

/// Why `corgi spawn` must not start a worker in `project` with `checkout`,
/// telling the caller how to fix the command, or `None` when it may. A worker
/// never belongs in Corgi's state directory `state_dir` (a supervisor that
/// ran spawn from its own would get one there), and a worktree needs a Git
/// work tree: the new-agent form falls back to a plain workspace outside Git,
/// spawn does not. The home directory is the scratch agent's and allowed.
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

/// The running agent of the project at `root` that a spawn with request id
/// `id` started, as the JSON a fresh spawn prints, marked `"existing"`.
pub(super) fn requested_agent(app: &App, root: &str, id: &str) -> Option<serde_json::Value> {
    let agent = app.agents.iter().find(|agent| {
        agent.project_root == root
            && agent
                .info
                .tokens
                .get(CORGI_REQUEST_TOKEN)
                .map(String::as_str)
                == Some(id)
    })?;
    let info = &agent.info;
    let root_tab = app.workspaces.iter().any(|workspace| {
        workspace.workspace_id == info.workspace_id
            && workspace.tokens.get(CORGI_PROJECT_MAIN_TAB_TOKEN) == Some(&info.tab_id)
    });
    let checkout = if agent.worktree_checkout.is_some() {
        Checkout::Worktree
    } else if root_tab {
        Checkout::ProjectRoot
    } else {
        Checkout::Directory
    };
    Some(serde_json::json!({
        "name": info.name.as_deref().unwrap_or_else(|| info.display_name()),
        "pane_id": info.pane_id,
        "workspace_id": info.workspace_id,
        "tab_id": info.tab_id,
        "cwd": info.cwd,
        "kind": info.kind(),
        "model": agent.model.as_deref().unwrap_or_default(),
        "effort": agent.effort.as_deref().unwrap_or_default(),
        "checkout": checkout.value(),
        "supervisor": agent.supervisor,
        "location": format!(
            "already running in {} ({})",
            info.cwd(),
            info.state.label().to_lowercase()
        ),
        "existing": true,
    }))
}

/// The harness `corgi spawn` starts: the one asked for, else the calling
/// supervisor's own, else the default the new-agent form would preset.
fn spawn_harness(
    requested: Option<Harness>,
    supervisor: Option<Harness>,
    default: impl FnOnce() -> Harness,
) -> Harness {
    requested.or(supervisor).unwrap_or_else(default)
}

/// The warning about pre-rename state beside the state directory of the
/// supervisor in `pane_id`, when a supervisor is the one spawning, so that it sees
/// it and can tell the user.
fn calling_supervisor_state_conflict(app: &App, pane_id: Option<&str>) -> Option<String> {
    let pane_id = pane_id?;
    app.agents
        .iter()
        .find(|agent| agent.supervisor && agent.info.pane_id == pane_id)
        .and_then(|agent| crate::supervisor::state_dir_conflict(&agent.project_root))
}

pub const SUPERVISOR_USAGE: &str = "\
Usage: corgi supervisor [PROJECT] [OPTIONS] [-- AGENT_ARGS...] [< task.md]

Starts the supervisor of PROJECT (default: the project containing the current
directory), the agent that directs its workers, in the root tab of its Corgi
workspace, and prints it as JSON. A task on stdin or in --task-file becomes
its first request; without one the supervisor greets you with the state of
the project.

Options:
  --harness KIND     claude (default) or codex
  --model MODEL      Model to start on (default: the harness's own setting)
  --effort LEVEL     low, medium, high, xhigh, or max
  --name NAME        Agent name (default: supervisor-<project>)
  --task-file PATH   Read the first request from PATH

Arguments after -- go to the agent CLI unchanged, after the model and effort.";

/// `corgi supervisor`: launches a project's supervisor, the way the new-agent
/// form does for a project with no agent sessions.
pub fn supervisor_command(args: &[String]) -> Result<()> {
    let (positional, args) = match args.first() {
        Some(first) if !first.starts_with('-') => (Some(first.as_str()), &args[1..]),
        _ => (None, args),
    };
    let options = parse_spawn_options(args)?;
    anyhow::ensure!(
        options.request_id.is_none(),
        "--request-id is for corgi spawn; a project has one supervisor anyway"
    );
    let harness = options
        .harness
        .unwrap_or_else(|| Harness::SUPERVISOR_HARNESSES[0].clone());
    anyhow::ensure!(
        harness.supports_supervisor(),
        supervisor_harness_error(&harness)
    );
    let project = existing_project_dir(positional.or(options.project.as_deref()))?;
    anyhow::ensure!(!is_home(&project), SUPERVISOR_HOME_REFUSAL);
    let task = read_task(options.task_file.as_deref(), false)?;

    let (client, app) = connected_app()?;
    let root = project_root_of(&client, &project)?;
    let mut plan = supervisor_plan(&app, &root, harness, options.model, options.effort, task)?;
    if let Some(requested) = options.name.as_deref() {
        plan.name = free_agent_name(&client, requested)?;
    }
    plan.extra_args = options.agent_args;
    print_launch(&run_launch(client, app, plan)?)
}

pub const FLEET_USAGE: &str = "\
Usage: corgi fleet [PROJECT]

Lists the agents of PROJECT (default: the project containing the current
directory), one tab-separated row each under a header line:
NAME, ROLE (supervisor or worker), STATE, TASK, MODEL, CTX, CACHE, CWD and TAG.
CACHE is the agent's prompt cache: warm 42m while it holds, with the time left;
cold 180k once it has lapsed, with the context tokens the next prompt re-reads
at the full input price; warm? and cold? when the lifetime is only estimated
(Codex), so the cache may outlive it; - when unknown. TAG is merge for an agent
a supervisor tagged ready to merge (corgi tag) that still rests where it was
tagged, else -. When the project's supervisor has missed inbox items, a line on
stderr says so.";

/// `corgi fleet`: one tab-separated row per agent of a project (default: the
/// project containing the current directory), for a supervisor to read.
pub fn fleet(args: &[String]) -> Result<()> {
    anyhow::ensure!(args.len() <= 1, "Usage: corgi fleet [PROJECT]");
    let project = existing_project_dir(args.first().map(String::as_str))?;
    let (client, app) = connected_app()?;
    let root = project_root_of(&client, &project)?;
    println!("NAME\tROLE\tSTATE\tTASK\tMODEL\tCTX\tCACHE\tCWD\tTAG");
    for row in fleet_rows(&app, &root) {
        println!("{row}");
    }
    print_inbox_footer(&root);
    Ok(())
}

/// The rows `corgi fleet` prints for the agents of the project at `root`.
pub(super) fn fleet_rows(app: &App, root: &str) -> Vec<String> {
    let field = |text: &str| text.replace(['\t', '\n'], " ");
    let now = unix_now();
    app.agents
        .iter()
        .filter(|agent| agent.project_root == root)
        .map(|agent| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                field(agent.info.display_name()),
                if agent.supervisor {
                    "supervisor"
                } else {
                    "worker"
                },
                agent.info.state.label().to_lowercase(),
                field(&agent.task),
                field(agent.model.as_deref().unwrap_or("-")),
                agent
                    .context_percent
                    .map_or_else(|| "-".to_string(), |percent| format!("{percent}%")),
                agent
                    .cache
                    .map_or_else(|| "-".to_string(), |cache| fleet_cache(&cache, now)),
                field(agent.info.cwd()),
                if agent.ready_to_merge() { "merge" } else { "-" },
            )
        })
        .collect()
}

/// The CACHE column of `corgi fleet`: the status bar's cache field in plain
/// words for an agent to read. A `?` marks an estimated lifetime, which the
/// cache may outlive, so neither warm nor cold is certain.
fn fleet_cache(cache: &PromptCache, now: u64) -> String {
    let mark = if cache.kind == PromptCacheKind::Estimated {
        "?"
    } else {
        ""
    };
    match cache.remaining(now) {
        0 => format!("cold{mark} {}", compact_tokens(cache.tokens)),
        remaining => format!("warm{mark} {}", cache_countdown(remaining)),
    }
}

pub const TAG_USAGE: &str = "\
Usage: corgi tag NAME merge [--project PATH]
       corgi tag NAME --clear [--project PATH]

Tags the agent NAME (a Herdr agent name or pane ID) of PROJECT (default: the
project containing the current directory) ready for the user to merge, or
clears its tag. The dashboard then shows the agent, while it rests, with a
MERGE badge instead of DONE. A tag is refused for an agent that is not done
or idle, or whose checkout has no commits ahead of the branch checked out in
the project's primary checkout. The tag goes away by itself when the user
merges the branch with m, or once the agent works again.

Options:
  --project PATH  Project directory (default: the current directory)
  --clear         Remove the tag";

/// What `corgi tag` was asked to do.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct TagOptions {
    pub(super) name: String,
    /// Tag ready to merge, or else clear the tag.
    pub(super) merge: bool,
    pub(super) project: Option<String>,
}

fn parse_tag_options(args: &[String]) -> Result<TagOptions> {
    let mut name = None;
    let mut tag = None;
    let mut clear = false;
    let mut project = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--project" => project = Some(args.next().context("--project needs a value")?.clone()),
            "--clear" => clear = true,
            other if other.starts_with('-') => bail!("unknown option {other:?}\n\n{TAG_USAGE}"),
            _ if name.is_none() => name = Some(arg.clone()),
            _ if tag.is_none() => tag = Some(arg.clone()),
            _ => bail!("{TAG_USAGE}"),
        }
    }
    let name = name.with_context(|| format!("name the agent to tag\n\n{TAG_USAGE}"))?;
    match (tag.as_deref(), clear) {
        (Some("merge"), false) => {}
        (None, true) => {}
        (Some(tag), false) => bail!("merge is the only tag, not {tag:?}\n\n{TAG_USAGE}"),
        _ => bail!("give the merge tag or --clear, not both or neither\n\n{TAG_USAGE}"),
    }
    Ok(TagOptions {
        name,
        merge: !clear,
        project,
    })
}

/// `corgi tag`: a supervisor tags one of its project's agents ready for the user
/// to merge, or clears the tag, for the dashboard to show.
pub fn tag_command(args: &[String]) -> Result<()> {
    let options = parse_tag_options(args)?;
    let project = existing_project_dir(options.project.as_deref())?;
    let (client, app) = connected_app()?;
    let root = project_root_of(&client, &project)?;
    println!("{}", tag_agent(&client, &app, &root, &options)?);
    Ok(())
}

/// Tags or untags the agent `options` names among the agents of the project
/// at `root`, and says what was done.
pub(super) fn tag_agent(
    client: &HerdrClient,
    app: &App,
    root: &str,
    options: &TagOptions,
) -> Result<String> {
    let name = options.name.as_str();
    let agent = app
        .agents
        .iter()
        .find(|agent| {
            agent.project_root == root
                && (agent.info.name.as_deref() == Some(name) || agent.info.pane_id == name)
        })
        .with_context(|| format!("no agent named {name} in {root}"))?;
    let info = &agent.info;
    let agent_name = info.name.as_deref().unwrap_or(name);
    let session = markers::session_of(info);
    if !options.merge {
        let had = info.tokens.contains_key(crate::supervisor::MERGE_TOKEN);
        markers::tag_pane(client, &info.pane_id, agent_name, session, None)?;
        return Ok(if had {
            format!("Cleared the merge tag of {agent_name}")
        } else {
            format!("{agent_name} had no merge tag")
        });
    }
    if let Some(questions) = open_questions(root, agent_name) {
        if questions.is_empty() {
            bail!(
                "{agent_name} has open questions (its ledger status is needs-answer); answer them first"
            );
        }
        bail!(
            "{agent_name} has open questions: {}; answer them first",
            questions.join("; ")
        );
    }
    let state = info.state;
    anyhow::ensure!(
        matches!(
            state,
            crate::model::AgentState::Done | crate::model::AgentState::Idle
        ),
        "{agent_name} is {}; tag it once it is done or idle",
        state.label().to_lowercase()
    );
    let checkout = agent
        .worktree_checkout
        .as_deref()
        .unwrap_or_else(|| info.cwd());
    let base = git_current_branch(std::path::Path::new(root))
        .with_context(|| format!("read the branch checked out in {root}"))?;
    let ahead = commits_ahead(std::path::Path::new(checkout), &base)?;
    anyhow::ensure!(
        ahead > 0,
        "{agent_name} has no commits ahead of {base} in {checkout}; there is nothing to merge"
    );
    let value = crate::supervisor::merge_tag_value(state, info.state_change_seq);
    markers::tag_pane(client, &info.pane_id, agent_name, session, Some(&value))?;
    Ok(format!(
        "Tagged {agent_name} ready to merge: {ahead} commit{} ahead of {base}",
        if ahead == 1 { "" } else { "s" }
    ))
}

/// The questions the project supervisor's ledger still has open about the agent
/// `agent` (see [`digest::open_questions`]). A project without a ledger, or
/// one that cannot be read, has none.
fn open_questions(root: &str, agent: &str) -> Option<Vec<String>> {
    let state_dir = crate::supervisor::state_dir(root).ok()?;
    let ledger = fs::read_to_string(state_dir.join("ledger.jsonl")).ok()?;
    digest::open_questions(&ledger, agent)
}

/// How many commits `checkout`'s HEAD has that the branch `base` has not.
fn commits_ahead(checkout: &std::path::Path, base: &str) -> Result<usize> {
    let range = format!("{base}..HEAD");
    git_output(checkout, &["rev-list", "--count", &range])?
        .trim()
        .parse()
        .with_context(|| {
            format!(
                "count the commits of {} ahead of {base}",
                checkout.display()
            )
        })
}

pub const DIGEST_USAGE: &str = "\
Usage: corgi digest [PROJECT] [--decision WORDS | --search WORDS | --show FILE[:LINE]
                               | --ledger ID | --brief ID]

Prints a bounded digest of the memory of PROJECT's supervisor (default: the
project containing the current directory), for the start of a supervisor's
session: the base branch, the handover note if there is one (else the open
threads of the newest archived one), open ledger work joined with the
running agents (about 4 KB; `reported` work whose agent is gone and whose
newest line is 2+ days old is counted on one line), recently finished work,
the newest decisions in full (about 8 KB, at least 3) and the titles of
older ones (about 3 KB, and a line counting the rest). Decisions named by a later entry's `Supersedes:` line are left
out, and unreadable ledger lines are warned about. Reads only; it never
changes the state directory. When the supervisor has missed inbox items, a
line on stderr says so.

Options:
  --decision WORDS   Print in full every decision, superseded or not, whose
                     heading contains all of WORDS (ignoring case); when no
                     heading does, those whose text has them all; when none
                     does, search all of the memory as --search does
  --search WORDS     Search all of the memory: decisions (superseded ones
                     marked), the ledger and ledger-archive.jsonl, briefs,
                     kept reports (reports/), notes (notes/), handover.md
                     and archived handover notes. A word matches the start of
                     a word, ignoring case. Hits with WORDS as a phrase come
                     first, then those with the most of them, then the
                     newest; near-duplicates fold into one, which names the
                     others. Each hit is a file:line, its date and the lines
                     around it, about 7 KB at most
  --show FILE[:LINE] Print whole the block of the state file FILE (relative
                     to the state directory, as --search names it) that
                     holds LINE: the decision entry, the ledger line with its
                     id's history, the brief, the report, or the section of
                     any other Markdown file under its nearest heading.
                     Without LINE, the whole file
  --ledger ID        Print the newest ledger line of the id or agent name
                     ID, its history (ledger-archive.jsonl first) and its
                     brief and kept reports by name
  --brief ID         Print a brief whole, by its id (YYYYMMDD-<worker>) or
                     its worker's name (the newest one)";

/// What `corgi digest` prints: the digest, or one lookup in the memory.
enum DigestLookup {
    Decision(String),
    Search(String),
    Show(String),
    Ledger(String),
    Brief(String),
}

/// `corgi digest`: a bounded view of the state of a project's supervisor, joined
/// with its running agents, for a supervisor to read at the start of its session.
pub fn digest(args: &[String]) -> Result<()> {
    let mut project = None;
    let mut lookup = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let make: fn(String) -> DigestLookup = match arg.as_str() {
            "--decision" => DigestLookup::Decision,
            "--search" => DigestLookup::Search,
            "--show" => DigestLookup::Show,
            "--ledger" => DigestLookup::Ledger,
            "--brief" => DigestLookup::Brief,
            other if other.starts_with('-') => bail!("unknown option {other:?}\n\n{DIGEST_USAGE}"),
            _ if project.is_none() => {
                project = Some(arg.as_str());
                continue;
            }
            _ => bail!("{DIGEST_USAGE}"),
        };
        let value = args
            .next()
            .filter(|value| !value.trim().is_empty())
            .with_context(|| format!("{arg} needs a value"))?;
        anyhow::ensure!(
            lookup.is_none(),
            "use one of --decision, --search, --show, --ledger and --brief at a time"
        );
        lookup = Some(make(value.clone()));
    }
    let project = existing_project_dir(project)?;
    let (client, app) = connected_app()?;
    let root = project_root_of(&client, &project)?;
    let state_dir = crate::supervisor::state_dir(&root)?;
    if let Some(lookup) = lookup {
        print!("{}", digest_lookup(&state_dir, &root, lookup)?);
        return Ok(());
    }

    let read = |name: &str| fs::read_to_string(state_dir.join(name)).ok();
    let decisions = read("decisions.md").unwrap_or_default();
    let state = state_dir.to_string_lossy();
    let handovers = markdown_files(&state_dir.join(crate::supervisor::HANDOVERS_DIR));
    let root_path = std::path::Path::new(&root);
    let branch = git_current_branch(root_path).unwrap_or_else(|_| "(detached)".to_string());
    let head = git_output(root_path, &["rev-parse", "--short", "HEAD"])
        .map(|head| head.trim().to_string())
        .unwrap_or_else(|_| "(no commit)".to_string());
    let fleet: Vec<_> = app
        .agents
        .iter()
        .filter(|agent| agent.project_root == root)
        .map(|agent| digest::LiveAgent {
            name: agent.info.display_name().to_string(),
            state: agent.info.state.label().to_lowercase(),
            context_percent: agent.context_percent,
        })
        .collect();
    let corgi_bin = corgi_bin_text();
    let decision_command = format!("{corgi_bin} digest {root} --decision \"<words>\"");
    let search_command = format!("{corgi_bin} digest {root} --search \"<words>\"");
    let handover = read(crate::supervisor::HANDOVER_NOTE);
    // Names are UTC stamps, so the last sorts newest.
    let previous_handover = handovers
        .last()
        .map(|(name, text)| (name.as_str(), text.as_str()));
    let ledger = read(digest::lookup::LEDGER_FILE).unwrap_or_default();
    print!(
        "{}",
        digest::render(&digest::DigestInput {
            project: &root,
            branch: &branch,
            head: &head,
            state_dir: &state,
            handover: handover.as_deref(),
            previous_handover,
            decisions: &decisions,
            ledger: &ledger,
            fleet: &fleet,
            now: crate::time::unix_now(),
            decision_command: &decision_command,
            search_command: &search_command,
        })
    );
    print_inbox_footer(&root);
    Ok(())
}

/// What `lookup` prints from the state directory `state_dir` of the project
/// at `root`. Reads only.
fn digest_lookup(state_dir: &Path, root: &str, lookup: DigestLookup) -> Result<String> {
    let read = |name: &str| fs::read_to_string(state_dir.join(name)).ok();
    let ledger = || read(digest::lookup::LEDGER_FILE).unwrap_or_default();
    let archive = || read(digest::lookup::LEDGER_ARCHIVE_FILE).unwrap_or_default();
    let names = |files: Vec<(String, String)>| -> Vec<String> {
        files.into_iter().map(|(name, _)| name).collect()
    };
    match lookup {
        DigestLookup::Decision(words) => {
            let text = read("decisions.md").unwrap_or_default();
            let decisions = digest::parse_decisions(&text);
            let found = digest::matching_decisions(&decisions, &words);
            if !found.is_empty() {
                return Ok(format!("{}\n", found.join("\n\n")));
            }
            let found = digest::decisions_containing(&decisions, &words);
            if !found.is_empty() {
                let count = found.len();
                return Ok(format!(
                    "No decision heading contains {words:?}; {count} decision{} {} it in {}text:\n\n{}\n",
                    if count == 1 { "" } else { "s" },
                    if count == 1 { "has" } else { "have" },
                    if count == 1 { "its " } else { "their " },
                    found.join("\n\n")
                ));
            }
            Ok(format!(
                "No decision contains {words:?}; searching all of your memory instead.\n\n{}",
                digest_search(state_dir, root, &words)
            ))
        }
        DigestLookup::Search(words) => Ok(digest_search(state_dir, root, &words)),
        DigestLookup::Show(handle) => {
            let state = state_dir.to_string_lossy();
            let (path, line) = digest::lookup::parse_handle(&state, &handle)?;
            let text = fs::read_to_string(state_dir.join(&path))
                .with_context(|| format!("read {path} in {state}"))?;
            digest::lookup::show(&path, &text, line, &ledger(), &archive())
        }
        DigestLookup::Ledger(id) => digest::lookup::ledger(
            &ledger(),
            &archive(),
            &id,
            &names(markdown_files(&state_dir.join("briefs"))),
            &names(markdown_files(&state_dir.join(digest::lookup::REPORTS_DIR))),
        ),
        DigestLookup::Brief(id) => {
            digest::lookup::brief(&markdown_files(&state_dir.join("briefs")), &id)
        }
    }
}

/// `--search`'s output for `words` in the state directory `state_dir`.
fn digest_search(state_dir: &Path, root: &str, words: &str) -> String {
    let read = |name: &str| fs::read_to_string(state_dir.join(name)).ok();
    let state = state_dir.to_string_lossy();
    let decisions = read("decisions.md").unwrap_or_default();
    let ledger = read(digest::lookup::LEDGER_FILE).unwrap_or_default();
    let ledger_archive = read(digest::lookup::LEDGER_ARCHIVE_FILE).unwrap_or_default();
    let handover_path = state_dir.join(crate::supervisor::HANDOVER_NOTE);
    let handover = fs::read_to_string(&handover_path)
        .ok()
        .map(|text| (modified_text(&handover_path), text));
    let briefs = markdown_files(&state_dir.join("briefs"));
    let handovers = markdown_files(&state_dir.join(crate::supervisor::HANDOVERS_DIR));
    let reports = markdown_files(&state_dir.join(digest::lookup::REPORTS_DIR));
    let notes = note_files(&state_dir.join(digest::lookup::NOTES_DIR));
    let show_command = format!("{} digest {root} --show <file:line>", corgi_bin_text());
    digest::search::search(
        &digest::search::SearchInput {
            state_dir: &state,
            show_command: &show_command,
            decisions: &decisions,
            ledger: &ledger,
            ledger_archive: &ledger_archive,
            handover: handover
                .as_ref()
                .map(|(when, text)| (when.as_str(), text.as_str())),
            briefs: &briefs,
            handovers: &handovers,
            reports: &reports,
            notes: &notes,
        },
        words,
    )
}

/// When `path` was last written, `YYYY-MM-DD HH:MM UTC`, or empty when that
/// cannot be read.
fn modified_text(path: &Path) -> String {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| {
            let stamp = crate::time::rfc3339_utc(age.as_secs());
            format!("{} {} UTC", &stamp[..10], &stamp[11..16])
        })
        .unwrap_or_default()
}

/// The largest note `--search` reads; a bigger file is likely not one.
const NOTE_BYTES: u64 = 1024 * 1024;

/// `(path under dir, time last written, text)` of each text file in `dir`
/// and the directories in it, by path; none when `dir` does not exist.
/// Files that are not UTF-8 or past [`NOTE_BYTES`] are left out, and links
/// are not followed.
fn note_files(dir: &Path) -> Vec<(String, String, String)> {
    let mut notes = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current).into_iter().flatten().flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file()
                && entry.metadata().is_ok_and(|meta| meta.len() <= NOTE_BYTES)
                && let Ok(text) = fs::read_to_string(&path)
                && let Ok(relative) = path.strip_prefix(dir)
            {
                notes.push((
                    relative.to_string_lossy().into_owned(),
                    modified_text(&path),
                    text,
                ));
            }
        }
    }
    notes.sort();
    notes
}

/// `(file name, text)` of each readable `.md` file in `dir`, by name; none
/// when `dir` does not exist.
fn markdown_files(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let text = name
                .ends_with(".md")
                .then(|| fs::read_to_string(entry.path()).ok())??;
            Some((name, text))
        })
        .collect();
    files.sort();
    files
}

pub const REPORT_USAGE: &str = "\
Usage: corgi report NAME

Prints, in full, the report the agent NAME (a Herdr agent name or pane ID)
left. For a running agent: the one the Corgi dashboard kept in its supervisor's
inbox when the agent last stopped, if that is its current state, else the
newest thing it said, which for a worker that followed its brief is its
report, or its screen when its harness writes no readable transcript. For
an agent whose pane is gone: the newest report kept in any supervisor's inbox.
When the supervisor of the current directory's project has missed inbox items, a
line on stderr says so.";

/// `corgi report NAME`: the report an agent left, which for a worker that
/// followed its brief is its report.
pub fn report(args: &[String]) -> Result<()> {
    let [target] = args else {
        bail!("Usage: corgi report NAME");
    };
    let connected = connected_app().ok();
    let running = connected.as_ref().and_then(|(_, app)| {
        app.agents
            .iter()
            .find(|agent| {
                agent.info.name.as_deref() == Some(target.as_str()) || agent.info.pane_id == *target
            })
            .cloned()
    });
    let printed = match (&connected, running) {
        (Some((client, _)), Some(agent)) => print_running_report(client, &agent, target),
        _ => inbox_report(target)?
            .with_context(|| format!("no running agent named {target}, and no report kept for it"))
            .and_then(|(dir, item)| print_kept_report(target, &dir, &item)),
    };
    if let Ok(root) = project_root(None) {
        print_inbox_footer(&root);
    }
    printed
}

/// Prints the report of the running `agent`: the one kept in its own
/// project's inbox when it is for this pane and its current state change,
/// so it is never another agent's of the same name or one it has since
/// said more than; else the newest thing it said, or its screen.
fn print_running_report(
    client: &HerdrClient,
    agent: &crate::model::DashboardAgent,
    target: &str,
) -> Result<()> {
    let info = &agent.info;
    let name = info.name.as_deref().unwrap_or(target);
    let dir = crate::supervisor::state_dir(&agent.project_root)?;
    let saved = Inbox::try_read(&dir)?;
    let kept = current_report(&saved, agent, name)
        .cloned()
        .map(|item| (dir, item));
    if let Some((dir, item)) = kept {
        print_kept_report(name, &dir, &item)?;
    } else if let Some(entry) = SessionReader::default().facts(info).report {
        println!("{}", entry.text);
    } else {
        eprintln!("(no readable transcript for {target}; showing its screen)");
        let read = client.read_agent(&info.pane_id, ReadSource::RecentUnwrapped, Some(120))?;
        println!("{}", read.text.trim_end());
    }
    Ok(())
}

/// The report `inbox` keeps of the running `agent`, named `name`, if it is
/// of its project and pane and of its current state change.
fn current_report<'a>(
    inbox: &'a Inbox,
    agent: &crate::model::DashboardAgent,
    name: &str,
) -> Option<&'a Item> {
    let info = &agent.info;
    inbox.newest_report(name).filter(|item| {
        item.is_for(&agent.project_root)
            && item.pane.as_deref() == Some(info.pane_id.as_str())
            && item.change == Some(info.state_change_seq)
    })
}

/// Prints the report `item` of the inbox in the state directory `dir` kept
/// of the agent `name`.
fn print_kept_report(name: &str, dir: &std::path::Path, item: &Item) -> Result<()> {
    let state = item.state.as_deref().unwrap_or("stopped");
    eprintln!(
        "(the report {name} left when it was {state}, at {})",
        item.ts
    );
    let text = item.try_report_text(dir)?.unwrap_or_default();
    println!("{}", text.trim_end());
    Ok(())
}

/// The newest report of the agent `name` kept in any supervisor's inbox, with
/// the state directory it is in, for an agent whose pane is gone.
fn inbox_report(name: &str) -> Result<Option<(PathBuf, Item)>> {
    let Some(base) = corgi_state_dir() else {
        return Ok(None);
    };
    let mut reports = Vec::new();
    for dir in inbox::all_state_dirs(&base) {
        if let Some(item) = Inbox::try_read(&dir)?.newest_report(name).cloned() {
            reports.push((dir, item));
        }
    }
    Ok(reports.into_iter().max_by(|(_, a), (_, b)| a.ts.cmp(&b.ts)))
}

pub const NOTIFY_USAGE: &str = "\
Usage: corgi notify [PROJECT] [OPTIONS] < text

Adds an item to the inbox of the supervisor of PROJECT (default: the project
containing the current directory), the one way anything but the dashboard
reaches a supervisor. The Corgi dashboard that wakes supervisors types it
into the supervisor's input box once the supervisor is between turns and its
box holds no draft, together with any other undelivered items, and records
it delivered; it survives the dashboard closing or restarting meanwhile. The
text is read from stdin, and gets a leading [Corgi] when it has none (a
leading [corgi], as Corgi wrote it before, becomes [Corgi]). Prints the item's
id and inbox as JSON.

Options:
  --agent NAME        The agent the item is about
  --state STATE       That agent's state, such as done or blocked
  --kind KIND         wake, conflict, merge or note (default: note)
  --key KEY           Of undelivered items with the same key, only the
                      newest goes out (default: the item's own id)
  --source NAME       Who adds it (default: notify)
  --report-file PATH  The agent's report, kept with the item, which
                      corgi report NAME then prints";

/// `corgi notify`: adds an item to a supervisor's inbox through the same code the
/// dashboard uses, for scripts and hooks.
pub fn notify(args: &[String]) -> Result<()> {
    let mut project = None;
    let mut item = Item {
        source: "notify".into(),
        kind: Kind::Note,
        ..Item::default()
    };
    let mut report_file = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .with_context(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--agent" => item.agent = Some(value()?),
            "--state" => item.state = Some(value()?.trim().to_lowercase()),
            "--kind" => {
                let kind = value()?;
                item.kind = Kind::from_label(&kind).with_context(|| {
                    format!("--kind must be wake, conflict, merge or note, not {kind:?}")
                })?;
            }
            "--key" => item.key = value()?,
            "--source" => item.source = value()?,
            "--report-file" => report_file = Some(value()?),
            other if other.starts_with('-') => bail!("unknown option {other:?}\n\n{NOTIFY_USAGE}"),
            _ if project.is_none() => project = Some(arg.as_str()),
            _ => bail!("{NOTIFY_USAGE}"),
        }
    }
    let text = read_task(None, true)?;
    anyhow::ensure!(!text.is_empty(), "the text is empty\n\n{NOTIFY_USAGE}");
    item.text = notify_text(&text);
    if let Some(path) = report_file {
        item.report =
            Some(fs::read_to_string(&path).with_context(|| format!("read report file {path}"))?);
    }
    item.project = project_root(project)?;
    let dir = crate::supervisor::state_dir(&item.project)?;
    let item = inbox::append(&dir, item)?.context("the inbox already has an item with that id")?;
    serde_json::to_writer(
        io::stdout(),
        &serde_json::json!({"id": item.id, "inbox": dir.join(inbox::INBOX_FILE)}),
    )?;
    println!();
    Ok(())
}

/// `text` as a line a supervisor tells from the user's: it starts with
/// [`WAKE_PREFIX`], which replaces the `[corgi]` Corgi wrote before the
/// supervisor was given its name, so that a script written for it still
/// reads as Corgi's.
fn notify_text(text: &str) -> String {
    if text.starts_with(WAKE_PREFIX) {
        text.to_string()
    } else if let Some(rest) = text.strip_prefix(OLD_WAKE_PREFIX) {
        format!("{WAKE_PREFIX}{rest}")
    } else {
        format!("{WAKE_PREFIX} {text}")
    }
}

pub const ROLE_USAGE: &str = "\
Usage: corgi role handover [PROJECT]

Prints the part of the supervisor's role it reads only when it needs it.
handover: how a supervisor hands over to a fresh session when the Corgi
dashboard asks it to, with the paths of the state directory of PROJECT's
supervisor (default: the project containing the current directory) and of
this Corgi binary filled in. Reads only.";

/// `corgi role handover [PROJECT]`: the project, if one is named.
fn role_args(args: &[String]) -> Result<Option<&str>> {
    match args {
        [part, rest @ ..] if part == "handover" => match rest {
            [] => Ok(None),
            [option] if option.starts_with('-') => {
                bail!("unknown option {option:?}\n\n{ROLE_USAGE}")
            }
            [project] => Ok(Some(project.as_str())),
            _ => bail!("{ROLE_USAGE}"),
        },
        _ => bail!("{ROLE_USAGE}"),
    }
}

/// `corgi role`: a part of the supervisor's role, printed on demand.
pub fn role_command(args: &[String]) -> Result<()> {
    let project = role_args(args)?;
    let root = project_root(project)?;
    let state_dir = crate::supervisor::state_dir(&root)?;
    let corgi_bin = env::current_exe().context("find the Corgi binary")?;
    print!(
        "{}",
        crate::supervisor::handover_instructions(&corgi_bin, &state_dir)
    );
    Ok(())
}

pub const INBOX_USAGE: &str = "\
Usage: corgi inbox [PROJECT] [--delivered] [--take]

Prints the undelivered items in the inbox of the supervisor of PROJECT (default:
the project containing the current directory), oldest first: what the Corgi
dashboard has not typed into the supervisor's input box yet, each with its report
or where to find it. Printing marks nothing delivered, so the dashboard still
types them in. A supervisor handing over leaves them to its successor.

Options:
  --delivered  Also print the newest delivered items
  --take       Record the printed undelivered items delivered, so the
               dashboard does not type them in again: for the supervisor, which
               acts on them now";

/// The most undelivered items `corgi inbox` prints, the newest.
const INBOX_SHOWN: usize = 20;
/// The most delivered items `corgi inbox --delivered` prints, the newest.
const INBOX_DELIVERED_SHOWN: usize = 10;
/// The report text `corgi inbox` prints in all, in bytes; past it, each
/// report is only pointed to.
const INBOX_REPORT_BUDGET: usize = 16 * 1024;

/// `corgi inbox`: a bounded view of a supervisor's inbox.
pub fn inbox_command(args: &[String]) -> Result<()> {
    let mut project = None;
    let (mut delivered, mut take) = (false, false);
    for arg in args {
        match arg.as_str() {
            "--delivered" => delivered = true,
            "--take" => take = true,
            other if other.starts_with('-') => bail!("unknown option {other:?}\n\n{INBOX_USAGE}"),
            _ if project.is_none() => project = Some(arg.as_str()),
            _ => bail!("{INBOX_USAGE}"),
        }
    }
    let root = project_root(project)?;
    let dir = crate::supervisor::state_dir(&root)?;
    let inbox = Inbox::try_read(&dir)?;
    let (text, shown) = inbox_text(&inbox, &dir, &root, delivered);
    print!("{text}");
    if take {
        let ids: Vec<String> = shown.iter().map(|item| item.id.clone()).collect();
        inbox::mark_delivered(&dir, &ids, "corgi inbox --take")?;
        if !ids.is_empty() {
            println!("Recorded {} item(s) delivered.", ids.len());
        }
    }
    Ok(())
}

/// What `corgi inbox` prints of `inbox`, in the state directory `dir`, for
/// the project at `root`, and the undelivered items it shows.
fn inbox_text<'a>(
    inbox: &'a Inbox,
    dir: &std::path::Path,
    root: &str,
    delivered: bool,
) -> (String, Vec<&'a Item>) {
    let undelivered = inbox.undelivered_for(root);
    let shown: Vec<&Item> = undelivered
        .iter()
        .skip(undelivered.len().saturating_sub(INBOX_SHOWN))
        .copied()
        .collect();
    let mut out = format!(
        "{} undelivered item(s) in {}\n",
        undelivered.len(),
        dir.join(inbox::INBOX_FILE).display()
    );
    if undelivered.len() > shown.len() {
        out.push_str(&format!(
            "(the {} oldest are not shown)\n",
            undelivered.len() - shown.len()
        ));
    }
    let mut budget = INBOX_REPORT_BUDGET;
    let corgi_bin = corgi_bin_text();
    for item in &shown {
        out.push_str(&inbox_entry(item, &corgi_bin, Some(&mut budget)));
    }
    if delivered {
        let mut done = inbox.delivered();
        done.retain(|item| item.is_for(root));
        out.push_str(&format!("\nNewest delivered ({} in all):\n", done.len()));
        for item in done
            .iter()
            .skip(done.len().saturating_sub(INBOX_DELIVERED_SHOWN))
        {
            out.push_str(&inbox_entry(item, &corgi_bin, None));
        }
    }
    (out, shown)
}

/// One item as `corgi inbox` prints it: a heading, its line, and its report
/// while `budget` lasts, else where the report is.
fn inbox_entry(item: &Item, corgi_bin: &str, budget: Option<&mut usize>) -> String {
    let about = [item.agent.as_deref(), item.state.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = format!(
        "\n--- {} {} {about} (from {}, id {})\n{}\n",
        item.ts,
        item.kind.label(),
        item.source,
        item.id,
        item.text
    );
    let pointer = |out: &mut String| {
        if let Some(agent) = &item.agent {
            out.push_str(&format!("(report: run {corgi_bin} report {agent})\n"));
        }
    };
    match (budget, &item.report) {
        (Some(budget), Some(report)) if report.len() <= *budget => {
            *budget -= report.len();
            out.push_str(&format!("{}\n", report.trim_end()));
        }
        _ if item.has_report() => pointer(&mut out),
        _ => {}
    }
    out
}

/// The Corgi binary as the commands it prints name it.
fn corgi_bin_text() -> String {
    env::current_exe().map_or_else(
        |_| "corgi".to_string(),
        |path| path.to_string_lossy().into_owned(),
    )
}

/// Prints, on stderr, that the supervisor of the project at `root` has missed
/// inbox items, if it does: any undelivered while no dashboard delivers,
/// else those that have waited minutes, not just for the supervisor's turn.
fn print_inbox_footer(root: &str) {
    let delivering = HerdrClient::from_env()
        .is_ok_and(|client| super::waker::wake_lock_held(client.socket_path()));
    if let Some(footer) = crate::supervisor::state_dir(root).ok().and_then(|dir| {
        inbox::footer(
            &dir,
            &corgi_bin_text(),
            root,
            delivering,
            crate::time::unix_now(),
        )
    }) {
        eprintln!("{footer}");
    }
}

/// The root of `project` (default: the current directory): its
/// repository's primary checkout, as Herdr reports it, or as Git does when
/// Herdr cannot be asked.
fn project_root(project: Option<&str>) -> Result<String> {
    let project = existing_project_dir(project)?;
    if let Ok(client) = HerdrClient::from_env()
        && let Ok(root) = project_root_of(&client, &project)
    {
        return Ok(root);
    }
    Ok(trust_root(&project).to_string_lossy().into_owned())
}

/// The task for a command-line launch, from `task_file` or else stdin. A
/// launch that may go without a task reads stdin only when it is piped.
fn read_task(task_file: Option<&str>, required: bool) -> Result<String> {
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

/// `project`, or the current directory, as an existing absolute directory.
fn existing_project_dir(project: Option<&str>) -> Result<PathBuf> {
    let project = match project {
        Some(path) => PathBuf::from(expand_home(path.trim())),
        None => env::current_dir().context("read the current directory")?,
    };
    let project = std::path::absolute(&project)
        .with_context(|| format!("resolve project {}", project.display()))?;
    anyhow::ensure!(
        project.is_dir(),
        "project {} is not an existing directory",
        project.display()
    );
    Ok(project)
}

/// A client for the injected Herdr socket and an app that has read one
/// snapshot. The caller is an agent pane, not a dashboard, so its own agent
/// is listed like any other and its name counts as taken.
fn connected_app() -> Result<(HerdrClient, App)> {
    let client = HerdrClient::from_env()?;
    Ok((client.clone(), refreshed_app(client)?))
}

/// An app on `client` that has read one snapshot.
fn refreshed_app(client: HerdrClient) -> Result<App> {
    let mut app = App::headless(client);
    app.refresh();
    anyhow::ensure!(app.connected, "Herdr is unavailable: {}", app.status);
    Ok(app)
}

/// `requested` as a Herdr agent name no live agent has.
fn free_agent_name(client: &HerdrClient, requested: &str) -> Result<String> {
    let name = sanitize_agent_name(requested);
    anyhow::ensure!(
        !client
            .snapshot()?
            .agents
            .iter()
            .any(|agent| agent.name.as_deref() == Some(name.as_str())),
        "an agent named {name} is already running"
    );
    Ok(name)
}

/// Runs a command-line launch, with its progress on stderr, and returns the
/// started agent as the JSON object the command prints.
fn run_launch(client: HerdrClient, mut app: App, plan: LaunchPlan) -> Result<serde_json::Value> {
    let launched = launch_agent(
        &client,
        &plan,
        &mut Stderr {
            projects: &mut app.projects,
        },
    )?;
    let agent = &launched.agent;
    Ok(serde_json::json!({
        "name": plan.name,
        "pane_id": agent.pane_id,
        "workspace_id": agent.workspace_id,
        "tab_id": agent.tab_id,
        "cwd": agent.cwd,
        "kind": plan.harness.kind(),
        "model": plan.model,
        "effort": plan.effort,
        "checkout": launched.checkout.value(),
        "supervisor": matches!(plan.role, Role::Supervisor { .. }),
        "location": launched.location,
    }))
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
    fn fleet_shows_whether_a_prompt_cache_is_warm_or_what_it_costs_cold() {
        const NOW: u64 = 1_800_000_000;
        let cache = |expires_at: u64, kind: PromptCacheKind| PromptCache {
            expires_at,
            tokens: 180_000,
            kind,
        };
        let row = |cache: Option<PromptCache>| {
            cache.map_or_else(|| "-".to_string(), |cache| fleet_cache(&cache, NOW))
        };
        use PromptCacheKind::{Estimated, Exact};

        assert_eq!(row(Some(cache(NOW + 42 * 60, Exact))), "warm 42m");
        assert_eq!(row(Some(cache(NOW + 30, Exact))), "warm 30s");
        assert_eq!(row(Some(cache(NOW, Exact))), "cold 180k");
        assert_eq!(row(Some(cache(NOW + 2 * 3_600, Estimated))), "warm? 2h");
        assert_eq!(row(Some(cache(NOW - 1, Estimated))), "cold? 180k");
        assert_eq!(row(None), "-");
    }

    #[test]
    fn corgi_role_prints_only_the_handover_part_for_at_most_one_project() {
        let args = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(role_args(&args(&["handover"])).unwrap(), None);
        assert_eq!(
            role_args(&args(&["handover", "/repos/weather"])).unwrap(),
            Some("/repos/weather")
        );
        for wrong in [
            &[][..],
            &["merge"],
            &["handover", "--all"],
            &["handover", "/a", "/b"],
        ] {
            let error = role_args(&args(wrong)).expect_err("refused");
            assert!(error.to_string().contains(ROLE_USAGE), "{wrong:?}");
        }
    }

    #[test]
    fn corgi_corgi_refuses_the_home_directory() {
        if !crate::paths::home().is_some_and(|home| home.is_dir()) {
            return;
        }
        // Refused before stdin is read or Herdr is asked anything.
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
            .args(["init", "-q"])
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
        let refused = spawn(&args).expect_err("no worker in the state directory");
        assert!(refused.to_string().contains("state directory"), "{refused}");
    }

    #[test]
    fn tag_takes_a_name_and_merge_or_clear() {
        let args = |line: &str| {
            line.split_whitespace()
                .map(String::from)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            parse_tag_options(&args("w-brave merge --project /repos/corgi")).unwrap(),
            TagOptions {
                name: "w-brave".into(),
                merge: true,
                project: Some("/repos/corgi".into()),
            }
        );
        assert_eq!(
            parse_tag_options(&args("w-brave --clear")).unwrap(),
            TagOptions {
                name: "w-brave".into(),
                merge: false,
                project: None,
            }
        );
        for wrong in [
            "",
            "w-brave",
            "w-brave merge --clear",
            "w-brave ready",
            "w-brave merge x",
        ] {
            assert!(parse_tag_options(&args(wrong)).is_err(), "{wrong:?}");
        }
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
        // A spawn not made by a supervisor, or by one whose harness Herdr has
        // not detected, keeps the form's default.
        assert_eq!(spawn_harness(None, None, || Harness::Codex), Harness::Codex);
    }

    #[test]
    fn spawn_options_default_to_the_forms_choices_and_reject_what_it_would() {
        let args = |line: &str| {
            line.split_whitespace()
                .map(String::from)
                .collect::<Vec<_>>()
        };
        assert_eq!(parse_spawn_options(&[]).unwrap(), SpawnOptions::default());

        let options = parse_spawn_options(&args(
            "--project ~/repos/corgi --harness Claude --model opus --effort HIGH \
             --checkout directory --name fix-cache --task-file brief.md \
             --request-id 20261002-fix-cache \
             -- --append-system-prompt-file role.md --name x",
        ))
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
            }
        );
        assert_eq!(
            parse_spawn_options(&args("--checkout root"))
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
        ] {
            assert!(
                parse_spawn_options(&args(bad)).is_err(),
                "{bad} was accepted"
            );
        }
    }

    #[test]
    fn a_notified_line_is_marked_as_corgis() {
        assert_eq!(notify_text("deploy done"), "[Corgi] deploy done");
        assert_eq!(notify_text("[Corgi] deploy done"), "[Corgi] deploy done");
        // A script that adds the prefix Corgi used before the rename.
        assert_eq!(notify_text("[corgi] deploy done"), "[Corgi] deploy done");
    }

    #[test]
    fn the_inbox_shows_undelivered_items_with_their_reports_and_marks_nothing() {
        let dir = crate::test_support::ScratchDir::new("cli-inbox");
        let wake = |agent: &str, id: &str, report: Option<String>| Item {
            id: id.into(),
            source: "dashboard".into(),
            agent: Some(agent.into()),
            state: Some("done".into()),
            key: agent.into(),
            text: format!("[Corgi] {agent} is done."),
            own: true,
            report,
            ..Item::default()
        };
        inbox::append(&dir, wake("w-old", "a", Some("old report".into()))).unwrap();
        inbox::mark_delivered(&dir, &["a".into()], "supervisor-weather").unwrap();
        inbox::append(
            &dir,
            wake("w-forecast", "b", Some("forecast report".into())),
        )
        .unwrap();
        let long = "x".repeat(inbox::INLINE_REPORT_MAX + 1);
        inbox::append(&dir, wake("w-radar", "c", Some(long))).unwrap();
        let read = Inbox::read(&dir);
        let (text, shown) = inbox_text(&read, &dir, "", false);
        assert!(text.starts_with("2 undelivered item(s) in "), "{text}");
        assert!(
            text.contains("[Corgi] w-forecast is done.\nforecast report\n"),
            "{text}"
        );
        assert!(
            text.contains("[Corgi] w-radar is done.\n(report: run "),
            "{text}"
        );
        assert!(!text.contains("old report"), "{text}");
        assert_eq!(shown.len(), 2);
        let (text, _) = inbox_text(&read, &dir, "", true);
        assert!(text.contains("Newest delivered (1 in all):"), "{text}");
        assert!(
            text.contains("[Corgi] w-old is done.\n(report: run "),
            "{text}"
        );
        // Printing marked nothing.
        assert_eq!(Inbox::read(&dir).undelivered().len(), 2);
    }

    #[test]
    fn a_running_agent_gets_its_own_current_report_only() {
        let kept = |project: &str, pane: &str, change: u64| Item {
            id: format!("{project}-{pane}-{change}"),
            project: project.into(),
            agent: Some("w-docs".into()),
            pane: Some(pane.into()),
            change: Some(change),
            report: Some("the report".into()),
            ..Item::default()
        };
        let agent = crate::model::DashboardAgent {
            info: crate::model::AgentInfo {
                name: Some("w-docs".into()),
                pane_id: "w2:p1".into(),
                state_change_seq: 7,
                ..Default::default()
            },
            project_root: "/repos/web".into(),
            ..Default::default()
        };
        let found = |item: Item| {
            let dir = crate::test_support::ScratchDir::new("cli-current-report");
            inbox::append(&dir, item).unwrap();
            current_report(&Inbox::read(&dir), &agent, "w-docs").is_some()
        };
        assert!(found(kept("/repos/web", "w2:p1", 7)));
        // Another project's agent of the same name, another pane, or a
        // stop it has since worked past: its transcript says more.
        assert!(!found(kept("/oss/web", "w2:p1", 7)));
        assert!(!found(kept("/repos/web", "w9:p1", 7)));
        assert!(!found(kept("/repos/web", "w2:p1", 5)));
    }

    #[test]
    fn the_inbox_prints_reports_only_within_its_budget() {
        let dir = crate::test_support::ScratchDir::new("cli-inbox-budget");
        for n in 0..8 {
            inbox::append(
                &dir,
                Item {
                    agent: Some(format!("w-{n}")),
                    text: format!("[Corgi] w-{n} is done."),
                    report: Some("r".repeat(inbox::INLINE_REPORT_MAX)),
                    ..Item::default()
                },
            )
            .unwrap();
        }
        let read = Inbox::read(&dir);
        let (text, shown) = inbox_text(&read, &dir, "", false);
        assert_eq!(shown.len(), 8);
        assert!(text.len() < INBOX_REPORT_BUDGET + 4_096, "{}", text.len());
        assert_eq!(text.matches("(report: run ").count(), 4);
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
        // is refused before stdin is read or Herdr is asked anything.
        let args = ["--request-id".to_string(), "a b".to_string()];
        assert!(parse_spawn_options(&args).is_err());
        let args = ["--request-id".to_string(), "20261002-corgi".to_string()];
        let refused = supervisor_command(&args).expect_err("no request id for a supervisor");
        assert!(refused.to_string().contains("for corgi spawn"), "{refused}");
    }

    #[test]
    fn digest_lookups_read_the_state_directory_and_decision_falls_back_to_search() {
        let state = crate::test_support::ScratchDir::new("digest-lookup");
        let write = |name: &str, text: &str| {
            let path = state.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(
            "decisions.md",
            "# Decisions\n\n## 2026-10-13: Loop fix (Q13: C)\nDecision: guard in kafka.\n\n\
             ## 2026-10-14: Snapshots\nDecision: none, settled as Q21.\n",
        );
        write(
            "handovers/20261014-103547.md",
            "# Handover\n\n## Open threads\nSettled: Q23 A (lessons in CLAUDE.md).\n",
        );
        write(
            "notes/plans/rollout.md",
            "# Rollout\n\nQ23 follow-up: none.\n",
        );
        write(
            "ledger-archive.jsonl",
            r#"{"id":"20261001-w-a","agent":"w-a","status":"dispatched","summary":"Q23 lessons"}"#,
        );
        write("briefs/20261001-w-a.md", "# Lessons\n");
        let lookup = |what| digest_lookup(&state, "/repos/weather", what).unwrap();

        // A heading, then a body, then a search.
        assert!(
            lookup(DigestLookup::Decision("q13".into())).starts_with("## 2026-10-13: Loop fix")
        );
        assert_eq!(
            lookup(DigestLookup::Decision("Q21".into())),
            "No decision heading contains \"Q21\"; 1 decision has it in its text:\n\n\
             ## 2026-10-14: Snapshots\nDecision: none, settled as Q21.\n"
        );
        let searched = lookup(DigestLookup::Decision("Q23".into()));
        assert!(searched.starts_with(
            "No decision contains \"Q23\"; searching all of your memory instead.\n\n\
             # Search \"q23\": 3 hits"
        ));
        for place in [
            "handovers/20261014-103547.md:4",
            "notes/plans/rollout.md:3",
            "ledger-archive.jsonl:1",
        ] {
            assert!(searched.contains(place), "{place}: {searched}");
        }

        assert!(
            lookup(DigestLookup::Show("handovers/20261014-103547.md:4".into())).starts_with(
                "# handovers/20261014-103547.md:3 (lines 3-4 of 4)\n## Open threads\n"
            )
        );
        assert!(lookup(DigestLookup::Ledger("w-a".into())).contains(
            "dispatched  agent: w-a; summary: Q23 lessons\nBrief: briefs/20261001-w-a.md\n"
        ));
        assert_eq!(
            lookup(DigestLookup::Brief("w-a".into())),
            "# briefs/20261001-w-a.md (whole file)\n# Lessons\n"
        );
        assert!(
            digest_lookup(
                &state,
                "/repos/weather",
                DigestLookup::Show("nothing.md".into())
            )
            .is_err()
        );
    }
}
