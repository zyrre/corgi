//! Find orphaned sessions: agents in Herdr that Corgi gets wrong, and what
//! can be done about each (docs/orphaned-sessions.md is the design).
//!
//! [`detect`] is one read-only pass over Herdr's snapshot, the panes'
//! foreground processes, Git and the supervisors' files. It lists each
//! candidate with its evidence and the actions that evidence allows, and
//! writes nothing, so the dashboard's `o`, the window's button and
//! `corgi orphans` all run it whenever asked. [`apply`] does what the user
//! chose for one candidate, after detecting again so a pane whose occupant
//! changed since the list was read is refused. [`retag_proven`] is the part
//! that needs no asking: the hooks' pass (and a dashboard's) puts back the
//! two marks Corgi can prove from evidence it wrote itself.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::{
    close::{CloseTarget, Closed, close, close_form},
    digest::{LedgerEntry, parse_ledger},
    git::git_current_branch,
    herdr::{ForegroundProcess, HerdrClient, PaneInfo, SessionSnapshot},
    launch::CORGI_REQUEST_TOKEN,
    markers::{self, corgi_pane_tokens},
    model::{AgentInfo, DashboardAgent, WorkspaceInfo},
    paths::{dir_name, is_home},
    project_main::{dashboard_pane_ids, duplicate_project_mains},
    refresh::{Inputs, RefreshState},
    repo::{RepoCache, agent_repo, repo_of_dir},
    rows::{has_real_agent_session, is_corgi_workspace},
    supervisor::{self, ROLE_FILE, SUPERVISOR_TOKEN, is_supervisor, native_session},
    waker::Inboxes,
};

/// The file in Corgi's state directory that keeps the candidates the user
/// chose to leave, by their evidence, so they are not offered again until
/// it changes.
pub const LEFT_FILE: &str = "orphans-left.json";

/// How many left candidates the file keeps, the newest.
const LEFT_KEPT: usize = 200;

/// The agent CLIs whose process in a pane's foreground makes it an agent.
const AGENT_CLIS: [&str; 2] = ["claude", "codex"];

/// Script runners an agent CLI may run under, as `node …/claude-code/cli.js`.
const RUNNERS: [&str; 3] = ["node", "bun", "deno"];

/// What is wrong with a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    /// Herdr lists the pane as an agent, but Corgi shows no row: no session.
    Hidden,
    /// An agent CLI runs in the pane, but Herdr does not list it as an agent.
    NotDetected,
    /// A supervisor's process that is not marked as the supervisor.
    Supervisor,
    /// A worker whose request id is gone, which its supervisor's ledger
    /// still names by checkout and branch.
    Worker,
    /// An agent in a directory that is no repository and not home.
    NoProject,
    /// An empty duplicate of a project's Corgi workspace.
    Duplicate,
}

impl Cause {
    /// A few words for the list.
    pub fn label(self) -> &'static str {
        match self {
            Self::Hidden => "not shown",
            Self::NotDetected => "not detected",
            Self::Supervisor => "supervisor, unmarked",
            Self::Worker => "worker, untracked",
            Self::NoProject => "no project",
            Self::Duplicate => "duplicate workspace",
        }
    }
}

/// How sure the evidence makes a suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Evidence Corgi wrote itself for this very process or session.
    Certain,
    /// A strong match that a user's own unusual setup could fake.
    Likely,
    /// A hint worth showing, not acting on.
    Guess,
}

impl Confidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::Certain => "certain",
            Self::Likely => "likely",
            Self::Guess => "guess",
        }
    }
}

/// What the user can choose for a candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Mark it as the project's supervisor, bound to its session.
    Supervisor {
        project: String,
        project_root: String,
    },
    /// Give it back the request id of its ledger entry.
    Worker {
        project: String,
        project_root: String,
        request_id: String,
        /// The name the entry knows the worker by.
        agent: String,
    },
    /// Close it: `target` is `pane`, `tab`, `workspace` or `worktree`, as
    /// the close dialog says.
    Close { target: String },
    /// Nothing; not offered again until its evidence changes.
    Leave,
}

impl Action {
    /// The key that picks the action in the dialogs.
    pub fn key(&self) -> char {
        match self {
            Self::Supervisor { .. } => 's',
            Self::Worker { .. } => 'w',
            Self::Close { .. } => 'x',
            Self::Leave => 'l',
        }
    }

    /// What the dialogs call it.
    pub fn label(&self) -> String {
        match self {
            Self::Supervisor { project, .. } => format!("supervisor of {project}"),
            Self::Worker { project, agent, .. } => format!("worker {agent} of {project}"),
            Self::Close { target } if target == "worktree" => "close and remove worktree".into(),
            Self::Close { target } => format!("close {target}"),
            Self::Leave => "leave".into(),
        }
    }
}

/// One orphaned session, or a leftover workspace, with its evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// `pane:<pane id>` or `workspace:<workspace id>`: what [`apply`] names.
    pub key: String,
    pub cause: Cause,
    pub confidence: Confidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    pub workspace_id: String,
    /// The workspace's label, for where to look.
    pub workspace: String,
    /// `claude`, `codex`, or what Herdr calls it; empty for a workspace.
    pub harness: String,
    /// The agent's name, or the workspace's label.
    pub name: String,
    pub cwd: String,
    /// One line: what is wrong.
    pub summary: String,
    /// What the detector saw, a line each.
    pub evidence: Vec<String>,
    /// What the user can run to fix the cause, when it is outside Corgi.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    /// What the evidence allows; never empty (leave is always one).
    pub actions: Vec<Action>,
    /// The index in `actions` of the one the dialogs preselect.
    pub suggested: usize,
    /// The evidence in short, which [`apply`] checks is unchanged and
    /// leaving a candidate keeps.
    pub fingerprint: String,
}

