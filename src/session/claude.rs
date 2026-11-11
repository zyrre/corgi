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
        Conversation, Event, Mark, assistant_kind, is_spoken, record_effort, spoken_text,
        task_prompt, tool_call, unwrap_pasted,
    },
};
use crate::{
    harness::{claude_settings, first_setting},
    model::{ActivityKind, PromptCache, PromptCacheKind},
    time::parse_rfc3339,
};

/// Every Claude model answers within one of these windows, as Claude Code's own
/// model table has it; see [`claude_context_window`].
pub const CLAUDE_DEFAULT_WINDOW: u64 = 200_000;
pub const CLAUDE_LONG_WINDOW: u64 = 1_000_000;
/// The tag that asks Claude Code for a model's long-context variant, as in
/// `claude-opus-4-6[1m]`. Claude Code matches it in any case.
const LONG_CONTEXT_TAG: &str = "[1m]";
/// The environment variable that holds every session to the default window.
const DISABLE_LONG_CONTEXT: &str = "CLAUDE_CODE_DISABLE_1M_CONTEXT";
/// Digits in the build date some model IDs end with, as in
/// `claude-haiku-4-5-20251001`. Version numbers are far shorter.
pub const MODEL_DATE_DIGITS: usize = 8;
/// The two prompt-cache lifetimes Anthropic offers, in seconds. Claude Code
/// records which one a request used as `cache_creation.ephemeral_5m_input_tokens`
/// and `ephemeral_1h_input_tokens`, and can move between them mid-session.
pub const CLAUDE_CACHE_5M: u64 = 5 * 60;
pub const CLAUDE_CACHE_1H: u64 = 60 * 60;

/// A Claude Code transcript being followed: what its tail says, and the task
/// its head opens with.
#[derive(Debug, Default)]
pub struct ClaudeFile {
    scan: Tail<ClaudeScan>,
    task: Head<String>,
}

impl ClaudeFile {
    /// The facts of the transcript at `path`, after reading what it gained
    /// since the last call.
    pub fn facts(&mut self, path: &Path, settings: &ClaudeWindowSettings) -> SessionFacts {
        self.scan.update(path, &TAIL_STEPS, ClaudeScan::of);
        self.task.update(path, TASK_HEAD_BYTES, claude_task);
        self.scan.found().facts(self.task.found.clone(), settings)
    }
}

/// What the records of a Claude Code transcript say, newest first.
///
/// Subagent records are separate requests with a context and a cache of their
/// own, and describe nothing about the session a row shows.
#[derive(Debug, Default)]
pub struct ClaudeScan {
    conversation: Conversation,
    /// The newest request of the main conversation.
    request: Option<ClaudeRequest>,
    /// The newest moment the main conversation touched the API: an assistant
    /// reply, or a user turn or tool result whose request is in flight. The
    /// row shows it as the session's last activity.
    touched_at: Option<u64>,
    /// The cache lifetime the newest cache write asked for. A request that
    /// only read the cache writes zero for both lifetimes and says nothing.
    lifetime: Option<u64>,
}

/// The model and usage of one assistant message. The usage of a request is
/// what the next one will carry, so the input side of the newest message is
/// the size of the live context.
#[derive(Debug)]
pub struct ClaudeRequest {
    model: Option<String>,
    /// The model the session asked for, which keeps the long-context tag the
    /// answering `model` never has. Older Claude Code versions do not write it.
    requested_model: Option<String>,
    effort: Option<String>,
    tokens: u64,
}

