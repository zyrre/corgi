//! `corgi status --json` and `corgi merge NAME --json`: what the Herdr app's
//! Corgi mod reads and runs. Both see the agents Corgi runs on herdr-core
//! (`hc`), the Herdr app's backend, and nothing of the Herdr TUI. Neither asks
//! `hc` anything while its core's socket refuses connections, since any `hc`
//! call starts a stopped core, with the caller's environment.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use corgi_core::{
    backlog,
    hc::{self, HcClient},
    progress::Silent,
    projects::ProjectMemory,
    waker::Inboxes,
};

use crate::{
    digest::{
        all_open_questions,
        lookup::{LEDGER_FILE, QUESTIONS_DIR},
        parse_ledger,
    },
    git::git_current_branch,
    model::{DashboardAgent, PromptCacheKind},
    paths::expand_home,
    time::{rfc3339_utc, unix_now},
};

use super::{
    hc_cli::hc_project_root,
    launch::CORGI_REQUEST_TOKEN,
    merge::{
        MergeError, Upstream, abort_merge, conflict_item, merge_form, project_supervisor,
        run_merge, tell_supervisor_about_merge,
    },
};

/// The version of `corgi status --json`'s shape: fields may be added under
/// it, and a rename or removal bumps it. Version 2 reads hc alone: the
/// Herdr TUI's `herdr` object gave way to `hc`.
pub const STATUS_VERSION: u32 = 2;

pub const STATUS_USAGE: &str = "\
Usage: corgi status --json [--project DIR]

Prints one JSON object describing every project with a supervisor state
directory or a running Corgi agent on herdr-core (hc), the Herdr app's backend
(only DIR's project with --project): its supervisor, its agents (the supervisor
included, each with its role), the open questions its supervisor's ledger
keeps, the inbox items still waiting, ledger counts and the backlog's length.
Agents on the Herdr TUI are not listed. hc is asked only when its core already
runs, since asking it would start one: when it does not, the projects still
come from the state directories, without agents, and \"hc\".\"running\" is
false. \"daemon\" says whether corgi daemon runs for the core (\"running\",
\"pid\") and whether its service is installed (\"installed\"). It changes
nothing.";

pub const MERGE_USAGE: &str = "\
Usage: corgi merge NAME --project DIR [--json] [--pull | --skip-upstream]

Merges the branch of worker NAME of DIR's project on herdr-core (hc), from its
linked worktree, into the branch checked out in the project's primary checkout,
and pushes it: the merge the dashboard's m runs, with its checks, and clears
the worker's merge tag. The base branch's upstream is fetched first, and the
merge is refused when the upstream has commits the base lacks; --pull pulls
them in first, and --skip-upstream merges without fetching (after a failed
fetch). The project's supervisor is told of a merge, and of a failed push, in
its inbox. A merge that stops on conflicts is aborted and handed to the
project's running supervisor, as the dashboard's s does; without one it is left
in the primary checkout to resolve.