/// Where a pass looks besides Herdr.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The pane of the dashboard asking, which is never a candidate.
    pub dashboard_pane_id: Option<String>,
    /// Where each project's supervisor keeps its files.
    pub state: Inboxes,
    /// The file of left candidates; `None` keeps none and hides none.
    pub left_file: Option<PathBuf>,
}

impl Options {
    /// Corgi's own state directory, for the dashboard in `dashboard_pane_id`.
    pub fn new(dashboard_pane_id: Option<String>) -> Self {
        Self::in_state(dashboard_pane_id, Inboxes::default())
    }

    /// The supervisors' files and the left candidates where `state` keeps
    /// them, which is Corgi's state directory but in tests.
    pub fn in_state(dashboard_pane_id: Option<String>, state: Inboxes) -> Self {
        Self {
            dashboard_pane_id,
            left_file: state.state_file(LEFT_FILE),
            state,
        }
    }
}

/// The candidates on `client`'s Herdr, without those the user left whose
/// evidence has not changed since. Reads only.
pub fn detect(client: &HerdrClient, options: &Options) -> Result<Vec<Candidate>> {
    let (snapshot, rows) = rows_now(client, options)?;
    let left = options
        .left_file
        .as_deref()
        .map(read_left)
        .unwrap_or_default();
    Ok(detect_in(client, &snapshot, &rows, options)
        .into_iter()
        .filter(|candidate| !left.contains(&candidate.fingerprint))
        .collect())
}

/// Herdr's snapshot and the dashboard's rows of it.
fn rows_now(
    client: &HerdrClient,
    options: &Options,
) -> Result<(SessionSnapshot, Vec<DashboardAgent>)> {
    let snapshot = client.snapshot().context("read Herdr's session")?;
    let mut inputs = Inputs::new(client.clone());
    inputs.dashboard_pane_id = options.dashboard_pane_id.clone();
    let view = RefreshState::default().view(&inputs, snapshot.clone());
    Ok((snapshot, view.agents))
}

/// Does what the user chose for the candidate `key`, after detecting again:
/// refused when it is no longer a candidate, its `fingerprint` changed (its
/// pane holds another session or process now), or `action` is not one it
/// offers. Returns the status line.
pub fn apply(
    client: &HerdrClient,
    options: &Options,
    key: &str,
    fingerprint: &str,
    action: &Action,
) -> Result<String> {
    let (snapshot, rows) = rows_now(client, options)?;
    let candidates = detect_in(client, &snapshot, &rows, options);
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.key == key)
        .ok_or_else(|| anyhow!("That session is no longer an orphan; look again"))?;
    if candidate.fingerprint != fingerprint {
        bail!(
            "{} changed since the list was read (another session or process); look again",
            candidate.name
        );
    }
    if !candidate.actions.contains(action) {
        bail!("{} cannot be made {}", candidate.name, action.label());
    }
    match action {
        Action::Leave => {
            let path = options
                .left_file
                .as_deref()
                .context("no state directory to keep it in")?;
            leave(path, &candidate.fingerprint)?;
            Ok(format!("Left {} as it is", candidate.name))
        }
        Action::Supervisor { project, .. } => {
            let info = agent_info(&snapshot, candidate)?;
            let session = native_session(info).context("it has no session to bind the mark to")?;
            let mut tokens = corgi_pane_tokens(&info.tokens);
            tokens.retain(|(key, _)| *key != SUPERVISOR_TOKEN);
            tokens.push((SUPERVISOR_TOKEN, &candidate.name));
            markers::mark_pane(
                client,
                &info.pane_id,
                &candidate.name,
                Some(session),
                &tokens,
            )?;
            Ok(format!(
                "Marked {} as {project}'s supervisor",
                candidate.name
            ))
        }
        Action::Worker {
            project,
            request_id,
            ..
        } => {
            let info = agent_info(&snapshot, candidate)?;
            let mut tokens = corgi_pane_tokens(&info.tokens);
            tokens.retain(|(key, _)| *key != CORGI_REQUEST_TOKEN);
            tokens.push((CORGI_REQUEST_TOKEN, request_id));
            markers::mark_pane(
                client,
                &info.pane_id,
                &candidate.name,
                native_session(info),
                &tokens,
            )?;
            Ok(format!(
                "{} is {project}'s worker {request_id} again",
                candidate.name
            ))
        }
        Action::Close { .. } if candidate.cause == Cause::Duplicate => {
            client
                .close_workspace(&candidate.workspace_id)
                .context("close the duplicate workspace")?;
            Ok(format!("Closed duplicate workspace {}", candidate.name))
        }
        Action::Close { .. } => {
            let agent = closable(&snapshot, &rows, candidate)?;
            let form = close_form(&agent, &snapshot.workspaces, &rows);
            match close(client, options.dashboard_pane_id.as_deref(), form) {
                Closed::Done(status) => Ok(status),
                Closed::Dirty(_) => bail!(
                    "{}'s checkout has uncommitted changes; close it from the dashboard to discard them",
                    candidate.name
                ),
                Closed::Failed(error) => Err(anyhow!(error)),
            }
        }
    }
}

