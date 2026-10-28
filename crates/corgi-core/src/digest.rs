//! `corgi digest`: a bounded view of a supervisor's memory, read at the
//! start of its session instead of the raw files, which grow without limit.
//!
//! Everything here is pure text in, text out. The command in `app/cli.rs`
//! reads the state directory, Git, and the fleet, and never writes anything.

use std::{borrow::Cow, collections::HashMap, fmt::Write as _};

pub mod lookup;
pub mod search;

/// Full decision entries, newest first, are shown up to this many bytes...
pub const RECENT_DECISION_BYTES: usize = 8 * 1024;
/// ...but always at least this many, however long they are.
pub const MIN_RECENT_DECISIONS: usize = 3;
/// Titles of older decisions, newest first, are shown up to this many bytes.
pub const DECISION_TITLE_BYTES: usize = 3 * 1024;
/// Finished ledger ids shown.
pub const RECENT_FINISHED: usize = 10;
/// Open work entries are shown up to this many bytes, newest first; those of
/// running agents and those waiting on the user are shown whatever the size.
pub const OPEN_WORK_BYTES: usize = 4 * 1024;
/// A ledger summary or outcome is cut to this many characters in the digest;
/// the ledger keeps it whole.
pub const LEDGER_TEXT_CHARS: usize = 200;
/// A `reported` entry whose agent is not running and whose newest line is at
/// least this old is counted on one line instead of shown in full.
pub const STALE_REPORTED_SECS: u64 = 2 * 24 * 3600;
/// A list of ids in the digest stops near this many bytes.
const ID_LIST_BYTES: usize = 2 * 1024;

const SUPERSEDES: &str = "Supersedes:";

/// One `## <heading>` entry of `decisions.md`, in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision<'a> {
    /// The heading text after `## `, such as `2026-10-02: Use Rust`.
    pub heading: &'a str,
    /// The entry whole, heading included, without trailing blank lines.
    pub text: &'a str,
    /// The headings named by its `Supersedes:` lines.
    pub supersedes: Vec<&'a str>,
    /// The line of `decisions.md` its heading is on, counting from 1.
    pub line: usize,
}

impl Decision<'_> {
    /// `YYYY-MM-DD  <title>`, or the heading as it is when it has no date.
    fn title_line(&self) -> String {
        match self.heading.split_once(": ") {
            Some((date, title)) if is_date(date) => format!("{date}  {}", title.trim()),
            _ => self.heading.to_string(),
        }
    }
}

pub fn is_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

/// The entries of `decisions.md`. Text before the first `## ` heading (the
/// file's title) belongs to none, and a `## ` line inside a code fence starts
/// no entry.
pub fn parse_decisions(text: &str) -> Vec<Decision<'_>> {
    let mut starts = Vec::new();
    let mut offset = 0;
    let mut fenced = false;
    for (line_index, line) in text.split_inclusive('\n').enumerate() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced && line.starts_with("## ") {
            starts.push((offset, line_index + 1));
        }
        offset += line.len();
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, &(start, line))| {
            let end = starts.get(index + 1).map_or(text.len(), |next| next.0);
            let entry = text[start..end].trim_end();
            let heading = entry.lines().next().unwrap_or_default()[3..].trim();
            let supersedes = entry
                .lines()
                .filter_map(|line| line.strip_prefix(SUPERSEDES))
                .map(str::trim)
                .filter(|heading| !heading.is_empty())
                .collect();
            Decision {
                heading,
                text: entry,
                supersedes,
                line,
            }
        })
        .collect()
}

/// For each entry, by index, the heading of the latest one that supersedes
/// it, if any, and a warning for each `Supersedes:` line that names no
/// earlier heading.
pub fn superseded<'a>(decisions: &[Decision<'a>]) -> (Vec<Option<&'a str>>, Vec<String>) {
    let mut hidden = vec![None; decisions.len()];
    let mut warnings = Vec::new();
    for (index, decision) in decisions.iter().enumerate() {
        for &target in &decision.supersedes {
            let mut found = false;
            for (earlier, candidate) in decisions[..index].iter().enumerate() {
                if candidate.heading == target {
                    hidden[earlier] = Some(decision.heading);
                    found = true;
                }
            }
            if !found {
                warnings.push(format!(
                    "\"{SUPERSEDES} {target}\" in \"{}\" matches no earlier heading",
                    decision.heading
                ));
            }
        }
    }
    (hidden, warnings)
}

/// The decision sections of the digest: recent entries in full, the rest as
/// titles, and the footer lines about them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DecisionView<'a> {
    pub recent: Vec<&'a str>,
    pub titles: Vec<String>,
    /// Older titles beyond [`DECISION_TITLE_BYTES`].
    pub titles_left_out: usize,
    pub superseded: usize,
    pub warnings: Vec<String>,
}

