//! A supervisor's backlog: the things to build later it keeps in
//! `backlog.md` in its state directory, beside `decisions.md`. The
//! supervisor writes it; Corgi only reads it, to count its entries in
//! `corgi status --json` and to list them in `corgi backlog --json`, for
//! the Herdr app's Questions panel.
//!
//! The file is an optional `# ` title and intro, then one entry per
//! `## <heading>`, whose text runs to the next `## ` or the end. A missing
//! file, or one without a `## ` line, is no backlog.

use std::{
    fs::{self, File},
    io::Read,
    path::Path,
};

/// One `## <heading>` entry of a supervisor's `backlog.md`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BacklogEntry {
    /// The text after `## `.
    pub heading: String,
    /// The Markdown up to the next `## ` or the file's end, without its
    /// heading line.
    pub text: String,
}

/// The backlog, in the supervisor's state directory.
pub const BACKLOG_FILE: &str = "backlog.md";

/// The most of the file that is read; an entry cut there ends with `…`.
pub const BACKLOG_BYTES: u64 = 256 * 1024;

/// The entries of a backlog's text, in file order. Text before the first
/// `## ` line (the title and intro) belongs to none, and a `## ` line inside
/// a code fence starts no entry. Each entry's text leaves its heading line
/// out and is trimmed of blank lines around it.
pub fn parse(text: &str) -> Vec<BacklogEntry> {
    let mut entries: Vec<BacklogEntry> = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced && let Some(heading) = line.strip_prefix("## ") {
            entries.push(BacklogEntry {
                heading: heading.trim().to_string(),
                text: String::new(),
            });
            continue;
        }
        if let Some(entry) = entries.last_mut() {
            entry.text.push_str(line);
            entry.text.push('\n');
        }
    }
    for entry in &mut entries {
        entry.text = entry.text.trim_matches('\n').trim_end().to_string();
    }
    entries
}

/// The text before a backlog's first entry: its `# ` title, when its first
/// line that is not blank is one, and the rest, trimmed (the intro).
pub fn head(text: &str) -> (Option<String>, String) {
    let mut before = String::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced && line.starts_with("## ") {
            break;
        }
        before.push_str(line);
        before.push('\n');
    }
    let before = before.trim();
    match before.split_once('\n').unwrap_or((before, "")) {
        (first, rest) if first.starts_with("# ") => {
            (Some(first[2..].trim().to_string()), rest.trim().to_string())
        }
        _ => (None, before.to_string()),
    }
}

/// The backlog's text in the state directory `dir`: at most
/// [`BACKLOG_BYTES`] of it, and whether it was cut there; none when the
/// file is missing or unreadable. Only a plain file is read: a link is not
/// followed, and a pipe would block.
pub fn read_text(dir: &Path) -> Option<(String, bool)> {
    let path = dir.join(BACKLOG_FILE);
    if !fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file()) {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(&path)
        .and_then(|file| file.take(BACKLOG_BYTES + 1).read_to_end(&mut bytes))
        .ok()?;
    let cut = bytes.len() as u64 > BACKLOG_BYTES;
    bytes.truncate(BACKLOG_BYTES as usize);
    Some((String::from_utf8_lossy(&bytes).into_owned(), cut))
}

/// The entries of `text`, as [`read_text`] gave it: the last one ends with
/// `…` when the file was `cut`.
pub fn entries_of(text: &str, cut: bool) -> Vec<BacklogEntry> {
    let mut entries = parse(text);
    if cut && let Some(last) = entries.last_mut() {
        // The cut can split a character.
        last.text = format!("{}…", last.text.trim_end_matches('\u{FFFD}').trim_end());
    }
    entries
}

/// The backlog's entries in the state directory `dir` ([`read_text`]);
/// none when there is no file.
pub fn read(dir: &Path) -> Vec<BacklogEntry> {
    read_text(dir).map_or_else(Vec::new, |(text, cut)| entries_of(&text, cut))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ScratchDir;

    fn entry(heading: &str, text: &str) -> BacklogEntry {
        BacklogEntry {
            heading: heading.into(),
            text: text.into(),
        }
    }

    #[test]
    fn the_head_is_the_title_and_the_intro_before_the_first_entry() {
        assert_eq!(
            head("# Backlog: later\nRaised 2026-10-30.\n\n## One\nText\n"),
            (Some("Backlog: later".into()), "Raised 2026-10-30.".into())
        );
        assert_eq!(
            head("Just an intro\n## One\n"),
            (None, "Just an intro".into())
        );
        assert_eq!(head("## One\n"), (None, String::new()));
    }

    #[test]
    fn the_title_and_intro_are_skipped_and_each_heading_is_an_entry() {
        let text = "# Backlog: things to build later\n\
                    Raised by the user 2026-10-30.\n\
                    \n\
                    ## 1. \"Stop all agents\" button\n\
                    For when the user is leaving the network.\n\
                    \n\
                    Second paragraph.\n\
                    \n\
                    ## Merge wake not delivered (parked)  \n\
                    The w-app-icon merge line.\n";
        assert_eq!(
            parse(text),
            vec![
                entry(
                    "1. \"Stop all agents\" button",
                    "For when the user is leaving the network.\n\nSecond paragraph."
                ),
                entry(
                    "Merge wake not delivered (parked)",
                    "The w-app-icon merge line."
                ),
            ]
        );
    }

    #[test]
    fn a_heading_in_a_code_fence_or_deeper_starts_no_entry() {
        let text = "## One\n```md\n## not a heading\n```\n### A subheading\nmore\n## Two";
        assert_eq!(
            parse(text),
            vec![
                entry(
                    "One",
                    "```md\n## not a heading\n```\n### A subheading\nmore"
                ),
                entry("Two", ""),
            ]
        );
    }

    #[test]
    fn a_file_without_entries_is_no_backlog() {
        assert!(parse("").is_empty());
        assert!(parse("# Backlog\nNothing yet.\n### Deeper\n").is_empty());
    }

    #[test]
    fn a_missing_file_or_a_link_is_no_backlog_and_a_long_one_is_cut() {
        let scratch = ScratchDir::new("backlog-read");
        let dir: &Path = &scratch;
        assert!(read(dir).is_empty());

        fs::write(dir.join(BACKLOG_FILE), "# Backlog\n## One\nText.\n").unwrap();
        assert_eq!(read(dir), vec![entry("One", "Text.")]);

        let elsewhere = dir.join("elsewhere.md");
        fs::rename(dir.join(BACKLOG_FILE), &elsewhere).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&elsewhere, dir.join(BACKLOG_FILE)).unwrap();
            assert!(read(dir).is_empty());
            fs::remove_file(dir.join(BACKLOG_FILE)).unwrap();
        }

        let long = format!("## Long\n{}", "é".repeat(BACKLOG_BYTES as usize));
        fs::write(dir.join(BACKLOG_FILE), long).unwrap();
        let entries = read(dir);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].text.ends_with("é…"), "{}", &entries[0].text[..8]);
        assert!(entries[0].text.len() <= BACKLOG_BYTES as usize + "…".len());
    }
}
