//! Facts read from the session files the agent CLIs write for themselves: the
//! model answering, its effort, how full its context window is, its prompt
//! cache, the original task, and the newest reply, which is an agent's
//! report.
//!
//! herdr-core does not say which model answers in a terminal, so Corgi reads
//! the transcripts. The same files hold every prompt, reply, thought, and
//! tool call in full, where the terminal shows an abbreviated and scrolling
//! picture of them:
//!
//! - Claude Code appends every turn to
//!   `~/.claude/projects/<slugged cwd>/<session id>.jsonl`. The newest
//!   assistant message carries the model that answered and the exact token
//!   usage of that request; `text`, `thinking`, and `tool_use` blocks carry
//!   the conversation. Every record is timestamped, and the usage says which
//!   prompt-cache lifetime the request was written with, which together give
//!   the moment the session's cache lapses.
//! - Codex appends `~/.codex/sessions/<y>/<m>/<d>/rollout-<stamp>-<thread>.jsonl`,
//!   whose `token_count` events carry the usage together with the model's own
//!   context window, and whose `response_item` records carry the conversation.
//!   Every record is timestamped there too. The first real user prompt is
//!   the session's task. OpenAI's prompt cache is automatic and its exact lifetime is not reported.
//!   For GPT-5.6, a cache hit or write does establish the documented 30-minute
//!   minimum retention period, so Corgi can show a clearly marked estimate.
//!
//! Corgi labels an agent's terminal with its session id at launch
//! (`agent_session.value`), so the right file is found without guessing
//! which of the running sessions a terminal belongs to. A Claude Code
//! terminal without one is found by its working directory, which no other
//! live Claude Code terminal may share.
//!
//! The daemon reads every agent's file at each pass, so the reading is
//! incremental. The first read of a file scans back from its end only as far
//! as it must to find every fact, and each later read parses only the records
//! appended since, keeping what older records said until a newer one says
//! otherwise. Both harnesses reduce a record to the same events.
//!
//! Every reader fails soft: a session file that is missing, unreadable, or
//! holds no usage record yet leaves the agent without a model and without a
//! percentage, rather than showing a wrong one.

mod claude;
mod codex;
mod locate;
mod tail;
mod text;

use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration, Instant},
};

use self::{
    claude::{
        ClaudeFile, ClaudeWindowSettings, claude_disable_long_context_env, claude_window_settings,
    },
    codex::CodexFile,
    locate::{Search, claude_transcript, codex_rollout},
    tail::file_length,
};
use crate::{
    harness::{SessionFormat, claude_config_dirs},
    model::{Activity, AgentInfo, PromptCache},
    paths::home,
};

/// Tails of a session file scanned for the newest usage record. A single tool
/// result can be megabytes, so the window grows before the file is given up on
/// rather than reading all of it at once. The same windows bound a read of
/// what was appended since, should a file grow by more than one read's worth.
const TAIL_STEPS: [u64; 3] = [64 * 1024, 512 * 1024, 4 * 1024 * 1024];
/// The first user prompt is normally close to the beginning of a transcript.
/// Bound this fallback so a large, active session never makes a read take
/// time proportional to its complete history.
const TASK_HEAD_BYTES: u64 = 512 * 1024;
/// How long a failed lookup is trusted. A session that has not written its
/// file yet must be found soon after, without rescanning on every read.
const MISS_RETRY: Duration = Duration::from_secs(5);
/// How long a failed Codex lookup keeps its retries to the newest days of the
/// sessions tree before searching all of it again, for a session resumed from
/// an older day in the meantime.
const FULL_SEARCH_RETRY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionFacts {
    /// Display name of the model that answered the newest turn.
    pub model: Option<String>,
    /// Reasoning effort the newest turn used, when the harness records it.
    pub effort: Option<String>,
    /// Share of the model's context window the session currently occupies.
    pub context_percent: Option<u8>,
    /// Tokens of that context, which a changed window does not rescale.
    pub context_tokens: Option<u64>,
    /// When the prompt cache holding that context lapses, and how much it then
    /// costs to rebuild. Only Claude Code records enough to say.
    pub cache: Option<PromptCache>,
    /// The first human prompt in this session. This is a cross-harness fallback
    /// for a generated task title that is not available yet.
    pub task: Option<String>,
    /// The newest assistant reply or question, whole: the source of `corgi
    /// report` and of the report a wake quotes.
    pub report: Option<Activity>,
    /// How the newest turn stands in the file, which says whether `report`
    /// is that turn's: unknown when the file shows no prompt, reply or tool
    /// call.
    pub turn: Option<TurnState>,
}