/// The snapshot's agent of a pane candidate.
fn agent_info<'a>(snapshot: &'a SessionSnapshot, candidate: &Candidate) -> Result<&'a AgentInfo> {
    let pane = candidate.pane_id.as_deref().context("not a pane")?;
    snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == pane)
        .context("Herdr no longer lists it as an agent")
}

/// The row `close` takes for a pane candidate: its own, or one built from
/// the snapshot for a pane Corgi shows no row of. A built one names no
/// checkout, so closing it never removes one.
fn closable(
    snapshot: &SessionSnapshot,
    rows: &[DashboardAgent],
    candidate: &Candidate,
) -> Result<DashboardAgent> {
    let pane = candidate.pane_id.as_deref().context("not a pane")?;
    if let Some(row) = rows.iter().find(|row| row.info.pane_id == pane) {
        return Ok(row.clone());
    }
    let info = match snapshot.agents.iter().find(|agent| agent.pane_id == pane) {
        Some(info) => info.clone(),
        None => pane_as_agent(
            snapshot
                .panes
                .iter()
                .find(|other| other.pane_id == pane)
                .context("the pane is gone")?,
        ),
    };
    Ok(DashboardAgent {
        project: candidate.name.clone(),
        project_root: info.cwd().to_string(),
        info,
        ..DashboardAgent::default()
    })
}

/// A pane Herdr does not list as an agent, as one, for closing it.
fn pane_as_agent(pane: &PaneInfo) -> AgentInfo {
    AgentInfo {
        pane_id: pane.pane_id.clone(),
        workspace_id: pane.workspace_id.clone(),
        tab_id: pane.tab_id.clone(),
        cwd: pane.cwd.clone(),
        foreground_cwd: pane.foreground_cwd.clone(),
        name: pane.label.clone(),
        tokens: pane.tokens.clone(),
        ..AgentInfo::default()
    }
}

