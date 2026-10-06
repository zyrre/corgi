//! The command-line tools: `corgi spawn`, `corgi start`, `corgi fleet`,
//! `corgi digest`, `corgi report`, `corgi notify`, `corgi inbox` and
//! `corgi tag`.

use std::{env, fs, io, path::PathBuf};

use anyhow::{Context, Result, bail};

use crate::{
    digest,
    git::{git_current_branch, git_output, trust_root},
    harness::Harness,
    herdr::{HerdrClient, METADATA_VALUE_MAX_CHARS, ReadSource},
    inbox::{self, Inbox, Item, Kind},
    paths::{corgi_state_dir, expand_home, is_home},
    session::SessionReader,
};

use super::{
    App,
    catalog::{EFFORT_LEVELS, default_harness},
    form::Checkout,
    launch::{
        CORGI_HOME_REFUSAL, CORGI_REQUEST_TOKEN, LaunchPlan, Role, calling_corgi_harness,
        corgi_harness_error, corgi_plan, launch_agent, sanitize_agent_name, unique_agent_name,
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
                     corgi's own, else as in the form)
  --model MODEL      Model to start on (default: the harness's own setting)
  --effort LEVEL     low, medium, high, xhigh, or max (Codex and Claude Code)
  --checkout MODE    worktree (default), directory, or root: the project
                     workspace's own root tab in the primary checkout
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
/// form, for a caller without a dashboard, such as a corgi agent. It runs
/// inside Herdr and uses the injected socket. Progress goes to stderr and the
/// started agent to stdout as one JSON object.
pub fn spawn(args: &[String]) -> Result<()> {
    let options = parse_spawn_options(args)?;
    let prompt = read_task(options.task_file.as_deref(), true)?;
    anyhow::ensure!(!prompt.is_empty(), "the task is empty\n\n{SPAWN_USAGE}");
    let project = existing_project_dir(options.project.as_deref())?;

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
    if let Some(warning) = calling_corgi_state_conflict(&app, pane_id.as_deref()) {
        eprintln!("warning: {warning}");
    }
    let corgi_harness = calling_corgi_harness(&app, pane_id.as_deref());
    let harness = spawn_harness(options.harness, corgi_harness, default_harness);
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
        "corgi": agent.corgi,
        "location": format!(
            "already running in {} ({})",
            info.cwd(),
            info.state.label().to_lowercase()
        ),
        "existing": true,
    }))
}

/// The harness `corgi spawn` starts: the one asked for, else the calling
/// corgi's own, else the default the new-agent form would preset.
fn spawn_harness(
    requested: Option<Harness>,
    corgi: Option<Harness>,
    default: impl FnOnce() -> Harness,
) -> Harness {
    requested.or(corgi).unwrap_or_else(default)
}

/// The warning about pre-rename state beside the state directory of the
/// corgi in `pane_id`, when a corgi is the one spawning, so that it sees
/// it and can tell the user.
fn calling_corgi_state_conflict(app: &App, pane_id: Option<&str>) -> Option<String> {
    let pane_id = pane_id?;
    app.agents
        .iter()
        .find(|agent| agent.corgi && agent.info.pane_id == pane_id)
        .and_then(|agent| crate::corgi::state_dir_conflict(&agent.project_root))
}

pub const START_USAGE: &str = "\
Usage: corgi start [PROJECT] [OPTIONS] [-- AGENT_ARGS...] [< task.md]

Starts the corgi of PROJECT (default: the project containing the current
directory), the agent that herds its workers, in the root tab of its Corgi
workspace, and prints it as JSON. A task on stdin or in --task-file becomes its first request; without
one the corgi greets you with the state of the project.

