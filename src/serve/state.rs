//! What `corgi serve` sends: the dashboard's rows, cards, questions and
//! colors turned into the protocol's data, and the changes between two
//! states as a delta's.

use std::{collections::HashMap, env, fs};

use corgi_core::{
    digest::{all_open_questions, lookup::LEDGER_FILE},
    model::{self, AgentState, DashboardAgent, PromptCacheKind},
    palette::{self, Palette, Rgb, Source, Theme},
    protocol::{
        Activity, ActivityKind, Agent, Blocked, Cache, Change, PaletteSet, Project, Question,
        State, ThemeColors, WorkerCounts,
    },
    waker::Inboxes,
};

use crate::app::{is_card, project_runs};

/// What a blocked worker waits on when its screen shows no question, as a
/// collapsed card says it.
pub(super) const WAITING_FOR_INPUT: &str = "waiting for your input";

/// `agent`'s row.
pub(super) fn agent_row(agent: &DashboardAgent) -> Agent {
    let info = &agent.info;
    Agent {
        pane_id: info.pane_id.clone(),
        workspace_id: info.workspace_id.clone(),
        tab_id: info.tab_id.clone(),
        name: info.display_name().to_string(),
        herdr_name: info.name.clone(),
        harness: info.kind().to_string(),
        state: info.state,
        state_change_seq: info.state_change_seq,
        ready_to_merge: agent.ready_to_merge(),
        focused: info.focused,
        supervisor: agent.supervisor,
        scratch: agent.scratch,
        project_group: agent.project_group.clone(),
        project: agent.project.clone(),
        project_root: agent.project_root.clone(),
        worktree: agent.worktree_checkout.clone(),
        checkout: agent.worktree_label.clone(),
        task: agent.task.clone(),
        model: agent.model.clone(),
        effort: agent.effort.clone(),
        context_percent: agent.context_percent,
        context_tokens: agent.context_tokens,
        cache: agent.cache.map(|cache| Cache {
            expires_at: cache.expires_at,
            tokens: cache.tokens,
            estimated: cache.kind == PromptCacheKind::Estimated,
        }),
        last_activity: agent.last_activity.map(|time| time.secs),
        message: activity(&agent.message),
        tool: activity(&agent.tool),
        can_prompt: info.state.can_prompt(),
        can_merge: agent.worktree_checkout.is_some(),
    }
}

pub(super) fn activity(entry: &model::Activity) -> Activity {
    let kind = match entry.kind {
        model::ActivityKind::Command => ActivityKind::Command,
        model::ActivityKind::Tool => ActivityKind::Tool,
        model::ActivityKind::Prompt => ActivityKind::Prompt,
        model::ActivityKind::Message => ActivityKind::Message,
        model::ActivityKind::Thinking => ActivityKind::Thinking,
        model::ActivityKind::Question => ActivityKind::Question,
        model::ActivityKind::Ready => ActivityKind::Ready,
    };
    Activity {
        kind,
        text: entry.text.clone(),
    }
}

/// The projects of `agents`, in the dashboard's order, each with the open
/// questions its supervisor's ledger in `inboxes` lists.
pub(super) fn projects(agents: &[DashboardAgent], inboxes: &Inboxes) -> Vec<Project> {
    project_runs(agents)
        .into_iter()
        .map(|run| {
            let card = is_card(agents, &run);
            let rows = &agents[run.clone()];
            let lead = &rows[0];
            let workers: Vec<&DashboardAgent> =
                rows.iter().filter(|agent| !agent.supervisor).collect();
            let root = if lead.scratch {
                String::new()
            } else {
                lead.project_root.clone()
            };
            let questions = if root.is_empty() {
                Vec::new()
            } else {
                open_questions(inboxes, &root)
            };
            Project {
                group: lead.project_group.clone(),
                root,
                card,
                scratch: lead.scratch,
                supervisor: rows
                    .iter()
                    .find(|agent| agent.supervisor)
                    .map(|agent| agent.info.pane_id.clone()),
                agents: rows
                    .iter()
                    .map(|agent| agent.info.pane_id.clone())
                    .collect(),
                workers: worker_counts(&workers),
                blocked: workers
                    .iter()
                    .filter(|worker| worker.info.state == AgentState::Blocked)
                    .map(|worker| Blocked {
                        pane_id: worker.info.pane_id.clone(),
                        task: worker.task.clone(),
                        waiting_on: match worker.message.kind {
                            model::ActivityKind::Question => worker.message.text.clone(),
                            _ => WAITING_FOR_INPUT.to_string(),
                        },
                    })
                    .collect(),
                questions,
            }
        })
        .collect()
}

/// A card's footer: workers tagged ready to merge count apart, as `merge`.
fn worker_counts(workers: &[&DashboardAgent]) -> WorkerCounts {
    let mut counts = WorkerCounts {
        total: workers.len(),
        ..WorkerCounts::default()
    };
    for worker in workers {
        let count = if worker.ready_to_merge() {
            &mut counts.merge
        } else {
            match worker.info.state {
                AgentState::Blocked => &mut counts.blocked,
                AgentState::Working => &mut counts.working,
                AgentState::Done => &mut counts.done,
                AgentState::Idle => &mut counts.idle,
                AgentState::Unknown => &mut counts.unknown,
            }
        };
        *count += 1;
    }
    counts
}

/// The numbered questions the ledger of `root`'s supervisor leaves open.
fn open_questions(inboxes: &Inboxes, root: &str) -> Vec<Question> {
    let Some(ledger) = inboxes
        .dir(root)
        .and_then(|dir| fs::read_to_string(dir.join(LEDGER_FILE)).ok())
    else {
        return Vec::new();
    };
    all_open_questions(&ledger)
        .into_iter()
        .map(|(agent, question)| {
            let (id, title) = split_question(&question);
            Question { id, title, agent }
        })
        .collect()
}

