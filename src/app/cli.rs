//! The command-line tools: `corgi spawn`, `corgi steward`, `corgi fleet` and
//! `corgi report`.

use std::{env, fs, io, path::PathBuf};

use anyhow::{Context, Result, bail};

use crate::{
    harness::Harness,
    herdr::{HerdrClient, ReadSource},
    model::ActivityKind,
    paths::{expand_home, is_home},
    session::SessionReader,
};

use super::{
    App,
    catalog::{EFFORT_LEVELS, default_harness},
    form::Checkout,
    launch::{
        LaunchPlan, Role, STEWARD_HOME_REFUSAL, calling_steward_harness, launch_agent,
        sanitize_agent_name, steward_harness_error, steward_plan, unique_agent_name,
    },
    progress::Stderr,
    project_main::project_root_of,
};

pub const SPAWN_USAGE: &str = "\
Usage: corgi spawn [OPTIONS] [-- AGENT_ARGS...] < task.md

Starts one agent the way the dashboard's new-agent form does and prints it as
JSON. The task is read from stdin, or from --task-file, never from arguments.

Options:
  --project PATH     Project directory (default: the current directory)
  --harness KIND     Agent CLI, as Herdr names it (default: the calling
                     Steward's own, else as in the form)
  --model MODEL      Model to start on (default: the harness's own setting)
  --effort LEVEL     low, medium, high, xhigh, or max (Codex and Claude Code)
  --checkout MODE    worktree (default), directory, or root: the project
                     workspace's own root tab in the primary checkout
  --name NAME        Agent name (default: taken from the project)
  --task-file PATH   Read the task from PATH instead of stdin

Arguments after -- go to the agent CLI unchanged, after the model and effort.";

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

/// `corgi spawn`: starts one agent through the same launch as the new-agent
/// form, for a caller without a dashboard, such as a Steward agent. It runs
/// inside Herdr and uses the injected socket. Progress goes to stderr and the
/// started agent to stdout as one JSON object.
pub fn spawn(args: &[String]) -> Result<()> {
    let options = parse_spawn_options(args)?;
    let prompt = read_task(options.task_file.as_deref(), true)?;
    anyhow::ensure!(!prompt.is_empty(), "the task is empty\n\n{SPAWN_USAGE}");
    let project = existing_project_dir(options.project.as_deref())?;

    let (client, app) = connected_app()?;
    let steward_harness = calling_steward_harness(&app, env::var("HERDR_PANE_ID").ok().as_deref());
    let harness = spawn_harness(options.harness, steward_harness, default_harness);
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
    };
    print_launch(&run_launch(client, app, plan)?)
}

/// The harness `corgi spawn` starts: the one asked for, else the calling
/// Steward's own, else the default the new-agent form would preset.
fn spawn_harness(
    requested: Option<Harness>,
    steward: Option<Harness>,
    default: impl FnOnce() -> Harness,
) -> Harness {
    requested.or(steward).unwrap_or_else(default)
}

pub const STEWARD_USAGE: &str = "\
Usage: corgi steward [PROJECT] [OPTIONS] [-- AGENT_ARGS...] [< task.md]

Starts PROJECT's Steward (default: the project containing the current
directory) in the root tab of its Corgi workspace, and prints it as JSON.
A task on stdin or in --task-file becomes its first request; without one the
Steward greets you with the state of the project.

