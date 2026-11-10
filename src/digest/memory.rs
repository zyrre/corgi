//! The lookups `corgi digest` makes in a supervisor's memory, its state
//! directory: a decision, a search of all of it, a block of one file, a
//! ledger id's history and a brief. They only read.

use std::{env, fs, path::Path};

use anyhow::{Context, Result};

use crate::{supervisor, time::rfc3339_utc};

/// One lookup in a supervisor's memory, as `corgi digest` takes them.
pub enum DigestLookup {
    Decision(String),
    Search(String),
    Show(String),
    Ledger(String),
    Brief(String),
}

/// What `lookup` prints from the state directory `state_dir` of the project
/// at `root`. Reads only.
pub fn digest_lookup(state_dir: &Path, root: &str, lookup: DigestLookup) -> Result<String> {
    let read = |name: &str| fs::read_to_string(state_dir.join(name)).ok();
    let ledger = || read(super::lookup::LEDGER_FILE).unwrap_or_default();
    let archive = || read(super::lookup::LEDGER_ARCHIVE_FILE).unwrap_or_default();
    let names = |files: Vec<(String, String)>| -> Vec<String> {
        files.into_iter().map(|(name, _)| name).collect()
    };
    match lookup {
        DigestLookup::Decision(words) => {
            let text = read("decisions.md").unwrap_or_default();
            let decisions = super::parse_decisions(&text);
            let found = super::matching_decisions(&decisions, &words);
            if !found.is_empty() {
                return Ok(format!("{}\n", found.join("\n\n")));
            }
            let found = super::decisions_containing(&decisions, &words);
            if !found.is_empty() {
                let count = found.len();
                return Ok(format!(
                    "No decision heading contains {words:?}; {count} decision{} {} it in {}text:\n\n{}\n",
                    if count == 1 { "" } else { "s" },
                    if count == 1 { "has" } else { "have" },
                    if count == 1 { "its " } else { "their " },
                    found.join("\n\n")
                ));
            }
            Ok(format!(
                "No decision contains {words:?}; searching all of your memory instead.\n\n{}",
                digest_search(state_dir, root, &words)
            ))
        }
        DigestLookup::Search(words) => Ok(digest_search(state_dir, root, &words)),
        DigestLookup::Show(handle) => {
            let state = state_dir.to_string_lossy();
            let (path, line) = super::lookup::parse_handle(&state, &handle)?;
            let text = fs::read_to_string(state_dir.join(&path))
                .with_context(|| format!("read {path} in {state}"))?;
            super::lookup::show(&path, &text, line, &ledger(), &archive())
        }
        DigestLookup::Ledger(id) => super::lookup::ledger(
            &ledger(),
            &archive(),
            &id,
            &names(markdown_files(&state_dir.join("briefs"))),
            &names(markdown_files(&state_dir.join(super::lookup::REPORTS_DIR))),
        ),
        DigestLookup::Brief(id) => {
            super::lookup::brief(&markdown_files(&state_dir.join("briefs")), &id)
        }
    }
}

/// `--search`'s output for `words` in the state directory `state_dir`.
pub fn digest_search(state_dir: &Path, root: &str, words: &str) -> String {
    let read = |name: &str| fs::read_to_string(state_dir.join(name)).ok();
    let state = state_dir.to_string_lossy();
    let decisions = read("decisions.md").unwrap_or_default();
    let ledger = read(super::lookup::LEDGER_FILE).unwrap_or_default();
    let ledger_archive = read(super::lookup::LEDGER_ARCHIVE_FILE).unwrap_or_default();
    let handover_path = state_dir.join(supervisor::HANDOVER_NOTE);
    let handover = fs::read_to_string(&handover_path)
        .ok()
        .map(|text| (modified_text(&handover_path), text));
    let briefs = markdown_files(&state_dir.join("briefs"));
    let handovers = markdown_files(&state_dir.join(supervisor::HANDOVERS_DIR));
    let reports = markdown_files(&state_dir.join(super::lookup::REPORTS_DIR));
    let notes = note_files(&state_dir.join(super::lookup::NOTES_DIR));
    let show_command = format!("{} digest {root} --show <file:line>", corgi_bin_text());
    super::search::search(
        &super::search::SearchInput {
            state_dir: &state,
            show_command: &show_command,
            decisions: &decisions,
            ledger: &ledger,
            ledger_archive: &ledger_archive,
            handover: handover
                .as_ref()
                .map(|(when, text)| (when.as_str(), text.as_str())),
            briefs: &briefs,
            handovers: &handovers,
            reports: &reports,
            notes: &notes,
        },
        words,
    )
}

