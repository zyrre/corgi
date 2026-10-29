//! `corgi digest --search`: plain words looked up in all of a supervisor's
//! memory, not only the bounded part the digest shows, so that it can look
//! something up before it says it does not know or that it never happened.
//!
//! The sources are cut into blocks: a decision entry; a ledger id, its lines
//! in `ledger-archive.jsonl` and `ledger.jsonl` together; and, in briefs,
//! kept reports, notes and handover notes (the current one and the archived
//! ones), a paragraph, heading or top-level list item with what is nested
//! under it. A query word hits a block when the block has a word starting
//! with it, ignoring case, so `reject` finds "rejected" but `pi` does not
//! find "api". Blocks that have the words as a phrase come first, then those
//! with the most words, then those with the most of them on one line, then
//! the newest. Blocks with nearly the same words, such as a paragraph that
//! daily briefs repeat, fold into the best of them, which names the others.
//! Past its first two hits, a file's other hits come after everything else,
//! so one long brief cannot fill the output. Each hit is named by its
//! `file:line`, which `--show` prints whole.

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
};

use super::{
    handover_time, is_date,
    lookup::{LEDGER_ARCHIVE_FILE, NOTES_DIR, REPORTS_DIR, ledger_ids, ledger_lines},
    parse_decisions, superseded,
};

/// The search's output is kept to about this many bytes.
pub const SEARCH_BYTES: usize = 7 * 1024;
/// Hits shown at most; the rest are counted.
const MAX_HITS: usize = 6;
/// Lines of a block shown for a hit at most...
const HIT_LINES: usize = 10;
/// ...and bytes.
const HIT_BYTES: usize = 1200;
/// Hits of one file shown before those of files with fewer hits.
const HITS_PER_FILE: usize = 2;
/// A line longer than this is cut around the first word it hits.
const LINE_BYTES: usize = 500;
/// Near-duplicates named by place under the hit they fold into; the rest are
/// counted.
const SIMILAR_NAMED: usize = 4;

/// The memory searched, already read.
#[derive(Default)]
pub struct SearchInput<'a> {
    pub state_dir: &'a str,
    /// The command that prints a hit's block whole, with `<file:line>` in it.
    pub show_command: &'a str,
    pub decisions: &'a str,
    pub ledger: &'a str,
    pub ledger_archive: &'a str,
    /// `handover.md`, when there is one, with the time it was last written,
    /// `YYYY-MM-DD HH:MM UTC`.
    pub handover: Option<(&'a str, &'a str)>,
    /// `(file name, text)` of each brief in `briefs/`.
    pub briefs: &'a [(String, String)],
    /// `(file name, text)` of each archived note in `handovers/`.
    pub handovers: &'a [(String, String)],
    /// `(file name, text)` of each kept report in `reports/`.
    pub reports: &'a [(String, String)],
    /// `(path under notes/, time last written, text)` of each note.
    pub notes: &'a [(String, String, String)],
}

/// What the search covers, for its header and the line saying it found
/// nothing.
const SOURCES: &str =
    "decisions, ledger (and its archive), briefs, reports, notes and handover notes";

/// The query's words, lowercased, each once, and the phrase they make.
struct Query {
    words: Vec<String>,
    /// The words as typed, cut at anything but letters and digits.
    phrase: Vec<String>,
}

impl Query {
    fn new(words: &str) -> Self {
        let mut unique: Vec<String> = Vec::new();
        for word in words.split_whitespace().map(str::to_lowercase) {
            if !unique.contains(&word) {
                unique.push(word);
            }
        }
        Self {
            words: unique,
            phrase: tokens(&words.to_lowercase()).map(str::to_string).collect(),
        }
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
        self.words
            .iter()
            .filter(|word| Self::position(&lower, word).is_some())
            .count()
    }

