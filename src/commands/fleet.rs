//! `corgi fleet`, `corgi tag` and `corgi report`: what a supervisor reads of
//! its project's agents, and the merge tag it sets on one.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::{
    hc::{self, HcClient},
    inbox::{self, Inbox, Item},
    model::{Agent, PromptCache, PromptCacheKind, cache_countdown, compact_tokens},
    paths::corgi_state_dir,
    session::SessionReader,
    tag::TagOptions,
    time::unix_now,
};

use super::{existing_project_dir, print_inbox_footer, project_root, project_root_of};

/// Corgi's agents on hc, and the client that read them.
pub(super) struct Seen {
    pub(super) client: HcClient,
    pub(super) agents: Vec<Agent>,
}

/// What a command run here sees of hc: inside one of its terminals, that
/// terminal's core; anywhere else, the core that already runs, since asking
/// a stopped one would start it.
pub(super) fn seen() -> Result<Seen> {
    anyhow::ensure!(
        hc::in_terminal() || hc::core_running(),
        "herdr-core (hc) is not running; run this inside one of its terminals, or start it"
    );
    let client = HcClient::from_env()?;
    let agents = hc::agents::agents(&client)?;
    Ok(Seen { client, agents })
}

impl Seen {
    /// The running agent named or addressed `target`.
    pub(super) fn find(&self, target: &str) -> Option<&Agent> {
        self.agents.iter().find(|agent| {
            agent.info.name.as_deref() == Some(target) || agent.info.address == target
        })
    }

    /// `corgi tag` on the agent `options` names, of the project at `root`.
    pub(super) fn tag(&self, root: &str, options: &TagOptions) -> Result<String> {
        hc::agents::tag(&self.client, &self.agents, root, options)
    }
}

pub const FLEET_USAGE: &str = "\
Usage: corgi fleet [PROJECT]

Lists the agents of PROJECT (default: the project containing the current
directory), one tab-separated row each under a header line:
NAME, ROLE (supervisor or worker), STATE, TASK, MODEL, CTX, CACHE, CWD and TAG.
CACHE is the agent's prompt cache: warm 42m while it holds, with the time left;
cold 180k once it has lapsed, with the context tokens the next prompt re-reads
at the full input price; warm? and cold? when the lifetime is only estimated
(Codex), so the cache may outlive it; - when unknown. TAG is merge for an agent
a supervisor tagged ready to merge (corgi tag) that still rests where it was
tagged, else -. When the project's supervisor has missed inbox items, a line
on stderr says so.";

/// `corgi fleet`: one tab-separated row per agent of a project (default: the
/// project containing the current directory), for a supervisor to read.
pub fn fleet_command(args: &[String]) -> Result<()> {
    anyhow::ensure!(args.len() <= 1, "Usage: corgi fleet [PROJECT]");
    let project = existing_project_dir(args.first().map(String::as_str))?;
    let seen = seen()?;
    let root = project_root_of(&project);
    println!("NAME\tROLE\tSTATE\tTASK\tMODEL\tCTX\tCACHE\tCWD\tTAG");
    for row in fleet_rows(&seen.agents, &root) {
        println!("{row}");
    }
    print_inbox_footer(&root);
    Ok(())
}

/// The rows `corgi fleet` prints for those of `agents` in the project at
/// `root`.
fn fleet_rows(agents: &[Agent], root: &str) -> Vec<String> {
    let field = |text: &str| text.replace(['\t', '\n'], " ");
    let now = unix_now();
    agents
        .iter()
        .filter(|agent| agent.project_root == root)
        .map(|agent| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                field(agent.info.display_name()),
                if agent.supervisor {
                    "supervisor"
                } else {
                    "worker"
                },
                agent.info.state.label().to_lowercase(),
                field(&agent.task),
                field(agent.model.as_deref().unwrap_or("-")),
                agent
                    .context_percent
                    .map_or_else(|| "-".to_string(), |percent| format!("{percent}%")),
                agent
                    .cache
                    .map_or_else(|| "-".to_string(), |cache| fleet_cache(&cache, now)),
                field(agent.info.cwd()),
                if agent.ready_to_merge() { "merge" } else { "-" },
            )
        })
        .collect()
}

/// The CACHE column of `corgi fleet`: the prompt cache in plain words for an
/// agent to read. A `?` marks an estimated lifetime, which the
/// cache may outlive, so neither warm nor cold is certain.
fn fleet_cache(cache: &PromptCache, now: u64) -> String {
    let mark = if cache.kind == PromptCacheKind::Estimated {
        "?"
    } else {
        ""
    };
    match cache.remaining(now) {
        0 => format!("cold{mark} {}", compact_tokens(cache.tokens)),
        remaining => format!("warm{mark} {}", cache_countdown(remaining)),
    }
}