/// How the newest turn of a session stands in its file.
///
/// A turn starts with a user record: a prompt, a brief, a `[supervisor]` or
/// `[Corgi]` line typed in, or something the CLI starts one with itself (a
/// background task's notification, another session's message); a tool
/// result does not. The CLIs write their files a little after the screen
/// shows the turn over, so a reader can find the turn's prompt or its last
/// tool call there and not yet its reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    /// A reply is the newest thing since the turn started and since its
    /// last tool call: the turn's report.
    Answered,
    /// The file marks the turn ended with no reply after its start or its
    /// last tool call: the turn said nothing to report.
    Silent,
    /// Neither yet: the turn's reply may still be on its way to the file.
    Open,
}

/// Resolves the session file of every agent and follows what it says.
///
/// Lookups and reads are cached: a file is read again only when it has grown,
/// and then only the records appended since, which is what makes a
/// call per pass cheap even while several agents are working.
#[derive(Debug, Default)]
pub struct SessionReader {
    files: HashMap<String, Lookup>,
    parsed: HashMap<PathBuf, ParsedFacts>,
    /// What Claude Code's settings say about its window, read once per reader.
    claude_window_settings: OnceLock<ClaudeWindowSettings>,
    /// The directory session files are looked for under, when it is not
    /// `$HOME`. It stands in for `CLAUDE_CONFIG_DIR` too.
    home: Option<PathBuf>,
    /// Working directories more than one live Claude Code agent runs in, as
    /// of the last [`retain`](Self::retain). The newest transcript of one of
    /// them could be any of those agents', so it is never one's fallback.
    shared_claude_cwds: HashSet<String>,
}

/// Where the session file of one agent was found, if anywhere, and when.
#[derive(Debug)]
struct Lookup {
    path: Option<PathBuf>,
    at: Instant,
    /// Whether the file is only the newest of the working directory, which a
    /// session started since in the same terminal replaces.
    by_cwd: bool,
    /// When a lookup last searched every Codex day rather than the newest.
    searched_everywhere_at: Instant,
}

/// The facts of one session file, as of the length it had when read.
#[derive(Debug)]
struct ParsedFacts {
    length: Option<u64>,
    facts: SessionFacts,
    file: HarnessFile,
}

#[derive(Debug)]
enum HarnessFile {
    Claude(ClaudeFile),
    Codex(CodexFile),
}

