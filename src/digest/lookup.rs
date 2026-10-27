//! `corgi digest --show`, `--ledger` and `--brief`: one item of a
//! supervisor's memory printed whole, so that a search hit, a ledger id or a
//! brief is read in full through Corgi rather than by opening the state
//! files raw.
//!
//! Everything here is pure text in, text out; the command in `app/cli.rs`
//! reads the files.

use std::{collections::HashMap, fmt::Write as _, path::Component};

use anyhow::{Result, bail};
use serde_json::{Map, Value};

use super::parse_decisions;

/// The ledger, append-only, in the state directory.
pub const LEDGER_FILE: &str = "ledger.jsonl";
/// Older ledger lines, moved out of [`LEDGER_FILE`] once a newer line of their
/// id superseded them: read as coming before every line of the ledger.
pub const LEDGER_ARCHIVE_FILE: &str = "ledger-archive.jsonl";
/// The reports of the workers the supervisor spawned, as the dashboard
/// captured them from their transcripts when they stopped, kept for good
/// (see `inbox::append`).
pub const REPORTS_DIR: &str = "reports";
/// The supervisor's own working notes and artifacts.
pub const NOTES_DIR: &str = "notes";
/// Files whose items are Corgi's to print, through `inbox` and `report`.
const INBOX_PATHS: [&str; 2] = ["inbox.jsonl", "inbox-reports"];

/// One line of the ledger or its archive that is a JSON object with an `id`.
pub struct LedgerLine<'a> {
    /// [`LEDGER_FILE`] or [`LEDGER_ARCHIVE_FILE`].
    pub file: &'static str,
    /// Its line in that file, counting from 1.
    pub line: usize,
    pub raw: &'a str,
    pub object: Map<String, Value>,
}

impl LedgerLine<'_> {
    /// The field `name` as text: a string as it is, nothing as empty, and
    /// anything else as JSON.
    pub fn field(&self, name: &str) -> String {
        value_text(self.object.get(name))
    }

    /// `file:line`, what `--show` takes.
    pub fn place(&self) -> String {
        format!("{}:{}", self.file, self.line)
    }
}

fn value_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// The lines with an id of `archive` and then of `ledger`, each in file
/// order, so oldest first.
pub fn ledger_lines<'a>(ledger: &'a str, archive: &'a str) -> Vec<LedgerLine<'a>> {
    [(LEDGER_ARCHIVE_FILE, archive), (LEDGER_FILE, ledger)]
        .into_iter()
        .flat_map(|(file, text)| {
            text.lines().enumerate().filter_map(move |(index, raw)| {
                let Ok(Value::Object(object)) = serde_json::from_str(raw) else {
                    return None;
                };
                let line = LedgerLine {
                    file,
                    line: index + 1,
                    raw,
                    object,
                };
                (!line.field("id").is_empty()).then_some(line)
            })
        })
        .collect()
}

