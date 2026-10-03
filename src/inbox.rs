//! A corgi's inbox: every wake, merge-conflict line and note meant for a
//! project's corgi, kept in `inbox.jsonl` in its state directory so that
//! none is lost when the dashboard that types them into the corgi's input
//! box closes, restarts, or another dashboard takes over the wake lock.
//!
//! The file is append-only JSON lines of two kinds: an item, and a record
//! that items were delivered. An item no record names is undelivered. A later
//! item with the same id replaces an earlier one, as in the ledger. Only this
//! module writes the file, each write under `inbox.lock`: the dashboard calls
//! it in process, and everything else through `corgi notify`. Readers take no
//! lock and skip a last line still being written. Once the file grows past
//! [`COMPACT_AT`] bytes it is rewritten with every undelivered item and only
//! the newest delivered ones, so reading it stays cheap.

use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::time::{rfc3339_utc, unix_now, utc_stamp};

/// The inbox, in the corgi's state directory.
pub const INBOX_FILE: &str = "inbox.jsonl";
/// Where reports too long to keep in their item go, one file per item,
/// beside the inbox.
pub const REPORTS_DIR: &str = "inbox-reports";
/// Taken by every write, so that appends and a compaction never interleave.
const LOCK_FILE: &str = "inbox.lock";
/// The longest report an item keeps inline, in bytes; a longer one goes to
/// its own file in [`REPORTS_DIR`]. Only an inline report is typed into the
/// corgi's box with its wake.
pub const INLINE_REPORT_MAX: usize = 4 * 1024;
/// The size past which a write compacts the inbox.
const COMPACT_AT: u64 = 256 * 1024;
/// Delivered items a compaction keeps, the newest, for `corgi inbox
/// --delivered` and `corgi report`.
const KEEP_DELIVERED: usize = 100;

/// What an item is about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// An agent of the project stopped working.
    #[default]
    Wake,
    /// The user's merge of a worker's branch conflicted.
    Conflict,
    /// Anything else, such as a line a script added with `corgi notify`.
    Note,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Wake => "wake",
            Self::Conflict => "conflict",
            Self::Note => "note",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        [Self::Wake, Self::Conflict, Self::Note]
            .into_iter()
            .find(|kind| kind.label() == label)
    }
}

/// One thing for the corgi to hear.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Item {
    /// Unique within the inbox. A wake's id is made from the state change it
    /// reports, so the same change is never added twice.
    pub id: String,
    /// When it was added, RFC 3339 in UTC.
    pub ts: String,
    /// The project root, as the dashboard knows it.
    pub project: String,
    /// Who added it: `dashboard`, `merge`, `notify`, or a name a script gave.
    pub source: String,
    pub kind: Kind,
    /// The agent it is about, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// That agent's state, lowercase, for a wake.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Undelivered items with the same key go out as the newest one only,
    /// as a newer wake for an agent replaces an unsent one. The id when
    /// nothing else is given.
    pub key: String,
    /// The `[corgi]` line typed into the corgi's box.
    pub text: String,
    /// Whether the agent is one the corgi spawned (it carries a request id),
    /// whose inline report goes out with the wake.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub own: bool,
    /// The agent's report, when it is at most [`INLINE_REPORT_MAX`] bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
    /// A longer report's file, relative to the state directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_file: Option<String>,
}

impl Item {
    /// Whether the item carries a report, inline or in its own file.
    pub fn has_report(&self) -> bool {
        self.report.is_some() || self.report_file.is_some()
    }

    /// The item's report in full, read from its file when it has one there.
    pub fn report_text(&self, dir: &Path) -> Option<String> {
        self.report.clone().or_else(|| {
            self.report_file
                .as_ref()
                .and_then(|file| fs::read_to_string(dir.join(file)).ok())
        })
    }

