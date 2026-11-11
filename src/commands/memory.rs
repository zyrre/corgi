//! `corgi digest`, `corgi inbox`, `corgi notify` and `corgi role`: what a
//! supervisor reads of its own state directory, and how a line gets into
//! its inbox.

use std::{env, fs, io, path::Path};

use anyhow::{Context, Result, bail};

use crate::{
    digest::{
        self,
        memory::{DigestLookup, corgi_bin_text, digest_lookup, markdown_files},
    },
    git::{git_current_branch, git_output},
    inbox::{self, Inbox, Item, Kind},
    launch::CORGI_REQUEST_TOKEN,
    model::Agent,
    supervisor::{OLD_WAKE_PREFIX, WAKE_PREFIX},
};

use super::{fleet::seen, launch::read_task, print_inbox_footer, project_root};

pub const DIGEST_USAGE: &str = "\
Usage: corgi digest [PROJECT] [--decision WORDS | --search WORDS | --show FILE[:LINE]
                               | --ledger ID | --brief ID]

Prints a bounded digest of the memory of PROJECT's supervisor (default: the
project containing the current directory), for the start of a supervisor's
session: the base branch, the handover note if there is one (else the open
threads of the newest archived one), open ledger work joined with the
running agents (about 4 KB; `reported` work whose agent is gone and whose
newest line is 2+ days old is counted on one line), recently finished work,
the newest decisions in full (about 8 KB, at least 3) and the titles of
older ones (about 3 KB, and a line counting the rest). Decisions named by a later entry's `Supersedes:` line are left
out, and unreadable ledger lines are warned about. Reads only; it never
changes the state directory. When the supervisor has missed inbox items, a
line on stderr says so.

Options:
  --decision WORDS   Print in full every decision, superseded or not, whose
                     heading contains all of WORDS (ignoring case); when no
                     heading does, those whose text has them all; when none
                     does, search all of the memory as --search does
  --search WORDS     Search all of the memory: decisions (superseded ones
                     marked), the ledger and ledger-archive.jsonl, briefs,
                     kept reports (reports/), notes (notes/), handover.md
                     and archived handover notes. A word matches the start of
                     a word, ignoring case. Hits with WORDS as a phrase come
                     first, then those with the most of them, then the
                     newest; near-duplicates fold into one, which names the
                     others. Each hit is a file:line, its date and the lines
                     around it, about 7 KB at most
  --show FILE[:LINE] Print whole the block of the state file FILE (relative
                     to the state directory, as --search names it) that
                     holds LINE: the decision entry, the ledger line with its
                     id's history, the brief, the report, or the section of
                     any other Markdown file under its nearest heading.
                     Without LINE, the whole file
  --ledger ID        Print the newest ledger line of the id or agent name
                     ID, its history (ledger-archive.jsonl first) and its
                     brief and kept reports by name
  --brief ID         Print a brief whole, by its id (YYYYMMDD-<worker>) or
                     its worker's name (the newest one)";

/// `corgi digest`: a bounded view of the state of a project's supervisor, joined
/// with its running agents, for a supervisor to read at the start of its session.
pub fn digest_command(args: &[String]) -> Result<()> {
    let mut project = None;
    let mut lookup = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let make: fn(String) -> DigestLookup = match arg.as_str() {
            "--decision" => DigestLookup::Decision,
            "--search" => DigestLookup::Search,
            "--show" => DigestLookup::Show,
            "--ledger" => DigestLookup::Ledger,
            "--brief" => DigestLookup::Brief,
            other if other.starts_with('-') => bail!("unknown option {other:?}\n\n{DIGEST_USAGE}"),
            _ if project.is_none() => {
                project = Some(arg.as_str());
                continue;
            }
            _ => bail!("{DIGEST_USAGE}"),
        };
        let value = args
            .next()
            .filter(|value| !value.trim().is_empty())
            .with_context(|| format!("{arg} needs a value"))?;
        anyhow::ensure!(
            lookup.is_none(),
            "use one of --decision, --search, --show, --ledger and --brief at a time"
        );
        lookup = Some(make(value.clone()));
    }
    let root = project_root(project)?;
    let state_dir = crate::supervisor::state_dir(&root)?;
    // A lookup reads the state directory alone, so it needs no running core.
    if let Some(lookup) = lookup {
        print!("{}", digest_lookup(&state_dir, &root, lookup)?);
        return Ok(());
    }

    let seen = seen()?;
    print!("{}", digest_summary(&root, &state_dir, &seen.agents));
    print_inbox_footer(&root);
    Ok(())
}

