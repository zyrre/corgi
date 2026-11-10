//! `corgi projects` and `corgi open`: what the Herdr app's Projects mod runs
//! for its "Open project" field. `projects` lists the projects Corgi knows,
//! filtered by what is typed, with the directories that complete a typed
//! path and where a new name would be created, exactly as the new-agent
//! forms list them ([`corgi_core::choices::ChoiceList`]). `open` opens one:
//! it says which terminal its supervisor runs in, or starts it on herdr-core
//! (`hc`), in a shell the mod offers when that shell waits in the project's
//! primary checkout, else in a new terminal.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use corgi_core::{
    choices::{Choice, ChoiceList, FreeText},
    hc::{self, HcClient},
    launch::{SUPERVISOR_HOME_REFUSAL, create_project, dashboard_supervisor_plan, supervisor_plan},
    model::{DashboardAgent, WorkspaceInfo, WorkspaceWorktreeInfo},
    paths::{expand_home, is_home},
    progress::Progress,
    projects::{Project, ProjectMemory, known_projects, new_project_parent, project_choices},
    supervisor,
};

use crate::harness::Harness;

use super::{
    catalog::{EFFORT_LEVELS, default_harness},
    hc_cli::{self, Shell, Started, hc_project_root},
};

pub const PROJECTS_USAGE: &str = "\
Usage: corgi projects --json [--query TEXT] [--open DIR]...