/// The candidates of `snapshot`, whose rows are `rows`, left ones included.
pub fn detect_in(
    client: &HerdrClient,
    snapshot: &SessionSnapshot,
    rows: &[DashboardAgent],
    options: &Options,
) -> Vec<Candidate> {
    let dashboards = dashboard_pane_ids(options.dashboard_pane_id.as_deref(), &snapshot.panes);
    let workspaces: HashMap<&str, &WorkspaceInfo> = snapshot
        .workspaces
        .iter()
        .map(|workspace| (workspace.workspace_id.as_str(), workspace))
        .collect();
    let label = |workspace_id: &str| {
        workspaces
            .get(workspace_id)
            .map(|workspace| workspace.label.clone())
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| workspace_id.to_string())
    };
    let shown: HashSet<&str> = rows.iter().map(|row| row.info.pane_id.as_str()).collect();
    let listed: HashSet<&str> = snapshot
        .agents
        .iter()
        .map(|agent| agent.pane_id.as_str())
        .collect();
    let mut probe = Probe::new(client);
    let mut found = Vec::new();

    // Hidden (cause 1): Herdr lists it, Corgi drops it.
    for info in &snapshot.agents {
        let pane = info.pane_id.as_str();
        if shown.contains(pane) || dashboards.contains(pane) {
            continue;
        }
        // Without an agent CLI in the foreground, Herdr's detector was wrong.
        let Some(process) = probe.agent_cli(pane) else {
            continue;
        };
        let program = process.program().to_string();
        let mut close_target = "pane".to_string();
        if let Ok(agent) = closable(snapshot, rows, &pane_candidate(info, &program)) {
            close_target = target_name(&close_form(&agent, &snapshot.workspaces, rows).target);
        }
        found.push(Candidate {
            key: format!("pane:{pane}"),
            cause: Cause::Hidden,
            confidence: Confidence::Likely,
            pane_id: Some(pane.to_string()),
            workspace_id: info.workspace_id.clone(),
            workspace: label(&info.workspace_id),
            harness: program.clone(),
            name: info.display_name().to_string(),
            cwd: info.cwd().to_string(),
            summary: "Not shown: Herdr gives it no session".into(),
            evidence: vec![
                ran(&process),
                format!(
                    "Herdr lists it as {} but gives it no session, and Corgi set no mark on it",
                    info.kind()
                ),
            ],
            fix: Some(integration_fix(&program)),
            actions: vec![
                Action::Leave,
                Action::Close {
                    target: close_target,
                },
            ],
            suggested: 0,
            fingerprint: format!("hidden {pane} {}", pid(&process)),
        });
    }

    // Not detected (cause 2): an agent CLI in a pane Herdr lists no agent in.
    for pane in &snapshot.panes {
        let id = pane.pane_id.as_str();
        if listed.contains(id) || dashboards.contains(id) {
            continue;
        }
        let Some((process, confidence)) = probe.agent_cli_or_runner(id) else {
            continue;
        };
        let info = pane_as_agent(pane);
        let program = cli_named(&process).unwrap_or("agent").to_string();
        let agent = DashboardAgent {
            project_root: info.cwd().to_string(),
            info: info.clone(),
            ..DashboardAgent::default()
        };
        let close_target = target_name(&close_form(&agent, &snapshot.workspaces, rows).target);
        let cwd = Some(process.cwd.clone())
            .filter(|cwd| !cwd.is_empty())
            .unwrap_or_else(|| info.cwd().to_string());
        found.push(Candidate {
            key: format!("pane:{id}"),
            cause: Cause::NotDetected,
            confidence,
            pane_id: Some(id.to_string()),
            workspace_id: pane.workspace_id.clone(),
            workspace: label(&pane.workspace_id),
            harness: program,
            name: pane.label.clone().unwrap_or_else(|| id.to_string()),
            cwd,
            summary: "Not detected: Herdr does not list it as an agent".into(),
            evidence: vec![
                ran(&process),
                "Herdr does not list the pane as an agent, so neither does Corgi".into(),
            ],
            fix: Some(format!(
                "`herdr agent explain {id}` says why Herdr missed it"
            )),
            actions: vec![
                Action::Leave,
                Action::Close {
                    target: close_target,
                },
            ],
            suggested: 0,
            fingerprint: format!("undetected {id} {}", pid(&process)),
        });
    }

    // Supervisors, workers and agents in no project (causes 3–6, 9), among
    // the rows.
    let seats: Vec<Seat> = rows.iter().map(Seat::of_row).collect();
    let sightings = supervisor_sightings(&seats, &options.state, &mut probe, |_| true);
    let taken = request_ids(&snapshot.agents);
    let mut ledgers = Ledgers::default();
    for (index, row) in rows.iter().enumerate() {
        if row.supervisor || row.scratch {
            continue;
        }
        let seat = &seats[index];
        let close_target = target_name(&close_form(row, &snapshot.workspaces, rows).target);
        let base = |cause: Cause, confidence: Confidence, summary: &str| Candidate {
            key: format!("pane:{}", row.info.pane_id),
            cause,
            confidence,
            pane_id: Some(row.info.pane_id.clone()),
            workspace_id: row.info.workspace_id.clone(),
            workspace: label(&row.info.workspace_id),
            harness: row.info.kind().to_string(),
            name: row.info.display_name().to_string(),
            cwd: row.info.cwd().to_string(),
            summary: summary.into(),
            evidence: Vec::new(),
            fix: None,
            actions: Vec::new(),
            suggested: 0,
            fingerprint: String::new(),
        };
        let close_action = Action::Close {
            target: close_target,
        };
        if let Some(sighting) = sightings.iter().find(|sighting| sighting.seat == index) {
            let verdict = supervisor_verdict(&seats, &sightings, sighting);
            let project = project_name(&seat.root);
            let mut candidate = base(
                Cause::Supervisor,
                verdict.confidence,
                "Supervisor without its mark",
            );
            candidate.evidence.push(format!(
                "Its process was started with {}, the role of {project}'s supervisor",
                sighting.dir.join(ROLE_FILE).display()
            ));
            candidate.evidence.push(
                "It is not marked as the supervisor: the mark goes when the session changes \
                 (/clear, /resume) or is lost in a restart"
                    .into(),
            );
            candidate.evidence.extend(verdict.notes);
            if verdict.offer {
                candidate.actions.push(Action::Supervisor {
                    project,
                    project_root: seat.root.clone(),
                });
            }
            candidate.actions.extend([Action::Leave, close_action]);
            candidate.fingerprint = format!(
                "supervisor {} {}",
                row.info.pane_id,
                native_session(&row.info).unwrap_or("-")
            );
            found.push(candidate);
            continue;
        }
        if let Some(dir) = options.state.dir(&seat.root).filter(|dir| dir.is_dir()) {
            if row.info.tokens.contains_key(CORGI_REQUEST_TOKEN) {
                continue;
            }
            let entries = ledgers.get(&dir);
            let Some(found_worker) = worker_match(&seats, index, entries, &taken) else {
                continue;
            };
            let project = project_name(&seat.root);
            let mut candidate = base(
                Cause::Worker,
                found_worker.confidence,
                "Worker its supervisor lost track of",
            );
            for entry in &found_worker.entries {
                candidate.evidence.push(format!(
                    "Ledger entry {} ({}, {}) names this checkout and branch {}",
                    entry.id, entry.agent, entry.status, entry.branch
                ));
            }
            candidate.evidence.push(
                "It has no request id, so the supervisor's digest and `corgi tag` miss it".into(),
            );
            if found_worker.shared {
                candidate
                    .evidence
                    .push("Another agent works in the same checkout".into());
            }
            candidate
                .actions
                .extend(found_worker.entries.iter().map(|entry| Action::Worker {
                    project: project.clone(),
                    project_root: seat.root.clone(),
                    request_id: entry.id.clone(),
                    agent: entry.agent.clone(),
                }));
            let workers = candidate.actions.len();
            candidate.actions.extend([Action::Leave, close_action]);
            // Only a single, unshared match is suggested.
            if found_worker.confidence != Confidence::Certain {
                candidate.suggested = workers;
            }
            candidate.fingerprint = format!(
                "worker {} {} {}",
                row.info.pane_id,
                native_session(&row.info).unwrap_or_else(|| row.info.display_name()),
                found_worker
                    .entries
                    .iter()
                    .map(|entry| entry.id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            found.push(candidate);
            continue;
        }
        // Cause 9: no repository, not home, no supervisor, not a workspace
        // Corgi created for a new project.
        if row.worktree_checkout.is_none()
            && !workspaces
                .get(row.info.workspace_id.as_str())
                .is_some_and(|workspace| is_corgi_workspace(workspace))
            && !is_home(row.info.cwd())
            && repo_of_dir(Path::new(row.info.cwd())).is_none()
        {
            let mut candidate = base(Cause::NoProject, Confidence::Guess, "In no repository");
            candidate.evidence = vec![
                format!(
                    "{} is in no Git repository and is not the home directory",
                    row.info.cwd()
                ),
                "No supervisor has kept files for it, so it is a heading of its own".into(),
            ];
            candidate.actions = vec![Action::Leave, close_action];
            candidate.fingerprint = format!(
                "noproject {} {} {}",
                row.info.pane_id,
                native_session(&row.info).unwrap_or_else(|| row.info.display_name()),
                row.info.cwd()
            );
            found.push(candidate);
        }
    }

    // Leftover workspaces (cause 11).
    for (workspace, root) in duplicate_project_mains(snapshot, options.dashboard_pane_id.as_deref())
    {
        found.push(Candidate {
            key: format!("workspace:{}", workspace.workspace_id),
            cause: Cause::Duplicate,
            confidence: Confidence::Certain,
            pane_id: None,
            workspace_id: workspace.workspace_id.clone(),
            workspace: label(&workspace.workspace_id),
            harness: String::new(),
            name: label(&workspace.workspace_id),
            cwd: root.to_string(),
            summary: "Empty duplicate project workspace".into(),
            evidence: vec![
                format!(
                    "A second Corgi workspace of {}, holding no agent and only its root tab",
                    project_name(root)
                ),
                "Left by a launch while Herdr had lost Corgi's marks".into(),
            ],
            fix: None,
            actions: vec![
                Action::Close {
                    target: "workspace".into(),
                },
                Action::Leave,
            ],
            suggested: 0,
            fingerprint: format!("duplicate {}", workspace.workspace_id),
        });
    }
    found
}

/// A stand-in candidate for a hidden pane, to build its close form.
fn pane_candidate(info: &AgentInfo, program: &str) -> Candidate {
    Candidate {
        key: format!("pane:{}", info.pane_id),
        cause: Cause::Hidden,
        confidence: Confidence::Likely,
        pane_id: Some(info.pane_id.clone()),
        workspace_id: info.workspace_id.clone(),
        workspace: String::new(),
        harness: program.into(),
        name: info.display_name().into(),
        cwd: info.cwd().into(),
        summary: String::new(),
        evidence: Vec::new(),
        fix: None,
        actions: vec![Action::Leave],
        suggested: 0,
        fingerprint: String::new(),
    }
}

/// How the close dialog names a close target.
fn target_name(target: &CloseTarget) -> String {
    match target {
        CloseTarget::Worktree { .. } => "worktree",
        CloseTarget::Tab { .. } => "tab",
        CloseTarget::Workspace => "workspace",
        CloseTarget::Pane => "pane",
    }
    .into()
}

/// What to run so Herdr learns the sessions of `program`.
fn integration_fix(program: &str) -> String {
    match program {
        "codex" => "Run `herdr integration install codex` so Herdr learns Codex sessions".into(),
        "claude" => "Herdr learns a Claude session at its first hook report; if it never does, \
                     run `herdr integration install claude`"
            .into(),
        other => format!("Herdr has no session integration for {other}"),
    }
}

/// The evidence line of a process.
fn ran(process: &ForegroundProcess) -> String {
    let program = cli_named(process).unwrap_or(process.program());
    match process.pid {
        Some(pid) => format!(
            "{program} runs in the foreground at {} (pid {pid})",
            process.cwd
        ),
        None => format!("{program} runs in the foreground at {}", process.cwd),
    }
}

fn pid(process: &ForegroundProcess) -> String {
    process
        .pid
        .map_or_else(|| "-".to_string(), |pid| pid.to_string())
}

/// The agent CLI `process` runs: its program, or the script a runner runs.
fn cli_named(process: &ForegroundProcess) -> Option<&'static str> {
    let program = process.program();
    if let Some(cli) = AGENT_CLIS.iter().find(|cli| **cli == program) {
        return Some(cli);
    }
    if !RUNNERS.contains(&program) {
        return None;
    }
    let script = process.argv.get(1)?;
    if script.contains("claude-code") || script.ends_with("/claude") {
        Some("claude")
    } else if script.contains("@openai/codex") || script.ends_with("/codex") {
        Some("codex")
    } else {
        None
    }
}