impl ClaudeScan {
    pub fn of(record: &Value) -> Self {
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
                requested_model: record["requestedModel"].as_str().map(str::to_string),
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
    pub fn facts(&self, task: Option<String>, settings: &ClaudeWindowSettings) -> SessionFacts {
        let mut facts = SessionFacts {
            task,
            report: self.conversation.report(),
            turn: self.conversation.turn(),
            ..SessionFacts::default()
        };
        let Some(request) = &self.request else {
            return facts;
        };
        facts.effort = request.effort.clone();
        let Some(model_id) = request.model.as_deref() else {
            return facts;
        };
        let window = claude_context_window(
            model_id,
            request.requested_model.as_deref(),
            settings,
            request.tokens,
        );
        facts.model = Some(claude_model_name(model_id, window));
        facts.context_percent = Some(context_percent(request.tokens, window));
        facts.context_tokens = Some(request.tokens);
        facts.cache = self.prompt_cache(request.tokens);
        facts
    }

    /// The prompt cache of the session, whose whole context is `tokens`.
    /// Anthropic re-warms a cache on every request that reads it, so the cache
    /// lapses one lifetime after the newest record of the conversation.
    pub fn prompt_cache(&self, tokens: u64) -> Option<PromptCache> {
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
/// CLI inserts for itself are not part of the conversation, though a meta
/// message, like any user record holding text, starts a turn: Claude Code
/// starts one with another session's message or a scheduled task. A
/// `turn_duration` record follows the turn's last reply and ends it.
pub fn claude_events<'a>(record: &'a Value, emit: &mut dyn FnMut(Event<'a>)) {
    if record["isSidechain"] == true {
        return;
    }
    let content = &record["message"]["content"];
    let meta = record["isMeta"] == true;
    match record["type"].as_str() {
        Some("user") => {
            let spoken = match content.as_str() {
                _ if meta => None,
                Some(text) => Some(unwrap_pasted(text)).filter(|text| is_spoken(text)),
                None => spoken_text(content, "text").map(Cow::Borrowed),
            };
            // A tool result is the only user record that holds no text.
            let holds_text = content.is_string()
                || content
                    .as_array()
                    .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "text"));
            match spoken {
                Some(text) => emit(Event::said(ActivityKind::Prompt, text)),
                None if holds_text => emit(Event::marks(Mark::Start)),
                None => {}
            }
        }
        Some("system") if record["subtype"] == "turn_duration" => emit(Event::marks(Mark::End)),
        _ if meta => {}
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
                            emit(Event::said(ActivityKind::Thinking, text));
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
/// sidechain records. The task Corgi labelled the terminal with and its
/// title come first for Claude Code; this is the fallback.
pub fn claude_task(record: &Value) -> Option<String> {
    if record["isSidechain"] == true || record["isMeta"] == true || record["type"] != "user" {
        return None;
    }
    let content = &record["message"]["content"];
    task_prompt(content.as_str().or_else(|| spoken_text(content, "text"))?)
}

/// The cache lifetime a usage record wrote with, in seconds, when it wrote.
pub fn claude_cache_lifetime(usage: &Value) -> Option<u64> {
    let created = |key: &str| usage["cache_creation"][key].as_u64().unwrap_or(0);
    if created("ephemeral_1h_input_tokens") > 0 {
        Some(CLAUDE_CACHE_1H)
    } else if created("ephemeral_5m_input_tokens") > 0 {
        Some(CLAUDE_CACHE_5M)
    } else {
        None
    }
}

/// What Claude Code's settings say about the context window of its sessions.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ClaudeWindowSettings {
    /// The configured model, which may carry the long-context tag.
    pub model: Option<String>,
    /// Whether `CLAUDE_CODE_DISABLE_1M_CONTEXT` is on. Nothing in a transcript
    /// shows it.
    pub long_context_disabled: bool,
}

/// The context window a Claude Code session is filling, in the order that
/// settles it:
///
/// 1. A session past the default window has a long one: no window is exceeded,
///    so this holds whatever the settings say now.
/// 2. `CLAUDE_CODE_DISABLE_1M_CONTEXT` holds the rest to the default window.
/// 3. The long-context tag on the model the session asked for. Only when the
///    record has no requested model, the tag on the configured model of the
///    same family stands in for it.
/// 4. The window the model has without being asked ([`claude_natively_long`]).
///
/// Everything else has the default window, which is also what Claude Code
/// assumes of a model it does not know.
pub fn claude_context_window(
    model_id: &str,
    requested_model: Option<&str>,
    settings: &ClaudeWindowSettings,
    tokens: u64,
) -> u64 {
    if tokens > CLAUDE_DEFAULT_WINDOW {
        return CLAUDE_LONG_WINDOW;
    }
    if settings.long_context_disabled {
        return CLAUDE_DEFAULT_WINDOW;
    }
    let tagged = match requested_model {
        Some(requested) => has_long_context_tag(requested),
        None => settings.model.as_deref().is_some_and(|configured| {
            has_long_context_tag(configured) && shares_model_family(model_id, configured)
        }),
    };
    if tagged || claude_natively_long(model_id) {
        CLAUDE_LONG_WINDOW
    } else {
        CLAUDE_DEFAULT_WINDOW
    }
}

fn has_long_context_tag(model: &str) -> bool {
    model.to_ascii_lowercase().contains(LONG_CONTEXT_TAG)
}

/// Whether a model has the long window without being asked for it. This follows
/// the model table inside Claude Code (2.1.318), which holds no older model
/// than these in the long window; a newer version of one of these families is
/// taken to keep it.
pub fn claude_natively_long(model_id: &str) -> bool {
    let Some((family, version)) = claude_model_version(model_id) else {
        return false;
    };
    match family {
        "fable" | "mythos" => true,
        "opus" => version >= (4, 7),
        "sonnet" | "haiku" => version >= (5, 0),
        _ => false,
    }
}

/// The family and the major and minor version of a Claude model ID, which a
/// cloud provider may prefix or date: `us.anthropic.claude-opus-4-8` is Opus
/// 4.8, and `claude-sonnet-4-20250514` is Sonnet 4.0. A name without `claude-`
/// is a gateway's own, and says nothing.
fn claude_model_version(model_id: &str) -> Option<(&str, (u32, u32))> {
    let (_, name) = model_id.rsplit_once("claude-")?;
    let mut parts = name.split(['-', '@', '[']);
    let family = parts.next()?;
    let mut version = parts
        .take_while(|part| part.len() < MODEL_DATE_DIGITS)
        .map_while(|part| part.parse().ok());
    Some((
        family,
        (version.next().unwrap_or(0), version.next().unwrap_or(0)),
    ))
}

/// Whether a configured model such as `opus[1m]` selects the family of a model
/// ID such as `claude-opus-5`.
pub fn shares_model_family(model_id: &str, configured_model: &str) -> bool {
    let family = claude_model_family(model_id);
    !family.is_empty() && configured_model.contains(&family)
}

pub fn claude_model_family(model_id: &str) -> String {
    model_id
        .trim_start_matches("claude-")
        .split('-')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A model ID as Claude Code writes it, shortened for `corgi fleet`'s MODEL:
/// `claude-fable-5-1` reads as `Fable 5.1`. A session in a long window its model
/// does not have by default says so, because it changes what its percentage
/// means.
pub fn claude_model_name(model_id: &str, window: u64) -> String {
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
    if window > CLAUDE_DEFAULT_WINDOW && !claude_natively_long(model_id) {
        name.push_str(" 1M");
    }
    name
}

/// What the settings in Claude Code's configuration `dirs` say about the window
/// of its sessions, local settings over the user's own, as Claude Code reads
/// them. `environment` is the value of `CLAUDE_CODE_DISABLE_1M_CONTEXT` a session
/// would inherit, which its settings override.
pub fn claude_window_settings(dirs: &[PathBuf], environment: Option<&str>) -> ClaudeWindowSettings {
    let settings = claude_settings(dirs);
    let disabled = first_setting(&settings, |value| &value["env"][DISABLE_LONG_CONTEXT]);
    ClaudeWindowSettings {
        model: first_setting(&settings, |value| &value["model"]),
        long_context_disabled: disabled.as_deref().or(environment).is_some_and(env_truthy),
    }
}

/// Whether Claude Code takes an environment variable's value as on.
fn env_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// The value of `CLAUDE_CODE_DISABLE_1M_CONTEXT` in Corgi's own environment,
/// the nearest thing it can read to what a session in a terminal inherited.
pub fn claude_disable_long_context_env() -> Option<String> {
    std::env::var(DISABLE_LONG_CONTEXT).ok()
}
