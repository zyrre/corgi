//! What both harnesses share: the events a record reduces to, the
//! conversation and transcript they feed, and the text and tool helpers that
//! shape them for display.

use std::{borrow::Cow, collections::VecDeque, path::Path, sync::LazyLock};

use regex::Regex;
use serde_json::Value;

use super::{TRANSCRIPT_ENTRIES, TurnState, tail::Fold};
use crate::{
    activity::{MAX_MESSAGE_CHARS, MAX_TOOL_CHARS, MAX_TRANSCRIPT_CHARS, describe, describe_block},
    model::{Activity, ActivityKind},
};

/// Claude Code wraps a prompt that arrived as a multi-line bracketed paste,
/// which is how Herdr submits one, in these tags when it records the turn.
pub static PASTED_CONTENT_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"</?pasted_content(?:\s[^>]*)?>").expect("valid pasted content regex")
});

/// Reduces one session record to its events, in the order it holds them.
pub type Events = for<'a> fn(&'a Value, &mut dyn FnMut(Event<'a>));

/// One thing a session record holds: something said, or a tool call.
#[derive(Debug)]
pub struct Event<'a> {
    kind: ActivityKind,
    text: Cow<'a, str>,
    /// A tool call or command rather than something said.
    call: bool,
    /// Whether the row reads it, as the newest message or tool.
    in_row: bool,
    /// Whether the expanded transcript lists it.
    in_transcript: bool,
    /// A turn's start or end the record marks without saying anything
    /// either view shows.
    mark: Option<Mark>,
}

/// A turn's start or end, as a record marks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Start,
    End,
}

impl<'a> Event<'a> {
    pub fn said(kind: ActivityKind, text: impl Into<Cow<'a, str>>) -> Self {
        Self {
            kind,
            text: text.into(),
            call: false,
            in_row: true,
            in_transcript: true,
            mark: None,
        }
    }

    /// A turn's start or end, which neither view shows.
    pub fn marks(mark: Mark) -> Self {
        Self {
            mark: Some(mark),
            in_row: false,
            in_transcript: false,
            ..Self::said(ActivityKind::Prompt, "")
        }
    }

    pub fn called(kind: ActivityKind, text: String) -> Self {
        Self {
            call: true,
            ..Self::said(kind, text)
        }
    }

    /// Shown in the row but not listed in the transcript: the agent's
    /// reasoning, or an echo of a record the transcript already lists.
    pub fn row_only(self) -> Self {
        Self {
            in_transcript: false,
            ..self
        }
    }

    pub fn transcript_only(self) -> Self {
        Self {
            in_row: false,
            ..self
        }
    }
}

/// The newest thing said and the newest tool call, as the session file wrote
/// them. Only what the row ends up showing is shaped for display.
#[derive(Debug, Default)]
pub struct Conversation {
    said: Option<(ActivityKind, String)>,
    called: Option<(ActivityKind, String)>,
    /// The newest assistant reply or question, kept separately and whole:
    /// `corgi report`'s source. `said` already holds this text when it is the
    /// newest thing in the session, but a user prompt or thinking sent since
    /// replaces `said` without being a report.
    reply: Option<(ActivityKind, String)>,
    /// How the newest turn stands, which says whether `reply` is its own.
    turn: Turn,
}

impl Conversation {
    /// Takes in the next event of a record, which is newer than the last.
    pub fn see(&mut self, event: &Event) {
        if let Some(mark) = event.mark {
            self.turn.see(match mark {
                Mark::Start => Step::Start,
                Mark::End => Step::End,
            });
            return;
        }
        if event.text.trim().is_empty() {
            return;
        }
        match event.kind {
            _ if event.call => self.turn.see(Step::Call),
            ActivityKind::Message | ActivityKind::Question => self.turn.see(Step::Reply),
            ActivityKind::Prompt => self.turn.see(Step::Start),
            _ => {}
        }
        if !event.call && matches!(event.kind, ActivityKind::Message | ActivityKind::Question) {
            self.reply
                .get_or_insert_with(|| (event.kind, event.text.clone().into_owned()));
        }
        if !event.in_row {
            return;
        }
        let slot = if event.call {
            &mut self.called
        } else {
            &mut self.said
        };
        *slot = Some((event.kind, event.text.clone().into_owned()));
    }

    pub fn or(self, older: Self) -> Self {
        Self {
            said: self.said.or(older.said),
            called: self.called.or(older.called),
            reply: self.reply.or(older.reply),
            turn: self.turn.or(older.turn),
        }
    }

    pub fn complete(&self) -> bool {
        self.said.is_some() && self.called.is_some()
    }