With --json it prints {\"result\", \"message\", \"commit\", \"reason\"}: result
is merged, conflicts, push-failed or refused; reason says why a refusal was
(unavailable, behind, fetch-failed, failed, no-agent). It exits 0 only when
the branch was merged and pushed. A core that is not running is not started:
the merge is refused as unavailable.";

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

/// What `corgi status` and `corgi merge` see of hc.
pub(crate) struct HcSeen {
    /// The client, once the core runs and an `hc` was found.
    pub(crate) client: Option<HcClient>,
    /// Corgi's agents on it, in the dashboard's order.
    pub(crate) agents: Vec<DashboardAgent>,
    /// Whether the core's socket accepts connections.
    pub(crate) running: bool,
    /// Why no agents could be read from a running core.
    pub(crate) error: Option<String>,
}

impl HcSeen {
    /// hc as this environment finds it, asked only when its core runs.
    fn look() -> Self {
        let mut seen = Self {
            client: None,
            agents: Vec::new(),
            running: hc::core_running(),
            error: None,
        };
        if !seen.running {
            return seen;
        }
        let Some(client) = HcClient::locate() else {
            seen.error =
                Some("hc (herdr-core) is not installed: no hc on PATH or in ~/.local/bin".into());
            return seen;
        };
        match hc::agents::agents(&client) {
            Ok(agents) => {
                seen.agents = agents;
                seen.client = Some(client);
            }
            Err(error) => seen.error = Some(format!("{error:#}")),
        }
        seen
    }

    /// Why a merge cannot be run on hc, if it cannot.
    fn unavailable(&self) -> Option<String> {
        match (&self.client, &self.error) {
            (Some(_), _) => None,
            (None, Some(error)) => Some(format!("herdr-core could not be asked: {error}")),
            (None, None) => {
                Some("herdr-core (hc) is not running; Corgi does not start it for a merge".into())
            }
        }
    }
}

/// `root` without symlinks, as hc's `corgi.project` labels hold a project, so
/// a remembered path and a label compare as text.
fn canonical_root(root: &str) -> String {
    fs::canonicalize(root)
        .map(|root| root.to_string_lossy().into_owned())
        .unwrap_or_else(|_| root.to_string())
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
    let only = project.map(|dir| hc_project_root(&dir));
    let seen = HcSeen::look();
    let remembered: Vec<String> = ProjectMemory::load()
        .roots()
        .iter()
        .map(|root| canonical_root(root))
        .collect();
    let mut value = status_value(
        &seen.agents,
        &remembered,
        &Inboxes::default(),
        (seen.running, seen.error.as_deref()),
        only.as_deref(),
        unix_now(),
    );
    // Whether supervisors on hc are woken while the app is closed.
    value["daemon"] = super::daemon::status_value();
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}

/// The object `corgi status --json` prints for `agents` (Corgi's rows on
/// hc, in the dashboard's order), the projects Corgi remembers
/// (`remembered`), of which those that had a supervisor are listed even with
/// no agent running, and the supervisors' state in `inboxes`. `hc` is
/// whether its core runs, and why its agents could not be read; `only` keeps
/// one project.
pub(crate) fn status_value(
    agents: &[DashboardAgent],
    remembered: &[String],
    inboxes: &Inboxes,
    hc: (bool, Option<&str>),
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
        "hc": {"running": hc.0, "error": hc.1},
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
    let root = hc_project_root(&project);
    let mut seen = HcSeen::look();
    let outcome = match (seen.unavailable(), &seen.client) {
        (None, Some(client)) => merge_agent(
            client,
            &mut seen.agents,
            &Inboxes::default(),
            &name,
            &root,
            upstream,
        ),
        (unavailable, _) => MergeOutcome::refused("unavailable", unavailable.unwrap_or_default()),
    };
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

/// Merges the branch of the worker `name` of the project at `root` among
/// `agents` (Corgi's rows on hc), as the dashboard's merge dialog does once
/// confirmed, with `upstream` choosing what happens to the base branch's
/// upstream: clears its merge tag on `client` and tells the supervisor
/// through `inboxes` once it lands, and hands a conflict to the running
/// supervisor.
pub(crate) fn merge_agent(
    client: &HcClient,
    agents: &mut [DashboardAgent],
    inboxes: &Inboxes,
    name: &str,
    root: &str,
    upstream: fn(&Upstream) -> Upstream,
) -> MergeOutcome {
    let Some(index) = agents.iter().position(|agent| {
        agent.project_root == root
            && !agent.supervisor
            && (agent.info.display_name() == name || agent.info.name.as_deref() == Some(name))
    }) else {
        return MergeOutcome::refused("no-agent", format!("No worker {name} runs in {root}"));
    };
    let agent = &agents[index];
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
            let mut message = format!("Merged and pushed {what}");
            clear_tag(client, &mut agents[index], &mut message);
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
            let mut message = format!("Merged {what}, but the push failed: {error}");
            clear_tag(client, &mut agents[index], &mut message);
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

/// Clears the merge tag of the merged `agent` on hc, as `corgi tag --clear`
/// does, adding to `message` when that fails.
fn clear_tag(client: &HcClient, agent: &mut DashboardAgent, message: &mut String) {
    if let Err(error) = hc::agents::clear_merge_tag(client, agent) {
        message.push_str(&format!("; its merge tag could not be cleared: {error:#}"));
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
    use corgi_core::hc::{
        PROJECT_LABEL, REQUEST_LABEL, ROLE_LABEL, TAG_AT_LABEL, TAG_LABEL, TASK_LABEL, Terminal,
        agents::dashboard_agents,
    };

    /// A fake `hc` in `dir` that records each call's arguments, one call a
    /// line, in `dir/calls`, and answers every call with a terminal record.
    fn fake_hc(dir: &Path) -> HcClient {
        fs::create_dir_all(dir).unwrap();
        let script = dir.join("hc");
        fs::write(
            &script,
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$(dirname \"$0\")/calls\"\necho '{\"address\":\"t9\"}'\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        fs::set_permissions(&script, permissions).unwrap();
        HcClient::new(script)
    }

    /// The calls the fake `hc` in `dir` got.
    fn hc_calls(dir: &Path) -> Vec<String> {
        fs::read_to_string(dir.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// An hc terminal record as `hc get` gives it.
    fn terminal(
        address: &str,
        name: &str,
        cwd: &Path,
        status: &str,
        turn: u64,
        labels: &[(&str, &str)],
    ) -> Terminal {
        let labels: std::collections::BTreeMap<&str, &str> = labels.iter().copied().collect();
        serde_json::from_value(json!({
            "address": address,
            "name": name,
            "state": "live",
            "status": {"state": status, "app": "claude-code"},
            "labels": labels,
            "cwd": cwd,
            "foreground": {"name": "claude", "cwd": cwd},
            "turn": turn,
        }))
        .unwrap()
    }

    #[test]
    fn status_lists_corgis_hc_terminals_as_the_projects_agents() {
        let scratch = ScratchDir::new("status-hc");
        let base = scratch.join("state");
        let app = git_repo(&scratch.join("app"));
        let fix = git_worktree(&app, &scratch.join("fix"), "worktree/fix");
        let busy = git_worktree(&app, &scratch.join("busy"), "worktree/busy");
        let root = app.to_string_lossy().into_owned();
        supervisor_state(&base, &app);
        let inboxes = corgi_core::waker::SupervisorWaker::in_dir(&base).inboxes();
        let project = |role| [(PROJECT_LABEL, root.as_str()), (ROLE_LABEL, role)];
        let terminals = [
            // A worker tagged where it rests now: ready to merge.
            terminal(
                "t2",
                "w-fix",
                &fix,
                "done",
                3,
                &[
                    project("worker")[0],
                    project("worker")[1],
                    (REQUEST_LABEL, "20261109-fix"),
                    (TASK_LABEL, "Fix it"),
                    (TAG_LABEL, "merge"),
                    (TAG_AT_LABEL, &merge_tag_value(AgentState::Done, 3)),
                ],
            ),
            // A worker that has worked past its tag: no MERGE shows.
            terminal(
                "t3",
                "w-busy",
                &busy,
                "working",
                5,
                &[
                    project("worker")[0],
                    project("worker")[1],
                    (TAG_LABEL, "merge"),
                    (TAG_AT_LABEL, &merge_tag_value(AgentState::Done, 2)),
                ],
            ),
            terminal(
                "t1",
                "app-supervisor",
                &app,
                "idle",
                7,
                &project("supervisor"),
            ),
        ];
        let home = scratch.join("home");
        let agents = dashboard_agents(&terminals, Some(&home));

        let value = status_value(&agents, &[], &inboxes, (true, None), None, NOW);
        let projects = value["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1, "{value:#}");
        let project = &projects[0];
        assert_eq!(project["dir"], root);
        assert_eq!(project["supervisor"]["name"], "app-supervisor");
        assert_eq!(project["supervisor"]["terminal"], "t1");
        assert_eq!(project["supervisor"]["state"], "idle");
        let rows = project["agents"].as_array().unwrap();
        let names: Vec<&str> = rows.iter().map(|a| a["name"].as_str().unwrap()).collect();
        assert_eq!(names[0], "app-supervisor", "{names:?}");
        let row = |name: &str| rows.iter().find(|a| a["name"] == name).unwrap().clone();
        let fixed = row("w-fix");
        assert_eq!(fixed["role"], "worker");
        assert_eq!(fixed["state"], "done");
        assert_eq!(fixed["terminal"], "t2");
        assert_eq!(fixed["request_id"], "20261109-fix");
        assert_eq!(fixed["harness"], "agent");
        assert_eq!(fixed["branch"], "worktree/fix");
        assert_eq!(fixed["worktree"], fix.to_string_lossy().as_ref());
        assert_eq!(fixed["tag"], "merge");
        let busy_row = row("w-busy");
        assert_eq!(busy_row["state"], "working");
        assert_eq!(busy_row["tag"], json!(null));
        assert_eq!(busy_row["terminal"], "t3");
    }

    #[test]
    fn without_a_running_core_the_projects_still_come_from_the_state_directories() {
        let scratch = ScratchDir::new("status-hc-down");
        let base = scratch.join("state");
        let app = git_repo(&scratch.join("app"));
        supervisor_state(&base, &app);
        let inboxes = corgi_core::waker::SupervisorWaker::in_dir(&base).inboxes();
        let remembered = [app.to_string_lossy().into_owned()];
        let value = status_value(&[], &remembered, &inboxes, (false, None), None, NOW);
        assert_eq!(value["hc"], json!({"running": false, "error": null}));
        assert_eq!(value["projects"][0]["name"], "app");
        assert_eq!(value["projects"][0]["supervisor"], json!(null));
        assert_eq!(value["projects"][0]["agents"], json!([]));
    }

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
            (true, None),
            None,
            NOW,
        );
        assert_eq!(value["version"], 2);
        assert_eq!(value["generated"], "2027-01-15T08:00:00Z");
        assert_eq!(value["hc"], json!({"running": true, "error": null}));
        assert_eq!(value.get("herdr"), None);
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
            (false, None),
            Some(&idle.to_string_lossy()),
            NOW,
        );
        assert_eq!(only["hc"]["running"], false);
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

        /// The folder of the fake `hc` the merges label terminals with.
        fn hc_dir(&self) -> PathBuf {
            self.state.with_file_name("hc")
        }

        fn merge(
            &self,
            agents: &mut [DashboardAgent],
            upstream: fn(&Upstream) -> Upstream,
        ) -> MergeOutcome {
            let client = fake_hc(&self.hc_dir());
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
    fn merging_a_tagged_hc_worker_clears_its_tag_on_its_terminal() {
        let project = Mergeable::new("status-merge-hc");
        supervisor_state(&project.state, &project.repo);
        let mut worker = project.worker();
        worker.info.pane_id = "t4".into();
        worker.info.state = AgentState::Done;
        worker
            .info
            .tokens
            .insert(MERGE_TOKEN.into(), merge_tag_value(AgentState::Done, 0));
        let mut agents = vec![agent("app-supervisor", &project.repo, true), worker];
        let outcome = project.merge(&mut agents, Upstream::checked);
        assert_eq!(outcome.result, "merged", "{}", outcome.message);
        assert_eq!(outcome.message, "Merged and pushed worktree/fix into main");
        assert_eq!(project.head("origin/main"), project.head("main"));
        assert_eq!(
            hc_calls(&project.hc_dir()),
            ["label t4 corgi.tag= corgi.tag.at= --json"]
        );
        assert!(!agents[1].info.tokens.contains_key(MERGE_TOKEN));
        assert_eq!(project.pending().len(), 1);

        // An untagged worker's terminal is left alone.
        let project = Mergeable::new("status-merge-hc-untagged");
        let mut agents = vec![project.worker()];
        let outcome = project.merge(&mut agents, Upstream::checked);
        assert_eq!(outcome.result, "merged", "{}", outcome.message);
        assert!(hc_calls(&project.hc_dir()).is_empty());
    }

    #[test]
    fn a_tag_that_cannot_be_cleared_is_said_after_the_merge() {
        let project = Mergeable::new("status-merge-hc-label");
        let mut worker = project.worker();
        worker
            .info
            .tokens
            .insert(MERGE_TOKEN.into(), merge_tag_value(AgentState::Done, 0));
        let mut agents = vec![worker];
        let client = HcClient::new(project.state.join("no-hc"));
        let outcome = merge_agent(
            &client,
            &mut agents,
            &corgi_core::waker::SupervisorWaker::in_dir(&project.state).inboxes(),
            "w-fix",
            &project.root(),
            Upstream::checked,
        );
        assert_eq!(outcome.result, "merged", "{}", outcome.message);
        assert!(
            outcome.message.contains("merge tag could not be cleared"),
            "{}",
            outcome.message
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
