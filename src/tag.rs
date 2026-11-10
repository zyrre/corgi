//! `corgi tag`: a supervisor tags one of its project's agents ready for the
//! user to merge, or clears the tag, for the Projects list to show.

use std::{fs, path::Path};

use anyhow::{Context, Result, bail};

use crate::{
    digest,
    git::{git_current_branch, git_output},
    launch::CORGI_REQUEST_TOKEN,
    model::{AgentState, DashboardAgent},
    supervisor,
};

/// What `corgi tag` was asked to do.
#[derive(Debug, PartialEq, Eq)]
pub struct TagOptions {
    pub name: String,
    /// Tag ready to merge, or else clear the tag.
    pub merge: bool,
    pub project: Option<String>,
}

/// Tags the agent `options` names, of the project at `root`, ready to merge,
/// or clears its tag, as `write` writes it: given the agent, and the
/// [`supervisor::MERGE_TOKEN`] value to tag it with, or `None` to clear its
/// tag. Every check is made before `write` is called.
pub fn tag_agent_with(
    agents: &[DashboardAgent],
    root: &str,
    options: &TagOptions,
    write: impl FnOnce(&DashboardAgent, Option<&str>) -> Result<()>,
) -> Result<String> {
    let name = options.name.as_str();
    let agent = agents
        .iter()
        .find(|agent| {
            agent.project_root == root
                && (agent.info.name.as_deref() == Some(name) || agent.info.pane_id == name)
        })
        .with_context(|| format!("no agent named {name} in {root}"))?;
    let info = &agent.info;
    let agent_name = info.name.as_deref().unwrap_or(name);
    if !options.merge {
        let had = info.tokens.contains_key(supervisor::MERGE_TOKEN);
        write(agent, None)?;
        return Ok(if had {
            format!("Cleared the merge tag of {agent_name}")
        } else {
            format!("{agent_name} had no merge tag")
        });
    }
    let request = info.tokens.get(CORGI_REQUEST_TOKEN).map(String::as_str);
    if let Some(questions) = open_questions(root, agent_name, request) {
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
        matches!(state, AgentState::Done | AgentState::Idle),
        "{agent_name} is {}; tag it once it is done or idle",
        state.label().to_lowercase()
    );
    let checkout = agent
        .worktree_checkout
        .as_deref()
        .unwrap_or_else(|| info.cwd());
    let base = git_current_branch(Path::new(root))
        .with_context(|| format!("read the branch checked out in {root}"))?;
    let ahead = commits_ahead(Path::new(checkout), &base)?;
    anyhow::ensure!(
        ahead > 0,
        "{agent_name} has no commits ahead of {base} in {checkout}; there is nothing to merge"
    );
    let value = supervisor::merge_tag_value(state, info.state_change_seq);
    write(agent, Some(&value))?;
    Ok(format!(
        "Tagged {agent_name} ready to merge: {ahead} commit{} ahead of {base}",
        if ahead == 1 { "" } else { "s" }
    ))
}

/// The questions the project supervisor's ledger still has open about the agent
/// `agent`, spawned with the request id `request` (see
/// [`digest::open_questions`]). A project without a ledger, or one that
/// cannot be read, has none.
fn open_questions(root: &str, agent: &str, request: Option<&str>) -> Option<Vec<String>> {
    let state_dir = supervisor::state_dir(root).ok()?;
    let ledger = fs::read_to_string(state_dir.join("ledger.jsonl")).ok()?;
    digest::open_questions(&ledger, agent, request)
}

/// How many commits `checkout`'s HEAD has that the branch `base` has not.
fn commits_ahead(checkout: &Path, base: &str) -> Result<usize> {
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