/// The name a project's state directory and headings use.
fn project_name(root: &str) -> String {
    dir_name(root)
        .unwrap_or(supervisor::UNNAMED_PROJECT)
        .to_string()
}

/// The request ids live agents carry.
fn request_ids(agents: &[AgentInfo]) -> HashSet<String> {
    agents
        .iter()
        .filter_map(|agent| agent.tokens.get(CORGI_REQUEST_TOKEN))
        .filter(|id| !id.is_empty())
        .cloned()
        .collect()
}

/// Whether `process` was started with the supervisor's files in `dir`: its
/// role file, or the directory itself (`--add-dir`, both harnesses).
fn names_state_dir(process: &ForegroundProcess, dir: &Path) -> bool {
    let dir = dir.to_string_lossy();
    let dir = dir.trim_end_matches('/');
    let role = format!("{dir}/{ROLE_FILE}");
    process.argv.iter().any(|arg| {
        let arg = arg.trim_end_matches('/');
        arg == dir || arg == role
    })
}

/// Whether two spellings of a directory are the same, ignoring a trailing
/// `/`.
fn same_dir(a: &str, b: &str) -> bool {
    !a.is_empty() && a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// The panes' foreground processes, each asked once per pass.
struct Probe<'a> {
    client: &'a HerdrClient,
    asked: HashMap<String, Vec<ForegroundProcess>>,
}

impl<'a> Probe<'a> {
    fn new(client: &'a HerdrClient) -> Self {
        Self {
            client,
            asked: HashMap::new(),
        }
    }