/// `corgi digest`'s summary of the memory of `root`'s supervisor in
/// `state_dir`, with `agents` as the project's running fleet.
fn digest_summary(root: &str, state_dir: &Path, agents: &[Agent]) -> String {
    let read = |name: &str| fs::read_to_string(state_dir.join(name)).ok();
    let decisions = read("decisions.md").unwrap_or_default();
    let state = state_dir.to_string_lossy();
    let handovers = markdown_files(&state_dir.join(crate::supervisor::HANDOVERS_DIR));
    let root_path = Path::new(root);
    let branch = git_current_branch(root_path).unwrap_or_else(|_| "(detached)".to_string());
    let head = git_output(root_path, &["rev-parse", "--short", "HEAD"])
        .map(|head| head.trim().to_string())
        .unwrap_or_else(|_| "(no commit)".to_string());
    let fleet: Vec<_> = agents
        .iter()
        .filter(|agent| agent.project_root == root)
        .map(|agent| digest::LiveAgent {
            name: agent.info.display_name().to_string(),
            request: agent.info.tokens.get(CORGI_REQUEST_TOKEN).cloned(),
            state: agent.info.state.label().to_lowercase(),
            context_percent: agent.context_percent,
        })
        .collect();
    let corgi_bin = corgi_bin_text();
    let decision_command = format!("{corgi_bin} digest {root} --decision \"<words>\"");
    let search_command = format!("{corgi_bin} digest {root} --search \"<words>\"");
    let handover = read(crate::supervisor::HANDOVER_NOTE);
    // Names are UTC stamps, so the last sorts newest.
    let previous_handover = handovers
        .last()
        .map(|(name, text)| (name.as_str(), text.as_str()));
    let ledger = read(digest::lookup::LEDGER_FILE).unwrap_or_default();
    digest::render(&digest::DigestInput {
        project: root,
        branch: &branch,
        head: &head,
        state_dir: &state,
        handover: handover.as_deref(),
        previous_handover,
        decisions: &decisions,
        ledger: &ledger,
        fleet: &fleet,
        now: crate::time::unix_now(),
        decision_command: &decision_command,
        search_command: &search_command,
    })
}

pub const INBOX_USAGE: &str = "\
Usage: corgi inbox [PROJECT] [--delivered] [--take]

Prints the undelivered items in the inbox of the supervisor of PROJECT (default:
the project containing the current directory), oldest first: what corgi
daemon has not typed into the supervisor's input box yet, each with its report
or where to find it. Printing marks nothing delivered, so the daemon still
types them in. A supervisor handing over leaves them to its successor.

Options:
  --delivered  Also print the newest delivered items
  --take       Record the printed undelivered items delivered, so the
               daemon does not type them in again: for the supervisor, which
               acts on them now";

/// The most undelivered items `corgi inbox` prints, the newest.
const INBOX_SHOWN: usize = 20;

/// The most delivered items `corgi inbox --delivered` prints, the newest.
const INBOX_DELIVERED_SHOWN: usize = 10;

/// The report text `corgi inbox` prints in all, in bytes; past it, each
/// report is only pointed to.
const INBOX_REPORT_BUDGET: usize = 16 * 1024;