    /// What goes into the corgi's box for this item: its line, followed for
    /// one of the corgi's own agents by its inline report.
    pub fn prompt(&self) -> String {
        match (&self.report, &self.agent) {
            (Some(report), Some(agent)) if self.own => format!(
                "{}\n{}\n[corgi] End of {agent}'s report.",
                self.text,
                report.trim_end()
            ),
            _ => self.text.clone(),
        }
    }
}

/// A record that the items `delivered` went out, to the corgi named `to`.
#[derive(Debug, Serialize, Deserialize)]
struct Delivery {
    delivered: Vec<String>,
    ts: String,
    #[serde(default)]
    to: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Line {
    Delivery(Delivery),
    Item(Item),
}

/// An inbox as read: its items, oldest first, and the ids delivered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inbox {
    items: Vec<Item>,
    delivered: HashSet<String>,
}

impl Inbox {
    /// The inbox in the state directory `dir`; empty when there is none.
    pub fn read(dir: &Path) -> Self {
        fs::read_to_string(dir.join(INBOX_FILE))
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    /// The inbox written as `text`. A line that does not parse, such as a
    /// last one still being written, is skipped.
    pub fn parse(text: &str) -> Self {
        let complete = match text.rfind('\n') {
            Some(end) => &text[..end],
            None => "",
        };
        let mut items = Vec::new();
        let mut delivered = HashSet::new();
        for line in complete.lines().filter(|line| !line.trim().is_empty()) {
            match serde_json::from_str::<Line>(line) {
                Ok(Line::Delivery(record)) => delivered.extend(record.delivered),
                Ok(Line::Item(item)) if !item.id.is_empty() => items.push(item),
                _ => {}
            }
        }
        // The newest line for an id wins, in the place of that line.
        let mut seen = HashSet::new();
        items.reverse();
        items.retain(|item: &Item| seen.insert(item.id.clone()));
        items.reverse();
        Self { items, delivered }
    }

    pub fn contains(&self, id: &str) -> bool {
        self.items.iter().any(|item| item.id == id)
    }

    /// The items not delivered yet, oldest first.
    pub fn undelivered(&self) -> Vec<&Item> {
        self.items
            .iter()
            .filter(|item| !self.delivered.contains(&item.id))
            .collect()
    }

    /// The delivered items, oldest first.
    pub fn delivered(&self) -> Vec<&Item> {
        self.items
            .iter()
            .filter(|item| self.delivered.contains(&item.id))
            .collect()
    }

    /// The newest item that carries a report of `agent`.
    pub fn newest_report(&self, agent: &str) -> Option<&Item> {
        self.items
            .iter()
            .rev()
            .find(|item| item.agent.as_deref() == Some(agent) && item.has_report())
    }
}

/// `items` as they go out together: the newest of each key, in the order
/// those were added.
pub fn coalesce<'a>(items: &[&'a Item]) -> Vec<&'a Item> {
    let mut keys = HashSet::new();
    let mut newest: Vec<&Item> = items
        .iter()
        .rev()
        .filter(|item| keys.insert(item.key.as_str()))
        .copied()
        .collect();
    newest.reverse();
    newest
}

