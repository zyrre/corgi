//! What a dashboard row shows, derived from Herdr's metadata and the session
//! facts: whether a pane is an agent at all, its project and heading
//! (`repo/checkout` for a linked worktree), its task summary, and the
//! status-line bridge fallbacks for model, effort and context.

use std::path::Path;

use crate::{
    harness::Harness,
    model::{AgentInfo, WorkspaceInfo, WorkspaceWorktreeInfo},
    paths::dir_name,
    steward,
};

use super::project_main::project_main_root;

/// The heading of the agents running in the home directory, which is never a
/// project.
pub(super) const SCRATCH_GROUP: &str = "Scratch";
/// Shown until an agent CLI has summarized what the session is about.
const NO_TASK_SUMMARY: &str = "No task yet";
/// Shown in the tool row until the agent has run its first command or tool.
pub(super) const NO_TOOL_YET: &str = "No command yet";
/// Herdr's detector may identify a pane from its visible terminal content
/// without there being an agent session attached to it. Only a non-empty
/// session identity represents an actual agent for the dashboard.
pub(super) fn has_real_agent_session(info: &AgentInfo) -> bool {
    info.agent_session
        .as_ref()
        .is_some_and(|session| !session.value.trim().is_empty())
}

/// The workspace's Git provenance, but only when the agent actually runs inside
/// that checkout.
///
/// Herdr records provenance when a workspace is created and does not follow
/// the shell afterwards, so a workspace opened in one repository whose agent
/// later moved to another still claims the first. Trusting that would create
/// the new worktree in the wrong repository, or remove the wrong checkout.
pub(super) fn agent_worktree<'a>(
    info: &AgentInfo,
    workspace: Option<&'a WorkspaceInfo>,
) -> Option<&'a WorkspaceWorktreeInfo> {
    let worktree = workspace?.worktree.as_ref()?;
    if worktree.checkout_path.is_empty() {
        return None;
    }
    let cwd = Path::new(info.cwd());
    cwd.starts_with(&worktree.checkout_path).then_some(worktree)
}

/// The project shown on an agent row.
///
/// Linked worktrees are shown as `repo/checkout` so sibling agents of one
/// repository stay distinguishable while the repository name stays visible.
/// Other workspaces use Herdr's label, then the cwd's last path component.
pub(super) fn agent_project(info: &AgentInfo, workspace: Option<&WorkspaceInfo>) -> String {
    if let Some(worktree) = agent_worktree(info, workspace)
        .filter(|worktree| worktree.is_linked_worktree && !worktree.repo_name.is_empty())
        && let Some(checkout) = checkout_label(&worktree.checkout_path)
    {
        return format!("{}/{checkout}", worktree.repo_name);
    }
    // Corgi's project workspace is labelled for its Steward; the other agents
    // in it, such as shared-checkout tabs, are named after the repository.
    if let Some(root) = workspace.and_then(project_main_root) {
        return dir_name(root)
            .unwrap_or(steward::UNNAMED_PROJECT)
            .to_string();
    }
    workspace
        .map(|workspace| workspace.label.trim())
        .filter(|label| {
            !label.is_empty() && !label.chars().all(|character| character.is_ascii_digit())
        })
        .map(str::to_string)
        .or_else(|| dir_name(info.cwd()).map(str::to_string))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown project".into())
}