    fn processes(&mut self, pane_id: &str) -> &[ForegroundProcess] {
        let client = self.client;
        self.asked
            .entry(pane_id.to_string())
            .or_insert_with(|| client.pane_processes(pane_id).unwrap_or_default())
    }

    /// An agent CLI in the pane's foreground, by its program's file name.
    fn agent_cli(&mut self, pane_id: &str) -> Option<ForegroundProcess> {
        self.processes(pane_id)
            .iter()
            .find(|process| AGENT_CLIS.contains(&process.program()))
            .cloned()
    }

    /// As [`Probe::agent_cli`], likely; else a script runner running one, a
    /// guess.
    fn agent_cli_or_runner(&mut self, pane_id: &str) -> Option<(ForegroundProcess, Confidence)> {
        if let Some(process) = self.agent_cli(pane_id) {
            return Some((process, Confidence::Likely));
        }
        self.processes(pane_id)
            .iter()
            .find(|process| cli_named(process).is_some())
            .map(|process| (process.clone(), Confidence::Guess))
    }

    /// The process in the pane's foreground started with the supervisor's
    /// files in `dir`.
    fn naming(&mut self, pane_id: &str, dir: &Path) -> Option<ForegroundProcess> {
        self.processes(pane_id)
            .iter()
            .find(|process| names_state_dir(process, dir))
            .cloned()
    }
}

/// An agent as the supervisor and worker rules see it.
struct Seat<'a> {
    info: &'a AgentInfo,
    /// Its project's root.
    root: String,
    supervisor: bool,
    scratch: bool,
}

impl<'a> Seat<'a> {
    fn of_row(row: &'a DashboardAgent) -> Self {
        Self {
            info: &row.info,
            root: row.project_root.clone(),
            supervisor: row.supervisor,
            scratch: row.scratch,
        }
    }
}

/// A seat whose process names a project's supervisor files.
struct Sighting {
    seat: usize,
    dir: PathBuf,
}

/// The seats that are not the supervisor but whose process names their
/// project's supervisor files, among those `consider` lets through.
fn supervisor_sightings(
    seats: &[Seat],
    state: &Inboxes,
    probe: &mut Probe,
    mut consider: impl FnMut(&Seat) -> bool,
) -> Vec<Sighting> {
    let mut dirs: HashMap<&str, Option<PathBuf>> = HashMap::new();
    let mut sightings = Vec::new();
    for (index, seat) in seats.iter().enumerate() {
        if seat.supervisor || seat.scratch || !consider(seat) {
            continue;
        }
        let Some(dir) = dirs
            .entry(seat.root.as_str())
            .or_insert_with(|| state.dir(&seat.root).filter(|dir| dir.is_dir()))
            .clone()
        else {
            continue;
        };
        if probe.naming(&seat.info.pane_id, &dir).is_some() {
            sightings.push(Sighting { seat: index, dir });
        }
    }
    sightings
}

/// What a supervisor sighting allows.
struct Verdict {
    confidence: Confidence,
    /// Marking it is offered: no running supervisor, no other process
    /// naming the same files, and a session to bind the mark to.
    offer: bool,
    notes: Vec<String>,
}

fn supervisor_verdict(seats: &[Seat], sightings: &[Sighting], sighting: &Sighting) -> Verdict {
    let seat = &seats[sighting.seat];
    let mut notes = Vec::new();
    let running = seats
        .iter()
        .find(|other| other.supervisor && other.root == seat.root);
    if let Some(running) = running {
        notes.push(format!(
            "{} is the project's supervisor now; a handover may be under way",
            running.info.display_name()
        ));
    }
    let others = sightings
        .iter()
        .filter(|other| other.seat != sighting.seat && other.dir == sighting.dir)
        .count();
    if others > 0 {
        notes.push(format!(
            "{others} other session(s) were started with the same files; a handover may be under way"
        ));
    }
    let session = native_session(seat.info).is_some();
    if !session {
        notes.push("Herdr gives it no session, so there is nothing to bind the mark to".into());
    }
    let offer = running.is_none() && others == 0 && session;
    Verdict {
        confidence: if offer {
            Confidence::Certain
        } else {
            Confidence::Likely
        },
        offer,
        notes,
    }
}

/// The ledger entries a worker matches by checkout and branch.
struct WorkerMatch {
    entries: Vec<LedgerEntry>,
    /// Another agent works in the same checkout.
    shared: bool,
    confidence: Confidence,
}

/// The unfinished ledger entries whose checkout and branch are those of the
/// agent of seat `index`, and which no live agent carries the id of. None
/// when an unfinished entry already names the agent (the digest finds it by
/// name), or none matches.
fn worker_match(
    seats: &[Seat],
    index: usize,
    entries: &[LedgerEntry],
    taken: &HashSet<String>,
) -> Option<WorkerMatch> {
    let seat = &seats[index];
    let cwd = seat.info.cwd();
    let name = seat.info.display_name();
    let unfinished: Vec<&LedgerEntry> = entries.iter().filter(|entry| !entry.finished()).collect();
    if unfinished.iter().any(|entry| entry.agent == name) {
        return None;
    }
    let by_cwd: Vec<&LedgerEntry> = unfinished
        .into_iter()
        .filter(|entry| !taken.contains(&entry.id) && same_dir(&entry.cwd, cwd))
        .collect();
    if by_cwd.is_empty() {
        return None;
    }
    let branch = git_current_branch(Path::new(cwd)).ok()?;
    let entries: Vec<LedgerEntry> = by_cwd
        .into_iter()
        .filter(|entry| entry.branch == branch)
        .cloned()
        .collect();
    if entries.is_empty() {
        return None;
    }
    let shared = seats
        .iter()
        .enumerate()
        .any(|(other, seat)| other != index && same_dir(seat.info.cwd(), cwd));
    let confidence = if entries.len() == 1 && !shared {
        Confidence::Certain
    } else {
        Confidence::Likely
    };
    Some(WorkerMatch {
        entries,
        shared,
        confidence,
    })
}

