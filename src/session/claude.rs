//! Claude Code transcripts.

use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{
    SessionFacts, TAIL_STEPS, TASK_HEAD_BYTES, context_percent,
    tail::{Fold, Head, Tail},
    text::{
        Conversation, Event, assistant_kind, is_spoken, record_effort, spoken_text, task_prompt,
        tool_call, unwrap_pasted,
    },
};
use crate::{
    harness::{claude_settings, first_setting},
    model::{ActivityKind, PromptCache, PromptCacheKind},
    time::parse_rfc3339,
};

/// Every Claude model answers within one of these windows. A session that has
/// already exceeded its model's default window is running a long-context
/// variant, which is the only case the configured model cannot reveal.
pub(super) const CLAUDE_DEFAULT_WINDOW: u64 = 200_000;
pub(super) const CLAUDE_LONG_WINDOW: u64 = 1_000_000;
/// Digits in the build date some model IDs end with, as in
/// `claude-haiku-4-5-20251001`. Version numbers are far shorter.
pub(super) const MODEL_DATE_DIGITS: usize = 8;
/// The two prompt-cache lifetimes Anthropic offers, in seconds. Claude Code
/// records which one a request used as `cache_creation.ephemeral_5m_input_tokens`
/// and `ephemeral_1h_input_tokens`, and can move between them mid-session.
pub(super) const CLAUDE_CACHE_5M: u64 = 5 * 60;
pub(super) const CLAUDE_CACHE_1H: u64 = 60 * 60;

/// A Claude Code transcript being followed: what its tail says, and the task
/// its head opens with.
#[derive(Debug, Default)]
pub(super) struct ClaudeFile {
    scan: Tail<ClaudeScan>,
    task: Head<String>,
}

impl ClaudeFile {
    /// The facts of the transcript at `path`, after reading what it gained
    /// since the last call.
    pub(super) fn facts(&mut self, path: &Path, configured_model: Option<&str>) -> SessionFacts {
        self.scan.update(path, &TAIL_STEPS, ClaudeScan::of);
        self.task.update(path, TASK_HEAD_BYTES, claude_task);
        self.scan
            .found()
            .facts(self.task.found.clone(), configured_model)
    }
}

/// What the records of a Claude Code transcript say, newest first.
///
/// Subagent records are separate requests with a context and a cache of their
/// own, and describe nothing about the session a row shows.
#[derive(Debug, Default)]
pub(super) struct ClaudeScan {
    conversation: Conversation,
    /// The newest request of the main conversation.
    request: Option<ClaudeRequest>,
    /// The newest moment the main conversation touched the API: an assistant
    /// reply, or a user turn or tool result whose request is in flight.
    touched_at: Option<u64>,
    /// The cache lifetime the newest cache write asked for. A request that
    /// only read the cache writes zero for both lifetimes and says nothing.
    lifetime: Option<u64>,
}

/// The model and usage of one assistant message. The usage of a request is
/// what the next one will carry, so the input side of the newest message is
/// the size of the live context.
#[derive(Debug)]
pub(super) struct ClaudeRequest {
    model: Option<String>,
    effort: Option<String>,
    tokens: u64,
}

impl ClaudeScan {
    pub(super) fn of(record: &Value) -> Self {
        let mut scan = Self::default();
        claude_events(record, &mut |event| scan.conversation.see(&event));
        if record["isSidechain"] == true {
            return scan;
        }
        scan.touched_at = record["timestamp"].as_str().and_then(parse_rfc3339);
        let usage = &record["message"]["usage"];
        scan.lifetime = claude_cache_lifetime(usage);
        if record["type"] == "assistant" && usage.is_object() {
            scan.request = Some(ClaudeRequest {
                model: record["message"]["model"].as_str().map(str::to_string),
                // Claude Code records the effort of a turn beside the
                // message, so the level shows for a Claude session exactly as
                // it does for a Codex one.
                effort: record_effort(record).map(str::to_string),
                tokens: [
                    "input_tokens",
                    "cache_creation_input_tokens",
                    "cache_read_input_tokens",
                ]
                .iter()
                .filter_map(|key| usage[*key].as_u64())
                .sum(),
            });
        }
        scan
    }

    /// A session that has only received its first prompt has a conversation
    /// but no usage yet.
    pub(super) fn facts(
        &self,
        task: Option<String>,
        configured_model: Option<&str>,
    ) -> SessionFacts {
        let mut facts = SessionFacts {
            task,
            message: self.conversation.message(),
            tool: self.conversation.tool(),
            report: self.conversation.report(),
            ..SessionFacts::default()
        };
        let Some(request) = &self.request else {
            return facts;
        };
        facts.effort = request.effort.clone();
        let Some(model_id) = request.model.as_deref() else {
            return facts;
        };
        let window = claude_context_window(model_id, configured_model, request.tokens);
        facts.model = Some(claude_model_name(model_id, window));
        facts.context_percent = Some(context_percent(request.tokens, window));
        facts.context_tokens = Some(request.tokens);
        facts.cache = self.prompt_cache(request.tokens);
        facts
    }

    /// The prompt cache of the session, whose whole context is `tokens`.
    /// Anthropic re-warms a cache on every request that reads it, so the cache
    /// lapses one lifetime after the newest record of the conversation.
    pub(super) fn prompt_cache(&self, tokens: u64) -> Option<PromptCache> {
        Some(PromptCache {
            expires_at: self.touched_at? + self.lifetime?,
            tokens,
            kind: PromptCacheKind::Exact,
        })
    }
}