Prints the projects to offer for \"Open project\" as one JSON object, in the
order the new-agent form lists them: projects with agents on herdr-core (hc),
the ones open in the Herdr app (each --open DIR, a folder a space is in),
Git repositories beside those, then the ones Corgi remembers. With --query only
those matching every word of TEXT, and, as the form does:

  an absolute path (or ~/…)  the directories that complete it, the directory
                             itself once it exists, and the path as a new
                             project while it does not
  a name alone               a new project of that name beside most of the
                             known projects (\"parent\"), unless it is taken

It prints {\"query\", \"parent\", \"projects\": [{\"dir\", \"name\", \"badge\",
\"new\", \"supervisor\"}]}: badge says where a project is known from (2 agents,
open in Herdr, nearby, recent, typed path, new project), new that the
directory does not exist yet, and supervisor the hc terminal its supervisor
runs in, or null. hc is asked only when its core already runs. It changes
nothing.";

pub const OPEN_USAGE: &str = "\
Usage: corgi open PROJECT [--create] [--terminal T]... [--json]
                  [--harness KIND] [--model MODEL] [--effort LEVEL]

Opens the project in the directory PROJECT on herdr-core (hc), the Herdr
app's backend, as the app's \"Open project\" does:

  its supervisor runs      nothing starts; the result names its terminal, for
                           the caller to show (result \"running\")
  else                     its supervisor is started (result \"started\"): in
                           the project's own supervisor terminal when that
                           waits at its shell; else in the first --terminal T
                           that is a shell waiting at its prompt in the
                           project's primary checkout, which becomes the
                           supervisor's terminal and its space the project's
                           (see corgi supervisor --terminal); else in a new
                           terminal and space, as corgi supervisor does

It starts on the harness, model and effort the project's supervisor last ran
with, else the new-agent form's preset harness; the options override them.
A PROJECT that does not exist is an error unless --create is given: then the
directory is created, as the new-agent form creates a new project, and
recorded as Corgi's own, so Claude Code's folder-trust question about it is
answered. With --json it prints {\"result\", \"message\", \"project\", \"name\",
\"terminal\", \"created\", \"place\"} (place: new, own or shell), and on a failure
{\"result\": \"failed\", \"message\"} with exit code 1.";

/// `corgi projects`.
pub fn projects_command(args: &[String]) -> Result<()> {
    let mut json_output = false;
    let mut query = String::new();
    let mut open = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .with_context(|| format!("{arg} needs a value\n\n{PROJECTS_USAGE}"))
        };
        match arg.as_str() {
            "--json" => json_output = true,
            "--query" => query = value()?,
            "--open" => open.push(value()?),
            other => bail!("unknown option {other:?}\n\n{PROJECTS_USAGE}"),
        }
    }
    anyhow::ensure!(
        json_output,
        "corgi projects prints JSON only for now: pass --json\n\n{PROJECTS_USAGE}"
    );
    // Asking hc would start a stopped core; a list must not.
    let agents = hc::core_running()
        .then(HcClient::locate)
        .flatten()
        .and_then(|client| hc::agents::agents(&client).ok())
        .unwrap_or_default();
    let value = projects_value(&agents, &open, &ProjectMemory::load(), &query);
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}

/// The object `corgi projects --json` prints: the projects known from
/// `agents` (Corgi's on hc), the folders `open` in the Herdr app and
/// `memory`, as the rows the new-agent form's project list shows for `query`.
pub(crate) fn projects_value(
    agents: &[DashboardAgent],
    open: &[String],
    memory: &ProjectMemory,
    query: &str,
) -> Value {
    let workspaces: Vec<WorkspaceInfo> = open
        .iter()
        .map(|dir| hc_project_root(Path::new(&expand_home(dir.trim()))))
        .filter(|root| !is_home(root))
        .map(|root| WorkspaceInfo {
            worktree: Some(WorkspaceWorktreeInfo {
                checkout_path: root.clone(),
                repo_root: root,
                ..WorkspaceWorktreeInfo::default()
            }),
            ..WorkspaceInfo::default()
        })
        .collect();
    let known = without_duplicates(known_projects(agents, &workspaces, memory));
    let parent = new_project_parent(&known);
    let mut list = ChoiceList::new(project_choices(&known), "", FreeText::Directory);
    if let Some(parent) = &parent {
        list = list.with_new_project_parent(parent.clone());
    }
    list.filter = query.to_string();
    let rows: Vec<Value> = list
        .rows()
        .iter()
        .map(|choice| project_row(choice, agents))
        .collect();
    json!({ "query": query, "parent": parent, "projects": rows })
}

/// `projects` with each directory once: hc's labels name a project without
/// symlinks, and a path Corgi wrote down earlier may name the same one
/// through one.
fn without_duplicates(projects: Vec<Project>) -> Vec<Project> {
    let canonical = |root: &str| fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let mut seen: Vec<PathBuf> = Vec::new();
    projects
        .into_iter()
        .filter(|project| {
            let root = canonical(&project.root);
            let new = !seen.contains(&root);
            if new {
                seen.push(root);
            }
            new
        })
        .collect()
}

/// One row of `corgi projects --json`.
fn project_row(choice: &Choice, agents: &[DashboardAgent]) -> Value {
    json!({
        "dir": choice.value,
        "name": choice.label,
        "badge": choice.badge,
        "new": choice.badge == Choice::new_project("").badge,
        "supervisor": running_supervisor(agents, &choice.value).map(|agent| &agent.info.pane_id),
    })
}

/// The supervisor of the project at `root` among `agents`, while its agent
/// runs: one at its shell waits to be started again.
fn running_supervisor<'a>(agents: &'a [DashboardAgent], root: &str) -> Option<&'a DashboardAgent> {
    agents
        .iter()
        .find(|agent| agent.supervisor && !agent.exited && agent.project_root == root)
}

/// What `corgi open` was asked for.
#[derive(Debug, Default, PartialEq, Eq)]
struct OpenOptions {
    project: String,
    create: bool,
    terminals: Vec<String>,
    harness: Option<Harness>,
    model: Option<String>,
    effort: Option<String>,
    json: bool,
}

fn parse_open_options(args: &[String]) -> Result<OpenOptions> {
    let mut options = OpenOptions::default();
    let mut project = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .with_context(|| format!("{arg} needs a value\n\n{OPEN_USAGE}"))
        };
        match arg.as_str() {
            "--create" => options.create = true,
            "--json" => options.json = true,
            "--terminal" => options.terminals.push(value()?.trim().to_string()),
            "--harness" => options.harness = Some(Harness::from_typed(&value()?)),
            "--model" => options.model = Some(value()?.trim().to_string()),
            "--effort" => options.effort = Some(value()?.trim().to_lowercase()),
            other if other.starts_with('-') => bail!("unknown option {other:?}\n\n{OPEN_USAGE}"),
            other => {
                anyhow::ensure!(
                    project.is_none(),
                    "corgi open takes one PROJECT, not also {other:?}\n\n{OPEN_USAGE}"
                );
                project = Some(other.to_string());
            }
        }
    }
    options.project = project
        .filter(|project| !project.trim().is_empty())
        .with_context(|| format!("corgi open needs a PROJECT directory\n\n{OPEN_USAGE}"))?;
    if let Some(harness) = &options.harness {
        anyhow::ensure!(
            harness.supports_supervisor(),
            super::launch::supervisor_harness_error(harness)
        );
    }
    if let Some(effort) = &options.effort {
        anyhow::ensure!(
            EFFORT_LEVELS.contains(&effort.as_str()),
            "--effort must be one of {}",
            EFFORT_LEVELS.join(", ")
        );
    }
    Ok(options)
}