pub fn decision_view<'a>(decisions: &[Decision<'a>]) -> DecisionView<'a> {
    let (hidden, warnings) = superseded(decisions);
    let mut visible = decisions
        .iter()
        .zip(&hidden)
        .rev()
        .filter(|(_, hidden)| hidden.is_none())
        .map(|(decision, _)| decision)
        .peekable();
    let mut view = DecisionView {
        superseded: hidden.iter().filter(|hidden| hidden.is_some()).count(),
        warnings,
        ..DecisionView::default()
    };
    let mut bytes = 0;
    while let Some(decision) = visible.peek() {
        let fits = bytes + decision.text.len() <= RECENT_DECISION_BYTES;
        if !fits && view.recent.len() >= MIN_RECENT_DECISIONS {
            break;
        }
        bytes += decision.text.len();
        view.recent.push(decision.text);
        visible.next();
    }
    let mut bytes = 0;
    for decision in visible {
        let title = decision.title_line();
        // Each title is a line of its own.
        if view.titles_left_out == 0 && bytes + title.len() < DECISION_TITLE_BYTES {
            bytes += title.len() + 1;
            view.titles.push(title);
        } else {
            view.titles_left_out += 1;
        }
    }
    view
}

/// The entries, superseded or not, whose heading contains every word of
/// `words`, ignoring case, newest first.
pub fn matching_decisions<'a>(decisions: &[Decision<'a>], words: &str) -> Vec<&'a str> {
    let words: Vec<String> = words.split_whitespace().map(str::to_lowercase).collect();
    decisions
        .iter()
        .rev()
        .filter(|decision| {
            let heading = decision.heading.to_lowercase();
            words.iter().all(|word| heading.contains(word.as_str()))
        })
        .map(|decision| decision.text)
        .collect()
}

/// The entries, superseded or not, whose text has every word of `words` as
/// the start of one of its words, as `--search` matches, newest first: for
/// `--decision` when no heading has the words.
pub fn decisions_containing<'a>(decisions: &[Decision<'a>], words: &str) -> Vec<&'a str> {
    decisions
        .iter()
        .rev()
        .filter(|decision| search::has_every_word(decision.text, words))
        .map(|decision| decision.text)
        .collect()
}

/// The newest ledger line of one id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LedgerEntry {
    pub id: String,
    pub ts: String,
    pub agent: String,
    pub status: String,
    pub branch: String,
    pub summary: String,
    pub outcome: String,
    /// The supervisor's questions to the user about this work, still unanswered,
    /// such as `Q2: Keep the old flag?`.
    pub questions: Vec<String>,
    /// The line of `ledger.jsonl` this newest line is on, counting from 1.
    pub line: usize,
}

impl LedgerEntry {
    /// Statuses whose work needs nothing more: `done` is finished work with
    /// nothing to merge (a check, a review, an investigation).
    fn finished(&self) -> bool {
        matches!(self.status.as_str(), "merged" | "abandoned" | "done")
    }
}

/// What one line of `ledger.jsonl` holds.
enum LedgerLine {
    Blank,
    Entry(LedgerEntry),
    /// Not a JSON object with an `id`, and why.
    Unreadable(String),
}

/// Reads `line`, which is line `number` (from 1) of `ledger.jsonl`.
fn read_ledger_line(line: &str, number: usize) -> LedgerLine {
    if line.trim().is_empty() {
        return LedgerLine::Blank;
    }
    let object = match serde_json::from_str(line) {
        Ok(serde_json::Value::Object(object)) => object,
        Ok(_) => return LedgerLine::Unreadable("is not a JSON object".into()),
        Err(error) => return LedgerLine::Unreadable(format!("is not valid JSON ({error})")),
    };
    let field = |name: &str| match object.get(name) {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    };
    let id = field("id");
    if id.is_empty() {
        return LedgerLine::Unreadable("has no \"id\"".into());
    }
    LedgerLine::Entry(LedgerEntry {
        id,
        ts: field("ts"),
        agent: field("agent"),
        status: field("status"),
        branch: field("branch"),
        summary: field("summary"),
        outcome: field("outcome"),
        questions: match object.get("questions") {
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .map(|item| match item {
                    serde_json::Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .collect(),
            _ => Vec::new(),
        },
        line: number,
    })
}

/// The newest line of each id in `ledger.jsonl`, in the order those lines
/// appear. Lines that are not a JSON object with an `id` are skipped
/// ([`unreadable_ledger_lines`] names them).
pub fn parse_ledger(text: &str) -> Vec<LedgerEntry> {
    let mut entries: Vec<LedgerEntry> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for (index, line) in text.lines().enumerate() {
        let LedgerLine::Entry(entry) = read_ledger_line(line, index + 1) else {
            continue;
        };
        match by_id.get(&entry.id) {
            Some(&slot) => entries[slot] = entry,
            None => {
                by_id.insert(entry.id.clone(), entries.len());
                entries.push(entry);
            }
        }
    }
    entries.sort_by_key(|entry| entry.line);
    entries
}

/// The lines of `ledger.jsonl` that are neither blank nor a JSON object with
/// an `id`, as `(line from 1, what is wrong)`.
pub fn unreadable_ledger_lines(text: &str) -> Vec<(usize, String)> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| match read_ledger_line(line, index + 1) {
            LedgerLine::Unreadable(why) => Some((index + 1, why)),
            _ => None,
        })
        .collect()
}

