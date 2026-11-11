//! Corgi's agents among hc's terminals: the terminals Corgi labelled (see the
//! label constants in [`super`]), each as an [`Agent`] that `corgi fleet`,
//! `status`, `report`, `digest`, `tag` and the daemon read.
//!
//! A terminal's hc `turn` is the agent's `state_change_seq`, and its labels
//! are the tokens Corgi reads (`corgi.request` as `corgi_request`,
//! `corgi.task` as `task`, a merge tag as `corgi_merge`).

use std::{collections::BTreeMap, path::Path};

use anyhow::Result;

use crate::{
    launch::CORGI_REQUEST_TOKEN,
    model::{Agent, AgentInfo, AgentSession, AgentState},
    paths::{dir_name, is_home},
    rows::{SCRATCH_GROUP, SUPERVISOR_NAME, checkout_label, task_summary},
    session::SessionReader,
    supervisor::{self, MERGE_TOKEN},
    tag::{TagOptions, tag_agent_with},
};

use super::{
    BASELINE_LABEL, HANDOVER_LABEL, HARNESS_LABEL, HcClient, MERGE_TAG, PROJECT_LABEL,
    REQUEST_LABEL, ROLE_LABEL, SESSION_LABEL, SPACE_LABEL, SUPERVISOR_ROLE, TAG_AT_LABEL,
    TAG_LABEL, TASK_LABEL, Terminal, WORKER_ROLE,
};

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

/// Every agent Corgi runs on hc, sorted ([`sort_agents`]), with what their
/// session files say.
pub fn agents(client: &HcClient) -> Result<Vec<Agent>> {
    let terminals = corgi_terminals(client)?;
    Ok(agents_of(&terminals, None))
}

/// The agents in `terminals`, reading session files under `home` in place
/// of `$HOME` when it is given (for tests).
pub fn agents_of(terminals: &[Terminal], home: Option<&Path>) -> Vec<Agent> {
    let infos: Vec<AgentInfo> = terminals.iter().map(agent_info).collect();
    let mut sessions = session_reader(home);
    sessions.retain(infos.iter());
    let mut agents: Vec<Agent> = terminals
        .iter()
        .zip(infos)
        .filter_map(|(terminal, info)| agent_of(terminal, info, &mut sessions))
        .collect();
    sort_agents(&mut agents);
    agents
}

#[cfg(test)]
fn session_reader(home: Option<&Path>) -> SessionReader {
    home.map(SessionReader::in_home).unwrap_or_default()
}

#[cfg(not(test))]
fn session_reader(_home: Option<&Path>) -> SessionReader {
    SessionReader::default()
}

/// What hc and Corgi's labels say of the agent in `terminal`.
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
        value: value.to_string(),
    });
    AgentInfo {
        agent: Some(kind),
        // A terminal nobody named (a shell of the Herdr app that became a
        // supervisor's) goes by its address, which hc takes wherever it
        // takes a name.
        name: Some(terminal.display_name().to_string()),
        state,
        address: terminal.address.clone(),
        space: terminal.label(SPACE_LABEL).unwrap_or_default().to_string(),
        cwd: Some(agent_cwd(terminal).to_string()),
        state_change_seq: terminal.turn,
        title: Some(terminal.title.clone()).filter(|title| !title.trim().is_empty()),
        tokens,
        agent_session,
    }
}

