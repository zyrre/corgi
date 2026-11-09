//! `corgi status --json` and `corgi merge NAME --json`: what a frontend that
//! is not a Herdr pane, such as the Herdr app's Corgi mod, reads and runs.
//! Both find the Herdr socket as `corgi serve` does, since the mod's `exec`
//! runs with the user's environment rather than inside a pane.

use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use corgi_core::{backlog, progress::Silent, project_main::project_root_of, waker::Inboxes};

use crate::{
    digest::{
        all_open_questions,
        lookup::{LEDGER_FILE, QUESTIONS_DIR},
        parse_ledger,
    },
    git::{git_current_branch, trust_root},
    herdr::HerdrClient,
    model::{DashboardAgent, PromptCacheKind},
    paths::{expand_home, home},
    time::{rfc3339_utc, unix_now},
};

use super::{
    App,
    launch::CORGI_REQUEST_TOKEN,
    merge::{
        MergeError, Upstream, abort_merge, clear_merged_tag, conflict_item, merge_form,
        project_supervisor, run_merge, tell_supervisor_about_merge,
    },
};

/// The version of `corgi status --json`'s shape: fields may be added under
/// it, and a rename or removal bumps it.
pub const STATUS_VERSION: u32 = 1;

pub const STATUS_USAGE: &str = "\
Usage: corgi status --json [--project DIR]

Prints one JSON object describing every project with a supervisor state
directory or a running Corgi agent (only DIR's project with --project): its
supervisor, its agents (the rows corgi fleet lists, the supervisor included,
each with its role), the open questions its supervisor's ledger keeps, the
inbox items still waiting, ledger counts and the backlog's length. It works
outside a Herdr pane: the Herdr socket is $HERDR_SOCKET_PATH, else the default
session's (~/.config/herdr/herdr.sock). When Herdr cannot be reached, the
projects still come from the state directories, without agents, and
\"herdr\".\"connected\" is false. It changes nothing.";

pub const MERGE_USAGE: &str = "\
Usage: corgi merge NAME --project DIR [--json] [--pull | --skip-upstream]

Merges the branch of agent NAME of DIR's project, from its linked worktree,
into the branch checked out in the project's primary checkout, and pushes it:
the merge the dashboard's m runs, with its checks. The base branch's upstream
is fetched first, and the merge is refused when the upstream has commits the
base lacks; --pull pulls them in first, and --skip-upstream merges without
fetching (after a failed fetch). The project's supervisor is told of a merge,
and of a failed push, in its inbox. A merge that stops on conflicts is
aborted and handed to the project's running supervisor, as the dashboard's s
does; without one it is left in the primary checkout to resolve.

With --json it prints {\"result\", \"message\", \"commit\", \"reason\"}: result
is merged, conflicts, push-failed or refused; reason says why a refusal was
(unavailable, behind, fetch-failed, failed, no-agent). It exits 0 only when
the branch was merged and pushed. The Herdr socket is found as for corgi status.";

/// The Herdr socket for a command run outside a pane: the one Herdr gave
/// this process, else the default session's.
fn herdr_socket() -> Result<PathBuf> {
    match env::var_os("HERDR_SOCKET_PATH").filter(|path| !path.is_empty()) {
        Some(path) => Ok(PathBuf::from(path)),
        None => Ok(home()
            .context("HOME is not set")?
            .join(".config/herdr/herdr.sock")),
    }
}

/// `dir` as an existing absolute directory.
fn existing_dir(dir: &str) -> Result<PathBuf> {
    let dir = PathBuf::from(expand_home(dir.trim()));
    let dir =
        std::path::absolute(&dir).with_context(|| format!("resolve project {}", dir.display()))?;
    anyhow::ensure!(
        dir.is_dir(),
        "project {} is not an existing directory",
        dir.display()
    );
    Ok(dir)
}

/// The primary checkout of the project at `dir`, as Herdr reports it, or as
/// Git does when Herdr cannot be asked.
fn project_root(client: &HerdrClient, connected: bool, dir: &Path) -> String {
    connected
        .then(|| project_root_of(client, dir).ok())
        .flatten()
        .unwrap_or_else(|| trust_root(dir).to_string_lossy().into_owned())
}

/// A headless app on the out-of-pane Herdr socket that has tried one
/// snapshot; `connected` says whether it got one.
fn status_app() -> Result<App> {
    let mut app = App::headless(HerdrClient::from_socket_path(herdr_socket()?));
    app.refresh();
    Ok(app)
}

/// `corgi status`.
pub fn status_command(args: &[String]) -> Result<()> {
    let mut json_output = false;
    let mut project = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json_output = true,
            "--project" => {
                project = Some(args.next().context("--project needs a directory")?.clone())
            }
            other => bail!("unknown option {other:?}\n\n{STATUS_USAGE}"),
        }
    }
    anyhow::ensure!(
        json_output,
        "corgi status prints JSON only for now: pass --json\n\n{STATUS_USAGE}"
    );
    let project = project.as_deref().map(existing_dir).transpose()?;
    let app = status_app()?;
    let only = project.map(|dir| project_root(&app.client, app.connected, &dir));
    let value = status_value(
        &app.agents,
        app.projects.roots(),
        &Inboxes::default(),
        (app.connected, &app.status),
        only.as_deref(),
        unix_now(),
    );
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}

