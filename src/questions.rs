//! A supervisor's open questions to the user, and the user's answers to
//! them from the Herdr app's Questions panel.
//!
//! The supervisor writes each open question whole to `questions/Q<n>.md` in
//! its state directory, in the form its role asks for, and deletes the
//! file once the question is answered, closed, dropped or replaced:
//!
//! ```text
//! ### Q<n>: <short title>   [blocks: <task title> merge | blocks: <plan> | no block]
//! Context: …
//! Options:
//!   A. <choice>: <what happens, what it costs>
//!   B. <choice>: …
//! Recommendation: <A/B>, because …
//! Until answered: …
//! ```
//!
//! [`read`] parses them; a file that strays from the form is still listed,
//! with its Markdown alone (`parsed: false`).
//!
//! An answer (`corgi answer`) is one `[Corgi]` line in the supervisor's
//! inbox, keyed by its question ([`answer_key`]), so a newer answer to the
//! same question replaces one not typed into the supervisor's box yet; the
//! daemon types it once the supervisor is between turns and its box holds
//! no draft. Each answer is also recorded in `answers.jsonl` beside the
//! inbox, which the inbox's compaction does not touch, so the card can say
//! the answer went out long after its item is gone ([`AnswerState`]). A
//! backlog entry's Start is recorded and sent the same way.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    digest::lookup::QUESTIONS_DIR,
    inbox::{self, Inbox, Item, Kind},
    inbox_privacy as privacy,
    supervisor::WAKE_PREFIX,
    time::{rfc3339_utc, unix_now},
};

/// Where the answers given in the app are recorded, in the state directory.
pub const ANSWERS_FILE: &str = "answers.jsonl";
/// The most of a question file that is read.
const QUESTION_BYTES: u64 = 64 * 1024;
/// The longest free-text answer kept, in bytes.
pub const OTHER_MAX: usize = 4 * 1024;

/// What a question holds up, from the `[…]` at the end of its heading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Blocks {
    /// `merge` (a task's merge), `plan` or `none`.
    pub kind: &'static str,
    /// The task or plan, without `blocks:` and `merge`; empty for `none`.
    pub what: String,
}

/// One of a question's options: `A. <choice>: <cost>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choice {
    pub key: String,
    pub choice: String,
    pub cost: String,
}

/// A question as its file says it. With `parsed` false only `id`, `number`,
/// `title` (when the heading had one) and `raw` are to be trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Question {
    /// `Q<n>`, from the file's name.
    pub id: String,
    pub number: u64,
    pub title: String,
    pub blocks: Option<Blocks>,
    pub context: String,
    pub options: Vec<Choice>,
    /// The recommended option's key, and the whole recommendation line.
    pub recommended: Option<String>,
    pub recommendation: String,
    pub until: String,
    pub parsed: bool,
    pub raw: String,
}

/// `Q<n>.md` → `n`.
fn number_of(name: &str) -> Option<u64> {
    let digits = name.strip_prefix('Q')?.strip_suffix(".md")?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// The heading's `[…]`: what the question blocks.
fn parse_blocks(tag: &str) -> Blocks {
    let tag = tag.trim();
    if tag.eq_ignore_ascii_case("no block") {
        return Blocks {
            kind: "none",
            what: String::new(),
        };
    }
    let what = tag
        .strip_prefix("blocks:")
        .or_else(|| tag.strip_prefix("Blocks:"))
        .unwrap_or(tag)
        .trim();
    match what.strip_suffix(" merge") {
        Some(task) => Blocks {
            kind: "merge",
            what: task.trim().to_string(),
        },
        None => Blocks {
            kind: "plan",
            what: what.to_string(),
        },
    }
}

/// `### Q<n>: <title>   [<tag>]` → (title, blocks).
fn parse_heading(line: &str, id: &str) -> Option<(String, Option<Blocks>)> {
    let rest = line.trim().strip_prefix("###")?.trim_start();
    let rest = rest.strip_prefix(id)?.strip_prefix(':')?.trim();
    if let Some(open) = rest.rfind('[')
        && rest.ends_with(']')
    {
        let tag = &rest[open + 1..rest.len() - 1];
        return Some((rest[..open].trim().to_string(), Some(parse_blocks(tag))));
    }
    Some((rest.to_string(), None))
}

/// An option line, `A. <choice>: <cost>` (or `A) …`), indented or not.
fn option_line(line: &str) -> Option<Choice> {
    let line = line.trim();
    let mut chars = line.chars();
    let key = chars.next().filter(char::is_ascii_uppercase)?;
    let rest = chars.as_str();
    let rest = rest
        .strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))?;
    let (choice, cost) = match rest.split_once(": ") {
        Some((choice, cost)) => (choice.trim(), cost.trim()),
        None => (rest.trim(), ""),
    };
    Some(Choice {
        key: key.to_string(),
        choice: choice.to_string(),
        cost: cost.to_string(),
    })
}

