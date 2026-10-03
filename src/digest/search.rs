//! `corgi digest --search`: plain words looked up in all of a corgi's
//! memory, not only the bounded part the digest shows, so that it can look
//! something up before it says it does not know or that it never happened.
//!
//! The sources are cut into blocks: a decision entry, the newest ledger line
//! of an id, and, in briefs and archived handover notes, a paragraph, heading
//! or top-level list item with what is nested under it. A query word hits a
//! block when the block has a word starting with it, ignoring case, so
//! `reject` finds "rejected" but `pi` does not find "api". Blocks are ranked
//! by how many of the query's words they hit, then newest first, and any
//! block hitting at least one word is a hit: the best ones come first, and
//! the cap leaves the weakest out. Past its first two hits, a file's other
//! hits come after everything else, so one long brief cannot fill the output.

use std::{collections::HashMap, fmt::Write as _};

use super::{handover_time, is_date, parse_decisions, parse_ledger, superseded};

/// The search's output is kept to about this many bytes.
pub const SEARCH_BYTES: usize = 5 * 1024;
/// Lines of context shown for a hit.
const CONTEXT_LINES: usize = 3;
/// Hits of one file shown before those of files with fewer hits.
const HITS_PER_FILE: usize = 2;
/// A context line longer than this is cut around the first word it hits.
const LINE_BYTES: usize = 240;

/// The memory searched, already read.
pub struct SearchInput<'a> {
    pub state_dir: &'a str,
    pub decisions: &'a str,
    pub ledger: &'a str,
    /// `(file name, text)` of each brief in `briefs/`.
    pub briefs: &'a [(String, String)],
    /// `(file name, text)` of each archived note in `handovers/`.
    pub handovers: &'a [(String, String)],
}

/// The query's words, lowercased, each once.
struct Query(Vec<String>);

impl Query {
    fn new(words: &str) -> Self {
        let mut unique: Vec<String> = Vec::new();
        for word in words.split_whitespace().map(str::to_lowercase) {
            if !unique.contains(&word) {
                unique.push(word);
            }
        }
        Self(unique)
    }

    /// Where `word` first starts a word of `lower`, already lowercased.
    fn position(lower: &str, word: &str) -> Option<usize> {
        lower.match_indices(word).map(|(at, _)| at).find(|&at| {
            lower[..at]
                .chars()
                .next_back()
                .is_none_or(|before| !before.is_alphanumeric())
        })
    }

    /// How many of the words hit `text`.
    fn hits(&self, text: &str) -> usize {
        let lower = text.to_lowercase();
        self.0
            .iter()
            .filter(|word| Self::position(&lower, word).is_some())
            .count()
    }

    /// `line`, trimmed, and cut around the first word it hits when it is
    /// longer than [`LINE_BYTES`].
    fn clip(&self, line: &str) -> String {
        let line = line.trim();
        if line.len() <= LINE_BYTES {
            return line.to_string();
        }
        let lower = line.to_lowercase();
        let first = (lower.len() == line.len())
            .then(|| {
                self.0
                    .iter()
                    .filter_map(|word| Self::position(&lower, word))
                    .min()
            })
            .flatten()
            .unwrap_or(0);
        let mut start = first.saturating_sub(LINE_BYTES / 3);
        while !line.is_char_boundary(start) {
            start -= 1;
        }
        let mut end = (start + LINE_BYTES).min(line.len());
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        format!(
            "{}{}{}",
            if start > 0 { "…" } else { "" },
            &line[start..end],
            if end < line.len() { "…" } else { "" }
        )
    }