    /// Whether `text` has the query's two or more words in a row, each as the
    /// start of a word, with only spaces or punctuation between them.
    fn phrase_in(&self, text: &str) -> bool {
        if self.phrase.len() < 2 {
            return false;
        }
        let lower = text.to_lowercase();
        let words: Vec<&str> = tokens(&lower).collect();
        words.windows(self.phrase.len()).any(|window| {
            window
                .iter()
                .zip(&self.phrase)
                .all(|(word, wanted)| word.starts_with(wanted.as_str()))
        })
    }

    /// `line`, trimmed, and cut around the first word it hits when it is
    /// longer than [`LINE_BYTES`].
    fn clip(&self, line: &str) -> String {
        let line = line.trim_end();
        if line.len() <= LINE_BYTES {
            return line.to_string();
        }
        let lower = line.to_lowercase();
        let first = (lower.len() == line.len())
            .then(|| {
                self.words
                    .iter()
                    .filter_map(|word| Self::position(&lower, word))
                    .min()
            })
            .flatten()
            .unwrap_or(0);
        let mut start = first.saturating_sub(LINE_BYTES / 4);
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
}

/// The runs of letters and digits in `text`.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
}

/// Whether `text` has every word of `words` as the start of one of its
/// words, ignoring case, as `--search` matches a word.
pub fn has_every_word(text: &str, words: &str) -> bool {
    let query = Query::new(words);
    !query.words.is_empty() && query.hits(text) == query.words.len()
}

