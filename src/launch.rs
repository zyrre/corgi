//! What a launch is made of, whoever starts it: the plan of a new agent,
//! its checkout and role, the agent CLI's arguments, and the supervisor's
//! plan. [`crate::hc::launch`] runs a plan in a herdr-core terminal.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::{
    git::trust_root,
    harness::Harness,
    model::Agent,
    paths::{dir_name, is_home},
    progress::Progress,
    projects::{is_created_project, record_created_project},
    supervisor,
};

/// Why the home directory gets no supervisor.
pub const SUPERVISOR_HOME_REFUSAL: &str = "The home directory is not a project, so it has no supervisor; corgi spawn --project ~ starts a scratch agent there";
/// The token that tags a worker `corgi spawn --request-id` started
/// with that id, so a retried spawn finds the worker instead of starting a
/// second one.
pub const CORGI_REQUEST_TOKEN: &str = "corgi_request";

/// Where a new agent works.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Checkout {
    /// A fresh Git worktree of the project's repository, so every agent edits
    /// its own checkout. Falls back to the directory itself outside Git.
    #[default]
    Worktree,
    /// The project directory as it is, shared with whatever already runs there.
    Directory,
    /// The project's primary checkout itself, where its supervisor lives.
    ProjectRoot,
}

impl Checkout {
    /// The token the checkout selector stores for this choice.
    pub fn value(self) -> &'static str {
        match self {
            Self::Worktree => "worktree",
            Self::Directory => "directory",
            Self::ProjectRoot => "root",
        }
    }

    pub fn from_value(value: &str) -> Option<Self> {
        [Self::Worktree, Self::Directory, Self::ProjectRoot]
            .into_iter()
            .find(|checkout| checkout.value().eq_ignore_ascii_case(value.trim()))
    }
}

/// Everything one new agent is started with. `corgi spawn`, `corgi
/// supervisor`, `corgi open` and a supervisor's handover all fill one in, so
/// every launch takes the same checkout, start, and first-prompt sequence.
pub struct LaunchPlan {
    pub name: String,
    pub harness: Harness,
    /// Empty leaves the model to the harness's own configuration.
    pub model: String,
    /// Empty leaves the effort level to the harness's own configuration.
    pub effort: String,
    pub prompt: String,
    pub project: PathBuf,
    pub checkout: Checkout,
    /// Arguments for the agent CLI itself, after the model and effort.
    pub extra_args: Vec<String>,
    pub role: Role,
    /// The `corgi spawn --request-id` the worker's terminal is labelled with.
    pub request_id: Option<String>,
}

/// Whom a launch starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    Worker,
    /// The project's supervisor.
    Supervisor {
        /// Whether it takes over from a session that handed over and
        /// exited, whose note it starts from.
        takes_over: bool,
    },
}

/// The agent CLI's arguments for `plan`, before a supervisor's own.
pub fn launch_args(plan: &LaunchPlan) -> Vec<String> {
    let mut args = plan.harness.launch_args(&plan.model, &plan.effort);
    args.extend(plan.extra_args.iter().cloned());
    args.extend(trusted_project_args(&plan.harness, &plan.project));
    args
}