    pub fn message(&self) -> Option<Activity> {
        let (kind, text) = self.said.as_ref()?;
        Some(describe(*kind, text, MAX_MESSAGE_CHARS))
    }

    pub fn tool(&self) -> Option<Activity> {
        let (kind, text) = self.called.as_ref()?;
        Some(describe(*kind, text, MAX_TOOL_CHARS))
    }

    /// The newest assistant reply or question, whole: `corgi report`'s
    /// source. Unlike [`Self::message`], the text keeps its paragraphs and is
    /// not cut to a dashboard row's length, because a worker's closing report
    /// is meant to be read in full rather than skimmed.
    pub fn report(&self) -> Option<Activity> {
        let (kind, text) = self.reply.as_ref()?;
        Some(describe_block(*kind, text, usize::MAX))
    }

    /// How the newest turn stands: whether [`Self::report`] is its own.
    pub fn turn(&self) -> Option<TurnState> {
        self.turn.state()
    }
}

/// One step of a turn, as far as its report goes: its start, a reply, a
/// tool call, or its end. Thinking is none: it comes before a reply or a
/// call that says more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Start,
    Reply,
    Call,
    End,
}

/// The steps of some records of a conversation, folded like the rest of it,
/// newest first.
#[derive(Debug, Default, Clone, Copy)]
struct Turn {
    /// The newest start, reply or tool call.
    last: Option<Step>,
    /// Whether the records hold a turn's start: an end older than that is
    /// another turn's.
    started: bool,
    /// Whether a turn ended after the newest start the records hold, or
    /// anywhere in them when they hold none.
    ended: bool,
}

impl Turn {
    fn see(&mut self, step: Step) {
        match step {
            Step::End => self.ended = true,
            Step::Start => {
                *self = Self {
                    last: Some(step),
                    started: true,
                    ended: false,
                }
            }
            Step::Reply | Step::Call => self.last = Some(step),
        }
    }

    fn or(self, older: Self) -> Self {
        Self {
            last: self.last.or(older.last),
            started: self.started || older.started,
            ended: self.ended || (!self.started && older.ended),
        }
    }

    /// A reply that is the newest step stands for the turn, ended or not:
    /// a turn ends on its reply, and waiting for the end to be written would
    /// slow every report down.
    fn state(&self) -> Option<TurnState> {
        Some(match (self.last?, self.ended) {
            (Step::Reply, _) => TurnState::Answered,
            (_, true) => TurnState::Silent,
            _ => TurnState::Open,
        })
    }
}

/// The newest entries of the expanded transcript, oldest first.
#[derive(Debug, Default)]
pub struct Entries(VecDeque<Activity>);

impl Entries {
    pub fn of(record: &Value, events: Events) -> Self {
        let mut entries = Self::default();
        events(record, &mut |event| entries.see(&event));
        entries
    }

    /// Appends one entry, skipping empty text and a turn identical to the one
    /// just before it. Both CLIs record some turns twice, and the same command
    /// repeated a moment later reads as noise rather than history.
    pub fn see(&mut self, event: &Event) {
        if !event.in_transcript || event.text.trim().is_empty() {
            return;
        }
        let entry = describe_block(event.kind, &event.text, MAX_TRANSCRIPT_CHARS);
        if self.0.back() == Some(&entry) {
            return;
        }
        self.0.push_back(entry);
        if self.0.len() > TRANSCRIPT_ENTRIES {
            self.0.pop_front();
        }
    }

    pub fn newest_first(&self) -> impl Iterator<Item = &Activity> {
        self.0.iter().rev()
    }
}

impl Fold for Entries {
    fn or(mut self, older: Self) -> Self {
        for entry in older.0.into_iter().rev() {
            if self.complete() {
                break;
            }
            if self.0.front() != Some(&entry) {
                self.0.push_front(entry);
            }
        }
        self
    }

    fn complete(&self) -> bool {
        self.0.len() >= TRANSCRIPT_ENTRIES
    }
}

/// The first block of `kind` in a content array that a person wrote or would
/// read, skipping the tagged blocks the CLI attaches for itself.
pub fn spoken_text<'a>(content: &'a Value, kind: &str) -> Option<&'a str> {
    content
        .as_array()?
        .iter()
        .filter(|block| block["type"] == kind)
        .filter_map(|block| block["text"].as_str())
        .find(|text| is_spoken(text))
}

/// The task a user turn describes, in its bounded, redacted display form,
/// when it is one rather than context injected by the harness.
pub fn task_prompt(text: &str) -> Option<String> {
    let text = unwrap_pasted(text);
    is_task_prompt(&text).then(|| task_text(&text))
}