/// Supervisors' ledgers, each read once, and again only when it changed.
#[derive(Debug, Default)]
struct Ledgers {
    read: HashMap<PathBuf, (Option<Stamp>, Vec<LedgerEntry>)>,
}

/// When a file was last written and how long it was then.
type Stamp = (std::time::SystemTime, u64);

/// A mark [`retag_proven`] puts back.
struct Mark {
    pane_id: String,
    name: String,
    /// The pane's Corgi tokens with the mark among them.
    tokens: Vec<(String, String)>,
    session: Option<String>,
    /// What the status line says of it.
    said: String,
}

impl Ledgers {
    /// The newest line of each id in the ledger of the state directory
    /// `dir`; nothing without one.
    fn get(&mut self, dir: &Path) -> &[LedgerEntry] {
        let path = dir.join("ledger.jsonl");
        let stamp = fs::metadata(&path)
            .ok()
            .and_then(|meta| Some((meta.modified().ok()?, meta.len())));
        let fresh = self.read.get(&path).is_some_and(|(read, _)| *read == stamp);
        if !fresh {
            let entries = fs::read_to_string(&path)
                .map(|text| parse_ledger(&text))
                .unwrap_or_default();
            self.read.insert(path.clone(), (stamp, entries));
        }
        &self.read[&path].1
    }
}

/// What [`retag_proven`] carries from one pass to the next in a long-lived
/// process: whose processes named no supervisor files, and the ledgers.
#[derive(Debug, Default)]
pub struct RetagCache {
    /// Panes whose session's process named no supervisor files, by pane
    /// and session; a session's arguments never change.
    not_supervisors: HashSet<(String, String)>,
    /// Panes without a session that named none, and when they were asked.
    not_supervisors_unbound: HashMap<String, Instant>,
    ledgers: Ledgers,
}

/// How long a pane without a session is not asked again.
const UNBOUND_RECHECK: Duration = Duration::from_secs(30);