pub const TAG_USAGE: &str = "\
Usage: corgi tag NAME merge [--project PATH]
       corgi tag NAME --clear [--project PATH]

Tags the agent NAME (its name or its terminal's address) of PROJECT (default:
the project containing the current directory) ready for the user to merge, or
clears its tag. The Herdr app's Projects list then shows the agent, while it
rests, with a MERGE mark instead of DONE. A tag is refused for an agent that is not done
or idle, or whose checkout has no commits ahead of the branch checked out in
the project's primary checkout. The tag goes away by itself when the user
merges the branch in Corgi, or once the agent works again.

Options:
  --project PATH  Project directory (default: the current directory)
  --clear         Remove the tag";

fn parse_tag_options(args: &[String]) -> Result<TagOptions> {
    let mut name = None;
    let mut tag = None;
    let mut clear = false;
    let mut project = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--project" => project = Some(args.next().context("--project needs a value")?.clone()),
            "--clear" => clear = true,
            other if other.starts_with('-') => bail!("unknown option {other:?}\n\n{TAG_USAGE}"),
            _ if name.is_none() => name = Some(arg.clone()),
            _ if tag.is_none() => tag = Some(arg.clone()),
            _ => bail!("{TAG_USAGE}"),
        }
    }
    let name = name.with_context(|| format!("name the agent to tag\n\n{TAG_USAGE}"))?;
    match (tag.as_deref(), clear) {
        (Some("merge"), false) => {}
        (None, true) => {}
        (Some(tag), false) => bail!("merge is the only tag, not {tag:?}\n\n{TAG_USAGE}"),
        _ => bail!("give the merge tag or --clear, not both or neither\n\n{TAG_USAGE}"),
    }
    Ok(TagOptions {
        name,
        merge: !clear,
        project,
    })
}

/// `corgi tag`: a supervisor tags one of its project's agents ready for the user
/// to merge, or clears the tag, for the Projects list to show.
pub fn tag_command(args: &[String]) -> Result<()> {
    let options = parse_tag_options(args)?;
    let project = existing_project_dir(options.project.as_deref())?;
    let seen = seen()?;
    let root = project_root_of(&project);
    println!("{}", seen.tag(&root, &options)?);
    Ok(())
}

pub const REPORT_USAGE: &str = "\
Usage: corgi report NAME

Prints, in full, the report the agent NAME (its name or its terminal's
address) left. For a running agent: the one corgi daemon kept in its
supervisor's inbox when the agent last stopped, if that is its current state,
else the newest thing it said, which for a worker that followed its brief is
its report, or its screen when its harness writes no readable transcript. For
an agent whose terminal is gone: the newest report kept in any supervisor's
inbox.
When the supervisor of the current directory's project has missed inbox items, a
line on stderr says so.";

/// `corgi report NAME`: the report an agent left, which for a worker that
/// followed its brief is its report.
pub fn report_command(args: &[String]) -> Result<()> {
    let [target] = args else {
        bail!("Usage: corgi report NAME");
    };
    let seen = seen().ok();
    let running = seen
        .as_ref()
        .and_then(|seen| Some((seen, seen.find(target)?)));
    let printed = match running {
        Some((seen, agent)) => print_running_report(&seen.agents, &seen.client, agent, target),
        None => inbox_report(target)?
            .with_context(|| format!("no running agent named {target}, and no report kept for it"))
            .and_then(|(dir, item)| print_kept_report(target, &dir, &item)),
    };
    if let Ok(root) = project_root(None) {
        print_inbox_footer(&root);
    }
    printed
}

/// Prints the report of the running `agent`: the one kept in its own
/// project's inbox when it is for this terminal and its current state change,
/// so it is never another agent's of the same name or one it has since
/// said more than; else the newest thing it said, or its screen.
fn print_running_report(
    agents: &[Agent],
    client: &HcClient,
    agent: &Agent,
    target: &str,
) -> Result<()> {
    let info = &agent.info;
    let name = info.name.as_deref().unwrap_or(target);
    let dir = crate::supervisor::state_dir(&agent.project_root)?;
    let saved = Inbox::try_read(&dir)?;
    let kept = current_report(&saved, agent, name)
        .cloned()
        .map(|item| (dir, item));
    if let Some((dir, item)) = kept {
        print_kept_report(name, &dir, &item)?;
    } else if let Some(entry) = SessionReader::among(agents.iter().map(|agent| &agent.info))
        .facts(info)
        .report
    {
        println!("{}", entry.text);
    } else {
        eprintln!("(no readable transcript for {target}; showing its screen)");
        println!("{}", client.read(&info.address, 120)?.trim_end());
    }
    Ok(())
}

