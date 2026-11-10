//! What an agent's row shows, derived from its terminal's metadata and the
//! session facts: its heading (`repo/checkout` for a linked worktree) and
//! its task summary.

use std::path::Path;

use crate::{
    harness::Harness,
    model::{AgentInfo, WorkspaceInfo},
    paths::dir_name,
};

/// What the dashboard calls a project's supervisor: its role, capitalised as
/// a label. There is one per project, so the project heading says whose it is.
pub const SUPERVISOR_NAME: &str = "Supervisor";

/// The heading of the agents running in the home directory, which is never a
/// project.
pub const SCRATCH_GROUP: &str = "Scratch";
/// Shown until an agent CLI has summarized what the session is about.
const NO_TASK_SUMMARY: &str = "No task yet";
/// The session ID a status-line bridge reports for a pane.
pub const STATUS_LINE_SESSION_TOKEN: &str = "session";

/// The directory name of a checkout, without the `worktree-` prefix Herdr adds
/// to every worktree it creates.
pub fn checkout_label(checkout_path: &str) -> Option<&str> {
    dir_name(checkout_path)
        .map(|part| part.strip_prefix("worktree-").unwrap_or(part))
        .filter(|part| !part.is_empty())
}

/// A few words describing what the agent is working on.
///
/// Claude Code normally supplies a generated terminal title, with any Herdr
/// task token taking priority. Codex's Herdr task token is currently a
/// machine-readable slug, so its generated thread name—and, while it is
/// pending, the first real transcript prompt—wins ahead of that token and its
/// often branch-like pane title.
///
/// Until the agent has summarized anything the title is still the CLI's own
/// name or the directory it started in, which says nothing about the task, so
/// those stand-ins are rejected in favour of an explicit placeholder.
pub fn task_summary(
    info: &AgentInfo,
    workspace: Option<&WorkspaceInfo>,
    project: &str,
    codex_title: Option<&str>,
    transcript_task: Option<&str>,
) -> String {
    let herdr_task = info.tokens.get("task").map(String::as_str);
    let workspace_task = workspace
        .and_then(|workspace| workspace.tokens.get("task"))
        .map(String::as_str);
    let candidates = if info.harness() == Harness::Codex {
        vec![
            codex_title,
            transcript_task,
            info.title.as_deref(),
            herdr_task,
            workspace_task,
            info.terminal_title_stripped.as_deref(),
        ]
    } else {
        vec![
            herdr_task,
            workspace_task,
            info.title.as_deref(),
            info.terminal_title_stripped.as_deref(),
            transcript_task,
        ]
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
    use crate::model::{AgentInfo, WorkspaceInfo};

    use super::*;

    #[test]
    fn the_task_summary_comes_from_the_agents_own_session_title() {
        let workspace = WorkspaceInfo {
            workspace_id: "w1".into(),
            label: "corgi".into(),
            ..WorkspaceInfo::default()
        };
        let titled = AgentInfo {
            agent: Some("claude".into()),
            cwd: Some("/repos/corgi".into()),
            terminal_title_stripped: Some("Agent session status line".into()),
            ..AgentInfo::default()
        };
        assert_eq!(
            task_summary(&titled, Some(&workspace), "corgi", None, None),
            "Agent session status line"
        );

        // Before the first turn the title is only the CLI's own name.
        let fresh = AgentInfo {
            terminal_title_stripped: Some("Claude Code".into()),
            ..titled.clone()
        };
        assert_eq!(
            task_summary(&fresh, Some(&workspace), "corgi", None, None),
            NO_TASK_SUMMARY
        );

        // Shells and CLIs that title a pane with its directory say nothing
        // about the task either.
        let directory = AgentInfo {
            terminal_title_stripped: Some("corgi".into()),
            ..titled.clone()
        };
        assert_eq!(
            task_summary(&directory, Some(&workspace), "corgi", None, None),
            NO_TASK_SUMMARY
        );

        // A reported token is metadata rather than a repurposed window title.
        let mut tokened = titled.clone();
        tokened
            .tokens
            .insert("task".into(), "Publish the release notes".into());
        assert_eq!(
            task_summary(&tokened, Some(&workspace), "corgi", None, None),
            "Publish the release notes"
        );

        // Codex's generated app-server title wins over its prompt fallback and
        // the machine-readable Herdr token that otherwise resembles a branch.
        let mut codex = AgentInfo {
            agent: Some("codex".into()),
            cwd: Some("/repos/corgi".into()),
            terminal_title_stripped: Some("after-the-agent-status-working-idle".into()),
            ..AgentInfo::default()
        };
        codex
            .tokens
            .insert("task".into(), "after-the-agent-status-working-idle".into());
        assert_eq!(
            task_summary(
                &codex,
                Some(&workspace),
                "corgi",
                Some("Investigate task summaries"),
                Some("Show a task summary for Codex sessions"),
            ),
            "Investigate task summaries"
        );
        assert_eq!(
            task_summary(
                &codex,
                Some(&workspace),
                "corgi",
                None,
                Some("Show a task summary for Codex sessions"),
            ),
            "Show a task summary for Codex sessions"
        );
    }
}