/// `lines` by id, each id's lines oldest first, and the ids in the order of
/// their newest line.
pub fn ledger_ids(lines: Vec<LedgerLine<'_>>) -> Vec<Vec<LedgerLine<'_>>> {
    let mut slots: HashMap<String, usize> = HashMap::new();
    let mut ids: Vec<(usize, Vec<LedgerLine>)> = Vec::new();
    for (order, line) in lines.into_iter().enumerate() {
        let id = line.field("id");
        match slots.get(&id) {
            Some(&slot) => {
                ids[slot].0 = order;
                ids[slot].1.push(line);
            }
            None => {
                slots.insert(id, ids.len());
                ids.push((order, vec![line]));
            }
        }
    }
    ids.sort_by_key(|(newest, _)| *newest);
    ids.into_iter().map(|(_, lines)| lines).collect()
}

/// A hit's handle, `file[:line]`, as a path relative to the state directory
/// `state_dir` and the line, if any. An absolute path inside the state
/// directory is taken too; one outside it, or one that leaves it with `..`,
/// is refused, as are the inbox's files.
pub fn parse_handle(state_dir: &str, handle: &str) -> Result<(String, Option<usize>)> {
    let handle = handle.trim();
    let (path, line) = match handle.rsplit_once(':') {
        Some((path, line)) if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) => {
            (path, Some(line.parse::<usize>()?))
        }
        _ => (handle, None),
    };
    let state = state_dir.trim_end_matches('/');
    let relative = match path.strip_prefix(state) {
        Some(rest) if rest.starts_with('/') => rest.trim_start_matches('/'),
        _ => path.trim_start_matches("./"),
    };
    let safe = !relative.is_empty()
        && std::path::Path::new(relative)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    if !safe {
        bail!(
            "{handle:?} is not a file in {state}; give it as search prints it, such as briefs/<id>.md:12"
        );
    }
    if INBOX_PATHS
        .iter()
        .any(|inbox| relative == *inbox || relative.starts_with(&format!("{inbox}/")))
    {
        bail!("{relative} is the inbox, Corgi's own; read it with `corgi inbox` or `corgi report`");
    }
    if line == Some(0) {
        bail!("lines count from 1");
    }
    Ok((relative.to_string(), line))
}

/// The block of the file `path` (relative to the state directory, with the
/// text `text`) that holds `line`, whole: a decision entry, a ledger line
/// with its id's history, a brief or a report whole, or the section of any
/// other Markdown file under its nearest heading. Without a line, or for a
/// file that is not Markdown, the whole file. `ledger` and `archive` are the
/// ledger's two files, for a ledger line's history.
pub fn show(
    path: &str,
    text: &str,
    line: Option<usize>,
    ledger: &str,
    archive: &str,
) -> Result<String> {
    let total = text.lines().count();
    let Some(line) = line else {
        return Ok(format!("# {path} (whole file)\n{}\n", text.trim_end()));
    };
    if line > total {
        bail!("{path} has {total} lines, not {line}");
    }
    if path == LEDGER_FILE || path == LEDGER_ARCHIVE_FILE {
        return Ok(show_ledger_line(path, text, line, ledger, archive));
    }
    let whole = path.starts_with("briefs/") || path.starts_with(&format!("{REPORTS_DIR}/"));
    let (from, to) = if path == "decisions.md" {
        decision_span(text, line)
    } else if whole || !path.ends_with(".md") {
        (1, total)
    } else {
        section_span(text, line)
    };
    let block: Vec<&str> = text.lines().skip(from - 1).take(to + 1 - from).collect();
    let what = if (from, to) == (1, total) {
        "whole file".to_string()
    } else {
        format!("lines {from}-{to} of {total}")
    };
    Ok(format!("# {path}:{from} ({what})\n{}\n", block.join("\n")))
}

/// The lines, from and to, counting from 1, of the decision entry holding
/// `line`, or of the file's title before the first entry.
fn decision_span(text: &str, line: usize) -> (usize, usize) {
    let decisions = parse_decisions(text);
    match decisions
        .iter()
        .rev()
        .find(|decision| decision.line <= line)
    {
        Some(decision) => (
            decision.line,
            decision.line + decision.text.lines().count() - 1,
        ),
        None => (
            1,
            decisions
                .first()
                .map_or(text.lines().count(), |first| first.line - 1)
                .max(1),
        ),
    }
}

