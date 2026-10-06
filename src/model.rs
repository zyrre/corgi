use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::harness::Harness;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    Idle,
    Working,
    Blocked,
    Done,
    #[default]
    Unknown,
}

impl AgentState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Working => "WORKING",
            Self::Blocked => "BLOCKED",
            Self::Done => "DONE",
            Self::Unknown => "UNKNOWN",
        }
    }

    pub fn can_prompt(self) -> bool {
        !matches!(self, Self::Blocked)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSession {
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInfo {
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub display_agent: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, rename = "agent_status")]
    pub state: AgentState,
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub state_change_seq: u64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
    #[serde(default)]
    pub agent_session: Option<AgentSession>,
}

impl AgentInfo {
    pub fn kind(&self) -> &str {
        self.agent.as_deref().unwrap_or("agent")
    }

    /// The harness Herdr detected in this pane.
    pub fn harness(&self) -> Harness {
        Harness::from_kind(self.kind())
    }

    pub fn display_name(&self) -> &str {
        self.display_agent
            .as_deref()
            .or(self.name.as_deref())
            .unwrap_or_else(|| self.kind())
    }

    pub fn cwd(&self) -> &str {
        self.foreground_cwd
            .as_deref()
            .or(self.cwd.as_deref())
            .unwrap_or("")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
    /// Git checkout provenance Herdr attaches to workspaces that belong to a
    /// worktree group. Absent for plain workspaces.
    #[serde(default)]
    pub worktree: Option<WorkspaceWorktreeInfo>,
}

impl WorkspaceInfo {
    /// The primary checkout of the repository this workspace works in, when
    /// Herdr knows it. New worktrees must be created from this directory.
    pub fn repo_root(&self) -> Option<&str> {
        self.worktree
            .as_ref()
            .map(|worktree| worktree.repo_root.as_str())
            .filter(|root| !root.is_empty())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceWorktreeInfo {
    #[serde(default)]
    pub repo_name: String,
    #[serde(default)]
    pub repo_root: String,
    #[serde(default)]
    pub checkout_path: String,
    #[serde(default)]
    pub is_linked_worktree: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DashboardAgent {
    pub info: AgentInfo,
    /// The repository or workspace shared by related sessions. This is kept
    /// separate from `project`, which may include a worktree checkout name.
    pub project_group: String,
    pub project: String,
    /// Directory a sibling agent should be created from: the repository's
    /// primary checkout when the agent works in a worktree, else its cwd.
    pub project_root: String,
    /// Path of the agent's linked worktree checkout, when it has one. Closing
    /// such an agent removes the checkout instead of only the workspace.
    pub worktree_checkout: Option<String>,
    /// Short name of that checkout, without the `worktree-` prefix Herdr adds
    /// to the directories it creates. `None` for a primary checkout.
    pub worktree_label: Option<String>,
    /// A few words describing what the agent is working on, as the agent CLI
    /// itself summarizes the session.
    pub task: String,
    /// Model answering this session, read from the agent CLI's own session
    /// file or from pane metadata.
    pub model: Option<String>,
    /// Thinking/reasoning effort selected for this session, when the agent
    /// reports one.
    pub effort: Option<String>,
    /// Share of the model's context window the session occupies.
    pub context_percent: Option<u8>,
    /// Tokens of that context, when the agent CLI's session file records them.
    pub context_tokens: Option<u64>,
    /// The prompt cache holding this session's context, when its harness
    /// records one. Claude Code records an exact lifetime; newer Codex models
    /// let Corgi derive a conservative minimum lifetime from a cache hit or
    /// write in the rollout.
    pub cache: Option<PromptCache>,
    /// The newest thing said in the session, whoever said it: the user's
    /// prompt, the assistant's reply, or the assistant's thinking.
    pub message: Activity,
    /// The command or tool call the agent is running, or the last one it ran,
    /// with its whole argument rather than the CLI's abbreviation of it.
    pub tool: Activity,
    /// The agent in the root tab of its project's Corgi workspace: the
    /// project's supervisor, shown by that name and first in its project.
    pub supervisor: bool,
    /// The agent runs in the home directory itself, which is never a
    /// project: a scratch session, grouped under Scratch, that no supervisor
    /// hears about.
    pub scratch: bool,
}

impl DashboardAgent {
    /// Whether a supervisor has tagged the agent ready for the user to merge, and
    /// it still rests where it was tagged, so it shows as MERGE.
    pub fn ready_to_merge(&self) -> bool {
        crate::supervisor::merge_tag(&self.info) == crate::supervisor::MergeTag::Applies
    }
}

/// The confidence Corgi has in a prompt-cache expiry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PromptCacheKind {
    /// The harness records the cache's configured lifetime.
    #[default]
    Exact,
    /// The harness records a cache hit or write, while the model documents a
    /// minimum retention period. The cache can remain usable after this time.
    Estimated,
}

/// The prompt cache of a session, as read from the newest request its harness
/// recorded. Every request re-warms the cache for the model's lifetime, so an
/// agent that sits idle or waits at a permission prompt for that long pays for
/// its whole context again on the next turn when the expiry is exact.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PromptCache {
    /// Unix seconds when an exact cache lapses, or when an estimated cache's
    /// documented minimum retention ends.
    pub expires_at: u64,
    /// Context tokens from the latest request. For an exact cache, this is the
    /// prompt the next request re-reads at the full input price after expiry.
    pub tokens: u64,
    pub kind: PromptCacheKind,
}

impl PromptCache {
    /// Seconds until the exact cache lapses, or until the estimated cache is
    /// no longer guaranteed warm.
    pub fn remaining(&self, now: u64) -> u64 {
        self.expires_at.saturating_sub(now)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ActivityKind {
    /// A shell command the agent runs.
    Command,
    /// Any other tool the agent calls, such as a file read or edit.
    Tool,
    /// Something the user said to the agent.
    Prompt,
    /// Something the assistant said to the user.
    Message,
    /// The assistant's reasoning, or a progress notice while it works.
    Thinking,
    /// A question the agent is waiting on.
    Question,
    #[default]
    Ready,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    pub kind: ActivityKind,
    pub text: String,
}