/// `corgi open`.
pub fn open_command(args: &[String]) -> Result<()> {
    let options = parse_open_options(args)?;
    let opened = HcClient::from_env().and_then(|client| open_project(&client, &options));
    if !options.json {
        let opened = opened?;
        println!("{}", opened["message"].as_str().unwrap_or_default());
        return Ok(());
    }
    match opened {
        Ok(opened) => println!("{}", serde_json::to_string(&opened)?),
        Err(error) => {
            let failed = json!({ "result": "failed", "message": format!("{error:#}") });
            println!("{}", serde_json::to_string(&failed)?);
            std::process::exit(1);
        }
    }
    Ok(())
}

/// Progress on stderr.
struct Said;

impl Progress for Said {
    fn report(&mut self, message: String) {
        eprintln!("{message}");
    }
}

/// The directory `typed` names, absolute, created when `create` allows it
/// and it is not there: whether it was created.
fn project_dir(typed: &str, create: bool) -> Result<(PathBuf, bool)> {
    let dir = PathBuf::from(expand_home(typed.trim()));
    let dir =
        std::path::absolute(&dir).with_context(|| format!("resolve project {}", dir.display()))?;
    if dir.is_dir() {
        return Ok((dir, false));
    }
    anyhow::ensure!(
        !dir.exists(),
        "{} is not a directory, so it cannot be a project",
        dir.display()
    );
    anyhow::ensure!(
        create,
        "project {} does not exist; --create makes it a new project",
        dir.display()
    );
    create_project(&dir, &mut Said)?;
    Ok((dir, true))
}