/// The lines, from and to, of the section holding `line`: from the nearest
/// heading at or above it to the next heading of the same level or higher,
/// trailing blank lines left out. Before the first heading, from the start
/// to it. Headings inside a code fence do not count.
pub(crate) fn section_span(text: &str, line: usize) -> (usize, usize) {
    let mut headings = Vec::new();
    let mut fenced = false;
    let lines: Vec<&str> = text.lines().collect();
    for (index, text) in lines.iter().enumerate() {
        if text.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced && text.starts_with('#') {
            let level = text.bytes().take_while(|b| *b == b'#').count();
            headings.push((index + 1, level));
        }
    }
    let (from, end) = match headings.iter().rev().find(|(at, _)| *at <= line) {
        Some(&(at, level)) => (
            at,
            headings
                .iter()
                .find(|(next, next_level)| *next > at && *next_level <= level)
                .map_or(lines.len(), |(next, _)| next - 1),
        ),
        None => (1, headings.first().map_or(lines.len(), |(at, _)| at - 1)),
    };
    let mut to = end.max(from);
    while to > from && lines[to - 1].trim().is_empty() {
        to -= 1;
    }
    (from, to)
}

fn show_ledger_line(path: &str, text: &str, line: usize, ledger: &str, archive: &str) -> String {
    let raw = text.lines().nth(line - 1).unwrap_or_default();
    let mut out = format!("# {path}:{line}\n");
    let Ok(value @ Value::Object(_)) = serde_json::from_str::<Value>(raw) else {
        let _ = writeln!(out, "{raw}");
        return out;
    };
    let _ = writeln!(
        out,
        "{}",
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| raw.to_string())
    );
    let id = value_text(value.get("id"));
    if let Some(lines) = ledger_ids(ledger_lines(ledger, archive))
        .into_iter()
        .find(|lines| lines[0].field("id") == id)
    {
        let _ = writeln!(out, "\nHistory of {id}, oldest first:");
        history(&mut out, &lines);
    }
    out
}

/// One line per ledger line of an id: where it is, its time and status, and
/// every other field but the id.
fn history(out: &mut String, lines: &[LedgerLine]) {
    for line in lines {
        let rest: Vec<String> = line
            .object
            .iter()
            .filter(|(name, _)| !matches!(name.as_str(), "id" | "ts" | "status"))
            .map(|(name, value)| format!("{name}: {}", value_text(Some(value))))
            .collect();
        let _ = writeln!(
            out,
            "- {}  {}  {}  {}",
            line.place(),
            or_dash(&line.field("ts")),
            or_dash(&line.field("status")),
            rest.join("; ")
        );
    }
}

/// Every ledger id `query` names, as an id or an agent name, newest line in
/// full and then its history, with its brief and kept reports by name.
/// Without an exact match, the one id or agent that contains `query`; with
/// several, an error lists them.
pub fn ledger(
    ledger: &str,
    archive: &str,
    query: &str,
    briefs: &[String],
    reports: &[String],
) -> Result<String> {
    let query = query.trim();
    let ids = ledger_ids(ledger_lines(ledger, archive));
    let named = |lines: &&Vec<LedgerLine>, exact: bool| {
        lines.iter().any(|line| {
            [line.field("id"), line.field("agent")]
                .iter()
                .any(|name| match exact {
                    true => name == query,
                    false => name.to_lowercase().contains(&query.to_lowercase()),
                })
        })
    };
    let mut found: Vec<_> = ids.iter().filter(|lines| named(lines, true)).collect();
    if found.is_empty() {
        found = ids.iter().filter(|lines| named(lines, false)).collect();
        if found.len() > 1 {
            let names: Vec<String> = found
                .iter()
                .rev()
                .map(|lines| {
                    let newest = lines.last().expect("an id has a line");
                    format!(
                        "{} (agent {}, {})",
                        newest.field("id"),
                        or_dash(&newest.field("agent")),
                        or_dash(&newest.field("status"))
                    )
                })
                .collect();
            bail!(
                "no ledger id or agent is {query:?}; {} contain it, newest first:\n{}",
                names.len(),
                names.join("\n")
            );
        }
    }
    if found.is_empty() {
        bail!("no ledger id or agent name contains {query:?}");
    }
    let mut out = String::new();
    for lines in found.iter().rev() {
        let newest = lines.last().expect("an id has a line");
        let (id, agent) = (newest.field("id"), newest.field("agent"));
        let statuses: Vec<String> =
            lines
                .iter()
                .map(|line| line.field("status"))
                .fold(Vec::new(), |mut seen, status| {
                    if !status.is_empty() && seen.last() != Some(&status) {
                        seen.push(status);
                    }
                    seen
                });
        let _ = writeln!(
            out,
            "# Ledger {id}, agent {}: {} ({} line{})",
            or_dash(&agent),
            statuses.join(" -> "),
            lines.len(),
            if lines.len() == 1 { "" } else { "s" }
        );
        let pretty =
            serde_json::to_string_pretty(&newest.object).unwrap_or_else(|_| newest.raw.to_string());
        let _ = writeln!(out, "Newest line, {}:\n{pretty}", newest.place());
        let _ = writeln!(out, "\nHistory, oldest first:");
        history(&mut out, lines);
        if let Some(brief) = briefs.iter().find(|name| **name == format!("{id}.md")) {
            let _ = writeln!(out, "Brief: briefs/{brief}");
        }
        let kept: Vec<_> = reports
            .iter()
            .filter(|name| {
                name.contains(&id)
                    || (!agent.is_empty()
                        && report_subject(name).is_some_and(|subject| subject == agent))
            })
            .map(|name| format!("{REPORTS_DIR}/{name}"))
            .collect();
        if !kept.is_empty() {
            let _ = writeln!(out, "Reports: {}", kept.join(", "));
        }
        out.push('\n');
    }
    Ok(out.trim_end().to_string() + "\n")
}