/// The object `corgi status --json` prints for `agents` (the fleet's rows,
/// in the dashboard's order), the projects Corgi remembers (`remembered`),
/// of which those that had a supervisor are listed even with no agent
/// running, and the supervisors' state in `inboxes`. `herdr` is whether
/// Herdr answered and the app's status line; `only` keeps one project.
pub(crate) fn status_value(
    agents: &[DashboardAgent],
    remembered: &[String],
    inboxes: &Inboxes,
    herdr: (bool, &str),
    only: Option<&str>,
    now: u64,
) -> Value {
    let mut roots: Vec<&str> = Vec::new();
    for agent in agents {
        let root = agent.project_root.as_str();
        if !agent.scratch && !root.is_empty() && !roots.contains(&root) {
            roots.push(root);
        }
    }
    for root in remembered {
        if !roots.contains(&root.as_str()) && inboxes.has_supervisor(root) {
            roots.push(root);
        }
    }
    let projects: Vec<Value> = roots
        .into_iter()
        .filter(|root| only.is_none_or(|only| only == *root))
        .map(|root| project_value(agents, inboxes, root, now))
        .collect();
    json!({
        "version": STATUS_VERSION,
        "generated": rfc3339_utc(now),
        "herdr": {"connected": herdr.0, "status": herdr.1},
        "projects": projects,
    })
}

/// One project of `corgi status --json`.
fn project_value(agents: &[DashboardAgent], inboxes: &Inboxes, root: &str, now: u64) -> Value {
    let rows: Vec<&DashboardAgent> = agents
        .iter()
        .filter(|agent| !agent.scratch && agent.project_root == root)
        .collect();
    let state_dir = inboxes.dir(root).filter(|dir| dir.is_dir());
    let ledger = state_dir
        .as_ref()
        .and_then(|dir| fs::read_to_string(dir.join(LEDGER_FILE)).ok())
        .unwrap_or_default();
    let mut statuses: HashMap<String, usize> = HashMap::new();
    for entry in parse_ledger(&ledger) {
        *statuses.entry(entry.status).or_default() += 1;
    }
    let count = |status: &str| statuses.get(status).copied().unwrap_or(0);
    let questions: Vec<Value> = all_open_questions(&ledger)
        .into_iter()
        .map(|(agent, question)| {
            let (id, title) = crate::serve::split_question(&question);
            let file = state_dir
                .as_ref()
                .filter(|_| !id.is_empty())
                .map(|dir| dir.join(QUESTIONS_DIR).join(format!("{id}.md")))
                .filter(|file| file.is_file());
            json!({
                "id": (!id.is_empty()).then_some(id),
                "title": title,
                "blocks": (!agent.is_empty()).then_some(agent),
                "file": file.map(|file| file.to_string_lossy().into_owned()),
            })
        })
        .collect();
    json!({
        "name": Path::new(root)
            .file_name()
            .map_or_else(|| root.to_string(), |name| name.to_string_lossy().into_owned()),
        "dir": root,
        "state_dir": state_dir.as_ref().map(|dir| dir.to_string_lossy().into_owned()),
        "base": git_current_branch(Path::new(root)).ok(),
        "supervisor": rows
            .iter()
            .find(|agent| agent.supervisor)
            .map(|agent| agent_value(agent, now)),
        "agents": rows.iter().map(|agent| agent_value(agent, now)).collect::<Vec<_>>(),
        "questions": questions,
        "inbox_pending": inboxes.undelivered(root),
        "ledger": {
            "needs_answer": count("needs-answer"),
            "ready_to_merge": count("ready-to-merge"),
            "dispatched": count("dispatched"),
        },
        "backlog": state_dir.as_ref().map_or(0, |dir| backlog::read(dir).len()),
    })
}