/// Opens the project `options` names on the hc of `client`, and returns
/// what `corgi open --json` prints.
fn open_project(client: &HcClient, options: &OpenOptions) -> Result<Value> {
    let (dir, created) = project_dir(&options.project, options.create)?;
    let root = hc_project_root(&dir);
    anyhow::ensure!(!is_home(&root), SUPERVISOR_HOME_REFUSAL);
    let agents = hc::agents::agents(client)?;
    if let Some(running) = running_supervisor(&agents, &root) {
        let name = running.info.display_name();
        return Ok(json!({
            "result": "running",
            "message": format!("{root}'s supervisor {name} runs in {}", running.info.pane_id),
            "project": root,
            "name": name,
            "terminal": running.info.pane_id,
            "created": false,
            "place": Value::Null,
        }));
    }
    let (exited, others) = hc_cli::exited_supervisor(agents, &root);
    let mut plan = match &options.harness {
        Some(harness) => supervisor_plan(
            &others,
            &root,
            harness.clone(),
            String::new(),
            String::new(),
            String::new(),
        )?,
        None => {
            let saved = supervisor::state_dir(&root)
                .ok()
                .and_then(|dir| supervisor::saved_launch(&dir));
            dashboard_supervisor_plan(&others, &root, saved, default_harness)?
        }
    };
    if let Some(model) = &options.model {
        plan.model = model.clone();
    }
    if let Some(effort) = &options.effort {
        anyhow::ensure!(
            effort.is_empty() || plan.harness.supports_effort(),
            "{} has no effort control; only codex and claude take --effort",
            plan.harness
        );
        plan.effort = effort.clone();
    }
    let (launched, started) = hc_cli::start_supervisor(
        client,
        &mut plan,
        exited.as_ref(),
        None,
        Shell::Offered(&options.terminals),
    )?;
    let place = match started {
        Started::New => "a new terminal",
        Started::Own => "its own terminal",
        Started::Shell => "the shell that was open there",
    };
    Ok(json!({
        "result": "started",
        "message": format!(
            "Started {root}'s supervisor {} in {place} ({})",
            launched.name, launched.terminal.address
        ),
        "project": root,
        "name": launched.name,
        "terminal": launched.terminal.address,
        "created": created,
        "place": started.word(),
    }))
}

#[cfg(test)]
mod tests {
    use corgi_core::test_support::ScratchDir;

    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    fn supervisor(root: &str, terminal: &str, exited: bool) -> DashboardAgent {
        let mut agent = DashboardAgent {
            project_root: root.into(),
            supervisor: true,
            exited,
            ..DashboardAgent::default()
        };
        agent.info.pane_id = terminal.into();
        agent
    }