/// The words of a block, lowercased, with every word holding a digit made
/// `0`, so blocks that differ only in dates and counts look the same.
fn signature(lines: &[(usize, String)]) -> HashSet<String> {
    lines
        .iter()
        .flat_map(|(_, line)| {
            tokens(&line.to_lowercase())
                .map(|word| {
                    if word.bytes().any(|b| b.is_ascii_digit()) {
                        "0".to_string()
                    } else {
                        word.to_string()
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Whether a block with the signature `words` is a near-duplicate of a
/// better one with the signature `better`: at least four fifths of its words
/// are in `better`, as when daily briefs repeat a paragraph, with a line
/// added or a count changed, and it is not much smaller, so that a heading
/// is not taken for a repeat of a long paragraph that happens to have its
/// words. A short block must have the same words.
fn repeats(words: &HashSet<String>, better: &HashSet<String>) -> bool {
    if words.len() < 8 {
        return words == better;
    }
    words.len() * 3 >= better.len() && words.intersection(better).count() * 5 >= words.len() * 4
}

/// One block that the query hits.
struct Hit {
    /// The file, relative to the state directory.
    file: String,
    /// The block's lines, numbered in `file`, blank ones left out.
    lines: Vec<(usize, String)>,
    /// The block's time, `YYYY-MM-DD[ HH:MM[:SS]]`, so it sorts; empty when
    /// it has none.
    when: String,
    /// The date as shown.
    shown_when: String,
    /// What the block is, such as `decision "Use Rust"`.
    what: String,
    /// The line of `lines` the most words hit.
    best: usize,
    phrase: bool,
    hits: usize,
    /// How many words the best line has.
    line_hits: usize,
    /// Whether it may fold into a near-duplicate. A decision or a ledger id
    /// never does: each is its own record, however alike.
    foldable: bool,
    /// The places of the near-duplicates folded into this one, best first.
    similar: Vec<String>,
}

impl Hit {
    fn new(file: String, lines: Vec<(usize, String)>, query: &Query) -> Option<Self> {
        let text = lines
            .iter()
            .map(|(_, line)| line.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let hits = query.hits(&text);
        if hits == 0 {
            return None;
        }
        let (mut best, mut line_hits) = (0, 0);
        for (index, (_, line)) in lines.iter().enumerate() {
            let count = query.hits(line);
            if count > line_hits {
                (best, line_hits) = (index, count);
            }
        }
        Some(Self {
            file,
            phrase: query.phrase_in(&text),
            lines,
            when: String::new(),
            shown_when: String::new(),
            what: String::new(),
            best,
            hits,
            line_hits,
            foldable: true,
            similar: Vec::new(),
        })
    }

    fn dated(mut self, when: &str, what: String) -> Self {
        self.when = when.to_string();
        self.shown_when = when.to_string();
        self.what = what;
        self
    }

    /// `file:line` of the line the most words hit: what `--show` takes.
    fn place(&self) -> String {
        format!(
            "{}:{}",
            self.file,
            self.lines.get(self.best).map_or(1, |(number, _)| *number)
        )
    }

    fn render(&self, index: usize, query: &Query) -> String {
        let words = query.words.len();
        let matched = if self.phrase {
            "phrase".to_string()
        } else if self.hits == words && words > 1 {
            format!("all {words} words")
        } else {
            format!("{}/{words} words", self.hits)
        };
        let mut out = format!(
            "\n[{index}] {}  {}  {matched}  {}\n",
            self.place(),
            if self.shown_when.is_empty() {
                "(no date)"
            } else {
                &self.shown_when
            },
            self.what
        );
        // The best line and what follows it, with one line before it, as
        // much as fits; a block that fits is shown whole.
        let fits = |lines: &[(usize, String)]| {
            lines.len() <= HIT_LINES
                && lines.iter().map(|(_, line)| line.len() + 3).sum::<usize>() <= HIT_BYTES
        };
        let (start, end) = if fits(&self.lines) {
            (0, self.lines.len())
        } else {
            let start = self.best.saturating_sub(1);
            let (mut end, mut bytes) = (start, 0);
            while end < self.lines.len() && end - start < HIT_LINES {
                let size = self.lines[end].1.len().min(LINE_BYTES) + 3;
                if bytes + size > HIT_BYTES && end > self.best {
                    break;
                }
                bytes += size;
                end += 1;
            }
            (start, end)
        };
        for (_, line) in &self.lines[start..end] {
            let _ = writeln!(out, "  {}", query.clip(line));
        }
        let left = self.lines.len() - (end - start);
        if left > 0 {
            let _ = writeln!(
                out,
                "  … {left} more line{} in this block",
                if left == 1 { "" } else { "s" }
            );
        }
        if !self.similar.is_empty() {
            let named: Vec<&str> = self
                .similar
                .iter()
                .take(SIMILAR_NAMED)
                .map(String::as_str)
                .collect();
            let more = self.similar.len() - named.len();
            let _ = writeln!(
                out,
                "  (+{} near-duplicate{}: {}{})",
                self.similar.len(),
                if self.similar.len() == 1 { "" } else { "s" },
                named.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            );
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

/// The hits among the Markdown blocks of `text`, the file `file`, each
/// described by `what` from its section.
fn markdown_hits(
    file: &str,
    text: &str,
    when: &str,
    query: &Query,
    what: impl Fn(&str) -> String,
    hits: &mut Vec<Hit>,
) {
    for (section, lines) in markdown_blocks(text) {
        let lines = lines
            .into_iter()
            .map(|(number, line)| (number, line.to_string()))
            .collect();
        if let Some(hit) = Hit::new(file.to_string(), lines, query) {
            hits.push(hit.dated(when, what(section)));
        }
    }
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

/// `YYYY-MM-DD HH:MM UTC` from a kept report's name,
/// `YYYYMMDD-HHMMSS-<subject>.md`.
fn report_time(name: &str) -> String {
    name.get(..15).and_then(handover_time).unwrap_or_default()
}

fn decision_hits(text: &str, query: &Query, hits: &mut Vec<Hit>) {
    let decisions = parse_decisions(text);
    let (superseded_by, _) = superseded(&decisions);
    for (decision, by) in decisions.iter().zip(superseded_by) {
        let lines = decision
            .text
            .lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(index, line)| (decision.line + index, line.to_string()))
            .collect();
        let Some(hit) = Hit::new("decisions.md".into(), lines, query) else {
            continue;
        };
        let (date, title) = match decision.heading.split_once(": ") {
            Some((date, title)) if is_date(date) => (date, title.trim()),
            _ => ("", decision.heading),
        };
        let mut what = format!("decision \"{title}\"");
        if let Some(by) = by {
            let _ = write!(what, " (SUPERSEDED by \"{by}\")");
        }
        let mut hit = hit.dated(date, what);
        hit.foldable = false;
        hits.push(hit);
    }
}

/// One hit per ledger id, all its lines in the ledger and its archive
/// searched together, placed at its newest line.
fn ledger_hits(ledger: &str, archive: &str, query: &Query, hits: &mut Vec<Hit>) {
    for lines in ledger_ids(ledger_lines(ledger, archive)) {
        let newest = lines.last().expect("an id has a line");
        // Every summary and outcome the id had, each once, oldest first;
        // then the newest line's questions. All are placed at the newest
        // line, which `--show` prints with the id's history.
        let mut said: Vec<(usize, String)> = Vec::new();
        for name in ["summary", "outcome"] {
            let mut seen = HashSet::new();
            for line in &lines {
                let text = line.field(name);
                if !text.is_empty() && seen.insert(text.clone()) {
                    said.push((newest.line, format!("{name}: {text}")));
                }
            }
        }
        let questions = newest.field("questions");
        if !matches!(questions.as_str(), "" | "[]") {
            said.push((newest.line, format!("questions: {questions}")));
        }
        let id = newest.field("id");
        let named = [
            id.clone(),
            newest.field("agent"),
            newest.field("status"),
            newest.field("branch"),
        ]
        .join(" ");
        // The names count as words of the block, but are not shown twice.
        let Some(mut hit) = Hit::new(
            newest.file.to_string(),
            std::iter::once((newest.line, named)).chain(said).collect(),
            query,
        ) else {
            continue;
        };
        hit.lines.remove(0);
        if hit.lines.is_empty() {
            hit.lines
                .push((newest.line, "(no summary or outcome)".into()));
        }
        hit.best = hit.best.saturating_sub(1);
        let status = newest.field("status");
        let mut what = format!(
            "ledger {id} [{}]",
            if status.is_empty() { "-" } else { &status }
        );
        if newest.file == LEDGER_ARCHIVE_FILE {
            what.push_str(" (archived)");
        }
        let ts = newest.field("ts");
        hit = hit.dated(
            &ts.replace('T', " ").chars().take(19).collect::<String>(),
            what,
        );
        hit.shown_when = ts.chars().take(10).collect();
        hit.foldable = false;
        hits.push(hit);
    }
}

fn brief_hits(briefs: &[(String, String)], query: &Query, hits: &mut Vec<Hit>) {
    for (name, text) in briefs {
        let title = text
            .lines()
            .find_map(|line| line.strip_prefix("# "))
            .map_or(name.as_str(), str::trim);
        markdown_hits(
            &format!("briefs/{name}"),
            text,
            &brief_date(name),
            query,
            |_| format!("brief \"{title}\""),
            hits,
        );
    }
}

fn handover_hits(input: &SearchInput, query: &Query, hits: &mut Vec<Hit>) {
    let with_section = |mut what: String, section: &str| {
        if !section.is_empty() {
            let _ = write!(what, "; section \"{section}\"");
        }
        what
    };
    if let Some((when, text)) = input.handover {
        markdown_hits(
            crate::supervisor::HANDOVER_NOTE,
            text,
            when,
            query,
            |section| {
                with_section(
                    "CURRENT HANDOVER NOTE, written for the session that takes over".into(),
                    section,
                )
            },
            hits,
        );
    }
    for (name, text) in input.handovers {
        let when = handover_time(name).unwrap_or_default();
        let what = format!(
            "HANDOVER NOTE of {}, a past session's view that may be out of date",
            if when.is_empty() {
                "unknown date"
            } else {
                &when
            }
        );
        markdown_hits(
            &format!("{}/{name}", crate::supervisor::HANDOVERS_DIR),
            text,
            &when,
            query,
            |section| with_section(what.clone(), section),
            hits,
        );
    }
}

fn report_hits(reports: &[(String, String)], query: &Query, hits: &mut Vec<Hit>) {
    for (name, text) in reports {
        let title = text
            .lines()
            .find_map(|line| line.strip_prefix("# "))
            .map_or(name.as_str(), str::trim);
        markdown_hits(
            &format!("{REPORTS_DIR}/{name}"),
            text,
            &report_time(name),
            query,
            |_| format!("kept report: {title}"),
            hits,
        );
    }
}

fn note_hits(notes: &[(String, String, String)], query: &Query, hits: &mut Vec<Hit>) {
    for (path, when, text) in notes {
        markdown_hits(
            &format!("{NOTES_DIR}/{path}"),
            text,
            when,
            query,
            |section| {
                if section.is_empty() {
                    format!("your note {path}")
                } else {
                    format!("your note {path}; section \"{section}\"")
                }
            },
            hits,
        );
    }
}

/// The hits, best first: see the module's documentation.
fn ranked(input: &SearchInput, query: &Query) -> Vec<Hit> {
    let mut hits = Vec::new();
    decision_hits(input.decisions, query, &mut hits);
    ledger_hits(input.ledger, input.ledger_archive, query, &mut hits);
    brief_hits(input.briefs, query, &mut hits);
    report_hits(input.reports, query, &mut hits);
    note_hits(input.notes, query, &mut hits);
    handover_hits(input, query, &mut hits);
    // Stable, so hits of equal rank and time keep file order, later last;
    // reversing that order puts the later one first.
    hits.reverse();
    hits.sort_by(|a, b| {
        (b.phrase, b.hits, b.line_hits, &b.when).cmp(&(a.phrase, a.hits, a.line_hits, &a.when))
    });

    // Each near-duplicate folds into the best hit it is like.
    let mut kept: Vec<(Hit, HashSet<String>)> = Vec::new();
    for hit in hits {
        let words = signature(&hit.lines);
        let like = hit
            .foldable
            .then(|| kept.iter_mut().find(|(_, better)| repeats(&words, better)))
            .flatten();
        match like {
            Some((first, _)) => first.similar.push(hit.place()),
            None => kept.push((hit, words)),
        }
    }

    // A long brief or note can hold many blocks that hit; past the first
    // few of a file, its hits wait until every other file's are shown.
    let mut per_file: HashMap<String, usize> = HashMap::new();
    let (first, later): (Vec<_>, Vec<_>) = kept.into_iter().map(|(hit, _)| hit).partition(|hit| {
        let seen = per_file.entry(hit.file.clone()).or_default();
        *seen += 1;
        *seen <= HITS_PER_FILE
    });
    first.into_iter().chain(later).collect()
}

/// The blocks of the memory that `words` hit, best first, as text of about
/// [`SEARCH_BYTES`] at most.
pub fn search(input: &SearchInput, words: &str) -> String {
    let query = Query::new(words);
    let hits = ranked(input, &query);

    let words_shown = query.words.join(" ");
    let mut out = String::new();
    if hits.is_empty() {
        let _ = writeln!(out, "No hits for \"{words_shown}\" in {SOURCES}.");
        return out;
    }
    let folded: usize = hits.iter().map(|hit| hit.similar.len()).sum();
    let _ = writeln!(
        out,
        "# Search \"{words_shown}\": {} hit{}{} in {SOURCES}",
        hits.len(),
        if hits.len() == 1 { "" } else { "s" },
        if folded > 0 {
            format!(", and {folded} near-duplicates folded into them")
        } else {
            String::new()
        }
    );
    let _ = writeln!(
        out,
        "Best first: the words as a phrase, then the most of them (a word matches the start of \
         a word, ignoring case), then newest. Paths are in {}.\n\
         Print a hit's whole block (decision, ledger id, brief, report or section): {}",
        input.state_dir, input.show_command
    );
    // Room for the line about the hits left out.
    const FOOTER_BYTES: usize = 120;
    let mut shown = 0;
    for hit in hits.iter().take(MAX_HITS) {
        let text = hit.render(shown + 1, &query);
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
            "\n... {cut} more hit{} left out; add words or use rarer ones.",
            if cut == 1 { "" } else { "s" }
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATE: &str = "/state/supervisors/weather";

    fn input<'a>(
        decisions: &'a str,
        ledger: &'a str,
        briefs: &'a [(String, String)],
        handovers: &'a [(String, String)],
    ) -> SearchInput<'a> {
        SearchInput {
            state_dir: STATE,
            show_command: "corgi digest /repos/weather --show <file:line>",
            decisions,
            ledger,
            briefs,
            handovers,
            ..SearchInput::default()
        }
    }

    /// The handles of the hits shown, in order.
    fn places(out: &str) -> Vec<&str> {
        out.lines()
            .filter(|line| line.starts_with('['))
            .map(|line| line.split_whitespace().nth(1).unwrap())
            .collect()
    }

    #[test]
    fn words_hit_the_start_of_words_ignoring_case() {
        let query = Query::new("Reject PI reject");
        assert_eq!(query.words, ["reject", "pi"]);
        assert_eq!(query.hits("We REJECTED Pi Durable."), 2);
        assert_eq!(query.hits("the api was rejected"), 1);
        assert_eq!(query.hits("pi-durable"), 1);
        assert_eq!(query.hits("nothing here"), 0);
        assert!(has_every_word("Q23 A: lessons", "q23 LESS"));
        assert!(!has_every_word("Q23 A", "q23 lessons"));
        assert!(!has_every_word("Q23", "  "));
    }

    #[test]
    fn a_phrase_is_the_words_in_a_row() {
        let query = Query::new("macos CI flaky");
        assert!(query.phrase_in("the macOS-CI flaky test"));
        assert!(query.phrase_in("macos ci flaky-test"));
        assert!(!query.phrase_in("macos ci flakiness"));
        assert!(!query.phrase_in("flaky macos CI"));
        assert!(!query.phrase_in("macos and CI flaky"));
        assert!(!Query::new("macos").phrase_in("macos"));
    }

    #[test]
    fn hits_rank_by_phrase_then_words_then_newest_first() {
        let decisions = "## 2026-09-01: Durable wakes\nDecision: wakes are durable.\n\n\
                         ## 2026-09-05: Pi Durable\nDecision: rejected Pi Durable.\n\n\
                         ## 2026-09-03: Pi Durable again\nDecision: still rejected.\n\n\
                         ## 2026-08-01: Old\nDecision: Pi durable rejected outright.\n";
        let ledger = r#"{"id":"20260910-w-pi","ts":"2026-09-10T08:00:00Z","status":"abandoned","summary":"Try Pi Durable","outcome":"rejected, too slow"}"#;
        let briefs = [(
            "20260902-w-wakes.md".to_string(),
            "# Wakes\n\n## Goal\nMake wakes durable.\n".to_string(),
        )];
        let out = search(
            &input(decisions, ledger, &briefs, &[]),
            "pi durable rejected",
        );
        assert_eq!(
            places(&out),
            [
                // The phrase, though oldest.
                "decisions.md:11",
                // All three words, the best line having the most.
                "decisions.md:5",
                "ledger.jsonl:1",
                "briefs/20260902-w-wakes.md:4",
                // A file's third hit and on wait for the others'.
                "decisions.md:7",
                "decisions.md:1",
            ]
        );
        assert!(out.starts_with("# Search \"pi durable rejected\": 6 hits in decisions"));
        assert!(out.contains(
            "\n[3] ledger.jsonl:1  2026-09-10  all 3 words  ledger 20260910-w-pi [abandoned]\n  \
             summary: Try Pi Durable\n  outcome: rejected, too slow\n"
        ));
        assert!(out.contains("[1] decisions.md:11  2026-08-01  phrase  decision \"Old\"\n"));
        assert!(out.contains("--show <file:line>\n"));
    }

    #[test]
    fn near_duplicate_blocks_fold_into_the_best_one() {
        let morning = |day: u32, extra: &str| {
            (
                format!("202610{day:02}-w-morning.md"),
                format!(
                    "# Morning check {day}\n\n## Goal\nRun the check.\n\n\
                     6. Keep confirming: telemetry-agent (one leader, leader_active=1, GRPC v8/v7 \
                     1.000, no restarts); alarm-agent-query (all configs per cycle){extra}.\n"
                ),
            )
        };
        let briefs = [
            morning(20, ""),
            morning(21, ", 0 timeouts"),
            morning(22, ""),
            // Older, with much more: not a repeat of the newer ones.
            morning(
                19,
                "; kafka lag, helix clears, ping volume, snmp poller baselines, disk queues, \
                 counter producer deadlines",
            ),
            (
                "20261020-w-grpc.md".to_string(),
                "# Study\n\nHow GRPC telemetry feeds alarms and the weathermap.\n".to_string(),
            ),
        ];
        let out = search(
            &input("", "", &briefs, &[]),
            "grpc telemetry alarm weathermap",
        );
        assert_eq!(
            places(&out),
            [
                "briefs/20261020-w-grpc.md:3",
                "briefs/20261022-w-morning.md:6",
                "briefs/20261019-w-morning.md:6",
            ]
        );
        assert!(out.starts_with(
            "# Search \"grpc telemetry alarm weathermap\": 3 hits, and 2 near-duplicates folded"
        ));
        assert!(out.contains(
            "  (+2 near-duplicates: briefs/20261021-w-morning.md:6, briefs/20261020-w-morning.md:6)\n"
        ));
        // Tiny blocks fold only when the same.
        let a: HashSet<String> = ["a", "b"].map(String::from).into();
        let b: HashSet<String> = ["a", "c"].map(String::from).into();
        assert!(!repeats(&a, &b) && repeats(&a, &a.clone()));
    }

    #[test]
    fn a_long_block_shows_its_best_lines_and_counts_the_rest() {
        let note: String = (0..40)
            .map(|n| {
                if n == 20 {
                    "the zebra line\n".to_string()
                } else {
                    format!("  line {n}\n")
                }
            })
            .collect();
        let briefs = [("20261001-w-a.md".to_string(), note)];
        let out = search(&input("", "", &briefs, &[]), "zebra");
        assert!(out.contains(
            "[1] briefs/20261001-w-a.md:21  2026-10-01  1/1 words  brief \"20261001-w-a.md\"\n    \
             line 19\n  the zebra line\n    line 21\n"
        ));
        assert!(out.contains("  … 30 more lines in this block\n"));
    }

    #[test]
    fn superseded_decisions_are_found_and_marked() {
        let decisions = "## 2026-09-01: Cap workers at four\nDecision: at most 4 workers.\n\n\
                         ## 2026-10-02: No worker cap\nDecision: lifted.\n\
                         Supersedes: 2026-09-01: Cap workers at four\n";
        let out = search(&input(decisions, "", &[], &[]), "worker");
        assert!(out.contains(
            "decisions.md:1  2026-09-01  1/1 words  decision \"Cap workers at four\" \
             (SUPERSEDED by \"2026-10-02: No worker cap\")"
        ));
        assert!(
            out.contains("decisions.md:4  2026-10-02  1/1 words  decision \"No worker cap\"\n")
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
        let mut searched = input("", "", &[], &handovers);
        searched.handover = Some((
            "2026-10-03 09:00 UTC",
            "# Handover\n\n## Open threads\n- TMPDIR again, now in CI.\n",
        ));
        let out = search(&searched, "tmpdir");
        assert!(out.contains(
            "handovers/20261002-132035.md:4  2026-10-02 13:20 UTC  1/1 words  HANDOVER NOTE \
             of 2026-10-02 13:20 UTC, a past session's view that may be out of date; section \
             \"Open threads with the user\"\n  - Tests fail under a long TMPDIR.\n    Sub-point.\n"
        ));
        assert_eq!(
            places(&out),
            ["handover.md:4", "handovers/20261002-132035.md:4"]
        );
        assert!(
            out.contains(
                "[1] handover.md:4  2026-10-03 09:00 UTC  1/1 words  CURRENT HANDOVER NOTE"
            )
        );
    }

    #[test]
    fn reports_notes_and_the_ledger_archive_are_searched() {
        let reports = [(
            "20261027-101500-20261027-w-a.md".to_string(),
            "# Report of w-a (done) at 2026-10-27 10:15:00 UTC\n\n### Report\n- Learned: the \
             kumquat cache needs a warm-up.\n"
                .to_string(),
        )];
        let notes = [(
            "plans/rollout.md".to_string(),
            "2026-10-26 08:00 UTC".to_string(),
            "# Rollout\n\n## Order\nKumquat first, then the rest.\n".to_string(),
        )];
        let archive = r#"{"id":"20261001-w-k","ts":"2026-10-01T08:00:00Z","agent":"w-k","status":"dispatched","summary":"Kumquat importer"}"#;
        let ledger = r#"{"id":"20261001-w-k","ts":"2026-10-02T08:00:00Z","agent":"w-k","status":"merged","outcome":"done"}"#;
        let searched = SearchInput {
            ledger,
            ledger_archive: archive,
            reports: &reports,
            notes: &notes,
            ..input("", "", &[], &[])
        };
        let out = search(&searched, "kumquat");
        assert_eq!(
            places(&out),
            [
                "reports/20261027-101500-20261027-w-a.md:4",
                "notes/plans/rollout.md:4",
                "ledger.jsonl:1",
            ]
        );
        assert!(out.contains(
            "  2026-10-27 10:15 UTC  1/1 words  kept report: Report of w-a (done) at 2026-10-27"
        ));
        assert!(out.contains(
            "notes/plans/rollout.md:4  2026-10-26 08:00 UTC  1/1 words  your note \
             plans/rollout.md; section \"Order\"\n"
        ));
        // The id's newest line is where it is placed, with every summary and outcome it had.
        assert!(out.contains(
            "[3] ledger.jsonl:1  2026-10-02  1/1 words  ledger 20261001-w-k [merged]\n  \
             summary: Kumquat importer\n  outcome: done\n"
        ));
        // An id only in the archive says so.
        let archived = SearchInput {
            ledger_archive: archive,
            ..input("", "", &[], &[])
        };
        assert!(
            search(&archived, "kumquat").contains("ledger 20261001-w-k [dispatched] (archived)")
        );
    }

    #[test]
    fn output_stays_under_the_cap_and_says_how_many_hits_were_cut() {
        let decisions: String = (0..200)
            .map(|n| {
                format!(
                    "## 2026-09-01: Decision {n}\nDecision: the same words, {}.\nWhy: {}.\n\n",
                    "long ".repeat(150),
                    (0..30).map(|k| format!("w{n}x{k} ")).collect::<String>()
                )
            })
            .collect();
        let out = search(&input(&decisions, "", &[], &[]), "the same");
        assert!(out.len() <= SEARCH_BYTES, "{} bytes", out.len());
        let shown = places(&out).len();
        assert!((3..=MAX_HITS).contains(&shown), "{shown}");
        assert!(out.ends_with(&format!(
            "\n... {} more hits left out; add words or use rarer ones.\n",
            200 - shown
        )));
        // Long lines are cut, around the first word they hit.
        assert!(out.lines().all(|line| line.len() <= LINE_BYTES + 8));
        assert!(!search(&input("## x\nshort\n", "", &[], &[]), "short").contains("left out"));
    }

    #[test]
    fn no_hits_say_so() {
        assert_eq!(
            search(&input("## 2026-09-01: A\nb\n", "", &[], &[]), "zebra"),
            format!("No hits for \"zebra\" in {SOURCES}.\n")
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