/// `ledger.jsonl` without its superseded lines, as `(kept, superseded)`, or
/// `None` when it has none. A line is superseded when a later line has the
/// same id. Every other line, blank and unreadable ones too, is kept as it
/// was and where it was, so the newest lines keep their order and
/// [`parse_ledger`] reads the same entries in the same order from `kept`.
/// `superseded` has the lines dropped, in their order, each ending in a
/// newline (a later line follows each, so each had one).
pub fn compact_ledger(text: &str) -> Option<(String, String)> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let ids: Vec<Option<String>> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| match read_ledger_line(line, index + 1) {
            LedgerLine::Entry(entry) => Some(entry.id),
            _ => None,
        })
        .collect();
    let mut newest: HashMap<&str, usize> = HashMap::new();
    for (index, id) in ids.iter().enumerate() {
        if let Some(id) = id {
            newest.insert(id, index);
        }
    }
    let (mut kept, mut superseded) = (String::new(), String::new());
    for (index, (line, id)) in lines.iter().zip(&ids).enumerate() {
        match id {
            Some(id) if newest[id.as_str()] != index => superseded.push_str(line),
            _ => kept.push_str(line),
        }
    }
    (!superseded.is_empty()).then_some((kept, superseded))
}

/// The questions the newest ledger entry of `agent` leaves open, or `None`
/// when it leaves none: its status is `needs-answer` or its `questions` are
/// not empty. An agent without an entry, such as one the user started, has
/// none. An entry with status `needs-answer` but no questions listed gives
/// an empty list.
pub fn open_questions(ledger: &str, agent: &str) -> Option<Vec<String>> {
    let entry = parse_ledger(ledger)
        .into_iter()
        .filter(|entry| entry.agent == agent)
        .max_by_key(|entry| entry.line)?;
    (entry.status == "needs-answer" || !entry.questions.is_empty()).then_some(entry.questions)
}

/// A running agent of the project, as `corgi fleet` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveAgent {
    pub name: String,
    pub state: String,
    pub context_percent: Option<u8>,
}

/// Everything the digest shows, already read.
pub struct DigestInput<'a> {
    pub project: &'a str,
    pub branch: &'a str,
    pub head: &'a str,
    pub state_dir: &'a str,
    pub handover: Option<&'a str>,
    /// The newest archived note in `handovers/`, as `(file name, text)`,
    /// whose open threads are shown when there is no `handover`.
    pub previous_handover: Option<(&'a str, &'a str)>,
    pub decisions: &'a str,
    pub ledger: &'a str,
    pub fleet: &'a [LiveAgent],
    /// The wall clock in Unix seconds, to tell stale ledger entries.
    pub now: u64,
    /// The command that prints one decision in full, with `<words>` in it.
    pub decision_command: &'a str,
    /// The command that searches all of the memory, with `<words>` in it.
    pub search_command: &'a str,
}

/// `YYYY-MM-DD HH:MM UTC` from an archived note's name,
/// `YYYYMMDD-HHMMSS.md`, or `None` for any other name.
pub fn handover_time(name: &str) -> Option<String> {
    let stamp = name.strip_suffix(".md").unwrap_or(name);
    let (date, time) = stamp.split_once('-')?;
    let digits = |text: &str, len| text.len() == len && text.bytes().all(|b| b.is_ascii_digit());
    (digits(date, 8) && digits(time, 6)).then(|| {
        format!(
            "{}-{}-{} {}:{} UTC",
            &date[..4],
            &date[4..6],
            &date[6..],
            &time[..2],
            &time[2..4]
        )
    })
}

/// The body of a handover note's "Open threads" section, under any heading
/// level, up to the next heading of the same level or higher.
pub fn open_threads(note: &str) -> Option<&str> {
    let level = |line: &str| line.bytes().take_while(|b| *b == b'#').count();
    let mut offset = 0;
    let mut start = None;
    let mut fenced = false;
    for line in note.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced && level(line) > 0 {
            match start {
                Some((from, heading)) if level(line) <= heading => {
                    return Some(note[from..offset].trim());
                }
                None if line.to_lowercase().contains("open threads") => {
                    start = Some((offset + line.len(), level(line)));
                }
                _ => {}
            }
        }
        offset += line.len();
    }
    start.map(|(from, _)| note[from..].trim())
}