/// One agent of `corgi status --json`, as `corgi fleet` lists it.
fn agent_value(agent: &DashboardAgent, now: u64) -> Value {
    let info = &agent.info;
    let cache = match agent.cache {
        None => {
            json!({"state": "unknown", "minutes_left": null, "tokens": null, "estimated": false})
        }
        Some(cache) => {
            let remaining = cache.remaining(now);
            json!({
                "state": if remaining > 0 { "warm" } else { "cold" },
                "minutes_left": remaining.div_ceil(60),
                "tokens": cache.tokens,
                "estimated": cache.kind == PromptCacheKind::Estimated,
            })
        }
    };
    let cwd = info.cwd();
    json!({
        "name": info.display_name(),
        "role": if agent.supervisor { "supervisor" } else { "worker" },
        "state": if agent.exited {
            "exited".to_string()
        } else {
            info.state.label().to_lowercase()
        },
        "task": agent.task,
        "model": agent.model,
        "harness": info.kind(),
        "ctx_pct": agent.context_percent,
        "cache": cache,
        "cwd": cwd,
        "branch": (!cwd.is_empty())
            .then(|| git_current_branch(Path::new(cwd)).ok())
            .flatten(),
        "worktree": agent.worktree_checkout,
        "tag": agent.ready_to_merge().then_some("merge"),
        "request_id": info.tokens.get(CORGI_REQUEST_TOKEN),
        "terminal": info.pane_id,
    })
}

/// How a `corgi merge` ended, as it prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeOutcome {
    /// `merged`, `conflicts`, `push-failed` or `refused`.
    pub(crate) result: &'static str,
    pub(crate) message: String,
    pub(crate) commit: Option<String>,
    /// Why a merge was refused: `unavailable`, `behind`, `fetch-failed`,
    /// `failed` or `no-agent`.
    pub(crate) reason: Option<&'static str>,
}

impl MergeOutcome {
    fn refused(reason: &'static str, message: String) -> Self {
        Self {
            result: "refused",
            message,
            commit: None,
            reason: Some(reason),
        }
    }

    fn json(&self) -> Value {
        json!({
            "result": self.result,
            "message": self.message,
            "commit": self.commit,
            "reason": self.reason,
        })
    }
}