/// Puts back the two marks Corgi can prove from evidence it wrote itself,
/// in Herdr, the marker record and `snapshot`, so the refresh that found
/// them lost already sees them (decision 2026-11-03, docs/orphaned-sessions.md):
///
/// - a supervisor's mark, on an agent that is not the supervisor but whose
///   foreground process was started with its project's `ROLE.md` or state
///   directory, when the project has no running supervisor, no other agent's
///   process names the same files, and the agent has a session to bind the
///   mark to;
/// - a worker's request id, on an agent without one whose cwd and branch are
///   those of exactly one unfinished ledger entry of its project that no
///   live agent carries, when no other agent works in that checkout.
///
/// Everything else waits for the user in the dialog. Only the pass holding
/// the wake lock calls this, after [`markers::restore_markers`]. Returns the
/// status line naming what was put back.
pub fn retag_proven(
    client: &HerdrClient,
    snapshot: &mut SessionSnapshot,
    repos: &mut RepoCache,
    state: &Inboxes,
    cache: &mut RetagCache,
) -> Option<String> {
    let workspaces: HashMap<String, WorkspaceInfo> = snapshot
        .workspaces
        .iter()
        .map(|workspace| (workspace.workspace_id.clone(), workspace.clone()))
        .collect();
    let roots: Vec<String> = snapshot
        .agents
        .iter()
        .map(|info| {
            agent_repo(info, workspaces.get(&info.workspace_id), repos)
                .map(|repo| repo.root)
                .unwrap_or_else(|| info.cwd().to_string())
        })
        .collect();
    let seats: Vec<Seat> = snapshot
        .agents
        .iter()
        .zip(&roots)
        .filter(|(info, _)| has_real_agent_session(info))
        .map(|(info, root)| Seat {
            info,
            root: root.clone(),
            supervisor: is_supervisor(info),
            scratch: root.is_empty() || is_home(root),
        })
        .collect();

    let mut marks: Vec<Mark> = Vec::new();

    // Supervisors: only in projects without a running one, and never an
    // agent Corgi spawned as a worker.
    let led: HashSet<&str> = seats
        .iter()
        .filter(|seat| seat.supervisor)
        .map(|seat| seat.root.as_str())
        .collect();
    let mut probe = Probe::new(client);
    let now = Instant::now();
    let sightings = supervisor_sightings(&seats, state, &mut probe, |seat| {
        if led.contains(seat.root.as_str()) || seat.info.tokens.contains_key(CORGI_REQUEST_TOKEN) {
            return false;
        }
        match native_session(seat.info) {
            Some(session) => !cache
                .not_supervisors
                .contains(&(seat.info.pane_id.clone(), session.to_string())),
            None => cache
                .not_supervisors_unbound
                .get(&seat.info.pane_id)
                .is_none_or(|asked| now.duration_since(*asked) >= UNBOUND_RECHECK),
        }
    });
    // Remember who named nothing, so a dashboard asks each session once.
    for (index, seat) in seats.iter().enumerate() {
        if seat.supervisor
            || seat.scratch
            || sightings.iter().any(|sighting| sighting.seat == index)
            || !probe.asked.contains_key(&seat.info.pane_id)
        {
            continue;
        }
        match native_session(seat.info) {
            Some(session) => {
                cache
                    .not_supervisors
                    .insert((seat.info.pane_id.clone(), session.to_string()));
            }
            None => {
                cache
                    .not_supervisors_unbound
                    .insert(seat.info.pane_id.clone(), now);
            }
        }
    }
    for sighting in &sightings {
        let verdict = supervisor_verdict(&seats, &sightings, sighting);
        if !verdict.offer {
            continue;
        }
        let seat = &seats[sighting.seat];
        let name = seat.info.display_name().to_string();
        let mut tokens: Vec<(String, String)> = corgi_pane_tokens(&seat.info.tokens)
            .into_iter()
            .filter(|(key, _)| *key != SUPERVISOR_TOKEN)
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        tokens.push((SUPERVISOR_TOKEN.into(), name.clone()));
        marks.push(Mark {
            pane_id: seat.info.pane_id.clone(),
            said: format!(
                "marked {name} as {}'s supervisor again",
                project_name(&seat.root)
            ),
            name,
            tokens,
            session: native_session(seat.info).map(str::to_string),
        });
    }

    // Workers' request ids.
    let taken = request_ids(&snapshot.agents);
    for (index, seat) in seats.iter().enumerate() {
        if seat.supervisor
            || seat.scratch
            || seat.info.tokens.contains_key(CORGI_REQUEST_TOKEN)
            || sightings.iter().any(|sighting| sighting.seat == index)
        {
            continue;
        }
        let Some(dir) = state.dir(&seat.root).filter(|dir| dir.is_dir()) else {
            continue;
        };
        let entries = cache.ledgers.get(&dir);
        let Some(found) = worker_match(&seats, index, entries, &taken) else {
            continue;
        };
        if found.confidence != Confidence::Certain {
            continue;
        }
        let entry = &found.entries[0];
        let name = seat.info.display_name().to_string();
        let mut tokens: Vec<(String, String)> = corgi_pane_tokens(&seat.info.tokens)
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        tokens.push((CORGI_REQUEST_TOKEN.into(), entry.id.clone()));
        marks.push(Mark {
            pane_id: seat.info.pane_id.clone(),
            said: format!("gave {name} back its request id {}", entry.id),
            name,
            tokens,
            session: native_session(seat.info).map(str::to_string),
        });
    }

    let mut done = Vec::new();
    for Mark {
        pane_id,
        name,
        tokens,
        session,
        said,
    } in marks
    {
        let pairs: Vec<(&str, &str)> = tokens
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        if markers::mark_pane(client, &pane_id, &name, session.as_deref(), &pairs).is_err() {
            continue;
        }
        let mut owned: Vec<(String, String)> = tokens;
        for (key, value) in owned.iter_mut() {
            if key == SUPERVISOR_TOKEN {
                *value = supervisor::marker(&name, session.as_deref());
            }
        }
        for agent in snapshot.agents.iter_mut().filter(|a| a.pane_id == pane_id) {
            agent.tokens.extend(owned.iter().cloned());
        }
        for pane in snapshot.panes.iter_mut().filter(|p| p.pane_id == pane_id) {
            pane.tokens.extend(owned.iter().cloned());
        }
        done.push(said);
    }
    (!done.is_empty()).then(|| {
        let mut line = done.join("; ");
        if let Some(first) = line.get(..1) {
            line = first.to_uppercase() + &line[1..];
        }
        line
    })
}

/// The candidates as `corgi orphans` prints them: one block each, its
/// evidence indented under it, then what it offers, the suggested first.
pub fn report(candidates: &[Candidate]) -> String {
    if candidates.is_empty() {
        return "No orphaned sessions.\n".into();
    }
    let mut out = format!(
        "{} orphaned session{}:\n",
        candidates.len(),
        if candidates.len() == 1 { "" } else { "s" }
    );
    for candidate in candidates {
        let what = match candidate.pane_id.as_deref() {
            Some(pane) => format!("{} {} ({pane})", candidate.harness, candidate.name),
            None => format!("workspace {} ({})", candidate.name, candidate.workspace_id),
        };
        out.push_str(&format!(
            "\n{} — {} [{}]\n  in {} at {}\n",
            candidate.cause.label(),
            what.trim(),
            candidate.confidence.label(),
            candidate.workspace,
            crate::paths::tilde(&candidate.cwd),
        ));
        for line in &candidate.evidence {
            out.push_str(&format!("  · {line}\n"));
        }
        if let Some(fix) = &candidate.fix {
            out.push_str(&format!("  Fix: {fix}\n"));
        }
        let choices: Vec<String> = candidate
            .actions
            .iter()
            .enumerate()
            .map(|(index, action)| {
                let suggested = if index == candidate.suggested {
                    " (suggested)"
                } else {
                    ""
                };
                format!("[{}] {}{suggested}", action.key(), action.label())
            })
            .collect();
        out.push_str(&format!("  Choices: {}\n", choices.join(" · ")));
    }
    out
}

/// The fingerprints of the candidates the user left.
pub fn read_left(path: &Path) -> HashSet<String> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// Keeps `fingerprint` among the left ones, the newest [`LEFT_KEPT`].
fn leave(path: &Path, fingerprint: &str) -> Result<()> {
    let mut left: Vec<String> = fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    left.retain(|kept| kept != fingerprint);
    left.push(fingerprint.to_string());
    let excess = left.len().saturating_sub(LEFT_KEPT);
    left.drain(..excess);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_string_pretty(&left)?)
        .with_context(|| format!("write {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests;