/// The digest, its seven sections in order.
pub fn render(input: &DigestInput) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Project {}", input.project);
    let _ = writeln!(out, "Base branch: {} at {}", input.branch, input.head);
    let _ = writeln!(out, "State: {}", input.state_dir);

    if let Some(handover) = input.handover {
        let _ = writeln!(
            out,
            "\n# Handover note (archive it as your role says)\n{}",
            handover.trim()
        );
    } else if let Some((name, note)) = input.previous_handover
        && let Some(threads) = open_threads(note)
    {
        let when = handover_time(name).unwrap_or_else(|| name.to_string());
        let _ = writeln!(
            out,
            "\n# Open threads from the previous session's note ({when}, handovers/{name}; \
             may be out of date, check before acting on it)\n{}",
            if threads.is_empty() { "none" } else { threads }
        );
    }

    let ledger = parse_ledger(input.ledger);
    let open: Vec<_> = ledger.iter().filter(|entry| !entry.finished()).collect();
    let _ = writeln!(out, "\n# Open work ({})", open.len());
    if open.is_empty() {
        out.push_str("none\n");
    }
    out.push_str(&open_work(&open, input.fleet, input.now));

    let _ = writeln!(out, "\n# Recently finished");
    let finished: Vec<_> = ledger
        .iter()
        .rev()
        .filter(|entry| entry.finished())
        .take(RECENT_FINISHED)
        .collect();
    if finished.is_empty() {
        out.push_str("none\n");
    }
    for entry in finished {
        let said = if entry.outcome.is_empty() {
            &entry.summary
        } else {
            &entry.outcome
        };
        let _ = writeln!(
            out,
            "- {}  {}  {}  {}",
            or_dash(entry.ts.get(..10).unwrap_or(&entry.ts)),
            entry.status,
            or_dash(&entry.agent),
            clip(said)
        );
    }

    let decisions = parse_decisions(input.decisions);
    let view = decision_view(&decisions);
    let _ = writeln!(out, "\n# Recent decisions (full text, newest first)");
    if view.recent.is_empty() {
        out.push_str("none\n");
    }
    for text in &view.recent {
        let _ = writeln!(out, "\n{text}");
    }

    let _ = writeln!(out, "\n# Older decisions (titles, newest first)");
    if view.titles.is_empty() {
        out.push_str("none\n");
    }
    for title in &view.titles {
        let _ = writeln!(out, "{title}");
    }
    if view.titles_left_out > 0 {
        let _ = writeln!(
            out,
            "... and {} older titles left out",
            view.titles_left_out
        );
    }

    let _ = writeln!(out, "\n# Notes");
    let _ = writeln!(
        out,
        "{} superseded decision{} hidden.",
        view.superseded,
        if view.superseded == 1 { "" } else { "s" }
    );
    let _ = writeln!(out, "Read any decision in full: {}", input.decision_command);
    let _ = writeln!(
        out,
        "Search all of your memory (decisions, ledger, briefs, reports, notes, handover notes) \
         before you say something is unknown or never happened: {}",
        input.search_command
    );
    for warning in &view.warnings {
        let _ = writeln!(out, "warning: {warning}");
    }
    for (line, why) in unreadable_ledger_lines(input.ledger) {
        let _ = writeln!(
            out,
            "warning: {}/ledger.jsonl:{line} {why}, so it is skipped; append a \
             corrected line for its id",
            input.state_dir
        );
    }
    out
}

/// The entries of the "Open work" section, `open` in ledger order. A
/// `reported` entry whose agent is not running and whose newest line is
/// [`STALE_REPORTED_SECS`] old is only counted, on one line with the others
/// like it. The rest are shown in full, newest first while they fit in
/// [`OPEN_WORK_BYTES`] (an entry of a running agent, or one `needs-answer` or
/// `blocked`, always), and listed in ledger order; one more line names those
/// left out.
fn open_work(open: &[&LedgerEntry], fleet: &[LiveAgent], now: u64) -> String {
    let running = |entry: &LedgerEntry| {
        fleet
            .iter()
            .find(|agent| !entry.agent.is_empty() && agent.name == entry.agent)
    };
    let stale = |entry: &LedgerEntry| {
        entry.status == "reported"
            && running(entry).is_none()
            && crate::time::parse_rfc3339(&entry.ts)
                .is_some_and(|ts| now.saturating_sub(ts) >= STALE_REPORTED_SECS)
    };
    let (stale, listed): (Vec<&LedgerEntry>, Vec<&LedgerEntry>) =
        open.iter().copied().partition(|entry| stale(entry));
    let blocks: Vec<(String, bool)> = listed
        .iter()
        .map(|entry| {
            let live = running(entry);
            let pinned =
                live.is_some() || matches!(entry.status.as_str(), "needs-answer" | "blocked");
            (open_entry(entry, live), pinned)
        })
        .collect();
    let mut shown = vec![false; blocks.len()];
    let mut bytes = 0;
    let mut full = false;
    for (index, (block, pinned)) in blocks.iter().enumerate().rev() {
        if !*pinned && (full || bytes + block.len() > OPEN_WORK_BYTES) {
            full = true;
            continue;
        }
        bytes += block.len();
        shown[index] = true;
    }
    let mut out = String::new();
    for ((block, _), shown) in blocks.iter().zip(&shown) {
        if *shown {
            out.push_str(block);
        }
    }
    if !stale.is_empty() {
        let _ = writeln!(
            out,
            "- {} more `reported`, agent not running, newest line 2+ days old (set each \
             `done` if it needs nothing more): {}",
            stale.len(),
            id_list(stale.iter().map(|entry| entry.id.as_str()))
        );
    }
    let left_out: Vec<_> = listed
        .iter()
        .zip(&shown)
        .filter(|(_, shown)| !**shown)
        .map(|(entry, _)| entry.id.as_str())
        .collect();
    if !left_out.is_empty() {
        let _ = writeln!(
            out,
            "- {} older left out for size (read one with --search): {}",
            left_out.len(),
            id_list(left_out.into_iter())
        );
    }
    out
}

/// One entry of "Open work", joined with its running agent `live`, if any.
fn open_entry(entry: &LedgerEntry, live: Option<&LiveAgent>) -> String {
    let live = live.map_or_else(
        || "**NOT RUNNING**".to_string(),
        |agent| {
            let context = agent
                .context_percent
                .map_or_else(|| "-".to_string(), |percent| format!("{percent}%"));
            format!("{}, ctx {context}", agent.state)
        },
    );
    let mut out = format!(
        "- {} [{}] agent {}: {live}; branch {}\n",
        entry.id,
        or_dash(&entry.status),
        or_dash(&entry.agent),
        or_dash(&entry.branch)
    );
    if !entry.summary.is_empty() {
        let _ = writeln!(out, "  summary: {}", clip(&entry.summary));
    }
    if !entry.outcome.is_empty() {
        let _ = writeln!(out, "  outcome: {}", clip(&entry.outcome));
    }
    out
}