    /// The line of `lines` (numbered, blank ones left out) the most words
    /// hit, the first of them if several do, and up to [`CONTEXT_LINES`]
    /// lines around it.
    fn context(&self, lines: &[(usize, &str)]) -> (usize, Vec<String>) {
        let mut best = 0;
        let mut best_hits = 0;
        for (index, (_, line)) in lines.iter().enumerate() {
            let hits = self.hits(line);
            if hits > best_hits {
                (best, best_hits) = (index, hits);
            }
        }
        let end = (best.saturating_sub(1) + CONTEXT_LINES).min(lines.len());
        let start = end.saturating_sub(CONTEXT_LINES);
        let shown = lines[start..end]
            .iter()
            .map(|(_, line)| self.clip(line))
            .collect();
        (lines.get(best).map_or(1, |(number, _)| *number), shown)
    }
}

/// One block that the query hits.
struct Hit {
    /// `file:line`, the path relative to the state directory.
    place: String,
    /// The block's time, `YYYY-MM-DD[ HH:MM[:SS]]`, so it sorts; empty when
    /// it has none.
    when: String,
    /// The date as shown.
    shown_when: String,
    /// What the block is, such as `decision "Use Rust"`.
    what: String,
    context: Vec<String>,
    hits: usize,
}

impl Hit {
    fn render(&self, words: usize) -> String {
        let mut out = format!(
            "\n{}  {}  [{}/{words} words]  {}\n",
            self.place,
            if self.shown_when.is_empty() {
                "(no date)"
            } else {
                &self.shown_when
            },
            self.hits,
            self.what
        );
        for line in &self.context {
            let _ = writeln!(out, "  {line}");
        }
        out
    }
}

/// The blocks of a Markdown file, each with the heading it is under and its
/// numbered non-blank lines: a blank line ends a block, and a heading or a
/// top-level list item starts one. Inside a code fence only blank lines do.
fn markdown_blocks(text: &str) -> Vec<(&str, Vec<(usize, &str)>)> {
    let mut blocks = Vec::new();
    let mut section = "";
    let mut current: Vec<(usize, &str)> = Vec::new();
    let mut current_section = "";
    let mut fenced = false;
    for (index, line) in text.lines().enumerate() {
        let fence = line.trim_start().starts_with("```");
        let heading = !fenced && line.starts_with('#');
        let item = !fenced
            && (line.starts_with("- ")
                || line.starts_with("* ")
                || line
                    .split_once(". ")
                    .is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())));
        if (line.trim().is_empty() || heading || item) && !current.is_empty() {
            blocks.push((current_section, std::mem::take(&mut current)));
        }
        if fence {
            fenced = !fenced;
        }
        if heading {
            section = line.trim_start_matches('#').trim();
        }
        if !line.trim().is_empty() {
            if current.is_empty() {
                current_section = section;
            }
            current.push((index + 1, line));
        }
    }
    if !current.is_empty() {
        blocks.push((current_section, current));
    }
    blocks
}

/// `YYYY-MM-DD` from a brief's id, `YYYYMMDD-<name>`.
fn brief_date(name: &str) -> String {
    match name.get(..8) {
        Some(date) if date.bytes().all(|b| b.is_ascii_digit()) && name[8..].starts_with('-') => {
            format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..])
        }
        _ => String::new(),
    }
}

fn decision_hits(text: &str, query: &Query, hits: &mut Vec<Hit>) {
    let decisions = parse_decisions(text);
    let (superseded_by, _) = superseded(&decisions);
    for (decision, by) in decisions.iter().zip(superseded_by) {
        let count = query.hits(decision.text);
        if count == 0 {
            continue;
        }
        let (date, title) = match decision.heading.split_once(": ") {
            Some((date, title)) if is_date(date) => (date, title.trim()),
            _ => ("", decision.heading),
        };
        let lines: Vec<_> = decision
            .text
            .lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(index, line)| (decision.line + index, line))
            .collect();
        let (line, context) = query.context(&lines);
        let mut what = format!("decision \"{title}\"");
        if let Some(by) = by {
            let _ = write!(what, " (SUPERSEDED by \"{by}\")");
        }
        hits.push(Hit {
            place: format!("decisions.md:{line}"),
            when: date.to_string(),
            shown_when: date.to_string(),
            what,
            context,
            hits: count,
        });
    }
}

