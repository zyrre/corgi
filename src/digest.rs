//! `corgi digest`: a bounded view of a Project handler's memory, read at the
//! start of its session instead of the raw files, which grow without limit.
//!
//! Everything here is pure text in, text out. The command in `app/cli.rs`
//! reads the state directory, Git, and the fleet, and never writes anything.

use std::{collections::HashMap, fmt::Write as _};

/// Full decision entries, newest first, are shown up to this many bytes...
pub const RECENT_DECISION_BYTES: usize = 12 * 1024;
/// ...but always at least this many, however long they are.
pub const MIN_RECENT_DECISIONS: usize = 3;
/// Titles of older decisions shown at most.
pub const MAX_DECISION_TITLES: usize = 500;
/// Finished ledger ids shown.
pub const RECENT_FINISHED: usize = 10;

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

fn is_date(text: &str) -> bool {
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
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced && line.starts_with("## ") {
            starts.push(offset);
        }
        offset += line.len();
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, &start)| {
            let end = starts.get(index + 1).copied().unwrap_or(text.len());
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
            }
        })
        .collect()
}

/// Which entries a later one supersedes, by index, and a warning for each
/// `Supersedes:` line that names no earlier heading.
fn superseded(decisions: &[Decision]) -> (Vec<bool>, Vec<String>) {
    let mut hidden = vec![false; decisions.len()];
    let mut warnings = Vec::new();
    for (index, decision) in decisions.iter().enumerate() {
        for &target in &decision.supersedes {
            let mut found = false;
            for (earlier, candidate) in decisions[..index].iter().enumerate() {
                if candidate.heading == target {
                    hidden[earlier] = true;
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
    /// Older titles beyond [`MAX_DECISION_TITLES`].
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
        .filter(|(_, hidden)| !**hidden)
        .map(|(decision, _)| decision)
        .peekable();
    let mut view = DecisionView {
        superseded: hidden.iter().filter(|hidden| **hidden).count(),
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
    for decision in visible {
        if view.titles.len() < MAX_DECISION_TITLES {
            view.titles.push(decision.title_line());
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
}

impl LedgerEntry {
    fn finished(&self) -> bool {
        matches!(self.status.as_str(), "merged" | "abandoned")
    }
}

/// The newest line of each id in `ledger.jsonl`, in the order those lines
/// appear. Lines that are not a JSON object with an `id` are skipped.
pub fn parse_ledger(text: &str) -> Vec<LedgerEntry> {
    let mut entries: Vec<(usize, LedgerEntry)> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for (line_number, line) in text.lines().enumerate() {
        let Ok(serde_json::Value::Object(object)) = serde_json::from_str(line) else {
            continue;
        };
        let field = |name: &str| match object.get(name) {
            Some(serde_json::Value::String(text)) => text.clone(),
            Some(serde_json::Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        let id = field("id");
        if id.is_empty() {
            continue;
        }
        let entry = LedgerEntry {
            id: id.clone(),
            ts: field("ts"),
            agent: field("agent"),
            status: field("status"),
            branch: field("branch"),
            summary: field("summary"),
            outcome: field("outcome"),
        };
        match by_id.get(&id) {
            Some(&slot) => entries[slot] = (line_number, entry),
            None => {
                by_id.insert(id, entries.len());
                entries.push((line_number, entry));
            }
        }
    }
    entries.sort_by_key(|(line_number, _)| *line_number);
    entries.into_iter().map(|(_, entry)| entry).collect()
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
    pub decisions: &'a str,
    pub ledger: &'a str,
    pub fleet: &'a [LiveAgent],
    /// The command that prints one decision in full, with `<words>` in it.
    pub decision_command: &'a str,
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
    }

    let ledger = parse_ledger(input.ledger);
    let open: Vec<_> = ledger.iter().filter(|entry| !entry.finished()).collect();
    let _ = writeln!(out, "\n# Open work ({})", open.len());
    if open.is_empty() {
        out.push_str("none\n");
    }
    for entry in open {
        let live = input
            .fleet
            .iter()
            .find(|agent| !entry.agent.is_empty() && agent.name == entry.agent);
        let live = live.map_or_else(
            || "**NOT RUNNING**".to_string(),
            |agent| {
                let context = agent
                    .context_percent
                    .map_or_else(|| "-".to_string(), |percent| format!("{percent}%"));
                format!("{}, ctx {context}", agent.state)
            },
        );
        let _ = writeln!(
            out,
            "- {} [{}] agent {}: {live}; branch {}",
            entry.id,
            or_dash(&entry.status),
            or_dash(&entry.agent),
            or_dash(&entry.branch)
        );
        if !entry.summary.is_empty() {
            let _ = writeln!(out, "  summary: {}", entry.summary);
        }
        if !entry.outcome.is_empty() {
            let _ = writeln!(out, "  outcome: {}", entry.outcome);
        }
    }

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
            said
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
    for warning in &view.warnings {
        let _ = writeln!(out, "warning: {warning}");
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
    fn recent_decisions_fill_twelve_kilobytes_but_never_fewer_than_three() {
        let big = "x".repeat(5000);
        let text: String = (1..=6)
            .map(|day| entry(&format!("2026-09-0{day}: Big {day}"), &big))
            .collect();
        let view = decision_view(&parse_decisions(&text));
        // Two fit in 12 KB; the third is shown anyway.
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
    fn at_most_five_hundred_titles_and_a_line_for_the_rest() {
        let text: String = (0..600)
            .map(|n| entry(&format!("2026-01-01: D{n}"), &"z".repeat(5000)))
            .collect();
        let decisions = parse_decisions(&text);
        let view = decision_view(&decisions);
        assert_eq!(view.recent.len(), 3);
        assert_eq!(view.titles.len(), MAX_DECISION_TITLES);
        assert_eq!(view.titles_left_out, 97);
        assert_eq!(view.titles[0], "2026-01-01  D596");
        let rendered = render(&input(&text, "", &[]));
        assert!(rendered.contains("\n... and 97 older titles left out\n"));
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
    }

    fn input<'a>(decisions: &'a str, ledger: &'a str, fleet: &'a [LiveAgent]) -> DigestInput<'a> {
        DigestInput {
            project: "/repos/weather",
            branch: "main",
            head: "abc1234",
            state_dir: "/state/handler/weather",
            handover: None,
            decisions,
            ledger,
            fleet,
            decision_command: "corgi digest /repos/weather --decision \"<words>\"",
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
}