/// When `path` was last written, `YYYY-MM-DD HH:MM UTC`, or empty when that
/// cannot be read.
pub fn modified_text(path: &Path) -> String {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| {
            let stamp = rfc3339_utc(age.as_secs());
            format!("{} {} UTC", &stamp[..10], &stamp[11..16])
        })
        .unwrap_or_default()
}

/// The largest note `--search` reads; a bigger file is likely not one.
const NOTE_BYTES: u64 = 1024 * 1024;

/// `(path under dir, time last written, text)` of each text file in `dir`
/// and the directories in it, by path; none when `dir` does not exist.
/// Files that are not UTF-8 or past [`NOTE_BYTES`] are left out, and links
/// are not followed.
pub fn note_files(dir: &Path) -> Vec<(String, String, String)> {
    let mut notes = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current).into_iter().flatten().flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file()
                && entry.metadata().is_ok_and(|meta| meta.len() <= NOTE_BYTES)
                && let Ok(text) = fs::read_to_string(&path)
                && let Ok(relative) = path.strip_prefix(dir)
            {
                notes.push((
                    relative.to_string_lossy().into_owned(),
                    modified_text(&path),
                    text,
                ));
            }
        }
    }
    notes.sort();
    notes
}

/// `(file name, text)` of each readable `.md` file in `dir`, by name; none
/// when `dir` does not exist.
pub fn markdown_files(dir: &Path) -> Vec<(String, String)> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let text = name
                .ends_with(".md")
                .then(|| fs::read_to_string(entry.path()).ok())??;
            Some((name, text))
        })
        .collect();
    files.sort();
    files
}

/// The Corgi binary as the commands it prints name it.
pub fn corgi_bin_text() -> String {
    env::current_exe().map_or_else(
        |_| "corgi".to_string(),
        |path| path.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use crate::test_support::ScratchDir;

    use super::*;

    #[test]
    fn digest_lookups_read_the_state_directory_and_decision_falls_back_to_search() {
        let state = ScratchDir::new("digest-lookup");
        let write = |name: &str, text: &str| {
            let path = state.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(
            "decisions.md",
            "# Decisions\n\n## 2026-10-13: Loop fix (Q13: C)\nDecision: guard in kafka.\n\n\
             ## 2026-10-14: Snapshots\nDecision: none, settled as Q21.\n",
        );
        write(
            "handovers/20261014-103547.md",
            "# Handover\n\n## Open threads\nSettled: Q23 A (lessons in CLAUDE.md).\n",
        );
        write(
            "notes/plans/rollout.md",
            "# Rollout\n\nQ23 follow-up: none.\n",
        );
        write(
            "ledger-archive.jsonl",
            r#"{"id":"20261001-w-a","agent":"w-a","status":"dispatched","summary":"Q23 lessons"}"#,
        );
        write("briefs/20261001-w-a.md", "# Lessons\n");
        let lookup = |what| digest_lookup(&state, "/repos/weather", what).unwrap();

        // A heading, then a body, then a search.
        assert!(
            lookup(DigestLookup::Decision("q13".into())).starts_with("## 2026-10-13: Loop fix")
        );
        assert_eq!(
            lookup(DigestLookup::Decision("Q21".into())),
            "No decision heading contains \"Q21\"; 1 decision has it in its text:\n\n\
             ## 2026-10-14: Snapshots\nDecision: none, settled as Q21.\n"
        );
        let searched = lookup(DigestLookup::Decision("Q23".into()));
        assert!(searched.starts_with(
            "No decision contains \"Q23\"; searching all of your memory instead.\n\n\
             # Search \"q23\": 3 hits"
        ));
        for place in [
            "handovers/20261014-103547.md:4",
            "notes/plans/rollout.md:3",
            "ledger-archive.jsonl:1",
        ] {
            assert!(searched.contains(place), "{place}: {searched}");
        }

        assert!(
            lookup(DigestLookup::Show("handovers/20261014-103547.md:4".into())).starts_with(
                "# handovers/20261014-103547.md:3 (lines 3-4 of 4)\n## Open threads\n"
            )
        );
        assert!(lookup(DigestLookup::Ledger("w-a".into())).contains(
            "dispatched  agent: w-a; summary: Q23 lessons\nBrief: briefs/20261001-w-a.md\n"
        ));
        assert_eq!(
            lookup(DigestLookup::Brief("w-a".into())),
            "# briefs/20261001-w-a.md (whole file)\n# Lessons\n"
        );
        assert!(
            digest_lookup(
                &state,
                "/repos/weather",
                DigestLookup::Show("nothing.md".into())
            )
            .is_err()
        );
    }
}