fn join(text: &mut String, line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(line);
}

/// The question `id` (`Q<n>`, `number` n) its file's text `raw` asks.
pub fn parse(id: &str, number: u64, raw: &str) -> Question {
    let mut question = Question {
        id: id.to_string(),
        number,
        title: String::new(),
        blocks: None,
        context: String::new(),
        options: Vec::new(),
        recommended: None,
        recommendation: String::new(),
        until: String::new(),
        parsed: false,
        raw: raw.trim().to_string(),
    };
    let mut lines = raw.lines().skip_while(|line| line.trim().is_empty());
    let Some((title, blocks)) = lines.next().and_then(|line| parse_heading(line, id)) else {
        return question;
    };
    question.title = title;
    question.blocks = blocks;
    #[derive(PartialEq)]
    enum Part {
        None,
        Context,
        Options,
        Recommendation,
        Until,
    }
    let mut part = Part::None;
    for line in lines {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Context:") {
            part = Part::Context;
            join(&mut question.context, rest);
        } else if trimmed.starts_with("Options:") {
            part = Part::Options;
        } else if let Some(rest) = trimmed.strip_prefix("Recommendation:") {
            part = Part::Recommendation;
            join(&mut question.recommendation, rest);
        } else if let Some(rest) = trimmed.strip_prefix("Until answered:") {
            part = Part::Until;
            join(&mut question.until, rest);
        } else if part == Part::Options
            && let Some(choice) = option_line(line)
        {
            question.options.push(choice);
        } else {
            match part {
                Part::Context => join(&mut question.context, line),
                // An option's text that went on to the next line.
                Part::Options => {
                    if let Some(last) = question.options.last_mut() {
                        let field = if last.cost.is_empty() {
                            &mut last.choice
                        } else {
                            &mut last.cost
                        };
                        join(field, line);
                    }
                }
                Part::Recommendation => join(&mut question.recommendation, line),
                Part::Until => join(&mut question.until, line),
                Part::None => {}
            }
        }
    }
    question.recommended = question
        .recommendation
        .chars()
        .next()
        .filter(char::is_ascii_uppercase)
        .map(String::from)
        .filter(|key| question.options.iter().any(|o| &o.key == key));
    question.parsed = !question.title.is_empty() && !question.options.is_empty();
    question
}

/// A plain file's text, at most `max` bytes of it; none for a link, a pipe
/// or what cannot be read.
fn read_plain(path: &Path, max: u64) -> Option<String> {
    if !fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()) {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(max).read_to_end(&mut bytes))
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The open questions in the state directory `dir`, by number.
pub fn read(dir: &Path) -> Vec<Question> {
    let mut found: Vec<(u64, String)> = fs::read_dir(dir.join(QUESTIONS_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            Some((number_of(&name)?, name))
        })
        .collect();
    found.sort();
    found
        .into_iter()
        .filter_map(|(number, name)| {
            let raw = read_plain(&dir.join(QUESTIONS_DIR).join(&name), QUESTION_BYTES)?;
            Some(parse(&format!("Q{number}"), number, &raw))
        })
        .collect()
}

/// One answer, or one backlog Start, recorded in [`ANSWERS_FILE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub ts: String,
    /// `answer` or `start`.
    pub kind: String,
    /// The question (`Q7`) or the backlog entry's heading.
    pub subject: String,
    /// The answer as given: an option's key, or `other: <text>`.
    #[serde(default)]
    pub answer: String,
    /// The inbox item that carries it.
    pub item: String,
}

/// Where an answer or a Start is: waiting in the inbox for the supervisor
/// to be between turns, or typed into its box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerState {
    Queued,
    Sent,
}

impl AnswerState {
    pub fn word(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Sent => "sent",
        }
    }
}