/// `value` as an agent's name: lower-case ASCII letters, digits, `-` and
/// `_`, starting with a letter and at most 32 characters long.
pub fn sanitize_agent_name(value: &str) -> String {
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

/// Creates the directory of a new project and records it as one Corgi made.
/// It stays a plain directory until the project's supervisor makes it a
/// repository.
pub fn create_project(project: &Path, progress: &mut dyn Progress) -> Result<()> {
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

/// The arguments that answer `harness`'s folder-trust question for an agent
/// in `project` on its command line, when Corgi created that project, so the
/// CLI never asks. Nothing is written to the harness's configuration. Codex,
/// like Claude Code, keys trust on a worktree's main checkout.
pub(crate) fn trusted_project_args(harness: &Harness, project: &Path) -> Vec<String> {
    let root = trust_root(project);
    match fs::canonicalize(&root) {
        Ok(root) if is_created_project(&root) => harness.trusted_project_args(&root),
        _ => Vec::new(),
    }
}

/// Why a supervisor cannot start on `harness`.
pub fn supervisor_harness_error(harness: &Harness) -> String {
    format!(
        "A supervisor runs on {}, not {harness}",
        Harness::supervisor_kinds()
    )
}

/// The launch `corgi open` starts for `root`: the harness, model, effort and
/// arguments its supervisor was last launched with, `saved`, else the
/// `preset` harness (or the first a supervisor runs on) with the harness's
/// own defaults. It has no task, so the supervisor greets with the state of
/// the project.
pub fn saved_supervisor_plan(
    agents: &[Agent],
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
    let mut plan = supervisor_plan(agents, root, harness, model, effort, String::new())?;
    plan.extra_args = extra_args;
    Ok(plan)
}

/// The launch of `root`'s supervisor on `harness`, refused while one is already
/// running.
pub fn supervisor_plan(
    agents: &[Agent],
    root: &str,
    harness: Harness,
    model: String,
    effort: String,
    task: String,
) -> Result<LaunchPlan> {
    anyhow::ensure!(!is_home(root), "{SUPERVISOR_HOME_REFUSAL}");
    if let Some(running) = agents
        .iter()
        .find(|agent| agent.supervisor && agent.project_root == root)
    {
        bail!(
            "{}'s supervisor is already running as {}",
            dir_name(root).unwrap_or(supervisor::UNNAMED_PROJECT),
            running.info.display_name()
        );
    }
    let name = sanitize_agent_name(&format!(
        "supervisor-{}",
        dir_name(root).unwrap_or(supervisor::UNNAMED_PROJECT)
    ));
    anyhow::ensure!(
        !agents
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
        checkout: Checkout::ProjectRoot,
        extra_args: Vec::new(),
        role: Role::Supervisor { takes_over: false },
        request_id: None,
    })
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use crate::model::{Agent, AgentInfo};

    use super::*;

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
    fn a_supervisor_launch_is_named_after_its_project_and_refused_while_one_runs() {
        let mut agents: Vec<Agent> = Vec::new();
        let plan = supervisor_plan(
            &agents,
            "/repos/weather",
            Harness::Codex,
            "gpt-5-codex".into(),
            "high".into(),
            "Hi".into(),
        )
        .expect("no supervisor runs yet");
        assert_eq!(plan.name, "supervisor-weather");
        assert_eq!(plan.harness, Harness::Codex);
        assert_eq!(plan.model, "gpt-5-codex");
        assert_eq!(plan.effort, "high");
        assert_eq!(plan.checkout, Checkout::ProjectRoot);
        assert!(matches!(plan.role, Role::Supervisor { .. }));

        agents = vec![Agent {
            info: AgentInfo {
                name: Some("supervisor-weather".into()),
                ..AgentInfo::default()
            },
            project_root: "/repos/weather".into(),
            supervisor: true,
            ..Agent::default()
        }];
        let refused = supervisor_plan(
            &agents,
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
                .contains("already running as supervisor-weather")
        );
        // Another project's supervisor is no reason to refuse.
        assert!(
            supervisor_plan(
                &agents,
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
    fn a_supervisor_is_opened_as_last_time_else_on_the_preset_harness() {
        let agents: Vec<Agent> = Vec::new();
        let root = "/repos/weather";
        // Nothing saved: the preset harness and its own defaults.
        let plan = saved_supervisor_plan(&agents, root, None, || Harness::Codex).expect("plan");
        assert_eq!(plan.harness, Harness::Codex);
        assert_eq!((plan.model.as_str(), plan.effort.as_str()), ("", ""));
        assert_eq!(plan.prompt, "");
        assert_eq!(plan.checkout, Checkout::ProjectRoot);
        assert_eq!(plan.project, Path::new(root));
        assert!(matches!(plan.role, Role::Supervisor { takes_over: false }));
        // A preset no supervisor runs on falls back to the first that does.
        let plan = saved_supervisor_plan(&agents, root, None, || Harness::OpenCode).expect("plan");
        assert_eq!(plan.harness, Harness::SUPERVISOR_HARNESSES[0]);
        // The last launch wins over the preset.
        let saved = supervisor::Launch {
            kind: "codex".into(),
            model: "gpt-5-codex".into(),
            effort: "high".into(),
            extra_args: vec!["--search".into()],
        };
        let plan =
            saved_supervisor_plan(&agents, root, Some(saved), || Harness::Claude).expect("plan");
        assert_eq!(plan.harness, Harness::Codex);
        assert_eq!(plan.model, "gpt-5-codex");
        assert_eq!(plan.effort, "high");
        assert_eq!(plan.extra_args, ["--search"]);
    }
}
