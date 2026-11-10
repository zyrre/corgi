use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::harness::Harness;

/// An agent's state, as its harness reports it to hc. It is written, in
/// lower case, into the daemon's record of the agents it follows.
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
}

/// The session an agent's harness keeps, as Corgi labelled its terminal
/// with it at launch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentSession {
    /// The harness whose session it is, as its kind.
    pub agent: String,
    /// The harness's own id of the session, which names its file.
    pub value: String,
}

/// What herdr-core says of an agent's terminal, and what Corgi labelled it
/// with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentInfo {
    /// The harness's kind: `claude`, `codex`, …
    pub agent: Option<String>,
    /// The terminal's name, else its address.
    pub name: Option<String>,
    pub state: AgentState,
    /// The terminal's address (`t7`), which every `hc` command takes.
    pub address: String,
    /// The Herdr app space the terminal is in (`herdr.space`), or empty.
    pub space: String,
    /// The folder the agent works in.
    pub cwd: Option<String>,
    /// hc's count of the terminal's turns, which goes up at every change of
    /// state.
    pub state_change_seq: u64,
    /// The title the harness gave its terminal.
    pub title: Option<String>,
    /// Corgi's labels on the terminal, by the short names the rest of Corgi
    /// reads them by: the spawn's request id, the task, the merge tag, and
    /// a supervisor's handover request and baseline.
    pub tokens: BTreeMap<String, String>,
    pub agent_session: Option<AgentSession>,
}

impl AgentInfo {
    pub fn kind(&self) -> &str {
        self.agent.as_deref().unwrap_or("agent")
    }

    /// The harness that runs in this terminal.
    pub fn harness(&self) -> Harness {
        Harness::from_kind(self.kind())
    }

    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or_else(|| self.kind())
    }

    pub fn cwd(&self) -> &str {
        self.cwd.as_deref().unwrap_or("")
    }
}

/// One agent Corgi runs: its terminal, and what its project and its
/// session file say of it.
#[derive(Debug, Clone, Default)]
pub struct Agent {
    pub info: AgentInfo,
    /// The name of the repository related sessions share, which they are
    /// sorted by; `Scratch` for the scratch sessions.
    pub project_group: String,
    /// The project the agent belongs to: its repository's primary checkout,
    /// as its terminal's `corgi.project` label holds it.
    pub project_root: String,
    /// Path of the agent's linked worktree checkout, when it has one: what
    /// `corgi merge` merges from, and `corgi close` removes.
    pub worktree_checkout: Option<String>,
    /// A few words describing what the agent is working on, as the agent CLI
    /// itself summarizes the session.
    pub task: String,
    /// Model answering this session, read from the agent CLI's own session
    /// file.
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
    /// The project's supervisor (`corgi.role=supervisor`), first in its
    /// project.
    pub supervisor: bool,
    /// The agent runs in the home directory itself, which is never a
    /// project: a scratch session, grouped under Scratch, that no supervisor
    /// hears about.
    pub scratch: bool,
    /// A supervisor whose agent has exited into the kept shell it was
    /// started from: `corgi supervisor` and `corgi open` start it again
    /// there.
    pub exited: bool,
}

impl Agent {
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

/// Seconds left on a prompt cache, at the precision that matters for it: hours
/// only when at least an hour is left, minutes otherwise, and seconds when the
/// cache is about to lapse, so the field visibly counts down at the end.
pub fn cache_countdown(remaining: u64) -> String {
    if remaining >= 3_600 {
        format!("{}h", remaining / 3_600)
    } else if remaining >= 60 {
        format!("{}m", remaining / 60)
    } else {
        format!("{remaining}s")
    }
}

/// A token count short enough for a row field: `40k`, `1.2M`.
pub fn compact_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}k", tokens.div_ceil(1_000))
    } else {
        tokens.to_string()
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