impl SessionReader {
    /// A reader for lookups among `live`, the agents now running, so one
    /// without a session id is not given the transcript of another.
    pub fn among<'a>(live: impl Iterator<Item = &'a AgentInfo>) -> Self {
        let mut reader = Self::default();
        reader.retain(live);
        reader
    }

    /// A reader that looks for session files under `home` in place of
    /// `$HOME`, which holds no Claude Code settings.
    #[cfg(test)]
    pub fn in_home(home: &Path) -> Self {
        Self {
            home: Some(home.to_path_buf()),
            ..Self::default()
        }
    }

    /// Facts for one agent, or an empty set when its CLI writes no session
    /// file, the file is not readable, or it holds no usage yet.
    pub fn facts(&mut self, info: &AgentInfo) -> SessionFacts {
        let Some(path) = self.session_file(info) else {
            return SessionFacts::default();
        };
        let length = file_length(&path);
        let parsed = match self.parsed.entry(path.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let file = match info.harness().session_format() {
                    Some(SessionFormat::Claude) => HarnessFile::Claude(ClaudeFile::default()),
                    Some(SessionFormat::Codex) => HarnessFile::Codex(CodexFile::default()),
                    None => return SessionFacts::default(),
                };
                entry.insert(ParsedFacts {
                    length: None,
                    facts: SessionFacts::default(),
                    file,
                })
            }
        };
        if parsed.length != Some(length) {
            parsed.facts = match &mut parsed.file {
                HarnessFile::Claude(file) => {
                    let settings = self.claude_window_settings.get_or_init(|| {
                        // A private home stands in for the whole environment.
                        let environment = self
                            .home
                            .is_none()
                            .then(claude_disable_long_context_env)
                            .flatten();
                        claude_window_settings(
                            &claude_dirs(self.home.as_deref()),
                            environment.as_deref(),
                        )
                    });
                    file.facts(&path, settings)
                }
                HarnessFile::Codex(file) => file.facts(&path),
            };
            parsed.length = Some(length);
        }
        parsed.facts.clone()
    }

    /// Forgets the sessions of terminals that are gone, so the daemon does
    /// not accumulate the transcripts of closed agents, and notes which
    /// working directories the live Claude Code terminals share.
    pub fn retain<'a>(&mut self, live: impl Iterator<Item = &'a AgentInfo>) {
        let live: Vec<&AgentInfo> = live.collect();
        let mut claude_cwds = HashSet::new();
        self.shared_claude_cwds = live
            .iter()
            .filter(|info| info.harness().session_format() == Some(SessionFormat::Claude))
            .filter(|info| !claude_cwds.insert(info.cwd()))
            .map(|info| info.cwd().to_string())
            .collect();
        let keys: HashSet<String> = live.into_iter().map(session_key).collect();
        self.files.retain(|key, _| keys.contains(key));
        let paths: HashSet<&PathBuf> = self
            .files
            .values()
            .filter_map(|lookup| lookup.path.as_ref())
            .collect();
        self.parsed.retain(|path, _| paths.contains(path));
    }

    fn session_file(&mut self, info: &AgentInfo) -> Option<PathBuf> {
        // A Codex agent without a session id shares its cwd with other live
        // and historical sessions: never borrow one of their transcripts.
        // The same holds for a Claude Code agent in a directory another
        // live one works in: the newest transcript there may be the other's.
        let session_id = session_id(info);
        let by_cwd = session_id.is_none();
        if by_cwd
            && match info.harness().session_format() {
                Some(SessionFormat::Codex) => true,
                Some(SessionFormat::Claude) => self.shared_claude_cwds.contains(info.cwd()),
                None => false,
            }
        {
            return None;
        }
        let key = session_key(info);
        let previous = self.files.get(&key);
        // A file found by name stays the session's; the newest of a
        // directory is looked for again once in a while like a miss.
        if let Some(lookup) = previous
            && ((lookup.path.is_some() && !lookup.by_cwd) || lookup.at.elapsed() < MISS_RETRY)
        {
            return lookup.path.clone();
        }
        // A retry after a miss looks only where a new session file appears,
        // and only once in a while everywhere again.
        let (search, searched_everywhere_at) = match previous {
            Some(lookup) if lookup.searched_everywhere_at.elapsed() < FULL_SEARCH_RETRY => {
                (Search::Recent, lookup.searched_everywhere_at)
            }
            _ => (Search::Everywhere, Instant::now()),
        };
        let path = match info.harness().session_format() {
            Some(SessionFormat::Claude) => claude_dirs(self.home.as_deref())
                .iter()
                .find_map(|dir| claude_transcript(dir, session_id, info.cwd())),
            Some(SessionFormat::Codex) => self
                .home
                .clone()
                .or_else(home)
                .and_then(|home| codex_rollout(&home, session_id, info.cwd(), search)),
            None => None,
        };
        self.files.insert(
            key,
            Lookup {
                path: path.clone(),
                at: Instant::now(),
                by_cwd,
                searched_everywhere_at,
            },
        );
        path
    }
}

/// Claude Code's configuration directories, which hold its settings and its
/// transcripts: the ones in [`claude_config_dirs`], or the one in `home` when
/// a directory stands in for `$HOME`.
fn claude_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    match home {
        Some(home) => vec![home.join(".claude")],
        None => claude_config_dirs(),
    }
}

/// The id of the session in the agent's terminal, as Corgi labelled the
/// terminal with it at launch.
fn session_id(info: &AgentInfo) -> Option<&str> {
    info.agent_session
        .as_ref()
        .map(|session| session.value.trim())
        // An identity names a file in a directory, never a path.
        .filter(|session| !session.is_empty() && !session.contains('/'))
}

/// Identity of the session in one terminal. The address alone would keep a
/// stale transcript after a new session starts in the same terminal.
fn session_key(info: &AgentInfo) -> String {
    let session = session_id(info).unwrap_or_default();
    format!("{}:{}:{session}", info.kind(), info.address)
}

fn context_percent(tokens: u64, window: u64) -> u8 {
    if window == 0 {
        return 0;
    }
    let percent = (tokens as f64 / window as f64 * 100.0).round();
    percent.clamp(0.0, 100.0) as u8
}

#[cfg(test)]
mod tests;