/// `corgi inbox`: a bounded view of a supervisor's inbox.
pub fn inbox_command(args: &[String]) -> Result<()> {
    let mut project = None;
    let (mut delivered, mut take) = (false, false);
    for arg in args {
        match arg.as_str() {
            "--delivered" => delivered = true,
            "--take" => take = true,
            other if other.starts_with('-') => bail!("unknown option {other:?}\n\n{INBOX_USAGE}"),
            _ if project.is_none() => project = Some(arg.as_str()),
            _ => bail!("{INBOX_USAGE}"),
        }
    }
    let root = project_root(project)?;
    let dir = crate::supervisor::state_dir(&root)?;
    let inbox = Inbox::try_read(&dir)?;
    let (text, shown) = inbox_text(&inbox, &dir, &root, delivered);
    print!("{text}");
    if take {
        let ids: Vec<String> = shown.iter().map(|item| item.id.clone()).collect();
        inbox::mark_delivered(&dir, &ids, "corgi inbox --take")?;
        if !ids.is_empty() {
            println!("Recorded {} item(s) delivered.", ids.len());
        }
    }
    Ok(())
}

/// What `corgi inbox` prints of `inbox`, in the state directory `dir`, for
/// the project at `root`, and the undelivered items it shows.
fn inbox_text<'a>(
    inbox: &'a Inbox,
    dir: &std::path::Path,
    root: &str,
    delivered: bool,
) -> (String, Vec<&'a Item>) {
    let undelivered = inbox.undelivered_for(root);
    let shown: Vec<&Item> = undelivered
        .iter()
        .skip(undelivered.len().saturating_sub(INBOX_SHOWN))
        .copied()
        .collect();
    let mut out = format!(
        "{} undelivered item(s) in {}\n",
        undelivered.len(),
        dir.join(inbox::INBOX_FILE).display()
    );
    if undelivered.len() > shown.len() {
        out.push_str(&format!(
            "(the {} oldest are not shown)\n",
            undelivered.len() - shown.len()
        ));
    }
    let mut budget = INBOX_REPORT_BUDGET;
    let corgi_bin = corgi_bin_text();
    for item in &shown {
        out.push_str(&inbox_entry(item, &corgi_bin, Some(&mut budget)));
    }
    if delivered {
        let mut done = inbox.delivered();
        done.retain(|item| item.is_for(root));
        out.push_str(&format!("\nNewest delivered ({} in all):\n", done.len()));
        for item in done
            .iter()
            .skip(done.len().saturating_sub(INBOX_DELIVERED_SHOWN))
        {
            out.push_str(&inbox_entry(item, &corgi_bin, None));
        }
    }
    (out, shown)
}