/// The one prompt that delivers `items`: each item's [`Item::prompt`], the
/// newest of each key only, one after the other.
pub fn prompt_text(items: &[&Item]) -> String {
    coalesce(items)
        .iter()
        .map(|item| item.prompt())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Adds `item` to the inbox in the state directory `dir`, filling in its id,
/// time and key when they are empty and moving a report longer than
/// [`INLINE_REPORT_MAX`] to its own file. Returns the item as written, or
/// `None` when the inbox already has an item with its id, so a wake for a
/// change already recorded is not added twice.
pub fn append(dir: &Path, mut item: Item) -> Result<Option<Item>> {
    let _lock = lock(dir)?;
    if item.id.is_empty() {
        item.id = fresh_id(item.agent.as_deref().unwrap_or(&item.source));
    } else if Inbox::read(dir).contains(&item.id) {
        return Ok(None);
    }
    if item.ts.is_empty() {
        item.ts = rfc3339_utc(unix_now());
    }
    if item.key.is_empty() {
        item.key = item.id.clone();
    }
    if let Some(report) = item
        .report
        .take_if(|report| report.len() > INLINE_REPORT_MAX)
    {
        let file = format!("{REPORTS_DIR}/{}.md", file_safe(&item.id));
        let path = dir.join(&file);
        fs::create_dir_all(dir.join(REPORTS_DIR))
            .with_context(|| format!("create {}", dir.join(REPORTS_DIR).display()))?;
        fs::write(&path, report).with_context(|| format!("write {}", path.display()))?;
        item.report_file = Some(file);
    }
    append_line(dir, &serde_json::to_string(&item)?)?;
    compact_if_large(dir)?;
    Ok(Some(item))
}

/// Records that the items `ids` went out to the corgi `to`.
pub fn mark_delivered(dir: &Path, ids: &[String], to: &str) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let _lock = lock(dir)?;
    let record = Delivery {
        delivered: ids.to_vec(),
        ts: rfc3339_utc(unix_now()),
        to: to.to_string(),
    };
    append_line(dir, &serde_json::to_string(&record)?)
}

/// The footer the command-line tools print when the corgi of the state
/// directory `dir` has undelivered items, naming the command that shows
/// them.
pub fn footer(dir: &Path, corgi_bin: &str, project: &str) -> Option<String> {
    let count = Inbox::read(dir).undelivered().len();
    (count > 0).then(|| {
        let items = if count == 1 { "item" } else { "items" };
        format!("{count} undelivered inbox {items}: run {corgi_bin} inbox {project}")
    })
}

/// The inbox's write lock, held until the file is dropped.
fn lock(dir: &Path) -> Result<File> {
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join(LOCK_FILE);
    let file = File::create(&path).with_context(|| format!("open {}", path.display()))?;
    file.lock()
        .with_context(|| format!("lock {}", path.display()))?;
    Ok(file)
}

/// Appends `line` to the inbox in one write.
fn append_line(dir: &Path, line: &str) -> Result<()> {
    let path = dir.join(INBOX_FILE);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(format!("{line}\n").as_bytes())
        .with_context(|| format!("append to {}", path.display()))
}

/// Rewrites the inbox, under the lock the caller holds, once it is past
/// [`COMPACT_AT`] bytes and holds more than [`KEEP_DELIVERED`] delivered
/// items: every undelivered item stays, with the newest delivered ones and
/// one record of their delivery. The report files of the items dropped go
/// too.
fn compact_if_large(dir: &Path) -> Result<()> {
    let path = dir.join(INBOX_FILE);
    if fs::metadata(&path).map_or(0, |meta| meta.len()) <= COMPACT_AT {
        return Ok(());
    }
    let inbox = Inbox::read(dir);
    let delivered = inbox.delivered();
    // Undelivered items are never dropped, so a file of those alone stays.
    if delivered.len() <= KEEP_DELIVERED {
        return Ok(());
    }
    let dropped: HashSet<&str> = delivered
        .iter()
        .take(delivered.len().saturating_sub(KEEP_DELIVERED))
        .map(|item| item.id.as_str())
        .collect();
    let mut text = String::new();
    for item in inbox
        .items
        .iter()
        .filter(|item| !dropped.contains(item.id.as_str()))
    {
        text.push_str(&serde_json::to_string(item)?);
        text.push('\n');
    }
    let kept: Vec<String> = delivered
        .iter()
        .filter(|item| !dropped.contains(item.id.as_str()))
        .map(|item| item.id.clone())
        .collect();
    if !kept.is_empty() {
        let record = Delivery {
            delivered: kept,
            ts: rfc3339_utc(unix_now()),
            to: "compaction".into(),
        };
        text.push_str(&serde_json::to_string(&record)?);
        text.push('\n');
    }
    let temporary = dir.join(format!("{INBOX_FILE}.tmp"));
    fs::write(&temporary, text).with_context(|| format!("write {}", temporary.display()))?;
    fs::rename(&temporary, &path).with_context(|| format!("replace {}", path.display()))?;
    for item in inbox
        .items
        .iter()
        .filter(|item| dropped.contains(item.id.as_str()))
    {
        if let Some(file) = &item.report_file {
            let _ = fs::remove_file(dir.join(file));
        }
    }
    Ok(())
}