    fn dirs(value: &Value) -> Vec<(String, String, bool)> {
        value["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["name"].as_str().unwrap().to_string(),
                    row["badge"].as_str().unwrap().to_string(),
                    row["new"].as_bool().unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn open_takes_one_project_and_its_options() {
        let options = parse_open_options(&args(&[
            "~/repos/weather",
            "--terminal",
            "t7",
            "--create",
            "--terminal",
            "t9",
            "--json",
            "--model",
            "haiku",
        ]))
        .unwrap();
        assert_eq!(
            options,
            OpenOptions {
                project: "~/repos/weather".into(),
                create: true,
                terminals: vec!["t7".into(), "t9".into()],
                model: Some("haiku".into()),
                json: true,
                ..OpenOptions::default()
            }
        );
        for (bad, why) in [
            (&["--json"][..], "needs a PROJECT"),
            (&["a", "b"], "takes one PROJECT"),
            (&["a", "--terminal"], "needs a value"),
            (&["a", "--bogus"], "unknown option"),
            (&["a", "--harness", "gemini"], "supervisor"),
            (&["a", "--effort", "huge"], "--effort must be one of"),
        ] {
            let error = parse_open_options(&args(bad)).unwrap_err().to_string();
            assert!(error.contains(why), "{bad:?}: {error}");
        }
    }

    #[test]
    fn a_missing_project_is_made_only_when_asked_and_a_file_is_never_one() {
        let scratch = ScratchDir::new("open-dir");
        let there = scratch.join("there");
        fs::create_dir_all(&there).unwrap();
        assert_eq!(
            project_dir(&there.to_string_lossy(), false).unwrap(),
            (there.clone(), false)
        );
        let missing = scratch.join("missing");
        let error = project_dir(&missing.to_string_lossy(), false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("--create makes it a new project"), "{error}");
        assert!(!missing.exists());
        let file = scratch.join("file");
        fs::write(&file, "").unwrap();
        let error = project_dir(&file.to_string_lossy(), true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("is not a directory"), "{error}");
    }

    #[test]
    fn projects_are_listed_as_the_form_lists_them_with_their_supervisors() {
        let scratch = ScratchDir::new("open-projects");
        let repos = fs::canonicalize(&*scratch).unwrap().join("repos");
        let [weather, zebra, _beside] = ["weather", "zebra", "beside"].map(|name| {
            let dir = repos.join(name);
            fs::create_dir_all(dir.join(".git")).unwrap();
            dir.to_string_lossy().into_owned()
        });
        // A plain folder beside them is no project to offer.
        fs::create_dir_all(repos.join("worktrees")).unwrap();
        let agents = [
            supervisor(&weather, "t3", false),
            DashboardAgent {
                project_root: weather.clone(),
                ..DashboardAgent::default()
            },
        ];
        let memory = ProjectMemory::default();
        let open = [zebra.clone()];

        let all = projects_value(&agents, &open, &memory, "");
        assert_eq!(
            dirs(&all),
            [
                ("weather".to_string(), "2 agents".to_string(), false),
                ("zebra".into(), "open in Herdr".into(), false),
                ("beside".into(), "nearby".into(), false),
            ]
        );
        assert_eq!(all["parent"], repos.to_string_lossy().as_ref());
        assert_eq!(all["projects"][0]["dir"], weather.as_str());
        assert_eq!(all["projects"][0]["supervisor"], "t3");
        assert_eq!(all["projects"][1]["supervisor"], Value::Null);

        // Typing narrows the known projects.
        assert_eq!(
            dirs(&projects_value(&agents, &open, &memory, "zeb")),
            [
                ("zebra".to_string(), "open in Herdr".to_string(), false),
                ("zeb".into(), "new project".into(), true),
            ]
        );
        // A name alone goes beside the others, unless it is taken.
        let new = projects_value(&agents, &open, &memory, "fresh");
        assert_eq!(
            dirs(&new),
            [("fresh".to_string(), "new project".to_string(), true)]
        );
        assert_eq!(
            new["projects"][0]["dir"],
            repos.join("fresh").to_string_lossy().as_ref()
        );
        assert_eq!(
            dirs(&projects_value(&agents, &open, &memory, "weather")),
            [("weather".to_string(), "2 agents".to_string(), false)]
        );

        // A path completes to the directories under it.
        let sub = repos.join("zebra/src");
        fs::create_dir_all(&sub).unwrap();
        let typed = format!("{}/zebra/s", repos.display());
        let completed = projects_value(&agents, &open, &memory, &typed);
        assert_eq!(
            dirs(&completed),
            [
                ("src".to_string(), "typed path".to_string(), false),
                ("s".into(), "new project".into(), true),
            ]
        );
        assert_eq!(
            completed["projects"][0]["dir"],
            sub.to_string_lossy().as_ref()
        );
    }

    #[test]
    fn a_supervisor_at_its_shell_does_not_count_as_running() {
        let agents = [supervisor("/repos/weather", "t3", true)];
        assert!(running_supervisor(&agents, "/repos/weather").is_none());
        let agents = [supervisor("/repos/weather", "t3", false)];
        assert!(running_supervisor(&agents, "/repos/other").is_none());
        assert_eq!(
            running_supervisor(&agents, "/repos/weather")
                .unwrap()
                .info
                .pane_id,
            "t3"
        );
    }

    #[test]
    fn a_project_known_twice_through_a_symlink_is_listed_once() {
        let scratch = ScratchDir::new("open-symlink");
        let real = fs::canonicalize(&*scratch).unwrap().join("repos/weather");
        fs::create_dir_all(&real).unwrap();
        let link = scratch.join("link");
        std::os::unix::fs::symlink(real.parent().unwrap(), &link).unwrap();
        let project = |root: &Path| Project {
            root: root.to_string_lossy().into_owned(),
            name: "weather".into(),
            source: corgi_core::projects::ProjectSource::Remembered,
        };
        let listed = without_duplicates(vec![project(&real), project(&link.join("weather"))]);
        assert_eq!(listed, [project(&real)]);
    }
}