Options:
  --harness KIND     claude (default) or codex
  --model MODEL      Model to start on (default: the harness's own setting)
  --effort LEVEL     low, medium, high, xhigh, or max
  --name NAME        Agent name (default: corgi-<project>)
  --task-file PATH   Read the first request from PATH

Arguments after -- go to the agent CLI unchanged, after the model and effort.";

/// `corgi start`: launches a project's corgi, the way the new-agent form
/// does for a project with no agent sessions.
pub fn start_command(args: &[String]) -> Result<()> {
    let (positional, args) = match args.first() {
        Some(first) if !first.starts_with('-') => (Some(first.as_str()), &args[1..]),
        _ => (None, args),
    };
    let options = parse_spawn_options(args)?;
    anyhow::ensure!(
        options.request_id.is_none(),
        "--request-id is for corgi spawn; a project has one corgi anyway"
    );
    let harness = options
        .harness
        .unwrap_or_else(|| Harness::CORGI_HARNESSES[0].clone());
    anyhow::ensure!(harness.supports_corgi(), corgi_harness_error(&harness));
    let project = existing_project_dir(positional.or(options.project.as_deref()))?;
    anyhow::ensure!(!is_home(&project), CORGI_HOME_REFUSAL);
    let task = read_task(options.task_file.as_deref(), false)?;

    let (client, app) = connected_app()?;
    let root = project_root_of(&client, &project)?;
    let mut plan = corgi_plan(&app, &root, harness, options.model, options.effort, task)?;
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
NAME, ROLE (corgi or worker), STATE, TASK, MODEL, CTX, CWD and TAG, which is
merge for an agent a corgi tagged ready to merge (corgi tag) that still rests
where it was tagged, else -. When the project's corgi has missed inbox items,
a line on stderr says so.";

/// `corgi fleet`: one tab-separated row per agent of a project (default: the
/// project containing the current directory), for a corgi to read.
pub fn fleet(args: &[String]) -> Result<()> {
    anyhow::ensure!(args.len() <= 1, "Usage: corgi fleet [PROJECT]");
    let project = existing_project_dir(args.first().map(String::as_str))?;
    let (client, app) = connected_app()?;
    let root = project_root_of(&client, &project)?;
    println!("NAME\tROLE\tSTATE\tTASK\tMODEL\tCTX\tCWD\tTAG");
    for row in fleet_rows(&app, &root) {
        println!("{row}");
    }
    print_inbox_footer(&root);
    Ok(())
}

/// The rows `corgi fleet` prints for the agents of the project at `root`.
pub(super) fn fleet_rows(app: &App, root: &str) -> Vec<String> {
    let field = |text: &str| text.replace(['\t', '\n'], " ");
    app.agents
        .iter()
        .filter(|agent| agent.project_root == root)
        .map(|agent| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                field(agent.info.display_name()),
                if agent.corgi { "corgi" } else { "worker" },
                agent.info.state.label().to_lowercase(),
                field(&agent.task),
                field(agent.model.as_deref().unwrap_or("-")),
                agent
                    .context_percent
                    .map_or_else(|| "-".to_string(), |percent| format!("{percent}%")),
                field(agent.info.cwd()),
                if agent.ready_to_merge() { "merge" } else { "-" },
            )
        })
        .collect()
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

/// `corgi tag`: a corgi tags one of its project's agents ready for the user
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
        let had = info.tokens.contains_key(crate::corgi::MERGE_TOKEN);
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
    let value = crate::corgi::merge_tag_value(state, info.state_change_seq);
    markers::tag_pane(client, &info.pane_id, agent_name, session, Some(&value))?;
    Ok(format!(
        "Tagged {agent_name} ready to merge: {ahead} commit{} ahead of {base}",
        if ahead == 1 { "" } else { "s" }
    ))
}

/// The questions the project corgi's ledger still has open about the agent
/// `agent` (see [`digest::open_questions`]). A project without a ledger, or
/// one that cannot be read, has none.
fn open_questions(root: &str, agent: &str) -> Option<Vec<String>> {
    let state_dir = crate::corgi::state_dir(root).ok()?;
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
Usage: corgi digest [PROJECT] [--decision WORDS | --search WORDS]

Prints a bounded digest of the memory of PROJECT's corgi (default: the
project containing the current directory), for the start of a corgi's
session: the base branch, the handover note if there is one (else the open
threads of the newest archived one), open ledger work joined with the
running agents, recently finished work, the newest decisions in full (about
12 KB, at least 3) and the titles of older ones. Decisions named by a later
entry's `Supersedes:` line are left out. Reads only; it never changes the
state directory. When the corgi has missed inbox items, a line on
stderr says so.

Options:
  --decision WORDS   Print in full every decision, superseded or not, whose
                     heading contains all of WORDS (ignoring case)
  --search WORDS     Search all of the memory: decisions (superseded ones
                     marked), the ledger, briefs and archived handover notes.
                     A word matches the start of a word, ignoring case. Hits
                     are ranked by how many of WORDS they have, then newest
                     first, and shown as file:line, date and a few lines of
                     context, about 5 KB at most";