/// A new id for an item about `subject`: the time, the subject and a
/// number no other id of this process or moment has.
fn fresh_id(subject: &str) -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.subsec_nanos());
    format!(
        "{}-{}-{}{nanos:09}{}",
        utc_stamp(unix_now()),
        file_safe(subject),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// `text` with everything but ASCII letters, digits, `.`, `_` and `-` made
/// `-`, so it can name a file.
pub fn file_safe(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// The inboxes of every corgi under Corgi's state directory `base`, for
/// finding the report of an agent whose pane is gone.
pub fn all_state_dirs(base: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(base.join("corgis"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join(INBOX_FILE).is_file())
        .collect();
    dirs.sort();
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ScratchDir;

    fn wake(agent: &str, id: &str) -> Item {
        Item {
            id: id.into(),
            project: "/repos/weather".into(),
            source: "dashboard".into(),
            agent: Some(agent.into()),
            state: Some("done".into()),
            key: agent.into(),
            text: format!("[corgi] {agent} is done. Run: corgi report {agent}"),
            ..Item::default()
        }
    }

    #[test]
    fn items_stay_undelivered_until_a_delivery_names_them_and_survive_rereading() {
        let dir = ScratchDir::new("inbox-deliver");
        let first = append(&dir, wake("w-forecast", "w1")).unwrap().unwrap();
        assert!(!first.ts.is_empty());
        append(&dir, wake("w-radar", "w2")).unwrap().unwrap();
        // The same change again is not added twice.
        assert_eq!(append(&dir, wake("w-forecast", "w1")).unwrap(), None);
        let inbox = Inbox::read(&dir);
        let ids = |items: Vec<&Item>| items.iter().map(|i| i.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(inbox.undelivered()), ["w1", "w2"]);
        mark_delivered(&dir, &["w1".into()], "corgi-weather").unwrap();
        let inbox = Inbox::read(&dir);
        assert_eq!(ids(inbox.undelivered()), ["w2"]);
        assert_eq!(ids(inbox.delivered()), ["w1"]);
        // A note gets an id, and its key is that id.
        let note = append(
            &dir,
            Item {
                source: "notify".into(),
                kind: Kind::Note,
                text: "[corgi] deploy finished".into(),
                ..Item::default()
            },
        )
        .unwrap()
        .unwrap();
        assert!(note.id.contains("notify"), "{}", note.id);
        assert_eq!(note.key, note.id);
    }

    #[test]
    fn a_half_written_last_line_and_garbage_are_skipped_and_the_newest_line_wins() {
        let first = serde_json::to_string(&wake("w-forecast", "w1")).unwrap();
        let mut newer = wake("w-forecast", "w1");
        newer.state = Some("idle".into());
        let newer = serde_json::to_string(&newer).unwrap();
        let text = format!("{first}\nnot json\n{newer}\n{{\"id\":\"w2\",\"te");
        let inbox = Inbox::parse(&text);
        assert_eq!(inbox.undelivered().len(), 1);
        assert_eq!(inbox.undelivered()[0].state.as_deref(), Some("idle"));
    }

    #[test]
    fn a_newer_item_with_the_same_key_supersedes_an_unsent_one() {
        let items = [
            wake("w-forecast", "a"),
            wake("w-radar", "b"),
            Item {
                text: "[corgi] w-forecast is blocked.".into(),
                ..wake("w-forecast", "c")
            },
        ];
        let refs: Vec<&Item> = items.iter().collect();
        assert_eq!(
            prompt_text(&refs),
            "[corgi] w-radar is done. Run: corgi report w-radar\n[corgi] w-forecast is blocked."
        );
    }

    #[test]
    fn only_an_own_agents_short_report_is_kept_inline_and_typed_with_its_wake() {
        let dir = ScratchDir::new("inbox-report");
        let short = "### Report\n- Result: done".to_string();
        let own = append(
            &dir,
            Item {
                own: true,
                text: "[corgi] w-forecast is done. Its report:".into(),
                report: Some(short.clone()),
                ..wake("w-forecast", "a")
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            own.prompt(),
            "[corgi] w-forecast is done. Its report:\n### Report\n- Result: done\n\
             [corgi] End of w-forecast's report."
        );
        // Another's report is kept, but the wake only points to it.
        let other = append(
            &dir,
            Item {
                report: Some(short.clone()),
                ..wake("w-user", "b")
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(other.prompt(), other.text);
        // A long one goes to its own file, and is not typed.
        let long = "x".repeat(INLINE_REPORT_MAX + 1);
        let big = append(
            &dir,
            Item {
                own: true,
                report: Some(long.clone()),
                ..wake("w-radar", "c")
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(big.report, None);
        assert_eq!(big.report_file.as_deref(), Some("inbox-reports/c.md"));
        assert_eq!(big.prompt(), big.text);
        let inbox = Inbox::read(&dir);
        assert_eq!(
            inbox.newest_report("w-radar").unwrap().report_text(&dir),
            Some(long)
        );
        assert_eq!(
            inbox.newest_report("w-forecast").unwrap().report_text(&dir),
            Some(short)
        );
        assert!(inbox.newest_report("w-nobody").is_none());
    }

    #[test]
    fn a_large_inbox_is_compacted_keeping_every_undelivered_item() {
        let dir = ScratchDir::new("inbox-compact");
        let padding = "y".repeat(2_000);
        let mut delivered = Vec::new();
        for n in 0..200 {
            let id = format!("old{n}");
            append(
                &dir,
                Item {
                    report: Some(padding.clone()),
                    report_file: None,
                    ..wake("w-forecast", &id)
                },
            )
            .unwrap();
            delivered.push(id);
        }
        mark_delivered(&dir, &delivered, "corgi-weather").unwrap();
        append(&dir, wake("w-radar", "fresh")).unwrap();
        // The next write finds the file past its limit.
        let big = "z".repeat(INLINE_REPORT_MAX + 1);
        append(
            &dir,
            Item {
                report: Some(big),
                ..wake("w-radar", "fresher")
            },
        )
        .unwrap();
        let size = fs::metadata(dir.join(INBOX_FILE)).unwrap().len();
        assert!(size <= COMPACT_AT, "{size}");
        let inbox = Inbox::read(&dir);
        let undelivered: Vec<_> = inbox.undelivered().iter().map(|i| i.id.clone()).collect();
        assert_eq!(undelivered, ["fresh", "fresher"]);
        assert_eq!(inbox.delivered().len(), KEEP_DELIVERED);
        assert_eq!(inbox.delivered()[0].id, "old100");
        assert!(dir.join(REPORTS_DIR).join("fresher.md").is_file());
    }

    #[test]
    fn the_footer_counts_undelivered_items() {
        let dir = ScratchDir::new("inbox-footer");
        assert_eq!(footer(&dir, "/opt/corgi", "/repos/weather"), None);
        append(&dir, wake("w-forecast", "a")).unwrap();
        assert_eq!(
            footer(&dir, "/opt/corgi", "/repos/weather").as_deref(),
            Some("1 undelivered inbox item: run /opt/corgi inbox /repos/weather")
        );
        append(&dir, wake("w-radar", "b")).unwrap();
        assert_eq!(
            footer(&dir, "corgi", "/r").as_deref(),
            Some("2 undelivered inbox items: run corgi inbox /r")
        );
        mark_delivered(&dir, &["a".into(), "b".into()], "corgi-weather").unwrap();
        assert_eq!(footer(&dir, "corgi", "/r"), None);
    }
}