/// `Q2: Keep the old flag?` as its handle, `Q2`, and title. A question
/// without a number keeps its whole text as its title.
pub(super) fn split_question(question: &str) -> (String, String) {
    let question = question.trim();
    if let Some((handle, title)) = question.split_once(':')
        && let Some(number) = question_number(handle)
    {
        return (format!("Q{number}"), title.trim().to_string());
    }
    (String::new(), question.to_string())
}

/// The number of a question's handle, `Q3`, `q3` or `3`.
pub(super) fn question_number(handle: &str) -> Option<u32> {
    let handle = handle.trim();
    let digits = handle
        .strip_prefix('Q')
        .or_else(|| handle.strip_prefix('q'))
        .unwrap_or(handle);
    (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// The newest turns of a session, as the expanded view shows them.
pub(super) fn transcript_entries(entries: &[model::Activity]) -> Vec<Activity> {
    entries.iter().map(activity).collect()
}

/// The palette the GUI paints with. A theme Corgi reads (the active
/// Omarchy one) or one `CORGI_THEME` forces is both the light and the dark
/// entry; otherwise Catppuccin's Latte and Mocha are, for the GUI to follow
/// the system's scheme. The terminal is never asked, since stdin and stdout
/// carry the protocol.
pub(super) fn palette_set() -> PaletteSet {
    let forced = env::var("CORGI_THEME").unwrap_or_default();
    let fixed = match forced.as_str() {
        "catppuccin-mocha" => Some(("forced", Palette::catppuccin(false))),
        "catppuccin-latte" => Some(("forced", Palette::catppuccin(true))),
        "terminal" => None,
        _ => palette::resolve::omarchy_palette().map(|palette| ("omarchy", palette)),
    };
    match fixed {
        Some((source, palette)) => {
            let colors = theme_colors(palette, Source::Omarchy);
            PaletteSet {
                source: source.into(),
                light: colors.clone(),
                dark: colors,
            }
        }
        None => PaletteSet {
            source: "catppuccin".into(),
            light: theme_colors(Palette::catppuccin(true), Source::Catppuccin),
            dark: theme_colors(Palette::catppuccin(false), Source::Catppuccin),
        },
    }
}

/// `palette`'s colors and the roles worked out from it, in RGB.
pub(super) fn theme_colors(palette: Palette, source: Source) -> ThemeColors {
    let theme = Theme::new(palette, source, false, true);
    let roles = theme.rgb_roles();
    ThemeColors {
        foreground: hex(palette.foreground),
        background: hex(palette.background),
        ansi: palette.ansi.iter().copied().map(hex).collect(),
        roles: roles
            .named()
            .into_iter()
            .map(|(name, rgb)| (name.to_string(), hex(rgb)))
            .collect(),
        context_ramp: roles.context_ramp.iter().copied().map(hex).collect(),
    }
}

fn hex((r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// What changed from `old` to `new`, as a delta carries it: rows and
/// projects that came or changed whole, those that went by key, and their
/// order whenever it is not the same.
pub(super) fn changes(old: &State, new: &State) -> Vec<Change> {
    let mut changes = Vec::new();
    if old.herdr != new.herdr {
        changes.push(Change::Herdr(new.herdr.clone()));
    }
    if old.status != new.status {
        changes.push(Change::Status {
            status: new.status.clone(),
        });
    }
    if old.usage != new.usage {
        changes.push(Change::Usage {
            usage: new.usage.clone(),
        });
    }
    keyed_changes(
        &old.agents,
        &new.agents,
        |agent| &agent.pane_id,
        &mut changes,
        |agent| Change::Agent(Box::new(agent.clone())),
        |pane_id| Change::AgentRemoved { pane_id },
        |pane_ids| Change::AgentOrder { pane_ids },
    );
    keyed_changes(
        &old.projects,
        &new.projects,
        |project| &project.group,
        &mut changes,
        |project| Change::Project(Box::new(project.clone())),
        |group| Change::ProjectRemoved { group },
        |groups| Change::ProjectOrder { groups },
    );
    if old.transcript != new.transcript {
        changes.push(Change::Transcript {
            transcript: new.transcript.clone(),
        });
    }
    if old.selection != new.selection {
        changes.push(Change::Selection {
            pane_id: new.selection.clone(),
        });
    }
    if old.palette != new.palette {
        changes.push(Change::Palette(Box::new(new.palette.clone())));
    }
    changes
}

/// The changes between two keyed lists: each item of `new` that is new or
/// differs, each key of `old` that `new` lacks, and `new`'s order when the
/// keys are not the same in the same order.
fn keyed_changes<T: PartialEq>(
    old: &[T],
    new: &[T],
    key: impl Fn(&T) -> &String,
    changes: &mut Vec<Change>,
    changed: impl Fn(&T) -> Change,
    removed: impl Fn(String) -> Change,
    order: impl Fn(Vec<String>) -> Change,
) {
    let before: HashMap<&String, &T> = old.iter().map(|item| (key(item), item)).collect();
    for item in new {
        if before.get(key(item)) != Some(&item) {
            changes.push(changed(item));
        }
    }
    let after: HashMap<&String, &T> = new.iter().map(|item| (key(item), item)).collect();
    for item in old {
        if !after.contains_key(key(item)) {
            changes.push(removed(key(item).clone()));
        }
    }
    let keys = |items: &[T]| {
        items
            .iter()
            .map(|item| key(item).clone())
            .collect::<Vec<_>>()
    };
    let new_keys = keys(new);
    if keys(old) != new_keys {
        changes.push(order(new_keys));
    }
}
