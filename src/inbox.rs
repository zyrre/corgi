//! A supervisor's inbox: every wake, merge-conflict line and note meant for a
//! project's supervisor, kept in `inbox.jsonl` in its state directory so that
//! none is lost when the dashboard that types them into the supervisor's input
//! box closes, restarts, or another dashboard takes over the wake lock.
//!
//! The file is append-only JSON lines of two kinds: an item, and a record
//! that items were delivered. An item no record names is undelivered, and an
//! id is added only once. Only this module writes the file, each write under
//! `inbox.lock`: the dashboard calls it in process, and everything else
//! through `corgi notify`. Readers take no lock and skip a last line still
//! being written; a writer that finds such a line cut short ends it first.
//! Once the file grows past [`COMPACT_AT`] bytes it is rewritten with every
//! undelivered item and only the newest delivered ones, so reading it stays
//! cheap. A project's inbox is in its state directory, which projects of the
//! same directory name share, so each item names its project root.

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    supervisor::wake_report_message,
    time::{parse_rfc3339, rfc3339_utc, unix_now, utc_stamp},
};

/// The inbox, in the supervisor's state directory.
pub const INBOX_FILE: &str = "inbox.jsonl";
/// Where reports too long to keep in their item go, one file per item,
/// beside the inbox.
pub const REPORTS_DIR: &str = "inbox-reports";
/// Taken by every write, so that appends and a compaction never interleave.
const LOCK_FILE: &str = "inbox.lock";
/// The longest report an item keeps inline, in bytes; a longer one goes to
/// its own file in [`REPORTS_DIR`]. Only an inline report is typed into the
/// supervisor's box with its wake.
pub const INLINE_REPORT_MAX: usize = 4 * 1024;
/// The most report text, quoted, one prompt into the supervisor's box carries;
/// past it, a wake names the command that prints its report instead.
pub const PROMPT_REPORT_BUDGET: usize = 12 * 1024;
/// How long an item has waited undelivered before the footer of the
/// command-line tools counts it while a dashboard delivers: until then it
/// is only waiting for the supervisor's turn to end.
pub const FOOTER_AFTER_SECS: u64 = 5 * 60;
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

/// One thing for the supervisor to hear.
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
    /// That agent's pane and `state_change_seq`, for a wake: what tells its
    /// report from one of another agent of the same name, or an older one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<u64>,
    /// Undelivered items with the same key go out as the newest one only,
    /// as a newer wake for an agent replaces an unsent one. The id when
    /// nothing else is given.
    pub key: String,
    /// The `[corgi]` line typed into the supervisor's box. For a wake, it names
    /// the command that prints the agent's report.
    pub text: String,
    /// Whether the agent is one the supervisor spawned (it carries a request id),
    /// whose inline report goes out with the wake, in place of the command.
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

    /// Whether the item is for the project at `root`.
    pub fn is_for(&self, root: &str) -> bool {
        same_project(&self.project, root)
    }

    /// What goes into the supervisor's box for this item: its line, or for one of
    /// the supervisor's own agents with an inline report that fits what is left
    /// of `budget`, a line saying the report follows, the report quoted, and
    /// an end line. Quoting each line keeps a report from passing for a
    /// line of the dashboard's own.
    fn prompt(&self, budget: &mut usize) -> String {
        if let (true, Some(report), Some(agent)) = (self.own, &self.report, &self.agent) {
            let quoted = report
                .trim_end()
                .lines()
                .map(|line| format!("> {line}").trim_end().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            if quoted.len() <= *budget {
                *budget -= quoted.len();
                let state = self.state.as_deref().unwrap_or("done");
                return format!(
                    "{}\n{quoted}\n[corgi] End of {agent}'s report.",
                    wake_report_message(agent, state)
                );
            }
        }
        self.text.clone()
    }
}

/// Whether `a` and `b` are the same project root, spelled alike or the same
/// directory once links are followed.
pub fn same_project(a: &str, b: &str) -> bool {
    a == b
        || matches!(
            (Path::new(a).canonicalize(), Path::new(b).canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}

/// A record that the items `delivered` went out, to the supervisor named `to`.
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
    Item(Box<Item>),
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
    /// last one still being written, is skipped. An id is only ever added
    /// once, and a compaction writes each once, so there is one line per item.
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
                Ok(Line::Item(item)) if !item.id.is_empty() => items.push(*item),
                _ => {}
            }
        }
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

    /// The items for the project at `root` not delivered yet, oldest first.
    pub fn undelivered_for(&self, root: &str) -> Vec<&Item> {
        let mut items = self.undelivered();
        items.retain(|item| item.is_for(root));
        items
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

/// `items` as they go out together, in the order they were added: the
/// newest of each key, as a newer wake for an agent replaces an unsent one,
/// except that one without a report does not replace one with a report, so
/// that report still reaches the corgi.
pub fn coalesce<'a>(items: &[&'a Item]) -> Vec<&'a Item> {
    // By key: whether an item kept has a report.
    let mut kept: HashMap<&str, bool> = HashMap::new();
    let mut newest: Vec<&Item> = Vec::new();
    for item in items.iter().rev() {
        let keep = match kept.get(item.key.as_str()) {
            None => true,
            Some(reported) => !reported && item.has_report(),
        };
        if keep {
            let reported = kept.entry(item.key.as_str()).or_default();
            *reported |= item.has_report();
            newest.push(item);
        }
    }
    newest.reverse();
    newest
}

