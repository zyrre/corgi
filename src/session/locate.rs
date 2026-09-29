//! Finding the session file of a pane: a Claude Code transcript under
//! `projects/` in its configuration directory (`CLAUDE_CONFIG_DIR` or
//! `~/.claude`), or a Codex rollout under `~/.codex/sessions`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{tail::Head, text::field};

/// Rollouts considered when a Codex pane has no session identity and has to be
/// matched by its working directory instead.
pub(super) const CODEX_ROLLOUT_SCAN: usize = 40;
/// Directory levels below `~/.codex/sessions` that hold rollouts: the year,
/// the month, and the day, with the rollouts themselves one level further.
pub(super) const CODEX_SESSIONS_DEPTH: usize = 4;
/// Head of a rollout read for the session metadata it opens with.
pub(super) const CODEX_META_HEAD_BYTES: u64 = 64 * 1024;
/// Day directories a retried Codex lookup searches. A rollout is filed under
/// the day its session started, so one that appears after a search can only
/// be in the newest day, or the one before it for a session started just
/// before midnight.
pub(super) const CODEX_RECENT_DAYS: usize = 2;

/// How much of the Codex sessions tree a lookup searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Search {
    Everywhere,
    /// Only the newest days, where a rollout written since the last search is.
    Recent,
}

/// Path of a Claude Code transcript under the configuration directory
/// `claude_dir`.
///
/// Claude Code names the transcript after the session and its directory after
/// the working directory the session started in, with every separator and dot
/// replaced by a dash. A session Herdr never learned the ID of, which happens
/// when its agent integration is not installed, falls back to the newest
/// transcript of that directory.
pub(super) fn claude_transcript(
    claude_dir: &Path,
    session_id: Option<&str>,
    cwd: &str,
) -> Option<PathBuf> {
    let projects = claude_dir.join("projects");
    let project = projects.join(
        cwd.chars()
            .map(|character| match character {
                '/' | '.' => '-',
                other => other,
            })
            .collect::<String>(),
    );
    let Some(session_id) = session_id else {
        return newest_file(&project);
    };
    let file = format!("{session_id}.jsonl");
    let direct = project.join(&file);
    if direct.is_file() {
        return Some(direct);
    }
    // The naming rule is Claude Code's own, so a session that does not sit
    // where it is expected is still found by name.
    fs::read_dir(&projects)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(&file))
        .find(|candidate| candidate.is_file())
}

/// The most recently written `jsonl` file directly inside `directory`.
pub(super) fn newest_file(directory: &Path) -> Option<PathBuf> {
    fs::read_dir(directory)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_jsonl(path))
        .max_by_key(|path| fs::metadata(path).and_then(|data| data.modified()).ok())
}

/// Path of a Codex rollout.
///
/// Codex stores rollouts in `sessions/<year>/<month>/<day>` and ends every
/// file name with the thread ID, so a known session is a match by name, found
/// by listing the days newest first without looking at any file's age. A pane
/// whose thread Herdr never learned falls back to the most recently written
/// rollout started in the same directory, which does have to compare the ages
/// of every candidate.
pub(super) fn codex_rollout(
    home: &Path,
    session_id: Option<&str>,
    cwd: &str,
    search: Search,
) -> Option<PathBuf> {
    let sessions = home.join(".codex").join("sessions");
    let days = match search {
        Search::Everywhere => usize::MAX,
        Search::Recent => CODEX_RECENT_DAYS,
    };
    let suffix = session_id.map(|session_id| format!("{session_id}.jsonl"));
    let named = |path: &PathBuf| {
        suffix
            .as_deref()
            .is_some_and(|suffix| path.to_string_lossy().ends_with(suffix))
    };
    if suffix.is_some()
        && let Some(path) = codex_days(&sessions)
            .take(days)
            .find_map(|day| jsonl_files(&day).into_iter().find(&named))
    {
        return Some(path);
    }
    let mut rollouts = Vec::new();
    match search {
        Search::Everywhere => collect_files(&sessions, CODEX_SESSIONS_DEPTH, &mut rollouts),
        Search::Recent => {
            for day in codex_days(&sessions).take(days) {
                rollouts.extend(jsonl_files(&day));
            }
        }
    }
    rollouts.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|data| data.modified())
            .ok()
            .map(std::cmp::Reverse)
    });
    // A rollout outside the dated layout is still found by name.
    if let Some(path) = rollouts.iter().find(|path| named(path)) {
        return Some(path.clone());
    }
    rollouts
        .into_iter()
        .take(CODEX_ROLLOUT_SCAN)
        .find(|path| codex_rollout_cwd(path).as_deref() == Some(cwd))
}

/// The day directories of Codex's sessions tree, newest first. The tree is
/// listed lazily, so a search that ends in a recent day lists no older month.
pub(super) fn codex_days(sessions: &Path) -> impl Iterator<Item = PathBuf> {
    subdirectories_newest_first(sessions)
        .into_iter()
        .flat_map(|year| subdirectories_newest_first(&year))
        .flat_map(|month| subdirectories_newest_first(&month))
}

/// Codex zero-pads every part of the date, so the names sort as dates do.
pub(super) fn subdirectories_newest_first(directory: &Path) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    directories.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
    directories
}

pub(super) fn jsonl_files(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_files(directory, 1, &mut files);
    files
}

pub(super) fn collect_files(directory: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    for entry in fs::read_dir(directory).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, depth - 1, files);
        } else if is_jsonl(&path) {
            files.push(path);
        }
    }
}

pub(super) fn is_jsonl(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "jsonl")
}

/// The directory a Codex rollout was started in, from the session metadata it
/// opens with.
pub(super) fn codex_rollout_cwd(path: &Path) -> Option<String> {
    let mut head = Head::default();
    head.update(path, CODEX_META_HEAD_BYTES, |record| {
        field(record, "cwd").map(str::to_string)
    });
    head.found
}
