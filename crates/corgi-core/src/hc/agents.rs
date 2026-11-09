//! Corgi's agents among hc's terminals: the terminals Corgi labelled (see the
//! label constants in [`super`]), as the same [`DashboardAgent`] rows the
//! Herdr TUI's snapshot gives, so `corgi fleet`, `report`, `digest` and `tag`
//! read both alike.
//!
//! A terminal stands in for a pane: its address is the row's `pane_id`, its
//! hc `turn` the `state_change_seq`, and its labels the pane tokens Corgi
//! reads (`corgi.request` as `corgi_request`, `corgi.task` as `task`, a
//! merge tag as `corgi_merge`).

use std::{collections::BTreeMap, path::Path};

use anyhow::Result;

use crate::{
    activity::message_from_state,
    launch::CORGI_REQUEST_TOKEN,
    model::{Activity, ActivityKind, AgentInfo, AgentSession, AgentState, DashboardAgent},
    paths::{dir_name, is_home},
    refresh::sort_agents,
    rows::{NO_TOOL_YET, SCRATCH_GROUP, SUPERVISOR_NAME, checkout_label, task_summary},
    session::SessionReader,
    supervisor::{self, MERGE_TOKEN},
    tag::{TagOptions, tag_agent_with},
    time::LocalTime,
};

use super::{
    BASELINE_LABEL, HANDOVER_LABEL, HARNESS_LABEL, HcClient, MERGE_TAG, PROJECT_LABEL,
    REQUEST_LABEL, ROLE_LABEL, SESSION_LABEL, SPACE_LABEL, SUPERVISOR_ROLE, TAG_AT_LABEL,
    TAG_LABEL, TASK_LABEL, Terminal, WORKER_ROLE,
};

/// The source an hc agent's session identity is recorded under.
const SESSION_SOURCE: &str = "hc";

/// The programs a kept supervisor terminal falls back to when its agent has
/// exited: the shells hc starts, by the name the OS gives their process.
const SHELLS: [&str; 8] = ["sh", "bash", "zsh", "fish", "nu", "dash", "ksh", "tcsh"];

/// The live terminals Corgi started as supervisors or workers, on this
/// machine.
pub fn corgi_terminals(client: &HcClient) -> Result<Vec<Terminal>> {
    let mut terminals = Vec::new();
    for role in [SUPERVISOR_ROLE, WORKER_ROLE] {
        terminals.extend(
            client
                .list(&[(ROLE_LABEL, role)])?
                .into_iter()
                .filter(|terminal| terminal.label(PROJECT_LABEL).is_some()),
        );
    }
    Ok(terminals)
}

/// Every agent Corgi runs on hc, as dashboard rows in the dashboard's order,
/// with what their session files say.
pub fn agents(client: &HcClient) -> Result<Vec<DashboardAgent>> {
    let terminals = corgi_terminals(client)?;
    Ok(dashboard_agents(&terminals, None))
}

/// The rows of `terminals`, reading session files under `home` in place of
/// `$HOME` when it is given (for tests).
pub fn dashboard_agents(terminals: &[Terminal], home: Option<&Path>) -> Vec<DashboardAgent> {
    let infos: Vec<AgentInfo> = terminals.iter().map(agent_info).collect();
    let mut sessions = session_reader(home);
    sessions.retain(infos.iter());
    let mut agents: Vec<DashboardAgent> = terminals
        .iter()
        .zip(infos)
        .filter_map(|(terminal, info)| dashboard_agent(terminal, info, &mut sessions))
        .collect();
    sort_agents(&mut agents);
    agents
}

#[cfg(any(test, feature = "test-support"))]
fn session_reader(home: Option<&Path>) -> SessionReader {
    home.map(SessionReader::in_home).unwrap_or_default()
}

#[cfg(not(any(test, feature = "test-support")))]
fn session_reader(_home: Option<&Path>) -> SessionReader {
    SessionReader::default()
}

/// The terminal as the agent pane it stands for.
pub fn agent_info(terminal: &Terminal) -> AgentInfo {
    let kind = terminal.label(HARNESS_LABEL).unwrap_or("agent").to_string();
    let mut tokens = BTreeMap::new();
    if let Some(request) = terminal.label(REQUEST_LABEL) {
        tokens.insert(CORGI_REQUEST_TOKEN.to_string(), request.to_string());
    }
    if let Some(task) = terminal.label(TASK_LABEL) {
        tokens.insert("task".to_string(), task.to_string());
    }
    // A supervisor's handover request and baseline, as the daemon keeps them.
    for (label, token) in [
        (HANDOVER_LABEL, supervisor::HANDOVER_TOKEN),
        (BASELINE_LABEL, supervisor::BASELINE_TOKEN),
    ] {
        if let Some(value) = terminal.label(label) {
            tokens.insert(token.to_string(), value.to_string());
        }
    }
    let state = agent_state(terminal);
    if terminal.label(TAG_LABEL) == Some(MERGE_TAG) {
        // A tag set without where it was set applies while the agent rests.
        let at = terminal
            .label(TAG_AT_LABEL)
            .map(str::to_string)
            .unwrap_or_else(|| supervisor::merge_tag_value(state, terminal.turn));
        tokens.insert(MERGE_TOKEN.to_string(), at);
    }
    let agent_session = terminal.label(SESSION_LABEL).map(|value| AgentSession {
        agent: kind.clone(),
        kind: "session".into(),
        source: SESSION_SOURCE.into(),
        value: value.to_string(),
    });
    AgentInfo {
        agent: Some(kind),
        name: terminal.name.clone().filter(|name| !name.is_empty()),
        state,
        pane_id: terminal.address.clone(),
        workspace_id: terminal.label(SPACE_LABEL).unwrap_or_default().to_string(),
        // The folder the agent started in, where its harness keeps its
        // session; never a tool's folder that happens to be in front.
        cwd: Some(terminal.cwd.clone()),
        revision: terminal.turn,
        state_change_seq: terminal.turn,
        title: Some(terminal.title.clone()).filter(|title| !title.trim().is_empty()),
        tokens,
        agent_session,
        terminal_id: Some(terminal.address.clone()),
        ..AgentInfo::default()
    }
}