Options:
  --harness KIND     claude (default) or codex
  --model MODEL      Model to start on (default: the harness's own setting)
  --effort LEVEL     low, medium, high, xhigh, or max
  --name NAME        Agent name (default: steward-<project>)
  --task-file PATH   Read the first request from PATH

Arguments after -- go to the agent CLI unchanged, after the model and effort.";

/// `corgi steward`: launches a project's Steward, the way the new-agent form
/// does for a project with no agent sessions.
pub fn steward_command(args: &[String]) -> Result<()> {
    let (positional, args) = match args.first() {
        Some(first) if !first.starts_with('-') => (Some(first.as_str()), &args[1..]),
        _ => (None, args),
    };
    let options = parse_spawn_options(args)?;
    let harness = options
        .harness
        .unwrap_or_else(|| Harness::STEWARDS[0].clone());
    anyhow::ensure!(harness.supports_steward(), steward_harness_error(&harness));
    let project = existing_project_dir(positional.or(options.project.as_deref()))?;
    anyhow::ensure!(!is_home(&project), STEWARD_HOME_REFUSAL);
    let task = read_task(options.task_file.as_deref(), false)?;

    let (client, app) = connected_app()?;
    let root = project_root_of(&client, &project)?;
    let mut plan = steward_plan(&app, &root, harness, options.model, options.effort, task)?;
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
NAME, ROLE (steward or worker), STATE, TASK, MODEL, CTX and CWD.";

/// `corgi fleet`: one tab-separated row per agent of a project (default: the
/// project containing the current directory), for a Steward to read.
pub fn fleet(args: &[String]) -> Result<()> {
    anyhow::ensure!(args.len() <= 1, "Usage: corgi fleet [PROJECT]");
    let project = existing_project_dir(args.first().map(String::as_str))?;
    let (client, app) = connected_app()?;
    let root = project_root_of(&client, &project)?;
    println!("NAME\tROLE\tSTATE\tTASK\tMODEL\tCTX\tCWD");
    for row in fleet_rows(&app, &root) {
        println!("{row}");
    }
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
                "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                field(agent.info.display_name()),
                if agent.steward { "steward" } else { "worker" },
                agent.info.state.label().to_lowercase(),
                field(&agent.task),
                field(agent.model.as_deref().unwrap_or("-")),
                agent
                    .context_percent
                    .map_or_else(|| "-".to_string(), |percent| format!("{percent}%")),
                field(agent.info.cwd()),
            )
        })
        .collect()
}

pub const REPORT_USAGE: &str = "\
Usage: corgi report NAME

Prints the newest thing the agent NAME (a Herdr agent name or pane ID) said,
which for a worker that followed its brief is its report. Shows the agent's
screen instead when its harness writes no readable transcript.";

/// `corgi report NAME`: the newest thing an agent said, which for a worker
/// that followed its brief is its report. Falls back to the agent's screen
/// when its harness writes no readable transcript.
pub fn report(args: &[String]) -> Result<()> {
    let [target] = args else {
        bail!("Usage: corgi report NAME");
    };
    let client = HerdrClient::from_env()?;
    let snapshot = client.snapshot()?;
    let agent = snapshot
        .agents
        .iter()
        .find(|agent| agent.name.as_deref() == Some(target.as_str()) || agent.pane_id == *target)
        .with_context(|| format!("no running agent named {target}"))?;
    let transcript = SessionReader::default().transcript(agent);
    let said = transcript
        .iter()
        .find(|entry| matches!(entry.kind, ActivityKind::Message | ActivityKind::Question));
    if let Some(entry) = said {
        println!("{}", entry.text);
        return Ok(());
    }
    eprintln!("(no readable transcript for {target}; showing its screen)");
    let read = client.read_agent(&agent.pane_id, ReadSource::RecentUnwrapped, Some(120))?;
    println!("{}", read.text.trim_end());
    Ok(())
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
    let mut app = App::headless(client.clone());
    app.refresh();
    anyhow::ensure!(app.connected, "Herdr is unavailable: {}", app.status);
    Ok((client, app))
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
        "steward": matches!(plan.role, Role::Steward { .. }),
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
    fn corgi_steward_refuses_the_home_directory() {
        if !crate::paths::home().is_some_and(|home| home.is_dir()) {
            return;
        }
        // Refused before stdin is read or Herdr is asked anything.
        let refused = steward_command(&["~".to_string()]).expect_err("no Steward for ~");
        assert!(refused.to_string().contains("not a project"), "{refused}");
    }

    #[test]
    fn a_stewards_workers_default_to_the_stewards_own_harness() {
        let default = || Harness::Claude;
        // A Codex Steward starts Codex workers, a Claude Steward Claude ones.
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
        // A spawn not made by a Steward, or by one whose harness Herdr has
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
        ] {
            assert!(
                parse_spawn_options(&args(bad)).is_err(),
                "{bad} was accepted"
            );
        }
    }
}