/// The records in the state directory `dir`, oldest first.
pub fn records(dir: &Path) -> Vec<Record> {
    let path = dir.join(ANSWERS_FILE);
    let Ok(mut file) = privacy::open(&path, OpenOptions::new().read(true)) else {
        return Vec::new();
    };
    let mut text = String::new();
    if file.read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The newest record of each subject of `kind`, with where it is: queued
/// while its inbox item is undelivered, sent once it was delivered (or the
/// item is gone, compacted away after going out).
pub fn latest(dir: &Path, kind: &str) -> Vec<(Record, AnswerState)> {
    let inbox = Inbox::read(dir);
    let undelivered: Vec<String> = inbox
        .undelivered()
        .iter()
        .map(|item| item.id.clone())
        .collect();
    let mut newest: Vec<Record> = Vec::new();
    for record in records(dir).into_iter().filter(|r| r.kind == kind) {
        newest.retain(|r| r.subject != record.subject);
        newest.push(record);
    }
    newest
        .into_iter()
        .map(|record| {
            let state = if undelivered.contains(&record.item) {
                AnswerState::Queued
            } else {
                AnswerState::Sent
            };
            (record, state)
        })
        .collect()
}

/// An answer's or a Start's state as JSON, or null.
pub fn state_value(found: Option<&(Record, AnswerState)>) -> Value {
    match found {
        None => Value::Null,
        Some((record, state)) => json!({
            "answer": record.answer,
            "state": state.word(),
            "at": record.ts,
        }),
    }
}

/// The inbox key of the answers to question `id`: a newer answer replaces
/// one not sent yet.
pub fn answer_key(id: &str) -> String {
    format!("answer-{id}")
}

/// The inbox key of a Start of the backlog entry `heading`.
pub fn start_key(heading: &str) -> String {
    format!("backlog-start-{}", inbox::file_safe(heading))
}

/// Text on one line: a line break would end the supervisor's prompt early.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What the user gave for `question`: an option's key, or `other: <text>`
/// (any text that is not a key is taken as `other`). `Err` says why it is
/// not an answer.
pub fn normalize(question: &Question, given: &str) -> Result<String, String> {
    let given = one_line(given);
    if given.is_empty() {
        return Err(format!("the answer to {} is empty", question.id));
    }
    if given.len() == 1 && given.chars().all(|c| c.is_ascii_alphabetic()) {
        let key = given.to_ascii_uppercase();
        if question.parsed && !question.options.iter().any(|o| o.key == key) {
            return Err(format!("{} has no option {key}", question.id));
        }
        return Ok(key);
    }
    let text = match given.split_once(':') {
        Some((head, rest)) if head.trim().eq_ignore_ascii_case("other") => rest.trim(),
        _ => given.as_str(),
    };
    if text.is_empty() {
        return Err(format!("the answer to {} is empty", question.id));
    }
    if text.len() > OTHER_MAX {
        return Err(format!(
            "the answer to {} is longer than {OTHER_MAX} bytes",
            question.id
        ));
    }
    Ok(format!("other: {text}"))
}

/// The `[Corgi]` line that brings the user's answer `answer` to
/// `question`. It says the answer is the user's, as their own typed answer
/// would be.
pub fn answer_text(question: &Question, answer: &str) -> String {
    let title = if question.title.is_empty() {
        String::new()
    } else {
        format!(" ({})", one_line(&question.title))
    };
    let what = match question.options.iter().find(|o| o.key == answer) {
        Some(option) => format!("{}: {answer}, {}", question.id, one_line(&option.choice)),
        None => format!("{}: {answer}", question.id),
    };
    format!(
        "{WAKE_PREFIX} The user's answer to {}{title}, given in the Herdr app's Questions panel: \
         \"{what}\". Handle it as if they had typed it.",
        question.id
    )
}

/// The `[Corgi]` line that asks for a plan of the backlog entry `heading`.
pub fn start_text(heading: &str) -> String {
    format!(
        "{WAKE_PREFIX} The user's request, from the Herdr app's backlog: plan the backlog entry \
         \"{}\". Propose the plan to them; start nothing until they agree.",
        one_line(heading)
    )
}

/// Puts `text` in the inbox of the project at `root` (state directory
/// `dir`) under `key`, and records it: what was queued.
pub fn queue(
    dir: &Path,
    root: &str,
    kind: &str,
    subject: &str,
    answer: &str,
    key: String,
    text: String,
) -> Result<Record> {
    let item = inbox::append(
        dir,
        Item {
            project: root.to_string(),
            source: "app".into(),
            kind: Kind::Note,
            key,
            text,
            ..Item::default()
        },
    )?
    .context("the inbox already has an item with that id")?;
    let record = Record {
        ts: rfc3339_utc(unix_now()),
        kind: kind.to_string(),
        subject: subject.to_string(),
        answer: answer.to_string(),
        item: item.id,
    };
    let path = dir.join(ANSWERS_FILE);
    let mut file = privacy::open(&path, OpenOptions::new().append(true).create(true))?;
    let mut line = serde_json::to_string(&record)?;
    line.push('\n');
    file.write_all(line.as_bytes())
        .with_context(|| format!("write {}", path.display()))?;
    Ok(record)
}

#[cfg(test)]
mod tests;