/// The agent's state from its harness's own report to hc. `error` needs a
/// person as `blocked` does; no report at all, or a shell in front of a kept
/// supervisor terminal, is unknown.
pub fn agent_state(terminal: &Terminal) -> AgentState {
    if shell_in_front(terminal) {
        return AgentState::Unknown;
    }
    match terminal.status_word() {
        Some("idle") => AgentState::Idle,
        Some("working") => AgentState::Working,
        Some("done") => AgentState::Done,
        Some("blocked" | "error") => AgentState::Blocked,
        _ => AgentState::Unknown,
    }
}

/// Whether a shell, not the agent, is in front of the terminal: a supervisor
/// whose agent has exited into the kept shell it was started from.
pub fn shell_in_front(terminal: &Terminal) -> bool {
    terminal.foreground.as_ref().is_some_and(|foreground| {
        let name = foreground.name.trim_start_matches('-');
        let name = name.rsplit('/').next().unwrap_or(name);
        SHELLS.contains(&name)
    })
}

/// The row of `terminal`, whose pane is `info`, or `None` when Corgi did not
/// label it with a project.
fn dashboard_agent(
    terminal: &Terminal,
    info: AgentInfo,
    sessions: &mut SessionReader,
) -> Option<DashboardAgent> {
    let root = terminal.label(PROJECT_LABEL)?.to_string();
    let supervisor = terminal.label(ROLE_LABEL) == Some(SUPERVISOR_ROLE);
    let repo_name = dir_name(&root)
        .unwrap_or(supervisor::UNNAMED_PROJECT)
        .to_string();
    // Spawn's worktrees are outside the primary checkout; a directory
    // checkout is the project itself or a folder in it.
    let worktree_checkout =
        (!Path::new(&terminal.cwd).starts_with(&root)).then(|| terminal.cwd.clone());
    let worktree_label = worktree_checkout
        .as_deref()
        .and_then(checkout_label)
        .map(str::to_string);
    let in_project = match &worktree_label {
        Some(checkout) => format!("{repo_name}/{checkout}"),
        None => repo_name.clone(),
    };
    let scratch = is_home(&root);
    let project_group = if scratch {
        SCRATCH_GROUP.to_string()
    } else {
        repo_name
    };
    let facts = sessions.facts(&info);
    let (project, task) = if supervisor {
        (SUPERVISOR_NAME.to_string(), SUPERVISOR_NAME.to_string())
    } else {
        let task = task_summary(&info, None, &in_project, None, facts.task.as_deref());
        (in_project, task)
    };
    let message = facts
        .message
        .unwrap_or_else(|| message_from_state(info.state));
    let tool = facts.tool.unwrap_or_else(|| Activity {
        kind: ActivityKind::Ready,
        text: NO_TOOL_YET.into(),
    });
    Some(DashboardAgent {
        project_group,
        project,
        project_root: root,
        worktree_checkout,
        worktree_label,
        task,
        model: facts.model,
        effort: facts.effort,
        context_percent: facts.context_percent,
        context_tokens: facts.context_tokens,
        cache: facts.cache,
        last_activity: facts.last_activity.map(LocalTime::at),
        message,
        tool,
        supervisor,
        scratch,
        exited: supervisor && shell_in_front(terminal),
        info,
        ..DashboardAgent::default()
    })
}

/// `corgi tag` on hc: the same checks as on the Herdr TUI, with the tag kept
/// in the terminal's [`TAG_LABEL`] and [`TAG_AT_LABEL`].
pub fn tag(
    client: &HcClient,
    agents: &[DashboardAgent],
    root: &str,
    options: &TagOptions,
) -> Result<String> {
    tag_agent_with(agents, root, options, |agent, at| {
        let address = agent.info.pane_id.as_str();
        let labels = match at {
            Some(at) => [(TAG_LABEL, MERGE_TAG), (TAG_AT_LABEL, at)],
            None => [(TAG_LABEL, ""), (TAG_AT_LABEL, "")],
        };
        client.label(address, &labels).map(drop)
    })
}
