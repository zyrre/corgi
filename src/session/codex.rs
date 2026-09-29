//! Codex rollouts.

use std::{path::Path, sync::LazyLock};

use regex::Regex;
use serde_json::Value;

use super::{
    SessionFacts, TAIL_STEPS, TASK_HEAD_BYTES, context_percent,
    tail::{Fold, Head, Tail},
    text::{
        Conversation, Event, assistant_kind, field, is_spoken, record_effort, shell_command,
        spoken_text, task_prompt, tool_call,
    },
};
use crate::{
    model::{ActivityKind, PromptCache, PromptCacheKind},
    time::parse_rfc3339,
};

/// GPT-5.6's documented minimum prompt-cache retention after a hit or write.
/// Codex does not record the configured policy, so this is deliberately used
/// only for the GPT-5.6 family and rendered as an estimate.
pub(super) const CODEX_CACHE_5_6: u64 = 30 * 60;
pub(super) static CODEX_EXEC_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)(?:"cmd"|cmd)\s*:\s*("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`)"#)
        .expect("valid Codex exec command regex")
});
pub(super) static JAVASCRIPT_STRING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`"#)
        .expect("valid JavaScript string regex")
});
pub(super) static JAVASCRIPT_TOOL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\btools\.([A-Za-z_$][A-Za-z0-9_$]*)\s*\(").expect("valid JavaScript tool regex")
});

/// A Codex rollout being followed: what its tail says, and the task its head
/// opens with.
#[derive(Debug, Default)]
pub(super) struct CodexFile {
    scan: Tail<CodexScan>,
    task: Head<String>,
}

impl CodexFile {
    /// The facts of the rollout at `path`, after reading what it gained since
    /// the last call.
    pub(super) fn facts(&mut self, path: &Path) -> SessionFacts {
        self.scan.update(path, &TAIL_STEPS, CodexScan::of);
        self.task.update(path, TASK_HEAD_BYTES, codex_task);
        self.scan.found().facts(self.task.found.clone())
    }
}

/// What the records of a Codex rollout say, newest first. Codex records the
/// window of the model it is talking to, so no table of models is needed.
#[derive(Debug, Default)]
pub(super) struct CodexScan {
    conversation: Conversation,
    /// The newest token count.
    usage: Option<CodexUsage>,
    /// Codex names the model in the turn context it records per turn, and in
    /// the session metadata the rollout opens with.
    model: Option<String>,
    effort: Option<String>,
    /// The newest request that read or wrote the prompt cache.
    cache_hit: Option<CodexCacheHit>,
}

#[derive(Debug)]
pub(super) struct CodexUsage {
    /// Absent when the count does not record the model's window.
    context_percent: Option<u8>,
    tokens: u64,
}

#[derive(Debug)]
pub(super) struct CodexCacheHit {
    at: Option<u64>,
    tokens: u64,
}

impl CodexScan {
    pub(super) fn of(record: &Value) -> Self {
        let mut scan = Self::default();
        codex_events(record, &mut |event| scan.conversation.see(&event));
        let payload = &record["payload"];
        if payload["type"] == "token_count" && payload["info"].is_object() {
            let info = &payload["info"];
            let last = &info["last_token_usage"];
            let tokens = ["input_tokens", "output_tokens"]
                .iter()
                .filter_map(|key| last[*key].as_u64())
                .sum::<u64>();
            scan.usage = Some(CodexUsage {
                context_percent: info["model_context_window"]
                    .as_u64()
                    .filter(|window| *window > 0)
                    .map(|window| context_percent(tokens, window)),
                tokens,
            });
        }
        scan.model = field(record, "model").map(str::to_string);
        scan.effort = record_effort(record).map(str::to_string);
        scan.cache_hit = codex_cache_usage(record)
            .filter(|usage| {
                usage["cached_input_tokens"].as_u64().unwrap_or(0) > 0
                    || usage["cache_write_input_tokens"].as_u64().unwrap_or(0) > 0
            })
            .map(|usage| CodexCacheHit {
                at: record["timestamp"].as_str().and_then(parse_rfc3339),
                tokens: usage["input_tokens"].as_u64().unwrap_or(0),
            });
        scan
    }

    pub(super) fn facts(&self, task: Option<String>) -> SessionFacts {
        SessionFacts {
            model: self.model.clone(),
            effort: self.effort.clone(),
            context_percent: self.usage.as_ref().and_then(|usage| usage.context_percent),
            context_tokens: self.usage.as_ref().map(|usage| usage.tokens),
            cache: self.prompt_cache(),
            task,
            message: self.conversation.message(),
            tool: self.conversation.tool(),
        }
    }

    /// A conservative Codex cache estimate. A cache read or write proves that
    /// a reusable prefix existed for this request; the documented GPT-5.6
    /// lifetime keeps it eligible for at least thirty minutes after that
    /// request. It is not an exact expiry: the backend can retain it longer,
    /// and a changed prefix can still miss it.
    pub(super) fn prompt_cache(&self) -> Option<PromptCache> {
        self.model
            .as_deref()
            .filter(|model| codex_has_documented_cache_minimum(model))?;
        let hit = self.cache_hit.as_ref()?;
        Some(PromptCache {
            expires_at: hit.at? + CODEX_CACHE_5_6,
            tokens: hit.tokens,
            kind: PromptCacheKind::Estimated,
        })
    }
}

impl Fold for CodexScan {
    fn or(self, older: Self) -> Self {
        Self {
            conversation: self.conversation.or(older.conversation),
            usage: self.usage.or(older.usage),
            model: self.model.or(older.model),
            effort: self.effort.or(older.effort),
            cache_hit: self.cache_hit.or(older.cache_hit),
        }
    }

    fn complete(&self) -> bool {
        self.conversation.complete()
            && self.usage.is_some()
            && self.model.is_some()
            && self.effort.is_some()
            && self.cache_hit.is_some()
    }
}

/// GPT-5.6 exposes a 30-minute minimum cache lifetime. Keeping this deliberately
/// narrow prevents us from guessing the retention policy of older Codex models.
pub(super) fn codex_has_documented_cache_minimum(model: &str) -> bool {
    matches!(model.trim(), "gpt-5.6") || model.trim().starts_with("gpt-5.6-")
}

/// Codex has used both a per-response usage record and a terminal token-count
/// event for the same facts across CLI releases.
pub(super) fn codex_cache_usage(record: &Value) -> Option<&Value> {
    match (record["type"].as_str(), record["payload"]["type"].as_str()) {
        (Some("token_usage_record"), _) => record["payload"].get("usage"),
        (Some("event_msg"), Some("token_count")) => {
            record["payload"]["info"].get("last_token_usage")
        }
        _ => None,
    }
}

/// Codex's initial real user prompt. The rollout also records harness context
/// as user messages, so XML-tagged context and the injected AGENTS block are
/// deliberately skipped. Codex's generated app-server name takes precedence
/// when available; this is the immediate, portable fallback.
pub(super) fn codex_task(record: &Value) -> Option<String> {
    let payload = &record["payload"];
    let text = match (record["type"].as_str(), payload["type"].as_str()) {
        (Some("event_msg"), Some("user_message")) => payload["message"].as_str(),
        (Some("response_item"), Some("message")) if payload["role"].as_str() == Some("user") => {
            spoken_text(&payload["content"], "input_text")
        }
        _ => None,
    };
    task_prompt(text?)
}

/// What one Codex record says.
///
/// Codex records the conversation as `response_item` payloads: `message`
/// items for either side, `reasoning` items with a summary of the model's
/// thinking, and `function_call`, `local_shell_call`, or `custom_tool_call`
/// items for tools. The `event_msg` stream repeats some of these as they
/// happen. The row reads it too, so a command shows while it still runs; the
/// transcript leaves it out, where it would double every entry.
pub(super) fn codex_events<'a>(record: &'a Value, emit: &mut dyn FnMut(Event<'a>)) {
    let payload = &record["payload"];
    match (record["type"].as_str(), payload["type"].as_str()) {
        (Some("response_item"), Some("message")) => {
            let kind = match payload["role"].as_str() {
                Some("user") => ActivityKind::Prompt,
                Some("assistant") => ActivityKind::Message,
                _ => return,
            };
            let kind_of = |text: &str| match kind {
                ActivityKind::Message => assistant_kind(text),
                kind => kind,
            };
            let content = &payload["content"];
            let input = spoken_text(content, "input_text");
            let output = spoken_text(content, "output_text");
            // The row takes whichever text a message holds; the transcript
            // reads the kind each side writes, input from the user and output
            // from the assistant.
            let row = input.or(output);
            let listed = if kind == ActivityKind::Prompt {
                input
            } else {
                output
            };
            if row == listed {
                if let Some(text) = row {
                    emit(Event::said(kind_of(text), text));
                }
                return;
            }
            if let Some(text) = row {
                emit(Event::said(kind_of(text), text).row_only());
            }
            if let Some(text) = listed {
                emit(Event::said(kind_of(text), text).transcript_only());
            }
        }
        (Some("response_item"), Some("reasoning")) => {
            let text = spoken_text(&payload["summary"], "summary_text")
                .or_else(|| spoken_text(&payload["content"], "reasoning_text"));
            if let Some(text) = text {
                emit(Event::said(ActivityKind::Thinking, text).row_only());
            }
        }
        (Some("response_item"), Some("function_call")) => {
            let name = payload["name"].as_str().unwrap_or("tool");
            let arguments = payload["arguments"]
                .as_str()
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .unwrap_or_else(|| payload["arguments"].clone());
            let (kind, text) = tool_call(name, &arguments);
            emit(Event::called(kind, text));
        }
        (Some("response_item"), Some("local_shell_call")) => {
            if let Some(command) = shell_command(&payload["action"]) {
                emit(Event::called(ActivityKind::Command, command));
            }
        }
        (Some("response_item"), Some("custom_tool_call")) => {
            let name = payload["name"].as_str().unwrap_or("tool");
            let (kind, text) = codex_custom_tool_call(name, payload);
            emit(Event::called(kind, text));
        }
        (Some("event_msg"), Some("exec_command_begin")) => {
            if let Some(command) = shell_command(payload) {
                emit(Event::called(ActivityKind::Command, command).row_only());
            }
        }
        (Some("event_msg"), Some("user_message")) => {
            if let Some(text) = payload["message"].as_str().filter(|text| is_spoken(text)) {
                emit(Event::said(ActivityKind::Prompt, text).row_only());
            }
        }
        (Some("event_msg"), Some("agent_message")) => {
            if let Some(text) = payload["message"].as_str() {
                emit(Event::said(assistant_kind(text), text).row_only());
            }
        }
        (Some("event_msg"), Some("agent_reasoning")) => {
            if let Some(text) = payload["text"].as_str() {
                emit(Event::said(ActivityKind::Thinking, text).row_only());
            }
        }
        _ => {}
    }
}

/// A Codex custom tool call. Newer Codex versions route tools through a small
/// JavaScript `exec` program, but that transport is not useful to somebody
/// scanning the dashboard. Recover literal shell commands from the program,
/// or at least name the tool(s) it invokes, rather than showing `exec(...)`.
pub(super) fn codex_custom_tool_call(name: &str, payload: &Value) -> (ActivityKind, String) {
    if name != "exec" {
        return tool_call(name, payload);
    }

    let Some(script) = payload["input"].as_str() else {
        return (ActivityKind::Tool, name.to_string());
    };
    let commands = javascript_commands(script);
    if !commands.is_empty() {
        return (ActivityKind::Command, commands.join(" · "));
    }

    let tools = javascript_tool_names(script);
    if !tools.is_empty() {
        return (ActivityKind::Tool, tools.join(" + "));
    }

    // Even an unfamiliar wrapper reads better without its entire program in
    // parentheses. The raw script is implementation detail, not the activity.
    (ActivityKind::Tool, name.to_string())
}

/// Literal `cmd` fields inside an exec program. Codex emits both JSON-style
/// and ordinary JavaScript strings, so only the small field is decoded.
pub(super) fn javascript_commands(script: &str) -> Vec<String> {
    CODEX_EXEC_COMMAND
        .captures_iter(script)
        .filter_map(|captures| captures.get(1))
        .map(|literal| decode_javascript_literal(literal.as_str()))
        .filter(|command| !command.trim().is_empty())
        .collect()
}

/// Tool method names used by an `exec` program, excluding occurrences inside
/// strings (a command may itself mention `tools.foo`, and is not a tool call).
pub(super) fn javascript_tool_names(script: &str) -> Vec<String> {
    let code = JAVASCRIPT_STRING.replace_all(script, "");
    let mut names = Vec::new();
    for name in JAVASCRIPT_TOOL
        .captures_iter(&code)
        .filter_map(|captures| captures.get(1))
        .map(|name| name.as_str().to_string())
    {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

pub(super) fn decode_javascript_literal(literal: &str) -> String {
    let quote = literal.as_bytes()[0];
    if quote == b'"'
        && let Ok(decoded) = serde_json::from_str::<String>(literal)
    {
        return decoded;
    }
    let raw = &literal[1..literal.len() - 1];
    let mut decoded = String::new();
    let mut chars = raw.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            decoded.push(character);
            continue;
        }
        let Some(escaped) = chars.next() else {
            decoded.push('\\');
            break;
        };
        match escaped {
            'n' => decoded.push('\n'),
            'r' => decoded.push('\r'),
            't' => decoded.push('\t'),
            '\\' => decoded.push('\\'),
            '"' if quote == b'"' => decoded.push('"'),
            '\'' if quote == b'\'' => decoded.push('\''),
            '`' if quote == b'`' => decoded.push('`'),
            other => {
                decoded.push('\\');
                decoded.push(other);
            }
        }
    }
    decoded
}