/// A user turn that can describe the session, rather than context injected by
/// the harness. Codex currently writes AGENTS.md as a plain user message, so
/// `is_spoken` alone would make the dashboard label the session with project
/// instructions instead of the user's task.
pub fn is_task_prompt(text: &str) -> bool {
    let trimmed = text.trim_start();
    is_spoken(trimmed)
        && !trimmed.starts_with("# AGENTS.md instructions for ")
        && !trimmed.starts_with("# AGENTS instructions for ")
}

/// A bounded, redacted display form of the first user prompt.
pub fn task_text(text: &str) -> String {
    describe(ActivityKind::Prompt, text, MAX_MESSAGE_CHARS).text
}

/// The prompt a user turn holds without the paste tags around it, so a pasted
/// prompt counts as spoken rather than as text the CLI injected.
pub fn unwrap_pasted(text: &str) -> Cow<'_, str> {
    PASTED_CONTENT_TAG.replace_all(text, "")
}

/// Whether text in a user turn was typed by the user. Both CLIs wrap what
/// they inject themselves, such as system reminders, slash-command expansions,
/// and environment context, in an XML-like tag on its first line.
pub fn is_spoken(text: &str) -> bool {
    let first = text.trim_start().lines().next().unwrap_or_default();
    !first.is_empty() && !(first.starts_with('<') && first.contains('>'))
}

/// An assistant reply that ends in a question is waiting on the user.
pub fn assistant_kind(text: &str) -> ActivityKind {
    if text.trim_end().ends_with('?') {
        ActivityKind::Question
    } else {
        ActivityKind::Message
    }
}

/// One line describing a tool call. A shell command is shown as itself; any
/// other tool as its name and the argument that identifies what it touches.
pub fn tool_call(name: &str, input: &Value) -> (ActivityKind, String) {
    if is_shell_tool(name)
        && let Some(command) = shell_command(input)
    {
        return (ActivityKind::Command, command);
    }
    const IDENTIFYING: [&str; 12] = [
        "command",
        "file_path",
        "notebook_path",
        "path",
        "pattern",
        "query",
        "url",
        "skill",
        "description",
        "prompt",
        "cmd",
        "input",
    ];
    let argument = IDENTIFYING.iter().find_map(|key| {
        input[*key]
            .as_str()
            .filter(|value| !value.trim().is_empty())
    });
    match argument {
        Some(argument) => (ActivityKind::Tool, format!("{name}({argument})")),
        None => (ActivityKind::Tool, name.to_string()),
    }
}

pub fn is_shell_tool(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "bash"
        || lower == "shell"
        || lower == "exec_command"
        || lower == "local_shell"
        || lower.ends_with("exec")
        || lower.ends_with("shell")
}

/// The command of a shell tool call, whether it is recorded as one string or,
/// as Codex does, as an argument vector.
pub fn shell_command(input: &Value) -> Option<String> {
    for key in ["command", "cmd"] {
        let value = &input[key];
        if let Some(command) = value.as_str() {
            return Some(command.to_string());
        }
        if let Some(words) = value.as_array() {
            let words: Vec<&str> = words.iter().filter_map(Value::as_str).collect();
            if !words.is_empty() {
                return Some(shell_words(&words));
            }
        }
    }
    None
}

/// Joins an argument vector the way it would be typed: a shell wrapper such
/// as `bash -lc <script>` reads as the script alone.
pub fn shell_words(words: &[&str]) -> String {
    match words {
        [shell, flag, script] if flag.starts_with('-') && flag.contains('c') => {
            let shell = Path::new(shell)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(shell);
            if matches!(shell, "bash" | "sh" | "zsh" | "fish") {
                return script.to_string();
            }
            words.join(" ")
        }
        _ => words.join(" "),
    }
}

/// A string field of a record, whether the CLI writes it at the top level or
/// inside the payload of an event.
pub fn field<'a>(record: &'a Value, name: &str) -> Option<&'a str> {
    record[name]
        .as_str()
        .or_else(|| record["payload"][name].as_str())
        .filter(|value| !value.is_empty())
}

/// The effort a session file records for a turn. Codex writes it in its turn
/// context and has used both top-level fields and nested collaboration
/// settings across CLI releases; Claude Code writes a plain `effort` beside
/// the assistant message. Accept each shape while keeping the value tied to
/// that session file.
pub fn record_effort(record: &Value) -> Option<&str> {
    [
        record["model_reasoning_effort"].as_str(),
        record["payload"]["model_reasoning_effort"].as_str(),
        record["effort"].as_str(),
        record["payload"]["effort"].as_str(),
        record["collaboration_mode"]["settings"]["reasoning_effort"].as_str(),
        record["payload"]["collaboration_mode"]["settings"]["reasoning_effort"].as_str(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|effort| !effort.is_empty())
}