/// `corgi merge`.
pub fn merge_command(args: &[String]) -> Result<()> {
    let mut name = None;
    let mut project = None;
    let mut json_output = false;
    let mut upstream: fn(&Upstream) -> Upstream = Upstream::checked;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json_output = true,
            "--pull" => upstream = Upstream::pulled,
            "--skip-upstream" => upstream = Upstream::skipped,
            "--project" => {
                project = Some(args.next().context("--project needs a directory")?.clone())
            }
            other if other.starts_with('-') => bail!("unknown option {other:?}\n\n{MERGE_USAGE}"),
            other if name.is_none() => name = Some(other.to_string()),
            other => bail!("unexpected argument {other:?}\n\n{MERGE_USAGE}"),
        }
    }
    let name = name.context(format!("name the agent to merge\n\n{MERGE_USAGE}"))?;
    let project =
        existing_dir(&project.context(format!("--project is required\n\n{MERGE_USAGE}"))?)?;
    let mut app = status_app()?;
    anyhow::ensure!(app.connected, "Herdr is unavailable: {}", app.status);
    let root = project_root(&app.client, app.connected, &project);
    let client = app.client.clone();
    let outcome = merge_agent(
        &client,
        &mut app.agents,
        &Inboxes::default(),
        &name,
        &root,
        upstream,
    );
    if json_output {
        println!("{}", serde_json::to_string(&outcome.json())?);
    } else if outcome.result == "merged" {
        println!("{}", outcome.message);
    } else {
        eprintln!("{}", outcome.message);
    }
    if outcome.result != "merged" {
        std::process::exit(1);
    }
    Ok(())
}