fn ledger_hits(text: &str, query: &Query, hits: &mut Vec<Hit>) {
    for entry in parse_ledger(text) {
        let searched = [
            &entry.id,
            &entry.agent,
            &entry.status,
            &entry.branch,
            &entry.summary,
            &entry.outcome,
        ]
        .map(String::as_str)
        .join("\n");
        let count = query.hits(&searched);
        if count == 0 {
            continue;
        }
        let context = [("summary", &entry.summary), ("outcome", &entry.outcome)]
            .iter()
            .filter(|(_, text)| !text.is_empty())
            .map(|(name, text)| query.clip(&format!("{name}: {text}")))
            .collect();
        hits.push(Hit {
            place: format!("ledger.jsonl:{}", entry.line),
            when: entry.ts.replace('T', " ").chars().take(19).collect(),
            shown_when: entry.ts.chars().take(10).collect(),
            what: format!(
                "ledger {} [{}]",
                entry.id,
                if entry.status.is_empty() {
                    "-"
                } else {
                    &entry.status
                }
            ),
            context,
            hits: count,
        });
    }
}

fn brief_hits(briefs: &[(String, String)], query: &Query, hits: &mut Vec<Hit>) {
    for (name, text) in briefs {
        let title = text
            .lines()
            .find_map(|line| line.strip_prefix("# "))
            .map_or(name.as_str(), str::trim);
        let date = brief_date(name);
        for (_, lines) in markdown_blocks(text) {
            let block: Vec<_> = lines.iter().map(|(_, line)| *line).collect();
            let count = query.hits(&block.join("\n"));
            if count == 0 {
                continue;
            }
            let (line, context) = query.context(&lines);
            hits.push(Hit {
                place: format!("briefs/{name}:{line}"),
                when: date.clone(),
                shown_when: date.clone(),
                what: format!("brief \"{title}\""),
                context,
                hits: count,
            });
        }
    }
}

fn handover_hits(handovers: &[(String, String)], query: &Query, hits: &mut Vec<Hit>) {
    for (name, text) in handovers {
        let when = handover_time(name);
        for (section, lines) in markdown_blocks(text) {
            let block: Vec<_> = lines.iter().map(|(_, line)| *line).collect();
            let count = query.hits(&block.join("\n"));
            if count == 0 {
                continue;
            }
            let (line, context) = query.context(&lines);
            let mut what = format!(
                "HANDOVER NOTE of {}, a past session's view that may be out of date",
                when.as_deref().unwrap_or("unknown date")
            );
            if !section.is_empty() {
                let _ = write!(what, "; section \"{section}\"");
            }
            hits.push(Hit {
                place: format!("handovers/{name}:{line}"),
                when: when.clone().unwrap_or_default(),
                shown_when: when.clone().unwrap_or_default(),
                what,
                context,
                hits: count,
            });
        }
    }
}