/// The folder the agent works in, where its harness keeps its session: the
/// one its terminal started in, never a tool's folder that happens to be in
/// front. A supervisor always runs in its project's primary checkout,
/// whatever folder the shell it was started in began in (a terminal the
/// person had open, see [`super::launch::adopt_terminal`]).
fn agent_cwd(terminal: &Terminal) -> &str {
    match terminal.label(PROJECT_LABEL) {
        Some(root) if terminal.label(ROLE_LABEL) == Some(SUPERVISOR_ROLE) => root,
        _ => &terminal.cwd,
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

/// The agent in `terminal`, which `info` describes, or `None` when Corgi
/// did not label it with a project.
fn agent_of(terminal: &Terminal, info: AgentInfo, sessions: &mut SessionReader) -> Option<Agent> {
    let root = terminal.label(PROJECT_LABEL)?.to_string();
    let supervisor = terminal.label(ROLE_LABEL) == Some(SUPERVISOR_ROLE);
    let repo_name = dir_name(&root)
        .unwrap_or(supervisor::UNNAMED_PROJECT)
        .to_string();
    // Spawn's worktrees are outside the primary checkout; a directory
    // checkout is the project itself or a folder in it.
    let cwd = agent_cwd(terminal);
    let worktree_checkout = (!Path::new(cwd).starts_with(&root)).then(|| cwd.to_string());
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
    let task = if supervisor {
        SUPERVISOR_NAME.to_string()
    } else {
        task_summary(&info, &in_project, facts.task.as_deref())
    };
    Some(Agent {
        project_group,
        project_root: root,
        worktree_checkout,
        task,
        model: facts.model,
        effort: facts.effort,
        context_percent: facts.context_percent,
        context_tokens: facts.context_tokens,
        cache: facts.cache,
        supervisor,
        scratch,
        exited: supervisor && shell_in_front(terminal),
        info,
    })
}

/// `corgi tag`: the checks of [`tag_agent_with`], with the tag kept in the
/// terminal's [`TAG_LABEL`] and [`TAG_AT_LABEL`].
pub fn tag(
    client: &HcClient,
    agents: &[Agent],
    root: &str,
    options: &TagOptions,
) -> Result<String> {
    tag_agent_with(agents, root, options, |agent, at| {
        write_tag(client, &agent.info.address, at)
    })
}

/// Clears the merge tag of `agent` once its branch is merged, as
/// `corgi tag --clear` does, and drops it from the row too. An agent without
/// one is left alone.
pub fn clear_merge_tag(client: &HcClient, agent: &mut Agent) -> Result<()> {
    if !agent.info.tokens.contains_key(MERGE_TOKEN) {
        return Ok(());
    }
    write_tag(client, &agent.info.address, None)?;
    agent.info.tokens.remove(MERGE_TOKEN);
    Ok(())
}

/// Tags the terminal at `address` for merging, `at` the state and turn it
/// was tagged at, or clears its tag with `None`.
fn write_tag(client: &HcClient, address: &str, at: Option<&str>) -> Result<()> {
    let labels = match at {
        Some(at) => [(TAG_LABEL, MERGE_TAG), (TAG_AT_LABEL, at)],
        None => [(TAG_LABEL, ""), (TAG_AT_LABEL, "")],
    };
    client.label(address, &labels).map(drop)
}

/// Keeps related worktree sessions together, each project led by its
/// supervisor, and the scratch sessions after every project. Within a project
/// the rest follow their live state, the ones needing attention first:
/// blocked, working, done, idle, then unknown. Agents sharing a state are
/// ordered by when they entered it, the most recent first
/// (`state_change_seq`), then by name, then by terminal address.
fn sort_agents(agents: &mut [Agent]) {
    agents.sort_by(|left, right| {
        left.scratch
            .cmp(&right.scratch)
            .then_with(|| {
                left.project_group
                    .to_lowercase()
                    .cmp(&right.project_group.to_lowercase())
            })
            .then_with(|| right.supervisor.cmp(&left.supervisor))
            .then_with(|| state_rank(left.info.state).cmp(&state_rank(right.info.state)))
            .then_with(|| right.info.state_change_seq.cmp(&left.info.state_change_seq))
            .then_with(|| {
                left.info
                    .display_name()
                    .to_lowercase()
                    .cmp(&right.info.display_name().to_lowercase())
            })
            .then_with(|| left.info.address.cmp(&right.info.address))
    });
}

/// Position of a state within a project, the most urgent first.
fn state_rank(state: AgentState) -> u8 {
    match state {
        AgentState::Blocked => 0,
        AgentState::Working => 1,
        AgentState::Done => 2,
        AgentState::Idle => 3,
        AgentState::Unknown => 4,
    }
}