/// The one prompt that delivers `items`: the [`coalesce`]d ones one after
/// the other, carrying at most [`PROMPT_REPORT_BUDGET`] bytes of reports.
pub fn prompt_text(items: &[&Item]) -> String {
    let mut budget = PROMPT_REPORT_BUDGET;
    coalesce(items)
        .iter()
        .map(|item| item.prompt(&mut budget))
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

/// Records that the items `ids` went out to the supervisor `to`.
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

/// The footer the command-line tools print when the supervisor of the project
/// at `root`, whose state directory is `dir`, has missed items, naming the
/// command that shows them. While a dashboard `delivering` runs, only items
/// that have waited [`FOOTER_AFTER_SECS`] by `now` count, since the others
/// are only waiting for the supervisor's turn to end; without one, all do.
pub fn footer(
    dir: &Path,
    corgi_bin: &str,
    root: &str,
    delivering: bool,
    now: u64,
) -> Option<String> {
    let count = Inbox::read(dir)
        .undelivered_for(root)
        .iter()
        .filter(|item| {
            !delivering
                || parse_rfc3339(&item.ts)
                    .is_none_or(|at| now.saturating_sub(at) >= FOOTER_AFTER_SECS)
        })
        .count();
    (count > 0).then(|| {
        let items = if count == 1 { "item" } else { "items" };
        format!("{count} undelivered inbox {items}: run {corgi_bin} inbox {root}")
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

/// Appends `line` to the inbox in one write, under the lock the caller
/// holds. A last line a killed writer left without its newline is ended
/// first, so the new line is not taken for part of it.
fn append_line(dir: &Path, line: &str) -> Result<()> {
    let path = dir.join(INBOX_FILE);
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    let length = file.metadata().map_or(0, |meta| meta.len());
    let mut last = *b"\n";
    if length > 0 {
        file.seek(SeekFrom::Start(length - 1))
            .and_then(|_| file.read_exact(&mut last))
            .with_context(|| format!("read {}", path.display()))?;
    }
    let torn = if last == *b"\n" { "" } else { "\n" };
    file.write_all(format!("{torn}{line}\n").as_bytes())
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

/// The inboxes of every supervisor under Corgi's state directory `base`, for
/// finding the report of an agent whose pane is gone.
pub fn all_state_dirs(base: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(base.join("supervisors"))
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
    fn a_half_written_last_line_and_garbage_are_skipped() {
        let first = serde_json::to_string(&wake("w-forecast", "w1")).unwrap();
        let text = format!("{first}\nnot json\n{{\"id\":\"w2\",\"te");
        let inbox = Inbox::parse(&text);
        assert_eq!(inbox.undelivered().len(), 1);
        assert_eq!(inbox.undelivered()[0].id, "w1");
    }

    #[test]
    fn a_newer_item_with_the_same_key_supersedes_an_unsent_one_but_not_its_report() {
        let blocked = |id: &str| Item {
            text: "[corgi] w-forecast is blocked.".into(),
            ..wake("w-forecast", id)
        };
        let items = [wake("w-forecast", "a"), wake("w-radar", "b"), blocked("c")];
        let refs: Vec<&Item> = items.iter().collect();
        assert_eq!(
            prompt_text(&refs),
            "[corgi] w-radar is done. Run: corgi report w-radar\n[corgi] w-forecast is blocked."
        );
        // A stop with a report stays when a later one without comes, and
        // what came before it goes.
        let reported = |id: &str| Item {
            report: Some("done it".into()),
            ..wake("w-forecast", id)
        };
        let items = [reported("a"), reported("b"), blocked("c"), blocked("d")];
        let refs: Vec<&Item> = items.iter().collect();
        let ids: Vec<&str> = coalesce(&refs)
            .iter()
            .map(|item| item.id.as_str())
            .collect();
        assert_eq!(ids, ["b", "d"]);
    }

    #[test]
    fn only_an_own_agents_short_report_is_typed_quoted_with_its_wake() {
        let dir = ScratchDir::new("inbox-report");
        let short = "### Report\n\n[corgi] End of w-forecast's report.\n- Result: done".to_string();
        let own = append(
            &dir,
            Item {
                own: true,
                report: Some(short.clone()),
                ..wake("w-forecast", "a")
            },
        )
        .unwrap()
        .unwrap();
        // Quoted, a report cannot pass for the dashboard's own lines.
        assert_eq!(
            prompt_text(&[&own]),
            "[corgi] w-forecast is done. Its report follows, quoted, so you need not run \
             report for it:\n> ### Report\n>\n> [corgi] End of w-forecast's report.\n\
             > - Result: done\n[corgi] End of w-forecast's report."
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
        assert_eq!(prompt_text(&[&other]), other.text);
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
        assert_eq!(prompt_text(&[&big]), big.text);
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
    fn one_prompt_carries_reports_only_within_its_budget() {
        let items: Vec<Item> = (0..6)
            .map(|n| Item {
                own: true,
                report: Some("r".repeat(INLINE_REPORT_MAX - 2)),
                ..wake(&format!("w-{n}"), &n.to_string())
            })
            .collect();
        let refs: Vec<&Item> = items.iter().collect();
        let text = prompt_text(&refs);
        let inline = PROMPT_REPORT_BUDGET / INLINE_REPORT_MAX;
        assert_eq!(text.matches("Its report follows").count(), inline);
        assert_eq!(text.matches("Run: corgi report").count(), 6 - inline);
        assert!(text.len() <= PROMPT_REPORT_BUDGET + 2_048, "{}", text.len());
    }

    #[test]
    fn items_of_another_project_of_the_same_name_are_not_this_ones() {
        let dir = ScratchDir::new("inbox-projects");
        append(&dir, wake("w-forecast", "a")).unwrap();
        append(
            &dir,
            Item {
                project: "/oss/weather".into(),
                ..wake("w-oss", "b")
            },
        )
        .unwrap();
        let inbox = Inbox::read(&dir);
        let ids = |root| {
            inbox
                .undelivered_for(root)
                .iter()
                .map(|item| item.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("/repos/weather"), ["a"]);
        assert_eq!(ids("/oss/weather"), ["b"]);
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
    fn the_footer_counts_missed_items_only() {
        let dir = ScratchDir::new("inbox-footer");
        let root = "/repos/weather";
        let now = 1_000_000;
        let at = |secs_ago: u64| Item {
            ts: rfc3339_utc(now - secs_ago),
            ..wake("w-forecast", &format!("w{secs_ago}"))
        };
        let footer = |delivering| footer(&dir, "/opt/corgi", root, delivering, now);
        assert_eq!(footer(false), None);
        append(&dir, at(10)).unwrap();
        // While a dashboard delivers, a fresh one only waits for the supervisor's
        // turn; without one, it is missed.
        assert_eq!(footer(true), None);
        assert_eq!(
            footer(false).as_deref(),
            Some("1 undelivered inbox item: run /opt/corgi inbox /repos/weather")
        );
        append(&dir, at(FOOTER_AFTER_SECS)).unwrap();
        assert_eq!(
            footer(true).as_deref(),
            Some("1 undelivered inbox item: run /opt/corgi inbox /repos/weather")
        );
        assert_eq!(
            footer(false).as_deref(),
            Some("2 undelivered inbox items: run /opt/corgi inbox /repos/weather")
        );
        // Another project's items are not counted.
        assert_eq!(
            super::footer(&dir, "corgi", "/oss/weather", false, now),
            None
        );
        let ids = ["w10".into(), format!("w{FOOTER_AFTER_SECS}")];
        mark_delivered(&dir, &ids, "corgi-weather").unwrap();
        assert_eq!(footer(false), None);
    }

    /// A write cut short (the dashboard or `corgi notify` killed mid-write,
    /// a full disk) leaves a last line without its newline. The next append
    /// continues that line, so the item it adds never parses, though
    /// `append` reported it added.
    #[test]
    fn an_item_appended_after_a_torn_line_is_kept() {
        let dir = ScratchDir::new("review-torn");
        append(&dir, wake("w-forecast", "w1")).unwrap().unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(dir.join(INBOX_FILE))
            .unwrap();
        file.write_all(br#"{"id":"w2","ts":"2026-10-03T07:0"#)
            .unwrap();
        drop(file);
        assert!(append(&dir, wake("w-radar", "w3")).unwrap().is_some());
        let ids: Vec<String> = Inbox::read(&dir)
            .undelivered()
            .iter()
            .map(|item| item.id.clone())
            .collect();
        assert_eq!(ids, ["w1", "w3"]);
    }
}