/// `text` cut to [`LEDGER_TEXT_CHARS`] characters, with `…` where it was cut.
fn clip(text: &str) -> Cow<'_, str> {
    match text.char_indices().nth(LEDGER_TEXT_CHARS) {
        Some((end, _)) => format!("{}…", text[..end].trim_end()).into(),
        None => text.into(),
    }
}

/// `ids` joined by commas, stopping near [`ID_LIST_BYTES`] with a count of
/// the rest.
fn id_list<'a>(ids: impl ExactSizeIterator<Item = &'a str>) -> String {
    let total = ids.len();
    let mut out = String::new();
    for (index, id) in ids.enumerate() {
        if !out.is_empty() && out.len() + id.len() > ID_LIST_BYTES {
            let _ = write!(out, ", … and {} more", total - index);
            break;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(id);
    }
    out
}

fn or_dash(text: &str) -> &str {
    if text.is_empty() { "-" } else { text }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(heading: &str, body: &str) -> String {
        format!("## {heading}\nDecision: {body}\nWhy: because.\n\n")
    }

    #[test]
    fn decisions_are_whole_entries_and_fenced_headings_start_none() {
        let text = format!(
            "# corgi decisions\n\n{}## 2026-10-01: Second\nDecision: x\n```\n## not a heading\n```\n",
            entry("2026-09-30: First", "a")
        );
        let decisions = parse_decisions(&text);
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].heading, "2026-09-30: First");
        assert_eq!(
            decisions[0].text,
            "## 2026-09-30: First\nDecision: a\nWhy: because."
        );
        assert!(decisions[1].text.ends_with("## not a heading\n```"));
        assert_eq!(decisions[1].title_line(), "2026-10-01  Second");
    }

    #[test]
    fn a_superseded_entry_is_hidden_and_an_unmatched_reference_warned_about() {
        let text = format!(
            "{}{}{}",
            entry("2026-09-01: Use tabs", "tabs"),
            entry("2026-09-02: Keep it", "kept"),
            "## 2026-09-03: Use spaces\nDecision: spaces\n\
             Supersedes: 2026-09-01: Use tabs\nSupersedes: 2026-08-01: Never was\n",
        );
        let view = decision_view(&parse_decisions(&text));
        assert_eq!(view.superseded, 1);
        assert_eq!(view.recent.len(), 2);
        assert!(view.recent[0].starts_with("## 2026-09-03: Use spaces"));
        assert!(view.recent[1].starts_with("## 2026-09-02: Keep it"));
        assert_eq!(
            view.warnings,
            [
                "\"Supersedes: 2026-08-01: Never was\" in \"2026-09-03: Use spaces\" \
              matches no earlier heading"
            ]
        );
        // A reference to a later entry supersedes nothing.
        let forward = format!(
            "## 2026-09-01: A\nSupersedes: 2026-09-02: B\n\n{}",
            entry("2026-09-02: B", "b")
        );
        let view = decision_view(&parse_decisions(&forward));
        assert_eq!((view.superseded, view.warnings.len()), (0, 1));
    }

    #[test]
    fn recent_decisions_fill_eight_kilobytes_but_never_fewer_than_three() {
        let big = "x".repeat(3000);
        let text: String = (1..=6)
            .map(|day| entry(&format!("2026-09-0{day}: Big {day}"), &big))
            .collect();
        let view = decision_view(&parse_decisions(&text));
        // Two fit in 8 KB; the third is shown anyway.
        assert_eq!(view.recent.len(), 3);
        assert!(view.recent[0].contains("Big 6"));
        assert_eq!(
            view.titles,
            [
                "2026-09-03  Big 3",
                "2026-09-02  Big 2",
                "2026-09-01  Big 1"
            ]
        );

        let small: String = (0..100)
            .map(|n| entry(&format!("2026-09-01: Small {n}"), &"y".repeat(200)))
            .collect();
        let view = decision_view(&parse_decisions(&small));
        let bytes: usize = view.recent.iter().map(|text| text.len()).sum();
        assert!(bytes <= RECENT_DECISION_BYTES && view.recent.len() > 3);
        assert_eq!(view.recent.len() + view.titles.len(), 100);
        // Whole entries only: the next one would not have fit.
        let next = parse_decisions(&small)[100 - view.recent.len() - 1]
            .text
            .len();
        assert!(bytes + next > RECENT_DECISION_BYTES);
    }

    #[test]
    fn older_titles_fill_three_kilobytes_and_a_line_counts_the_rest() {
        let text: String = (0..600)
            .map(|n| entry(&format!("2026-01-01: D{n:03}"), &"z".repeat(5000)))
            .collect();
        let decisions = parse_decisions(&text);
        let view = decision_view(&decisions);
        assert_eq!(view.recent.len(), 3);
        assert_eq!(view.titles[0], "2026-01-01  D596");
        // Each title and its newline is 17 bytes: 180 make 3060, 181 too many.
        assert_eq!(view.titles.len(), 180);
        assert_eq!(
            view.titles.last().map(String::as_str),
            Some("2026-01-01  D417")
        );
        assert_eq!(view.titles_left_out, 417);
        let shown: usize = view.titles.iter().map(|title| title.len() + 1).sum();
        assert!(shown <= DECISION_TITLE_BYTES);
        let rendered = render(&input(&text, "", &[]));
        assert!(rendered.contains("\n2026-01-01  D417\n... and 417 older titles left out\n"));
    }

    #[test]
    fn the_newest_ledger_line_wins_and_bad_lines_are_skipped() {
        let ledger = r#"{"id":"a","agent":"w-a","status":"dispatched","summary":"A"}
not json
{"status":"merged"}
{"id":"b","agent":"w-b","status":"dispatched","summary":"B"}
{"id":"a","agent":"w-a","status":"merged","ts":"2026-10-01T10:00:00Z","outcome":"done well"}
{"id":"c","agent":"w-c","status":"reported","branch":"worktree/c"}
"#;
        let entries = parse_ledger(ledger);
        let ids: Vec<_> = entries.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, ["b", "a", "c"]);
        assert_eq!(entries[1].status, "merged");
        assert_eq!(entries[1].summary, "");

        let fleet = [LiveAgent {
            name: "w-c".into(),
            state: "idle".into(),
            context_percent: Some(41),
        }];
        let rendered = render(&input("", ledger, &fleet));
        assert!(rendered.contains("# Open work (2)\n- b [dispatched] agent w-b: **NOT RUNNING**"));
        assert!(rendered.contains("- c [reported] agent w-c: idle, ctx 41%; branch worktree/c"));
        assert!(rendered.contains("# Recently finished\n- 2026-10-01  merged  w-a  done well\n"));
    }

    #[test]
    fn decisions_are_found_by_every_word_of_their_heading_superseded_or_not() {
        let text = format!(
            "{}{}",
            entry("2026-09-01: Use Tabs for indentation", "tabs"),
            "## 2026-09-03: Use spaces\nSupersedes: 2026-09-01: Use Tabs for indentation\n",
        );
        let decisions = parse_decisions(&text);
        let found = matching_decisions(&decisions, "tabs USE");
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("Decision: tabs"));
        assert_eq!(matching_decisions(&decisions, "use").len(), 2);
        assert!(matching_decisions(&decisions, "use rust").is_empty());
        // In the text, words match as --search matches them.
        let found = decisions_containing(&decisions, "SUPERSEDES indent");
        assert_eq!(found.len(), 1);
        assert!(found[0].starts_with("## 2026-09-03: Use spaces"));
        assert!(decisions_containing(&decisions, "dentation").is_empty());
    }

    #[test]
    fn open_questions_follow_the_agents_newest_ledger_line() {
        let dispatched = r#"{"id":"a","agent":"w-a","status":"dispatched","summary":"A"}"#;
        let asked = r#"{"id":"a","agent":"w-a","status":"needs-answer","questions":["Q2: Keep the flag?","Q3: Rename it?"]}"#;
        let answered = r#"{"id":"a","agent":"w-a","status":"reported","questions":[]}"#;
        let listed =
            r#"{"id":"a","agent":"w-a","status":"reported","questions":["Q4: Which default?"]}"#;
        let bare = r#"{"id":"a","agent":"w-a","status":"needs-answer"}"#;
        let other = r#"{"id":"b","agent":"w-b","status":"needs-answer","questions":["Q1: B?"]}"#;

        assert_eq!(
            open_questions(&format!("{dispatched}\n{asked}"), "w-a"),
            Some(vec!["Q2: Keep the flag?".into(), "Q3: Rename it?".into()])
        );
        assert_eq!(
            open_questions(listed, "w-a"),
            Some(vec!["Q4: Which default?".into()])
        );
        assert_eq!(open_questions(bare, "w-a"), Some(Vec::new()));
        // A newer line that clears the questions answers them.
        assert_eq!(open_questions(&format!("{asked}\n{answered}"), "w-a"), None);
        // Another agent's questions, no entry, no ledger, or a broken one.
        assert_eq!(
            open_questions(&format!("{dispatched}\n{other}"), "w-a"),
            None
        );
        assert_eq!(open_questions(other, "w-a"), None);
        assert_eq!(open_questions("", "w-a"), None);
        assert_eq!(open_questions("not json\n{", "w-a"), None);
    }

    /// 2026-10-27T12:00:00Z.
    const NOW: u64 = 1_793_102_400;

    fn line(id: &str, ts: &str, agent: &str, status: &str, outcome: &str) -> String {
        format!(
            r#"{{"id":"{id}","ts":"{ts}","agent":"{agent}","status":"{status}","outcome":"{outcome}"}}"#
        )
    }

    #[test]
    fn done_work_is_finished_not_open() {
        let ledger = [
            line("a", "2026-10-26T09:00:00Z", "w-a", "reported", "checked"),
            line("a", "2026-10-27T09:00:00Z", "w-a", "done", "all clear"),
            line("b", "2026-10-27T10:00:00Z", "w-b", "dispatched", ""),
        ]
        .join("\n");
        let rendered = render(&input("", &ledger, &[]));
        assert!(rendered.contains("# Open work (1)\n- b [dispatched]"));
        assert!(!rendered.contains("- a ["));
        assert!(rendered.contains("# Recently finished\n- 2026-10-27  done  w-a  all clear\n"));
    }

    #[test]
    fn stale_reported_work_of_a_stopped_agent_is_counted_on_one_line() {
        let ledger = [
            // Reported three days ago, its agent gone: collapsed.
            line("old-1", "2026-10-24T09:00:00Z", "w-1", "reported", "one"),
            line("old-2", "2026-10-25T11:59:00Z", "w-2", "reported", "two"),
            // Its agent still runs, it is recent, it has no time, or it is
            // not merely reported: shown in full.
            line("live", "2026-10-20T09:00:00Z", "w-live", "reported", "live"),
            line("new", "2026-10-26T09:00:00Z", "w-new", "reported", "new"),
            line("no-ts", "", "w-x", "reported", "no time"),
            line(
                "ask",
                "2026-10-01T09:00:00Z",
                "w-ask",
                "needs-answer",
                "ask",
            ),
        ]
        .join("\n");
        let fleet = [LiveAgent {
            name: "w-live".into(),
            state: "idle".into(),
            context_percent: None,
        }];
        let rendered = render(&input("", &ledger, &fleet));
        assert!(rendered.contains("# Open work (6)\n"));
        for shown in ["- live [", "- new [", "- no-ts [", "- ask ["] {
            assert!(rendered.contains(shown), "{shown}");
        }
        assert!(!rendered.contains("- old-1 [") && !rendered.contains("outcome: one"));
        assert!(rendered.contains(
            "- 2 more `reported`, agent not running, newest line 2+ days old (set each `done` \
             if it needs nothing more): old-1, old-2\n\n# Recently finished"
        ));
    }

    #[test]
    fn open_work_fills_its_bytes_newest_first_and_long_text_is_cut() {
        let long = "word ".repeat(100);
        let mut lines: Vec<String> = (0..40)
            .map(|n| line(&format!("w{n:02}"), "", "", "ready-to-merge", &long))
            .collect();
        // The oldest waits on the user, so it is shown however full it is.
        lines[0] = line("w00", "", "w-ask", "needs-answer", "Q1?");
        let finished = line("m", "2026-10-27T09:00:00Z", "w-m", "merged", &long);
        lines.push(finished);
        let rendered = render(&input("", &lines.join("\n"), &[]));
        let section = &rendered
            [rendered.find("# Open work").unwrap()..rendered.find("# Recently finished").unwrap()];
        let shown: Vec<_> = section
            .lines()
            .filter_map(|line| line.strip_prefix("- w"))
            .map(|rest| &rest[..2])
            .collect();
        // w00 first, then the newest run that fits, in ledger order.
        assert_eq!(shown[0], "00");
        assert_eq!(shown.last(), Some(&"39"));
        let kept = shown.len() - 1;
        assert!(kept > 1 && kept < 39);
        assert_eq!(shown[1], format!("{:02}", 40 - kept));
        assert!(section.len() < OPEN_WORK_BYTES + 1024);
        assert!(section.contains(&format!(
            "- {} older left out for size (read one with --search): w01, w02, ",
            39 - kept
        )));
        // Cut to 200 characters, the cut marked, in open and finished work.
        let cut = format!("{}…", long[..LEDGER_TEXT_CHARS].trim_end());
        assert!(section.contains(&format!("  outcome: {cut}\n")));
        assert!(rendered.contains(&format!("merged  w-m  {cut}\n")));
        assert_eq!(clip("short"), "short");
        assert_eq!(
            clip(&"é".repeat(LEDGER_TEXT_CHARS)),
            "é".repeat(LEDGER_TEXT_CHARS)
        );
    }

    #[test]
    fn a_long_list_of_ids_stops_with_a_count() {
        let ids: Vec<String> = (0..500).map(|n| format!("20261027-w-{n:03}")).collect();
        let list = id_list(ids.iter().map(String::as_str));
        assert!(list.len() < ID_LIST_BYTES + 32);
        let shown = list.matches("20261027-w-").count();
        assert!(list.ends_with(&format!(", … and {} more", 500 - shown)));
    }

    #[test]
    fn unreadable_ledger_lines_are_warned_about_by_file_and_line() {
        let ledger = "{\"id\":\"a\",\"status\":\"dispatched\"}\n\n\
                      {\"id\":\"a\",\"outcome\":\"bad \\q escape\"}\n\
                      [1]\n{\"status\":\"merged\"}\n";
        assert_eq!(
            unreadable_ledger_lines(ledger)
                .iter()
                .map(|(line, _)| *line)
                .collect::<Vec<_>>(),
            [3, 4, 5]
        );
        let rendered = render(&input("", ledger, &[]));
        assert!(rendered.contains(
            "warning: /state/supervisors/weather/ledger.jsonl:3 is not valid JSON (invalid \
             escape at line 1 column "
        ));
        assert!(rendered.contains(
            "warning: /state/supervisors/weather/ledger.jsonl:4 is not a JSON object, so it is \
             skipped; append a corrected line for its id\n"
        ));
        assert!(rendered.contains("ledger.jsonl:5 has no \"id\", so"));
        assert!(!render(&input("", "", &[])).contains("warning"));
    }

    #[test]
    fn compaction_keeps_the_newest_line_of_each_id_and_every_unreadable_line() {
        let ledger = format!(
            "{}\n{}\nnot json\n\n{}\n{}\n{}\n{}",
            line("a", "2026-10-01T00:00:00Z", "w-a", "dispatched", ""),
            line("b", "2026-10-02T00:00:00Z", "w-b", "dispatched", ""),
            line("a", "2026-10-03T00:00:00Z", "w-a", "reported", "first"),
            r#"{"id":"b","agent":"w-b","status":"needs-answer","questions":["Q1: B?"]}"#,
            line("c", "2026-10-04T00:00:00Z", "w-b", "dispatched", ""),
            line("a", "2026-10-05T00:00:00Z", "w-a", "merged", "done"),
        );
        let (kept, superseded) = compact_ledger(&ledger).unwrap();
        assert_eq!(
            kept,
            format!(
                "not json\n\n{}\n{}\n{}",
                r#"{"id":"b","agent":"w-b","status":"needs-answer","questions":["Q1: B?"]}"#,
                line("c", "2026-10-04T00:00:00Z", "w-b", "dispatched", ""),
                line("a", "2026-10-05T00:00:00Z", "w-a", "merged", "done"),
            )
        );
        assert_eq!(
            superseded,
            format!(
                "{}\n{}\n{}\n",
                line("a", "2026-10-01T00:00:00Z", "w-a", "dispatched", ""),
                line("b", "2026-10-02T00:00:00Z", "w-b", "dispatched", ""),
                line("a", "2026-10-03T00:00:00Z", "w-a", "reported", "first"),
            )
        );
        // The same entries, in the same order, and the same answers for tag.
        let strip = |entries: Vec<LedgerEntry>| -> Vec<LedgerEntry> {
            entries
                .into_iter()
                .map(|entry| LedgerEntry { line: 0, ..entry })
                .collect()
        };
        assert_eq!(strip(parse_ledger(&kept)), strip(parse_ledger(&ledger)));
        for agent in ["w-a", "w-b", "w-x"] {
            assert_eq!(
                open_questions(&kept, agent),
                open_questions(&ledger, agent),
                "{agent}"
            );
        }
        let digest = |ledger: &str| {
            let rendered = render(&input("", ledger, &[]));
            rendered[..rendered.find("# Notes").unwrap()].to_string()
        };
        assert_eq!(digest(&kept), digest(&ledger));
        // Once compacted, there is nothing more to do.
        assert_eq!(compact_ledger(&kept), None);
        assert_eq!(compact_ledger(""), None);
        assert_eq!(compact_ledger("not json\n"), None);
    }

    fn input<'a>(decisions: &'a str, ledger: &'a str, fleet: &'a [LiveAgent]) -> DigestInput<'a> {
        DigestInput {
            project: "/repos/weather",
            branch: "main",
            head: "abc1234",
            state_dir: "/state/supervisors/weather",
            handover: None,
            previous_handover: None,
            decisions,
            ledger,
            fleet,
            decision_command: "corgi digest /repos/weather --decision \"<words>\"",
            search_command: "corgi digest /repos/weather --search \"<words>\"",
            now: NOW,
        }
    }

    #[test]
    fn the_sections_come_in_order_and_empty_state_is_no_error() {
        let empty = render(&input("", "", &[]));
        let decisions = entry("2026-09-01: Old", "x");
        let mut handover = input(&decisions, r#"{"id":"a","status":"abandoned"}"#, &[]);
        handover.handover = Some("## Open threads\nnone\n");
        let full = render(&handover);
        let order = [
            "# Project /repos/weather\nBase branch: main at abc1234",
            "# Handover note",
            "# Open work",
            "# Recently finished",
            "# Recent decisions",
            "# Older decisions",
            "# Notes\n0 superseded decisions hidden.\nRead any decision in full: corgi digest",
        ];
        let positions: Vec<_> = order
            .iter()
            .map(|heading| full.find(heading).expect(heading))
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(!empty.contains("# Handover"));
        assert!(empty.contains("# Open work (0)\nnone\n"));
    }

    #[test]
    fn without_a_handover_note_the_newest_archived_open_threads_are_shown() {
        let note = "# Handover\n\n## Open threads with the user\n- Ask about X.\n  ### detail\n\n\
                    ### Sub\nkept\n## Promises to the user\nnone\n";
        // A deeper heading stays inside the section; the next `##` ends it.
        assert_eq!(
            open_threads(note),
            Some("- Ask about X.\n  ### detail\n\n### Sub\nkept")
        );
        assert_eq!(open_threads("# Handover\nnone\n"), None);
        assert_eq!(open_threads("## Open threads\n- last\n"), Some("- last"));
        assert_eq!(
            handover_time("20261003-153017.md").as_deref(),
            Some("2026-10-03 15:30 UTC")
        );
        assert_eq!(handover_time("notes.md"), None);

        let mut archived = input("", "", &[]);
        archived.previous_handover = Some(("20261003-153017.md", note));
        let shown = render(&archived);
        assert!(shown.contains(
            "\n# Open threads from the previous session's note (2026-10-03 15:30 UTC, \
             handovers/20261003-153017.md; may be out of date, check before acting on it)\n\
             - Ask about X.\n  ### detail\n\n### Sub\nkept\n\n# Open work"
        ));
        assert!(!shown.contains("Promises"));
        // A current note wins, and the archived one is not shown beside it.
        archived.handover = Some("## Open threads\nnew\n");
        let shown = render(&archived);
        assert!(shown.contains("# Handover note") && !shown.contains("previous session"));
        // Without either, the digest is as before, apart from the search line.
        let plain = render(&input("", "", &[]));
        assert!(!plain.contains("Open threads"));
        assert!(plain.contains(
            "before you say something is unknown or never happened: \
             corgi digest /repos/weather --search \"<words>\"\n"
        ));
    }
}