/// `corgi digest`: a bounded view of the state of a project's corgi, joined
/// with its running agents, for a corgi to read at the start of its session.
pub fn digest(args: &[String]) -> Result<()> {
    let mut project = None;
    let mut words = None;
    let mut search = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--decision" => {
                words = Some(args.next().context("--decision needs words")?.clone());
            }
            "--search" => {
                search = Some(args.next().context("--search needs words")?.clone());
            }
            other if other.starts_with('-') => bail!("unknown option {other:?}\n\n{DIGEST_USAGE}"),
            _ if project.is_none() => project = Some(arg.as_str()),
            _ => bail!("{DIGEST_USAGE}"),
        }
    }
    anyhow::ensure!(
        words.is_none() || search.is_none(),
        "use --decision or --search, not both"
    );
    if let Some(search) = &search {
        anyhow::ensure!(!search.trim().is_empty(), "--search needs words");
    }
    let project = existing_project_dir(project)?;
    let (client, app) = connected_app()?;
    let root = project_root_of(&client, &project)?;
    let state_dir = crate::corgi::state_dir(&root)?;
    let read = |name: &str| fs::read_to_string(state_dir.join(name)).ok();
    let decisions = read("decisions.md").unwrap_or_default();

    if let Some(words) = words {
        let decisions = digest::parse_decisions(&decisions);
        let found = digest::matching_decisions(&decisions, &words);
        anyhow::ensure!(!found.is_empty(), "no decision heading contains {words:?}");
        println!("{}", found.join("\n\n"));
        return Ok(());
    }

    let state = state_dir.to_string_lossy();
    let handovers = markdown_files(&state_dir.join(crate::corgi::HANDOVERS_DIR));
    if let Some(words) = search {
        let briefs = markdown_files(&state_dir.join("briefs"));
        let ledger = read("ledger.jsonl").unwrap_or_default();
        print!(
            "{}",
            digest::search::search(
                &digest::search::SearchInput {
                    state_dir: &state,
                    decisions: &decisions,
                    ledger: &ledger,
                    briefs: &briefs,
                    handovers: &handovers,
                },
                &words
            )
        );
        return Ok(());
    }

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
    let handover = read(crate::corgi::HANDOVER_NOTE);
    // Names are UTC stamps, so the last sorts newest.
    let previous_handover = handovers
        .last()
        .map(|(name, text)| (name.as_str(), text.as_str()));
    let ledger = read("ledger.jsonl").unwrap_or_default();
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
            decision_command: &decision_command,
            search_command: &search_command,
        })
    );
    print_inbox_footer(&root);
    Ok(())
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
left. For a running agent: the one the Corgi dashboard kept in its corgi's
inbox when the agent last stopped, if that is its current state, else the
newest thing it said, which for a worker that followed its brief is its
report, or its screen when its harness writes no readable transcript. For
an agent whose pane is gone: the newest report kept in any corgi's inbox.
When the corgi of the current directory's project has missed inbox items, a
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
        _ => inbox_report(target)
            .with_context(|| format!("no running agent named {target}, and no report kept for it"))
            .map(|(dir, item)| print_kept_report(target, &dir, &item)),
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
    let kept = crate::corgi::state_dir(&agent.project_root)
        .ok()
        .and_then(|dir| {
            let item = current_report(&Inbox::read(&dir), agent, name)?.clone();
            Some((dir, item))
        });
    if let Some((dir, item)) = kept {
        print_kept_report(name, &dir, &item);
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
fn print_kept_report(name: &str, dir: &std::path::Path, item: &Item) {
    let state = item.state.as_deref().unwrap_or("stopped");
    eprintln!(
        "(the report {name} left when it was {state}, at {})",
        item.ts
    );
    let text = item.report_text(dir).unwrap_or_default();
    println!("{}", text.trim_end());
}

/// The newest report of the agent `name` kept in any corgi's inbox, with
/// the state directory it is in, for an agent whose pane is gone.
fn inbox_report(name: &str) -> Option<(PathBuf, Item)> {
    let base = corgi_state_dir()?;
    inbox::all_state_dirs(&base)
        .into_iter()
        .filter_map(|dir| {
            let item = Inbox::read(&dir).newest_report(name)?.clone();
            Some((dir, item))
        })
        .max_by(|(_, a), (_, b)| a.ts.cmp(&b.ts))
}

pub const NOTIFY_USAGE: &str = "\
Usage: corgi notify [PROJECT] [OPTIONS] < text

Adds an item to the inbox of the corgi of PROJECT (default: the project
containing the current directory), the one way anything but the dashboard
reaches a corgi. The Corgi dashboard that wakes corgis types it into the
corgi's input box once the corgi is between turns and its box holds no
draft, together with any other undelivered items, and records it delivered;
it survives the dashboard closing or restarting meanwhile. The text is read
from stdin, and gets a leading [corgi] when it has none. Prints the item's
id and inbox as JSON.

Options:
  --agent NAME        The agent the item is about
  --state STATE       That agent's state, such as done or blocked
  --kind KIND         wake, conflict or note (default: note)
  --key KEY           Of undelivered items with the same key, only the
                      newest goes out (default: the item's own id)
  --source NAME       Who adds it (default: notify)
  --report-file PATH  The agent's report, kept with the item, which
                      corgi report NAME then prints";

/// `corgi notify`: adds an item to a corgi's inbox through the same code the
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
                    format!("--kind must be wake, conflict or note, not {kind:?}")
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
    let dir = crate::corgi::state_dir(&item.project)?;
    let item = inbox::append(&dir, item)?.context("the inbox already has an item with that id")?;
    serde_json::to_writer(
        io::stdout(),
        &serde_json::json!({"id": item.id, "inbox": dir.join(inbox::INBOX_FILE)}),
    )?;
    println!();
    Ok(())
}

/// `text` as a line a corgi tells from the user's: it starts with `[corgi]`.
fn notify_text(text: &str) -> String {
    if text.starts_with("[corgi]") {
        text.to_string()
    } else {
        format!("[corgi] {text}")
    }
}

pub const INBOX_USAGE: &str = "\
Usage: corgi inbox [PROJECT] [--delivered] [--take]

Prints the undelivered items in the inbox of the corgi of PROJECT (default:
the project containing the current directory), oldest first: what the Corgi
dashboard has not typed into the corgi's input box yet, each with its report
or where to find it. Printing marks nothing delivered, so the dashboard still
types them in. A corgi handing over leaves them to its successor.

Options:
  --delivered  Also print the newest delivered items
  --take       Record the printed undelivered items delivered, so the
               dashboard does not type them in again: for the corgi, which
               acts on them now";

/// The most undelivered items `corgi inbox` prints, the newest.
const INBOX_SHOWN: usize = 20;
/// The most delivered items `corgi inbox --delivered` prints, the newest.
const INBOX_DELIVERED_SHOWN: usize = 10;
/// The report text `corgi inbox` prints in all, in bytes; past it, each
/// report is only pointed to.
const INBOX_REPORT_BUDGET: usize = 16 * 1024;

/// `corgi inbox`: a bounded view of a corgi's inbox.
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
    let dir = crate::corgi::state_dir(&root)?;
    let inbox = Inbox::read(&dir);
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

/// Prints, on stderr, that the corgi of the project at `root` has missed
/// inbox items, if it does: any undelivered while no dashboard delivers,
/// else those that have waited minutes, not just for the corgi's turn.
fn print_inbox_footer(root: &str) {
    let delivering = HerdrClient::from_env()
        .is_ok_and(|client| super::waker::wake_lock_held(client.socket_path()));
    if let Some(footer) = crate::corgi::state_dir(root).ok().and_then(|dir| {
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
        "checkout": plan.checkout.value(),
        "corgi": matches!(plan.role, Role::Corgi { .. }),
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
    fn corgi_corgi_refuses_the_home_directory() {
        if !crate::paths::home().is_some_and(|home| home.is_dir()) {
            return;
        }
        // Refused before stdin is read or Herdr is asked anything.
        let refused = start_command(&["~".to_string()]).expect_err("no corgi for ~");
        assert!(refused.to_string().contains("not a project"), "{refused}");
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
    fn a_corgis_workers_default_to_the_corgis_own_harness() {
        let default = || Harness::Claude;
        // A Codex corgi starts Codex workers, a Claude corgi Claude ones.
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
        // A spawn not made by a corgi, or by one whose harness Herdr has
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
        assert_eq!(notify_text("deploy done"), "[corgi] deploy done");
        assert_eq!(notify_text("[corgi] deploy done"), "[corgi] deploy done");
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
            text: format!("[corgi] {agent} is done."),
            own: true,
            report,
            ..Item::default()
        };
        inbox::append(&dir, wake("w-old", "a", Some("old report".into()))).unwrap();
        inbox::mark_delivered(&dir, &["a".into()], "corgi-weather").unwrap();
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
            text.contains("[corgi] w-forecast is done.\nforecast report\n"),
            "{text}"
        );
        assert!(
            text.contains("[corgi] w-radar is done.\n(report: run "),
            "{text}"
        );
        assert!(!text.contains("old report"), "{text}");
        assert_eq!(shown.len(), 2);
        let (text, _) = inbox_text(&read, &dir, "", true);
        assert!(text.contains("Newest delivered (1 in all):"), "{text}");
        assert!(
            text.contains("[corgi] w-old is done.\n(report: run "),
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
                    text: format!("[corgi] w-{n} is done."),
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
        // The parser applies it, and the corgi does not take one, which
        // is refused before stdin is read or Herdr is asked anything.
        let args = ["--request-id".to_string(), "a b".to_string()];
        assert!(parse_spawn_options(&args).is_err());
        let args = ["--request-id".to_string(), "20261002-corgi".to_string()];
        let refused = start_command(&args).expect_err("no request id for a corgi");
        assert!(refused.to_string().contains("for corgi spawn"), "{refused}");
    }
}