/// What a kept report's file name, `YYYYMMDD-HHMMSS-<subject>.md`, names
/// after its time: a request id, or an agent's name.
pub fn report_subject(name: &str) -> Option<&str> {
    let rest = name.strip_suffix(".md")?;
    let (stamp, subject) = (rest.get(..15)?, rest.get(16..)?);
    let digits = |text: &str| text.bytes().all(|b| b.is_ascii_digit());
    (digits(&stamp[..8])
        && &stamp[8..9] == "-"
        && digits(&stamp[9..])
        && rest.as_bytes()[15] == b'-'
        && !subject.is_empty())
    .then_some(subject)
}

/// The brief `query` names, whole: by its id (`YYYYMMDD-<name>`, with or
/// without `.md`), else the newest one for the worker name `query`, else the
/// one brief whose name contains it. With several, an error lists them.
pub fn brief(briefs: &[(String, String)], query: &str) -> Result<String> {
    let query = query.trim().trim_end_matches(".md");
    let query = query.strip_prefix("briefs/").unwrap_or(query);
    let wanted = format!("{query}.md");
    let worker = format!("-{query}.md");
    let found = briefs.iter().find(|(name, _)| *name == wanted).or_else(|| {
        briefs
            .iter()
            .rev()
            .find(|(name, _)| name.ends_with(&worker))
    });
    let found = match found {
        Some(found) => found,
        None => {
            let lower = query.to_lowercase();
            let near: Vec<_> = briefs
                .iter()
                .rev()
                .filter(|(name, _)| name.to_lowercase().contains(&lower))
                .collect();
            match near.as_slice() {
                [one] => *one,
                [] => bail!("no brief in briefs/ is named {query:?} or contains it"),
                many => bail!(
                    "no brief is {query:?}; {} contain it, newest first:\n{}",
                    many.len(),
                    many.iter()
                        .map(|(name, _)| name.trim_end_matches(".md"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            }
        }
    };
    Ok(format!(
        "# briefs/{} (whole file)\n{}\n",
        found.0,
        found.1.trim_end()
    ))
}

fn or_dash(text: &str) -> &str {
    if text.is_empty() { "-" } else { text }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATE: &str = "/state/supervisors/weather";

    #[test]
    fn handles_are_paths_in_the_state_directory() {
        assert_eq!(
            parse_handle(STATE, "briefs/20261001-w-a.md:12").unwrap(),
            ("briefs/20261001-w-a.md".into(), Some(12))
        );
        assert_eq!(
            parse_handle(STATE, &format!("{STATE}/decisions.md:3")).unwrap(),
            ("decisions.md".into(), Some(3))
        );
        assert_eq!(
            parse_handle(STATE, "./notes/plan.md").unwrap(),
            ("notes/plan.md".into(), None)
        );
        for bad in [
            "../other/decisions.md:1",
            "/etc/passwd",
            "notes/../../x",
            "inbox.jsonl:3",
            "inbox-reports/a.md",
            "decisions.md:0",
        ] {
            assert!(parse_handle(STATE, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn show_prints_the_whole_block_holding_the_line() {
        let decisions = "# Decisions\n\n## 2026-09-01: A\nDecision: a.\nWhy: b.\n\n\
                         ## 2026-09-02: B\nDecision: c.\n";
        assert_eq!(
            show("decisions.md", decisions, Some(4), "", "").unwrap(),
            "# decisions.md:3 (lines 3-5 of 8)\n## 2026-09-01: A\nDecision: a.\nWhy: b.\n"
        );
        assert!(
            show("decisions.md", decisions, Some(1), "", "")
                .unwrap()
                .starts_with("# decisions.md:1 (lines 1-2 of 8)\n# Decisions\n")
        );

        let note = "# Handover\n\n## Open threads\n\n### Q1: Keep it?\nContext: x.\n\n\
                    ### Q2: Drop it?\n```\n# not a heading\n```\n\n## Promises\nnone\n";
        assert_eq!(
            show("handovers/20261002-132035.md", note, Some(6), "", "").unwrap(),
            "# handovers/20261002-132035.md:5 (lines 5-6 of 14)\n### Q1: Keep it?\nContext: x.\n"
        );
        assert_eq!(
            show("handover.md", note, Some(10), "", "").unwrap(),
            "# handover.md:8 (lines 8-11 of 14)\n### Q2: Drop it?\n```\n# not a heading\n```\n"
        );
        // A brief and a report are one block each.
        let brief = "# Task\n\n## Goal\nDo it.\n";
        assert_eq!(
            show("briefs/20261001-w-a.md", brief, Some(4), "", "").unwrap(),
            "# briefs/20261001-w-a.md:1 (whole file)\n# Task\n\n## Goal\nDo it.\n"
        );
        assert!(show("briefs/x.md", brief, Some(9), "", "").is_err());
        assert_eq!(
            show("notes/raw.txt", "a\nb\n", None, "", "").unwrap(),
            "# notes/raw.txt (whole file)\na\nb\n"
        );
    }

    const LEDGER: &str = r#"{"id":"20261001-w-a","ts":"2026-10-01T09:00:00Z","agent":"w-a","status":"reported","outcome":"done"}
not json
{"id":"20261002-w-b","ts":"2026-10-02T09:00:00Z","agent":"w-b","status":"dispatched","summary":"B"}
{"id":"20261001-w-a","ts":"2026-10-03T09:00:00Z","agent":"w-a","status":"merged"}
"#;
    const ARCHIVE: &str = r#"{"id":"20261001-w-a","ts":"2026-10-01T08:00:00Z","agent":"w-a","status":"dispatched","summary":"A","branch":"worktree/a"}
"#;

    #[test]
    fn a_ledger_id_is_shown_with_its_history_archive_first() {
        let out = ledger(
            LEDGER,
            ARCHIVE,
            "w-a",
            &["20261001-w-a.md".into()],
            &[
                "20261001-100000-20261001-w-a.md".into(),
                "20261001-110000-w-a.md".into(),
                "20261001-120000-w-ab.md".into(),
            ],
        )
        .unwrap();
        assert!(out.starts_with(
            "# Ledger 20261001-w-a, agent w-a: dispatched -> reported -> merged (3 lines)\n\
             Newest line, ledger.jsonl:4:\n{\n  \"agent\": \"w-a\",\n  \"id\": \"20261001-w-a\",\n"
        ));
        assert!(out.contains(
            "History, oldest first:\n\
             - ledger-archive.jsonl:1  2026-10-01T08:00:00Z  dispatched  agent: w-a; branch: worktree/a; summary: A\n\
             - ledger.jsonl:1  2026-10-01T09:00:00Z  reported  agent: w-a; outcome: done\n\
             - ledger.jsonl:4  2026-10-03T09:00:00Z  merged  agent: w-a\n\
             Brief: briefs/20261001-w-a.md\n\
             Reports: reports/20261001-100000-20261001-w-a.md, reports/20261001-110000-w-a.md\n"
        ));
        // By id too, and the archive alone is enough.
        assert!(
            ledger("", ARCHIVE, "20261001-w-a", &[], &[])
                .unwrap()
                .starts_with("# Ledger 20261001-w-a, agent w-a: dispatched (1 line)\n")
        );
        // Part of a name finds the one id that has it; several are listed.
        assert!(
            ledger(LEDGER, ARCHIVE, "b", &[], &[])
                .unwrap()
                .starts_with("# Ledger 20261002-w-b")
        );
        let error = ledger(LEDGER, ARCHIVE, "w-", &[], &[])
            .unwrap_err()
            .to_string();
        assert!(error.contains(
            "2 contain it, newest first:\n20261001-w-a (agent w-a, merged)\n20261002-w-b"
        ));
        assert!(ledger(LEDGER, ARCHIVE, "zebra", &[], &[]).is_err());
    }

    #[test]
    fn showing_a_ledger_line_adds_its_ids_history() {
        let out = show(LEDGER_FILE, LEDGER, Some(3), LEDGER, ARCHIVE).unwrap();
        assert!(
            out.starts_with(
                "# ledger.jsonl:3\n{\n  \"agent\": \"w-b\",\n  \"id\": \"20261002-w-b\""
            )
        );
        assert!(out.ends_with(
            "History of 20261002-w-b, oldest first:\n\
             - ledger.jsonl:3  2026-10-02T09:00:00Z  dispatched  agent: w-b; summary: B\n"
        ));
        assert_eq!(
            show(LEDGER_FILE, LEDGER, Some(2), LEDGER, ARCHIVE).unwrap(),
            "# ledger.jsonl:2\nnot json\n"
        );
    }

    #[test]
    fn a_brief_is_found_by_id_or_worker_name() {
        let briefs = [
            ("20261001-w-morning.md".to_string(), "# Old\n".to_string()),
            ("20261002-w-morning.md".to_string(), "# New\n".to_string()),
            ("20261002-w-other.md".to_string(), "# Other\n".to_string()),
        ];
        assert_eq!(
            brief(&briefs, "20261001-w-morning").unwrap(),
            "# briefs/20261001-w-morning.md (whole file)\n# Old\n"
        );
        assert!(brief(&briefs, "w-morning").unwrap().contains("# New"));
        assert!(
            brief(&briefs, "briefs/20261002-w-other.md")
                .unwrap()
                .contains("# Other")
        );
        assert!(brief(&briefs, "oth").unwrap().contains("# Other"));
        let error = brief(&briefs, "1002").unwrap_err().to_string();
        assert!(
            error.contains("2 contain it, newest first:\n20261002-w-other\n20261002-w-morning")
        );
        assert!(brief(&briefs, "zebra").is_err());
    }

    #[test]
    fn report_names_carry_a_time_and_a_subject() {
        assert_eq!(report_subject("20261027-101500-w-a.md"), Some("w-a"));
        assert_eq!(report_subject("20261027-101500-.md"), None);
        assert_eq!(report_subject("notes.md"), None);
    }
}