impl Fold for ClaudeScan {
    fn or(self, older: Self) -> Self {
        Self {
            conversation: self.conversation.or(older.conversation),
            request: self.request.or(older.request),
            touched_at: self.touched_at.or(older.touched_at),
            lifetime: self.lifetime.or(older.lifetime),
        }
    }

    fn complete(&self) -> bool {
        self.conversation.complete()
            && self.request.is_some()
            && self.touched_at.is_some()
            && self.lifetime.is_some()
    }
}

/// What one Claude Code record says.
///
/// Claude Code writes one record per turn and one content block per thing
/// that turn contained: `text` and `thinking` blocks for what was said,
/// `tool_use` blocks for tools. Records of subagents and the meta messages the
/// CLI inserts for itself are not part of the conversation.
pub(super) fn claude_events<'a>(record: &'a Value, emit: &mut dyn FnMut(Event<'a>)) {
    if record["isSidechain"] == true || record["isMeta"] == true {
        return;
    }
    let content = &record["message"]["content"];
    match record["type"].as_str() {
        Some("user") => {
            let spoken = match content.as_str() {
                Some(text) => Some(unwrap_pasted(text)).filter(|text| is_spoken(text)),
                None => spoken_text(content, "text").map(Cow::Borrowed),
            };
            if let Some(text) = spoken {
                emit(Event::said(ActivityKind::Prompt, text));
            }
        }
        Some("assistant") => {
            for block in content.as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(text) = block["text"].as_str() {
                            emit(Event::said(assistant_kind(text), text));
                        }
                    }
                    Some("thinking") => {
                        if let Some(text) = block["thinking"].as_str() {
                            emit(Event::said(ActivityKind::Thinking, text).row_only());
                        }
                    }
                    Some("tool_use") => {
                        let name = block["name"].as_str().unwrap_or("tool");
                        let (kind, text) = tool_call(name, &block["input"]);
                        emit(Event::called(kind, text));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Claude Code's initial human prompt, skipping the CLI's injected and
/// sidechain records. Terminal titles remain the preferred Claude summary;
/// this only fills a row before that title reaches Herdr or when it is absent.
pub(super) fn claude_task(record: &Value) -> Option<String> {
    if record["isSidechain"] == true || record["isMeta"] == true || record["type"] != "user" {
        return None;
    }
    let content = &record["message"]["content"];
    task_prompt(content.as_str().or_else(|| spoken_text(content, "text"))?)
}

/// The cache lifetime a usage record wrote with, in seconds, when it wrote.
pub(super) fn claude_cache_lifetime(usage: &Value) -> Option<u64> {
    let created = |key: &str| usage["cache_creation"][key].as_u64().unwrap_or(0);
    if created("ephemeral_1h_input_tokens") > 0 {
        Some(CLAUDE_CACHE_1H)
    } else if created("ephemeral_5m_input_tokens") > 0 {
        Some(CLAUDE_CACHE_5M)
    } else {
        None
    }
}

/// The context window a Claude Code session is filling.
///
/// A transcript records the model family but never the long-context variant of
/// it, so the window comes from the model configured for Claude Code, and from
/// the session itself once it has outgrown the default window.
pub(super) fn claude_context_window(
    model_id: &str,
    configured_model: Option<&str>,
    tokens: u64,
) -> u64 {
    let long_context = configured_model.is_some_and(|configured| {
        configured.contains("[1m]") && shares_model_family(model_id, configured)
    });
    if long_context || claude_model_family(model_id) == "fable" || tokens > CLAUDE_DEFAULT_WINDOW {
        return CLAUDE_LONG_WINDOW;
    }
    CLAUDE_DEFAULT_WINDOW
}

/// Whether a configured model such as `opus[1m]` selects the family of a model
/// ID such as `claude-opus-5`.
pub(super) fn shares_model_family(model_id: &str, configured_model: &str) -> bool {
    let family = claude_model_family(model_id);
    !family.is_empty() && configured_model.contains(&family)
}

pub(super) fn claude_model_family(model_id: &str) -> String {
    model_id
        .trim_start_matches("claude-")
        .split('-')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A model ID as Claude Code writes it, shortened for one dashboard row:
/// `claude-fable-5-1` reads as `Fable 5.1`, and a long-context session says so
/// because it changes what its percentage means.
pub(super) fn claude_model_name(model_id: &str, window: u64) -> String {
    let mut parts = model_id.trim_start_matches("claude-").split('-');
    let Some(family) = parts.next().filter(|family| !family.is_empty()) else {
        return model_id.to_string();
    };
    let mut name = family[..1].to_uppercase() + &family[1..];
    // Everything after the family is a version, except the dated build suffix
    // some model IDs carry.
    let version: Vec<&str> = parts
        .filter(|part| part.len() < MODEL_DATE_DIGITS || !part.chars().all(char::is_numeric))
        .collect();
    if !version.is_empty() {
        name.push(' ');
        name.push_str(&version.join("."));
    }
    if window > CLAUDE_DEFAULT_WINDOW && claude_model_family(model_id) != "fable" {
        name.push_str(" 1M");
    }
    name
}

/// The model Claude Code is configured to use, including the `[1m]` marker of
/// a long-context variant, from the settings in its configuration `dirs`.
/// Local settings override the user's own.
pub(super) fn claude_model_setting(dirs: &[PathBuf]) -> Option<String> {
    first_setting(&claude_settings(dirs), |settings| &settings["model"])
}