/// The report `inbox` keeps of the running `agent`, named `name`, if it is
/// of its project and terminal and of its current state change.
fn current_report<'a>(inbox: &'a Inbox, agent: &Agent, name: &str) -> Option<&'a Item> {
    let info = &agent.info;
    inbox.newest_report(name).filter(|item| {
        item.is_for(&agent.project_root)
            && item.pane.as_deref() == Some(info.address.as_str())
            && item.change == Some(info.state_change_seq)
    })
}

/// Prints the report `item` of the inbox in the state directory `dir` kept
/// of the agent `name`.
fn print_kept_report(name: &str, dir: &std::path::Path, item: &Item) -> Result<()> {
    let state = item.state.as_deref().unwrap_or("stopped");
    eprintln!(
        "(the report {name} left when it was {state}, at {})",
        item.ts
    );
    let text = item.try_report_text(dir)?.unwrap_or_default();
    println!("{}", text.trim_end());
    Ok(())
}

/// The newest report of the agent `name` kept in any supervisor's inbox, with
/// the state directory it is in, for an agent whose terminal is gone.
fn inbox_report(name: &str) -> Result<Option<(PathBuf, Item)>> {
    let Some(base) = corgi_state_dir() else {
        return Ok(None);
    };
    let mut reports = Vec::new();
    for dir in inbox::all_state_dirs(&base) {
        if let Some(item) = Inbox::try_read(&dir)?.newest_report(name).cloned() {
            reports.push((dir, item));
        }
    }
    Ok(reports.into_iter().max_by(|(_, a), (_, b)| a.ts.cmp(&b.ts)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fleet_shows_whether_a_prompt_cache_is_warm_or_what_it_costs_cold() {
        const NOW: u64 = 1_800_000_000;
        let cache = |expires_at: u64, kind: PromptCacheKind| PromptCache {
            expires_at,
            tokens: 180_000,
            kind,
        };
        let row = |cache: Option<PromptCache>| {
            cache.map_or_else(|| "-".to_string(), |cache| fleet_cache(&cache, NOW))
        };
        use PromptCacheKind::{Estimated, Exact};

        assert_eq!(row(Some(cache(NOW + 42 * 60, Exact))), "warm 42m");
        assert_eq!(row(Some(cache(NOW + 30, Exact))), "warm 30s");
        assert_eq!(row(Some(cache(NOW, Exact))), "cold 180k");
        assert_eq!(row(Some(cache(NOW + 2 * 3_600, Estimated))), "warm? 2h");
        assert_eq!(row(Some(cache(NOW - 1, Estimated))), "cold? 180k");
        assert_eq!(row(None), "-");
    }

    #[test]
    fn tag_takes_a_name_and_merge_or_clear() {
        let args = |line: &str| {
            line.split_whitespace()
                .map(String::from)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            parse_tag_options(&args("w-brave merge --project /repos/corgi")).unwrap(),
            TagOptions {
                name: "w-brave".into(),
                merge: true,
                project: Some("/repos/corgi".into()),
            }
        );
        assert_eq!(
            parse_tag_options(&args("w-brave --clear")).unwrap(),
            TagOptions {
                name: "w-brave".into(),
                merge: false,
                project: None,
            }
        );
        for wrong in [
            "",
            "w-brave",
            "w-brave merge --clear",
            "w-brave ready",
            "w-brave merge x",
        ] {
            assert!(parse_tag_options(&args(wrong)).is_err(), "{wrong:?}");
        }
    }

    #[test]
    fn a_running_agent_gets_its_own_current_report_only() {
        let kept = |project: &str, pane: &str, change: u64| Item {
            id: format!("{project}-{pane}-{change}"),
            project: project.into(),
            agent: Some("w-docs".into()),
            pane: Some(pane.into()),
            change: Some(change),
            report: Some("the report".into()),
            ..Item::default()
        };
        let agent = crate::model::Agent {
            info: crate::model::AgentInfo {
                name: Some("w-docs".into()),
                address: "w2:p1".into(),
                state_change_seq: 7,
                ..Default::default()
            },
            project_root: "/repos/web".into(),
            ..Default::default()
        };
        let found = |item: Item| {
            let dir = crate::test_support::ScratchDir::new("cli-current-report");
            inbox::append(&dir, item).unwrap();
            current_report(&Inbox::read(&dir), &agent, "w-docs").is_some()
        };
        assert!(found(kept("/repos/web", "w2:p1", 7)));
        // Another project's agent of the same name, another pane, or a
        // stop it has since worked past: its transcript says more.
        assert!(!found(kept("/oss/web", "w2:p1", 7)));
        assert!(!found(kept("/repos/web", "w9:p1", 7)));
        assert!(!found(kept("/repos/web", "w2:p1", 5)));
    }
}