/// The heading shared by all sessions belonging to one project.
///
/// Worktree rows retain their checkout in [`agent_project`], while their group
/// uses the repository name so every `corgi/<checkout>` session appears below
/// a single `corgi` heading.
pub(super) fn project_group(
    info: &AgentInfo,
    workspace: Option<&WorkspaceInfo>,
    project: &str,
) -> String {
    agent_worktree(info, workspace)
        .map(|worktree| worktree.repo_name.trim())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| project.to_string())
}
/// The directory name of a checkout, without the `worktree-` prefix Herdr adds
/// to every worktree it creates.
pub(super) fn checkout_label(checkout_path: &str) -> Option<&str> {
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
pub(super) fn task_summary(
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

/// The thread ID Codex's app server expects. Herdr already gives Corgi this
/// identity so the session reader can resolve the matching rollout directly.
pub(super) fn codex_thread_id(info: &AgentInfo) -> Option<&str> {
    (info.harness() == Harness::Codex)
        .then_some(())
        .and(info.agent_session.as_ref())
        .map(|session| session.value.trim())
        .filter(|session_id| !session_id.is_empty())
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

/// The model a status-line bridge reports for a pane.
///
/// Neither Herdr nor its agent integrations publish one, so this is only set
/// when something else reports pane metadata. [`SessionReader`](crate::session::SessionReader) reads the same
/// facts from the agent's own session file and takes precedence.
pub(super) fn reported_model(info: &AgentInfo) -> Option<String> {
    info.tokens
        .get("model")
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

/// The effort a status-line bridge reports for a pane. Session files
/// take precedence; this is a fallback for agents whose transcript is not
/// available yet.
pub(super) fn reported_effort(info: &AgentInfo) -> Option<String> {
    [
        "thinking_effort",
        "reasoning_effort",
        "model_reasoning_effort",
        "effort",
    ]
    .into_iter()
    .find_map(|key| {
        info.tokens
            .get(key)
            .map(|effort| effort.trim())
            .filter(|effort| !effort.is_empty())
            .map(str::to_string)
    })
}

/// The context share a status-line bridge reports for a pane, as a percentage
/// such as `13%`.
pub(super) fn reported_context_percent(info: &AgentInfo) -> Option<u8> {
    info.tokens
        .get("ctx")?
        .trim()
        .trim_end_matches('%')
        .parse::<f64>()
        .ok()
        .map(|percent| percent.clamp(0.0, 100.0).round() as u8)
}

#[cfg(test)]
mod tests {
    use crate::model::{AgentInfo, WorkspaceInfo, WorkspaceWorktreeInfo};

    use super::*;

    #[test]
    fn linked_worktrees_show_repo_and_checkout() {
        let info = AgentInfo {
            cwd: Some("/home/me/.herdr/worktrees/corgi/worktree-calm-otter-1f2e".into()),
            ..AgentInfo::default()
        };
        let linked = WorkspaceInfo {
            workspace_id: "w10".into(),
            label: "worktree-calm-otter-1f2e".into(),
            worktree: Some(WorkspaceWorktreeInfo {
                repo_name: "corgi".into(),
                repo_root: "/home/me/repos/corgi".into(),
                checkout_path: "/home/me/.herdr/worktrees/corgi/worktree-calm-otter-1f2e".into(),
                is_linked_worktree: true,
            }),
            ..WorkspaceInfo::default()
        };
        assert_eq!(agent_project(&info, Some(&linked)), "corgi/calm-otter-1f2e");
        assert_eq!(linked.repo_root(), Some("/home/me/repos/corgi"));

        let primary = WorkspaceInfo {
            label: "corgi".into(),
            worktree: Some(WorkspaceWorktreeInfo {
                is_linked_worktree: false,
                ..linked.worktree.clone().unwrap()
            }),
            ..linked.clone()
        };
        assert_eq!(agent_project(&info, Some(&primary)), "corgi");
        assert_eq!(
            project_group(&info, Some(&linked), "corgi/calm-otter-1f2e"),
            "corgi"
        );
    }

    #[test]
    fn stale_workspace_provenance_is_ignored_when_the_agent_runs_elsewhere() {
        let workspace = WorkspaceInfo {
            workspace_id: "wJ".into(),
            label: "webshop-backend".into(),
            worktree: Some(WorkspaceWorktreeInfo {
                repo_name: "claude-agents-dashboard".into(),
                repo_root: "/home/me/repos/test/claude-agents-dashboard".into(),
                checkout_path: "/home/me/repos/test/claude-agents-dashboard".into(),
                is_linked_worktree: false,
            }),
            ..WorkspaceInfo::default()
        };
        let elsewhere = AgentInfo {
            cwd: Some("/home/me/repos/webshop-backend".into()),
            ..AgentInfo::default()
        };
        assert!(agent_worktree(&elsewhere, Some(&workspace)).is_none());

        let inside = AgentInfo {
            cwd: Some("/home/me/repos/test/claude-agents-dashboard/cmd".into()),
            ..AgentInfo::default()
        };
        assert_eq!(
            agent_worktree(&inside, Some(&workspace)).map(|worktree| worktree.repo_root.as_str()),
            Some("/home/me/repos/test/claude-agents-dashboard")
        );
        // A prefix match must be on whole path components.
        let sibling = AgentInfo {
            cwd: Some("/home/me/repos/test/claude-agents-dashboard-2".into()),
            ..AgentInfo::default()
        };
        assert!(agent_worktree(&sibling, Some(&workspace)).is_none());
    }

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

    #[test]
    fn the_model_and_worktree_of_a_row_come_from_pane_metadata_and_the_checkout() {
        let mut info = AgentInfo {
            agent: Some("codex".into()),
            ..AgentInfo::default()
        };
        assert_eq!(reported_model(&info), None);
        info.tokens.insert("model".into(), " Opus 5 ".into());
        assert_eq!(reported_model(&info).as_deref(), Some("Opus 5"));
        info.tokens.insert("effort".into(), " high ".into());
        assert_eq!(reported_effort(&info).as_deref(), Some("high"));

        assert_eq!(
            checkout_label("/worktrees/corgi/worktree-silver-cloud-028f"),
            Some("silver-cloud-028f")
        );
        assert_eq!(checkout_label("/repos/corgi"), Some("corgi"));
        assert_eq!(checkout_label(""), None);
    }
}