/// Merges the branch of the agent `name` of the project at `root` among
/// `agents`, as the dashboard's merge dialog does once confirmed, with
/// `upstream` choosing what happens to the base branch's upstream: clears
/// its merge tag on `client` and tells the supervisor through `inboxes`
/// once it lands, and hands a conflict to the running supervisor.
pub(crate) fn merge_agent(
    client: &HerdrClient,
    agents: &mut [DashboardAgent],
    inboxes: &Inboxes,
    name: &str,
    root: &str,
    upstream: fn(&Upstream) -> Upstream,
) -> MergeOutcome {
    let Some(agent) = agents.iter().find(|agent| {
        agent.project_root == root
            && !agent.supervisor
            && (agent.info.display_name() == name || agent.info.name.as_deref() == Some(name))
    }) else {
        return MergeOutcome::refused("no-agent", format!("No worker {name} runs in {root}"));
    };
    let mut form = match merge_form(agent) {
        Ok(form) => form,
        Err(error) => {
            return MergeOutcome::refused("unavailable", format!("Merge unavailable: {error:#}"));
        }
    };
    form.upstream = upstream(&form.upstream);
    let what = format!("{} into {}", form.source_branch, form.target_branch);
    match run_merge(&form, &mut Silent) {
        Ok(commit) => {
            clear_merged_tag(client, agents, &form);
            let mut message = format!("Merged and pushed {what}");
            if let Err(error) = tell_supervisor_about_merge(inboxes, agents, &form, &commit, None) {
                message.push_str(&format!(
                    ", but the supervisor could not be told: {error:#}"
                ));
            }
            MergeOutcome {
                result: "merged",
                message,
                commit: (!commit.is_empty()).then_some(commit),
                reason: None,
            }
        }
        Err(MergeError::PushFailed { commit, error }) => {
            clear_merged_tag(client, agents, &form);
            let mut message = format!("Merged {what}, but the push failed: {error}");
            if let Err(error) =
                tell_supervisor_about_merge(inboxes, agents, &form, &commit, Some(&error))
            {
                message.push_str(&format!("; the supervisor could not be told: {error:#}"));
            }
            MergeOutcome {
                result: "push-failed",
                message,
                commit: (!commit.is_empty()).then_some(commit),
                reason: None,
            }
        }
        Err(MergeError::Conflicts(files)) => {
            let mut message = format!(
                "Merging {what} stopped on conflicts in {}",
                files.join(", ")
            );
            match project_supervisor(agents, &form.project_root) {
                Some(supervisor) => match abort_merge(&form.project_root) {
                    Ok(()) => match inboxes.notify(conflict_item(&form, &files)) {
                        Ok(()) => message.push_str(&format!(
                            "; aborted it and asked {supervisor} to have {} merge {} into its branch",
                            form.agent, form.target_branch
                        )),
                        Err(error) => message.push_str(&format!(
                            "; aborted it, but {supervisor} could not be asked: {error:#}"
                        )),
                    },
                    Err(error) => message.push_str(&format!("; the abort failed: {error:#}")),
                },
                None => message.push_str(&format!(
                    "; resolve them in {} and run git merge --continue, or git merge --abort",
                    form.project_root.display()
                )),
            }
            MergeOutcome {
                result: "conflicts",
                message,
                commit: None,
                reason: None,
            }
        }
        Err(MergeError::Behind(commits)) => MergeOutcome::refused(
            "behind",
            format!(
                "{} has {} commit(s) {} lacks; merge again with --pull to pull them in first",
                form.upstream.name().unwrap_or("The upstream"),
                commits.len(),
                form.target_branch
            ),
        ),
        Err(MergeError::FetchFailed(error)) => MergeOutcome::refused(
            "fetch-failed",
            format!(
                "Could not fetch {}: {error}; merge again with --skip-upstream to merge without it",
                form.upstream.name().unwrap_or("the upstream")
            ),
        ),
        Err(MergeError::Failed(error)) => {
            MergeOutcome::refused("failed", format!("Merge of {what} failed: {error}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::*;
    use crate::{
        model::{AgentInfo, AgentState, PromptCache},
        supervisor::{MERGE_TOKEN, ROLE_FILE, merge_tag_value},
        test_support::{ScratchDir, git, git_repo, git_worktree},
    };

    const NOW: u64 = 1_800_000_000;

    fn agent(name: &str, root: &Path, supervisor: bool) -> DashboardAgent {
        DashboardAgent {
            info: AgentInfo {
                pane_id: format!("{name}:p1"),
                name: Some(name.into()),
                agent: Some("claude".into()),
                cwd: Some(root.to_string_lossy().into_owned()),
                ..AgentInfo::default()
            },
            project_root: root.to_string_lossy().into_owned(),
            supervisor,
            ..DashboardAgent::default()
        }
    }

    /// A supervisor's state directory for `root` under `base`, as a launch
    /// prepares it.
    fn supervisor_state(base: &Path, root: &Path) -> PathBuf {
        let dir = base.join(root.file_name().unwrap());
        fs::create_dir_all(dir.join(QUESTIONS_DIR)).unwrap();
        fs::write(dir.join(ROLE_FILE), "role").unwrap();
        dir
    }

    #[test]
    fn status_lists_each_project_with_its_supervisor_agents_questions_and_counts() {
        let scratch = ScratchDir::new("status-json");
        let base = scratch.join("state");
        let app = git_repo(&scratch.join("app"));
        let idle = git_repo(&scratch.join("idle"));
        let never = git_repo(&scratch.join("never"));
        let dir = supervisor_state(&base, &app);
        let idle_dir = supervisor_state(&base, &idle);
        fs::write(
            dir.join(LEDGER_FILE),
            [
                r#"{"id":"a","agent":"w-a","status":"dispatched"}"#,
                r#"{"id":"b","agent":"w-b","status":"needs-answer","questions":["Q2: Keep the flag?"]}"#,
                r#"{"id":"c","agent":"w-c","status":"dispatched"}"#,
                r#"{"id":"c","agent":"w-c","status":"ready-to-merge"}"#,
                r#"{"id":"d","agent":"","status":"needs-answer","questions":["Which name?"]}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        fs::write(dir.join(QUESTIONS_DIR).join("Q2.md"), "The whole question").unwrap();
        fs::write(
            dir.join("backlog.md"),
            "# Backlog\n\n## One\nfirst\n## Two\n",
        )
        .unwrap();
        let inboxes = corgi_core::waker::SupervisorWaker::in_dir(&base).inboxes();
        inboxes
            .notify(crate::inbox::Item {
                project: app.to_string_lossy().into_owned(),
                source: "test".into(),
                key: "k".into(),
                text: "waiting".into(),
                ..Default::default()
            })
            .unwrap();

        let supervisor = agent("app-supervisor", &app, true);
        let mut worker = agent("w-c", &app, false);
        worker.info.state = AgentState::Done;
        worker.info.state_change_seq = 4;
        worker
            .info
            .tokens
            .insert(MERGE_TOKEN.into(), merge_tag_value(AgentState::Done, 4));
        worker
            .info
            .tokens
            .insert(CORGI_REQUEST_TOKEN.into(), "20261109-w-c".into());
        worker.task = "Fix it".into();
        worker.model = Some("Opus 5.5".into());
        worker.context_percent = Some(18);
        worker.cache = Some(PromptCache {
            expires_at: NOW + 55 * 60 - 30,
            tokens: 1200,
            kind: PromptCacheKind::Exact,
        });
        let mut scratch_agent = agent("scratch", &scratch, false);
        scratch_agent.scratch = true;
        let remembered: Vec<String> = [&idle, &never, &app]
            .iter()
            .map(|root| root.to_string_lossy().into_owned())
            .collect();

        let value = status_value(
            &[supervisor, worker, scratch_agent],
            &remembered,
            &inboxes,
            (true, "3 agents"),
            None,
            NOW,
        );
        assert_eq!(value["version"], 1);
        assert_eq!(value["generated"], "2027-01-15T08:00:00Z");
        assert_eq!(
            value["herdr"],
            json!({"connected": true, "status": "3 agents"})
        );
        let projects = value["projects"].as_array().unwrap();
        let names: Vec<&str> = projects
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        // Running projects first, then those that had a supervisor; the
        // scratch agent and a project that never had one are left out.
        assert_eq!(names, ["app", "idle"]);

        let project = &projects[0];
        assert_eq!(project["dir"], app.to_string_lossy().as_ref());
        assert_eq!(project["state_dir"], dir.to_string_lossy().as_ref());
        assert_eq!(project["base"], "main");
        assert_eq!(project["supervisor"]["name"], "app-supervisor");
        assert_eq!(project["supervisor"]["role"], "supervisor");
        assert_eq!(project["inbox_pending"], 1);
        assert_eq!(
            project["ledger"],
            json!({"needs_answer": 2, "ready_to_merge": 1, "dispatched": 1})
        );
        assert_eq!(project["backlog"], 2);
        assert_eq!(
            project["questions"],
            json!([
                {"id": "Q2", "title": "Keep the flag?", "blocks": "w-b",
                 "file": dir.join("questions/Q2.md").to_string_lossy()},
                {"id": null, "title": "Which name?", "blocks": null, "file": null},
            ])
        );
        let agents = project["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(
            agents[1],
            json!({
                "name": "w-c",
                "role": "worker",
                "state": "done",
                "task": "Fix it",
                "model": "Opus 5.5",
                "harness": "claude",
                "ctx_pct": 18,
                "cache": {"state": "warm", "minutes_left": 55, "tokens": 1200, "estimated": false},
                "cwd": app.to_string_lossy(),
                "branch": "main",
                "worktree": null,
                "tag": "merge",
                "request_id": "20261109-w-c",
                "terminal": "w-c:p1",
            })
        );
        assert_eq!(agents[0]["cache"]["state"], "unknown");
        assert_eq!(agents[0]["tag"], json!(null));

        let idle_project = &projects[1];
        assert_eq!(
            idle_project["state_dir"],
            idle_dir.to_string_lossy().as_ref()
        );
        assert_eq!(idle_project["supervisor"], json!(null));
        assert_eq!(idle_project["agents"], json!([]));

        let only = status_value(
            &[],
            &remembered,
            &inboxes,
            (false, "Herdr is not running"),
            Some(&idle.to_string_lossy()),
            NOW,
        );
        assert_eq!(only["herdr"]["connected"], false);
        assert_eq!(only["projects"].as_array().unwrap().len(), 1);
        assert_eq!(only["projects"][0]["name"], "idle");
    }

    /// A project with a bare `origin`, a supervisor's state directory under
    /// `state`, and a worker in a linked worktree with one commit to merge.
    struct Mergeable {
        _scratch: ScratchDir,
        repo: PathBuf,
        worktree: PathBuf,
        remote: PathBuf,
        state: PathBuf,
    }

    impl Mergeable {
        fn new(label: &str) -> Self {
            let scratch = ScratchDir::new(label);
            let remote = scratch.join("remote.git");
            fs::create_dir_all(&remote).unwrap();
            git(&remote, &["init", "-q", "--bare", "-b", "main"]);
            let repo = git_repo(&scratch.join("project"));
            fs::write(repo.join("status.txt"), "base\n").unwrap();
            git(&repo, &["add", "status.txt"]);
            git(&repo, &["commit", "-q", "-m", "base"]);
            git(
                &repo,
                &["remote", "add", "origin", &remote.to_string_lossy()],
            );
            git(&repo, &["push", "-q", "--set-upstream", "origin", "main"]);
            let worktree = git_worktree(&repo, &scratch.join("worktree"), "worktree/fix");
            fs::write(worktree.join("status.txt"), "agent\n").unwrap();
            git(&worktree, &["commit", "-q", "-am", "agent result"]);
            let state = scratch.join("state");
            Self {
                _scratch: scratch,
                repo,
                worktree,
                remote,
                state,
            }
        }

        fn root(&self) -> String {
            self.repo.to_string_lossy().into_owned()
        }

        fn worker(&self) -> DashboardAgent {
            let mut worker = agent("w-fix", &self.repo, false);
            worker.worktree_checkout = Some(self.worktree.to_string_lossy().into_owned());
            worker.info.cwd = worker.worktree_checkout.clone();
            worker.task = "Fix the status".into();
            worker
        }

        fn merge(
            &self,
            agents: &mut [DashboardAgent],
            upstream: fn(&Upstream) -> Upstream,
        ) -> MergeOutcome {
            let client = HerdrClient::from_socket_path(self.state.join("no.sock"))
                .with_marker_record(self.state.join("markers.json"));
            merge_agent(
                &client,
                agents,
                &corgi_core::waker::SupervisorWaker::in_dir(&self.state).inboxes(),
                "w-fix",
                &self.root(),
                upstream,
            )
        }

        /// The lines waiting for the project's supervisor.
        fn pending(&self) -> Vec<String> {
            corgi_core::waker::SupervisorWaker::in_dir(&self.state)
                .inboxes()
                .pending_for(&self.root())
        }

        fn head(&self, rev: &str) -> String {
            crate::git::git_output(&self.repo, &["rev-parse", "--short", rev])
                .unwrap()
                .trim()
                .to_string()
        }

        fn mid_merge(&self) -> bool {
            self.repo.join(".git/MERGE_HEAD").exists()
        }

        /// Commits `contents` to `status.txt` on `main` through another
        /// clone and pushes it, so `origin/main` moves on.
        fn push_from_elsewhere(&self, file: &str) {
            let other = self.state.with_file_name("other");
            git(
                self.state.parent().unwrap(),
                &["clone", "-q", &self.remote.to_string_lossy(), "other"],
            );
            fs::write(other.join(file), "elsewhere\n").unwrap();
            git(&other, &["add", file]);
            git(&other, &["commit", "-q", "-m", "elsewhere"]);
            git(&other, &["push", "-q", "origin", "main"]);
        }
    }

    #[test]
    fn a_merge_lands_pushes_and_tells_the_supervisor() {
        let project = Mergeable::new("status-merge");
        supervisor_state(&project.state, &project.repo);
        let mut agents = vec![
            agent("app-supervisor", &project.repo, true),
            project.worker(),
        ];
        let outcome = project.merge(&mut agents, Upstream::checked);
        let commit = project.head("main");
        assert_eq!(
            outcome,
            MergeOutcome {
                result: "merged",
                message: "Merged and pushed worktree/fix into main".into(),
                commit: Some(commit.clone()),
                reason: None,
            }
        );
        assert_eq!(project.head("origin/main"), commit);
        assert_eq!(
            outcome.json(),
            json!({"result": "merged", "message": "Merged and pushed worktree/fix into main",
                   "commit": commit, "reason": null})
        );
        let pending = project.pending();
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert!(
            pending[0].starts_with(&format!(
                "[Corgi] The user merged w-fix (task \"Fix the status\"), branch worktree/fix, \
                 into main at {commit}."
            )),
            "{pending:?}"
        );
    }

    #[test]
    fn a_merge_is_refused_for_an_unknown_agent_or_a_dirty_checkout() {
        let project = Mergeable::new("status-merge-refused");
        let mut agents = vec![agent("w-other", &project.repo, false)];
        let outcome = project.merge(&mut agents, Upstream::checked);
        assert_eq!(
            (outcome.result, outcome.reason),
            ("refused", Some("no-agent"))
        );

        let mut agents = vec![project.worker()];
        fs::write(project.repo.join("status.txt"), "edited\n").unwrap();
        let outcome = project.merge(&mut agents, Upstream::checked);
        assert_eq!(
            (outcome.result, outcome.reason),
            ("refused", Some("unavailable"))
        );
        assert!(
            outcome.message.contains("primary checkout"),
            "{}",
            outcome.message
        );
        assert_eq!(outcome.json()["commit"], json!(null));
    }

    #[test]
    fn an_upstream_with_new_commits_refuses_until_pulled() {
        let project = Mergeable::new("status-merge-behind");
        project.push_from_elsewhere("other.txt");
        let mut agents = vec![project.worker()];
        let outcome = project.merge(&mut agents, Upstream::checked);
        assert_eq!(
            (outcome.result, outcome.reason),
            ("refused", Some("behind"))
        );
        assert!(outcome.message.contains("--pull"), "{}", outcome.message);

        let outcome = project.merge(&mut agents, Upstream::pulled);
        assert_eq!(outcome.result, "merged", "{}", outcome.message);
        assert!(project.repo.join("other.txt").exists());
        assert_eq!(project.head("origin/main"), project.head("main"));
    }

    #[test]
    fn a_failed_push_still_reports_the_merge_and_tells_the_supervisor() {
        let project = Mergeable::new("status-merge-push");
        supervisor_state(&project.state, &project.repo);
        let hook = project.remote.join("hooks/pre-receive");
        fs::write(&hook, "#!/bin/sh\necho 'main is protected' >&2\nexit 1\n").unwrap();
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        fs::set_permissions(&hook, permissions).unwrap();
        let mut agents = vec![project.worker()];
        let outcome = project.merge(&mut agents, Upstream::checked);
        assert_eq!(outcome.result, "push-failed", "{}", outcome.message);
        assert_eq!(outcome.commit, Some(project.head("main")));
        assert!(
            outcome.message.contains("main is protected"),
            "{}",
            outcome.message
        );
        let pending = project.pending();
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert!(pending[0].contains("push"), "{pending:?}");
    }

    #[test]
    fn a_conflict_goes_to_the_running_supervisor_or_stays_to_resolve() {
        for supervised in [true, false] {
            let project = Mergeable::new("status-merge-conflict");
            supervisor_state(&project.state, &project.repo);
            fs::write(project.repo.join("status.txt"), "main\n").unwrap();
            git(&project.repo, &["commit", "-q", "-am", "main moved on"]);
            git(&project.repo, &["push", "-q"]);
            let mut agents = vec![project.worker()];
            if supervised {
                agents.push(agent("app-supervisor", &project.repo, true));
            }
            let outcome = project.merge(&mut agents, Upstream::checked);
            assert_eq!(outcome.result, "conflicts", "{}", outcome.message);
            assert!(
                outcome.message.contains("status.txt"),
                "{}",
                outcome.message
            );
            let pending = project.pending();
            if supervised {
                assert!(!project.mid_merge());
                assert!(
                    outcome.message.contains("asked app-supervisor"),
                    "{}",
                    outcome.message
                );
                assert_eq!(pending.len(), 1, "{pending:?}");
                assert!(pending[0].contains("conflict"), "{pending:?}");
            } else {
                assert!(project.mid_merge());
                assert!(
                    outcome.message.contains("git merge --abort"),
                    "{}",
                    outcome.message
                );
                assert!(pending.is_empty(), "{pending:?}");
            }
        }
    }
}