/// One item as `corgi inbox` prints it: a heading, its line, and its report
/// while `budget` lasts, else where the report is.
fn inbox_entry(item: &Item, corgi_bin: &str, budget: Option<&mut usize>) -> String {
    let about = [item.agent.as_deref(), item.state.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = format!(
        "\n--- {} {} {about} (from {}, id {})\n{}\n",
        item.ts,
        item.kind.label(),
        item.source,
        item.id,
        item.text
    );
    let pointer = |out: &mut String| {
        if let Some(agent) = &item.agent {
            out.push_str(&format!("(report: run {corgi_bin} report {agent})\n"));
        }
    };
    match (budget, &item.report) {
        (Some(budget), Some(report)) if report.len() <= *budget => {
            *budget -= report.len();
            out.push_str(&format!("{}\n", report.trim_end()));
        }
        _ if item.has_report() => pointer(&mut out),
        _ => {}
    }
    out
}

pub const NOTIFY_USAGE: &str = "\
Usage: corgi notify [PROJECT] [OPTIONS] < text

Adds an item to the inbox of the supervisor of PROJECT (default: the project
containing the current directory), the one way anything but Corgi itself
reaches a supervisor. corgi daemon types it into the supervisor's input box
once the supervisor is between turns and its box holds no draft, together
with any other undelivered items, and records it delivered; it survives the
daemon stopping or restarting meanwhile. The
text is read from stdin, and gets a leading [Corgi] when it has none (a
leading [corgi], as Corgi wrote it before, becomes [Corgi]). Prints the item's
id and inbox as JSON.

Options:
  --agent NAME        The agent the item is about
  --state STATE       That agent's state, such as done or blocked
  --kind KIND         wake, conflict, merge or note (default: note)
  --key KEY           Of undelivered items with the same key, only the
                      newest goes out (default: the item's own id)
  --source NAME       Who adds it (default: notify)
  --report-file PATH  The agent's report, kept with the item, which
                      corgi report NAME then prints";

/// `corgi notify`: adds an item to a supervisor's inbox through the same code
/// the daemon uses, for scripts.
pub fn notify_command(args: &[String]) -> Result<()> {
    let mut project = None;
    let mut item = Item {
        source: "notify".into(),
        kind: Kind::Note,
        ..Item::default()
    };
    let mut report_file = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .with_context(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--agent" => item.agent = Some(value()?),
            "--state" => item.state = Some(value()?.trim().to_lowercase()),
            "--kind" => {
                let kind = value()?;
                item.kind = Kind::from_label(&kind).with_context(|| {
                    format!("--kind must be wake, conflict, merge or note, not {kind:?}")
                })?;
            }
            "--key" => item.key = value()?,
            "--source" => item.source = value()?,
            "--report-file" => report_file = Some(value()?),
            other if other.starts_with('-') => bail!("unknown option {other:?}\n\n{NOTIFY_USAGE}"),
            _ if project.is_none() => project = Some(arg.as_str()),
            _ => bail!("{NOTIFY_USAGE}"),
        }
    }
    let text = read_task(None, true)?;
    anyhow::ensure!(!text.is_empty(), "the text is empty\n\n{NOTIFY_USAGE}");
    item.text = notify_text(&text);
    if let Some(path) = report_file {
        item.report =
            Some(fs::read_to_string(&path).with_context(|| format!("read report file {path}"))?);
    }
    item.project = project_root(project)?;
    let dir = crate::supervisor::state_dir(&item.project)?;
    let item = inbox::append(&dir, item)?.context("the inbox already has an item with that id")?;
    serde_json::to_writer(
        io::stdout(),
        &serde_json::json!({"id": item.id, "inbox": dir.join(inbox::INBOX_FILE)}),
    )?;
    println!();
    Ok(())
}

/// `text` as a line a supervisor tells from the user's: it starts with
/// [`WAKE_PREFIX`], which replaces the `[corgi]` Corgi wrote before the
/// supervisor was given its name, so that a script written for it still
/// reads as Corgi's.
fn notify_text(text: &str) -> String {
    if text.starts_with(WAKE_PREFIX) {
        text.to_string()
    } else if let Some(rest) = text.strip_prefix(OLD_WAKE_PREFIX) {
        format!("{WAKE_PREFIX}{rest}")
    } else {
        format!("{WAKE_PREFIX} {text}")
    }
}

pub const ROLE_USAGE: &str = "\
Usage: corgi role handover [PROJECT]

Prints the part of the supervisor's role it reads only when it needs it.
handover: how a supervisor hands over to a fresh session when Corgi asks it
to, with the paths of the state directory of PROJECT's
supervisor (default: the project containing the current directory) and of
this Corgi binary filled in. Reads only.";

/// `corgi role handover [PROJECT]`: the project, if one is named.
fn role_args(args: &[String]) -> Result<Option<&str>> {
    match args {
        [part, rest @ ..] if part == "handover" => match rest {
            [] => Ok(None),
            [option] if option.starts_with('-') => {
                bail!("unknown option {option:?}\n\n{ROLE_USAGE}")
            }
            [project] => Ok(Some(project.as_str())),
            _ => bail!("{ROLE_USAGE}"),
        },
        _ => bail!("{ROLE_USAGE}"),
    }
}

/// `corgi role`: a part of the supervisor's role, printed on demand.
pub fn role_command(args: &[String]) -> Result<()> {
    let project = role_args(args)?;
    let root = project_root(project)?;
    let state_dir = crate::supervisor::state_dir(&root)?;
    let corgi_bin = env::current_exe().context("find the Corgi binary")?;
    print!(
        "{}",
        crate::supervisor::handover_instructions(&corgi_bin, &state_dir)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corgi_role_prints_only_the_handover_part_for_at_most_one_project() {
        let args = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(role_args(&args(&["handover"])).unwrap(), None);
        assert_eq!(
            role_args(&args(&["handover", "/repos/weather"])).unwrap(),
            Some("/repos/weather")
        );
        for wrong in [
            &[][..],
            &["merge"],
            &["handover", "--all"],
            &["handover", "/a", "/b"],
        ] {
            let error = role_args(&args(wrong)).expect_err("refused");
            assert!(error.to_string().contains(ROLE_USAGE), "{wrong:?}");
        }
    }

    #[test]
    fn a_notified_line_is_marked_as_corgis() {
        assert_eq!(notify_text("deploy done"), "[Corgi] deploy done");
        assert_eq!(notify_text("[Corgi] deploy done"), "[Corgi] deploy done");
        // A script that adds the prefix Corgi used before the rename.
        assert_eq!(notify_text("[corgi] deploy done"), "[Corgi] deploy done");
    }

    #[test]
    fn the_inbox_shows_undelivered_items_with_their_reports_and_marks_nothing() {
        let dir = crate::test_support::ScratchDir::new("cli-inbox");
        let wake = |agent: &str, id: &str, report: Option<String>| Item {
            id: id.into(),
            source: "daemon".into(),
            agent: Some(agent.into()),
            state: Some("done".into()),
            key: agent.into(),
            text: format!("[Corgi] {agent} is done."),
            own: true,
            report,
            ..Item::default()
        };
        inbox::append(&dir, wake("w-old", "a", Some("old report".into()))).unwrap();
        inbox::mark_delivered(&dir, &["a".into()], "supervisor-weather").unwrap();
        inbox::append(
            &dir,
            wake("w-forecast", "b", Some("forecast report".into())),
        )
        .unwrap();
        let long = "x".repeat(inbox::INLINE_REPORT_MAX + 1);
        inbox::append(&dir, wake("w-radar", "c", Some(long))).unwrap();
        let read = Inbox::read(&dir);
        let (text, shown) = inbox_text(&read, &dir, "", false);
        assert!(text.starts_with("2 undelivered item(s) in "), "{text}");
        assert!(
            text.contains("[Corgi] w-forecast is done.\nforecast report\n"),
            "{text}"
        );
        assert!(
            text.contains("[Corgi] w-radar is done.\n(report: run "),
            "{text}"
        );
        assert!(!text.contains("old report"), "{text}");
        assert_eq!(shown.len(), 2);
        let (text, _) = inbox_text(&read, &dir, "", true);
        assert!(text.contains("Newest delivered (1 in all):"), "{text}");
        assert!(
            text.contains("[Corgi] w-old is done.\n(report: run "),
            "{text}"
        );
        // Printing marked nothing.
        assert_eq!(Inbox::read(&dir).undelivered().len(), 2);
    }

    #[test]
    fn the_inbox_prints_reports_only_within_its_budget() {
        let dir = crate::test_support::ScratchDir::new("cli-inbox-budget");
        for n in 0..8 {
            inbox::append(
                &dir,
                Item {
                    agent: Some(format!("w-{n}")),
                    text: format!("[Corgi] w-{n} is done."),
                    report: Some("r".repeat(inbox::INLINE_REPORT_MAX)),
                    ..Item::default()
                },
            )
            .unwrap();
        }
        let read = Inbox::read(&dir);
        let (text, shown) = inbox_text(&read, &dir, "", false);
        assert_eq!(shown.len(), 8);
        assert!(text.len() < INBOX_REPORT_BUDGET + 4_096, "{}", text.len());
        assert_eq!(text.matches("(report: run ").count(), 4);
    }
}