/// The blocks of the memory that `words` hit, best first, as text of about
/// [`SEARCH_BYTES`] at most.
pub fn search(input: &SearchInput, words: &str) -> String {
    let query = Query::new(words);
    let mut hits = Vec::new();
    decision_hits(input.decisions, &query, &mut hits);
    ledger_hits(input.ledger, &query, &mut hits);
    brief_hits(input.briefs, &query, &mut hits);
    handover_hits(input.handovers, &query, &mut hits);
    // Stable, so hits of equal rank and time keep file order, later last;
    // reversing that order puts the later one first.
    hits.reverse();
    hits.sort_by(|a, b| b.hits.cmp(&a.hits).then_with(|| b.when.cmp(&a.when)));
    // A long brief or note can hold many blocks that hit; past the first
    // few of a file, its hits wait until every other file's are shown.
    let mut per_file: HashMap<String, usize> = HashMap::new();
    let (first, later): (Vec<_>, Vec<_>) = hits.into_iter().partition(|hit| {
        let file = hit
            .place
            .rsplit_once(':')
            .map_or(hit.place.as_str(), |(file, _)| file);
        let seen = per_file.entry(file.to_string()).or_default();
        *seen += 1;
        *seen <= HITS_PER_FILE
    });
    let hits: Vec<_> = first.into_iter().chain(later).collect();

    let words_shown = query.0.join(" ");
    let mut out = String::new();
    if hits.is_empty() {
        let _ = writeln!(
            out,
            "No hits for \"{words_shown}\" in decisions, ledger, briefs or archived handover notes."
        );
        return out;
    }
    let _ = writeln!(
        out,
        "# Search \"{words_shown}\": {} hit{} in decisions, ledger, briefs and archived handover notes",
        hits.len(),
        if hits.len() == 1 { "" } else { "s" }
    );
    let _ = writeln!(
        out,
        "Ranked by how many of the words a block has (as the start of a word, ignoring case), \
         then newest first. Paths are in {}.",
        input.state_dir
    );
    // Room for the line about the hits left out.
    const FOOTER_BYTES: usize = 120;
    let mut shown = 0;
    for hit in &hits {
        let text = hit.render(query.0.len());
        if shown > 0 && out.len() + text.len() + FOOTER_BYTES > SEARCH_BYTES {
            break;
        }
        out.push_str(&text);
        shown += 1;
    }
    let cut = hits.len() - shown;
    if cut > 0 {
        let _ = writeln!(
            out,
            "\n... {cut} more hit{} cut to keep this short; add words or use rarer ones.",
            if cut == 1 { "" } else { "s" }
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(
        decisions: &'a str,
        ledger: &'a str,
        briefs: &'a [(String, String)],
        handovers: &'a [(String, String)],
    ) -> SearchInput<'a> {
        SearchInput {
            state_dir: "/state/corgis/weather",
            decisions,
            ledger,
            briefs,
            handovers,
        }
    }

    #[test]
    fn words_hit_the_start_of_words_ignoring_case() {
        let query = Query::new("Reject PI reject");
        assert_eq!(query.0, ["reject", "pi"]);
        assert_eq!(query.hits("We REJECTED Pi Durable."), 2);
        assert_eq!(query.hits("the api was rejected"), 1);
        assert_eq!(query.hits("pi-durable"), 1);
        assert_eq!(query.hits("nothing here"), 0);
    }

    #[test]
    fn hits_rank_by_words_then_newest_first() {
        let decisions = "## 2026-09-01: Durable wakes\nDecision: wakes are durable.\n\n\
                         ## 2026-09-05: Pi Durable\nDecision: rejected Pi Durable.\n\n\
                         ## 2026-09-03: Pi Durable again\nDecision: still rejected.\n";
        let ledger = r#"{"id":"20260910-w-pi","ts":"2026-09-10T08:00:00Z","status":"abandoned","summary":"Try Pi Durable","outcome":"rejected, too slow"}"#;
        let briefs = [(
            "20260902-w-wakes.md".to_string(),
            "# Wakes\n\n## Goal\nMake wakes durable.\n".to_string(),
        )];
        let out = search(
            &input(decisions, ledger, &briefs, &[]),
            "pi durable rejected",
        );
        let places: Vec<_> = out
            .lines()
            .filter(|line| line.contains("words]"))
            .map(|line| line.split_whitespace().next().unwrap())
            .collect();
        assert_eq!(
            places,
            [
                "ledger.jsonl:1",
                "decisions.md:5",
                "decisions.md:7",
                "briefs/20260902-w-wakes.md:4",
                "decisions.md:1",
            ]
        );
        assert!(out.starts_with("# Search \"pi durable rejected\": 5 hits"));
        assert!(out.contains(
            "ledger.jsonl:1  2026-09-10  [3/3 words]  ledger 20260910-w-pi [abandoned]\n  \
             summary: Try Pi Durable\n  outcome: rejected, too slow\n"
        ));
        // The line pointed at is the one most words hit.
        assert!(out.contains("decisions.md:5  2026-09-05  [3/3 words]  decision \"Pi Durable\"\n"));
    }

    #[test]
    fn superseded_decisions_are_found_and_marked() {
        let decisions = "## 2026-09-01: Cap workers at four\nDecision: at most 4 workers.\n\n\
                         ## 2026-10-02: No worker cap\nDecision: lifted.\n\
                         Supersedes: 2026-09-01: Cap workers at four\n";
        let out = search(&input(decisions, "", &[], &[]), "worker");
        assert!(out.contains(
            "decisions.md:1  2026-09-01  [1/1 words]  decision \"Cap workers at four\" \
             (SUPERSEDED by \"2026-10-02: No worker cap\")"
        ));
        assert!(
            out.contains("decisions.md:4  2026-10-02  [1/1 words]  decision \"No worker cap\"\n")
        );
        assert!(out.find("No worker cap\"\n").unwrap() < out.find("Cap workers").unwrap());
    }

    #[test]
    fn handover_hits_carry_their_date_and_section() {
        let handovers = [(
            "20261002-132035.md".to_string(),
            "# Handover\n\n## Open threads with the user\n- Tests fail under a long TMPDIR.\n  \
             Sub-point.\n- Another thread.\n"
                .to_string(),
        )];
        let out = search(&input("", "", &[], &handovers), "tmpdir");
        assert!(out.contains(
            "handovers/20261002-132035.md:4  2026-10-02 13:20 UTC  [1/1 words]  HANDOVER NOTE \
             of 2026-10-02 13:20 UTC, a past session's view that may be out of date; section \
             \"Open threads with the user\"\n  - Tests fail under a long TMPDIR.\n  Sub-point.\n"
        ));
        assert_eq!(out.matches("words]").count(), 1);
    }

    #[test]
    fn output_stays_under_the_cap_and_says_how_many_hits_were_cut() {
        let decisions: String = (0..200)
            .map(|n| {
                format!(
                    "## 2026-09-01: Decision {n}\nDecision: the same words, {}.\nWhy: the same.\n\n",
                    "long ".repeat(80)
                )
            })
            .collect();
        let out = search(&input(&decisions, "", &[], &[]), "the same");
        assert!(out.len() <= SEARCH_BYTES, "{} bytes", out.len());
        let shown = out.matches("words]").count();
        assert!(shown > 3);
        assert!(out.ends_with(&format!(
            "\n... {} more hits cut to keep this short; add words or use rarer ones.\n",
            200 - shown
        )));
        // Long lines are cut, around the first word they hit.
        assert!(out.lines().all(|line| line.len() <= LINE_BYTES + 8));
        assert!(!search(&input("## x\nshort\n", "", &[], &[]), "short").contains("cut"));
    }

    #[test]
    fn no_hits_say_so() {
        assert_eq!(
            search(&input("## 2026-09-01: A\nb\n", "", &[], &[]), "zebra"),
            "No hits for \"zebra\" in decisions, ledger, briefs or archived handover notes.\n"
        );
    }

    #[test]
    fn markdown_blocks_split_at_blank_lines_headings_and_list_items() {
        let text = "# T\nintro\n\n## S\n- a\n  more a\n- b\n1. c\n```\n# not a heading\n- nor an item\n```\n";
        let blocks: Vec<_> = markdown_blocks(text)
            .into_iter()
            .map(|(section, lines)| (section, lines.iter().map(|(n, _)| *n).collect::<Vec<_>>()))
            .collect();
        assert_eq!(
            blocks,
            [
                ("T", vec![1, 2]),
                ("S", vec![4]),
                ("S", vec![5, 6]),
                ("S", vec![7]),
                ("S", vec![8, 9, 10, 11, 12]),
            ]
        );
    }
}
