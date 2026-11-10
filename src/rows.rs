//! What an agent's row shows, derived from its terminal's metadata and the
//! session facts: its heading (`repo/checkout` for a linked worktree) and
//! its task summary.

use std::path::Path;

use crate::{harness::Harness, model::AgentInfo, paths::dir_name};

/// The task of a project's supervisor: its role, capitalised as a label.
pub const SUPERVISOR_NAME: &str = "Supervisor";

/// The heading of the agents running in the home directory, which is never a
/// project.
pub const SCRATCH_GROUP: &str = "Scratch";
/// Shown until an agent CLI has summarized what the session is about.
const NO_TASK_SUMMARY: &str = "No task yet";

/// The directory name of a checkout, without the `worktree-` prefix of the
/// worktrees Corgi makes.
pub fn checkout_label(checkout_path: &str) -> Option<&str> {
    dir_name(checkout_path)
        .map(|part| part.strip_prefix("worktree-").unwrap_or(part))
        .filter(|part| !part.is_empty())
}

/// A few words describing what the agent is working on.
///
/// Claude Code's task is the one Corgi labelled its terminal with at launch
/// (the brief's title), then the title it gave its terminal, then its first
/// real transcript prompt. Codex titles its terminal with a branch-like
/// slug, so for it the transcript prompt comes first.
///
/// Until the agent has summarized anything the title is still the CLI's own
/// name or the directory it started in, which says nothing about the task, so
/// those stand-ins are rejected in favour of an explicit placeholder.
pub fn task_summary(info: &AgentInfo, project: &str, transcript_task: Option<&str>) -> String {
    let labelled = info.tokens.get("task").map(String::as_str);
    let candidates = if info.harness() == Harness::Codex {
        [transcript_task, info.title.as_deref(), labelled]
    } else {
        [labelled, info.title.as_deref(), transcript_task]
    };
    candidates
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|candidate| describes_a_task(candidate, info, project))
        .map(str::to_string)
        .unwrap_or_else(|| NO_TASK_SUMMARY.to_string())
}

/// Whether a candidate title actually names a task rather than repeating the
/// agent, the project, or the directory the session runs in.
fn describes_a_task(candidate: &str, info: &AgentInfo, project: &str) -> bool {
    if candidate.is_empty() {
        return false;
    }
    let candidate = candidate.to_lowercase();
    // Every agent sets a title naming only its CLI before its first turn.
    let names_a_cli = Harness::KNOWN
        .iter()
        .flat_map(Harness::cli_titles)
        .any(|title| candidate == *title);
    let cwd = Path::new(info.cwd());
    !names_a_cli
        && [
            Some(project),
            Some(info.kind()),
            Some(info.display_name()),
            Some(info.cwd()),
            cwd.file_name().and_then(|part| part.to_str()),
        ]
        .into_iter()
        .flatten()
        .all(|stand_in| candidate != stand_in.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_task_summary_comes_from_the_agents_own_session_title() {
        let titled = AgentInfo {
            agent: Some("claude".into()),
            cwd: Some("/repos/corgi".into()),
            title: Some("Agent session status line".into()),
            ..AgentInfo::default()
        };
        assert_eq!(
            task_summary(&titled, "corgi", None),
            "Agent session status line"
        );

        // Before the first turn the title is only the CLI's own name.
        let fresh = AgentInfo {
            title: Some("Claude Code".into()),
            ..titled.clone()
        };
        assert_eq!(task_summary(&fresh, "corgi", None), NO_TASK_SUMMARY);

        // Shells and CLIs that title a terminal with its directory say
        // nothing about the task either.
        let directory = AgentInfo {
            title: Some("corgi".into()),
            ..titled.clone()
        };
        assert_eq!(task_summary(&directory, "corgi", None), NO_TASK_SUMMARY);

        // The task Corgi labelled the terminal with wins over its title.
        let mut labelled = titled.clone();
        labelled
            .tokens
            .insert("task".into(), "Publish the release notes".into());
        assert_eq!(
            task_summary(&labelled, "corgi", None),
            "Publish the release notes"
        );

        // Codex's first prompt wins over its branch-like title.
        let codex = AgentInfo {
            agent: Some("codex".into()),
            cwd: Some("/repos/corgi".into()),
            title: Some("after-the-agent-status-working-idle".into()),
            ..AgentInfo::default()
        };
        assert_eq!(
            task_summary(
                &codex,
                "corgi",
                Some("Show a task summary for Codex sessions")
            ),
            "Show a task summary for Codex sessions"
        );
        assert_eq!(
            task_summary(&codex, "corgi", None),
            "after-the-agent-status-working-idle"
        );
    }
}
