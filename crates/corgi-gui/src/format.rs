//! The words the window shows, as the dashboard words them: an agent's
//! fields, markers and badges, the plan-usage cards and the herd's counts.
//! Each piece carries a [`Tone`] that the view paints with the theme.

use corgi_core::{
    model::{AgentState, cache_countdown, compact_tokens},
    protocol::{ActivityKind, Agent, Cache, UsageCard, WorkerCounts},
    time::LocalTime,
    usage::UsageWindow,
};

/// A prompt cache with less than this left is worth prompting soon.
const CACHE_WARN_SECONDS: u64 = 5 * 60;
const CACHE_LAST_MINUTE: u64 = 60;

/// What a piece of text is, for its color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Text,
    Muted,
    Accent,
    Success,
    Warning,
    Danger,
    Merge,
    /// The model's vendor color.
    Model,
    /// The context-window ramp at this percentage.
    Context(u8),
    /// The agent state's color.
    State(AgentState),
}

/// One piece of a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub text: String,
    pub tone: Tone,
    pub bold: bool,
}

impl Piece {
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            bold: false,
        }
    }

    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
}

/// The badge an agent's row starts with: its state, or MERGE when its
/// supervisor tagged it ready.
pub fn badge(agent: &Agent) -> (&'static str, Tone) {
    if agent.ready_to_merge {
        ("MERGE", Tone::Merge)
    } else {
        (agent.state.label(), Tone::State(agent.state))
    }
}

/// The fields after the task: model and effort, context, cache, checkout.
pub fn fields(agent: &Agent, now: u64) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let model = agent.model.as_deref().unwrap_or(&agent.harness);
    let mut model_piece = Piece::new(model, Tone::Model);
    if let Some(effort) = &agent.effort {
        model_piece.text.push(' ');
        model_piece.text.push_str(effort);
    }
    pieces.push(model_piece);
    if let Some(percent) = agent.context_percent {
        let piece = Piece::new(format!("{percent}% ctx"), Tone::Context(percent));
        pieces.push(if percent >= 90 { piece.bold() } else { piece });
    }
    if let Some(cache) = &agent.cache {
        pieces.push(cache_piece(cache, now));
    }
    if let Some(checkout) = &agent.checkout {
        pieces.push(Piece::new(format!("⑂ {checkout}"), Tone::Muted));
    }
    pieces
}

/// The prompt cache's countdown, yellow in its last five minutes and red in
/// its last one; once cold, what the next prompt re-reads.
pub fn cache_piece(cache: &Cache, now: u64) -> Piece {
    let remaining = cache.expires_at.saturating_sub(now);
    if remaining == 0 {
        if cache.estimated {
            return Piece::new("⚠ cache may be cold", Tone::Warning);
        }
        return Piece::new(
            format!("⚠ {} re-read", compact_tokens(cache.tokens)),
            Tone::Danger,
        )
        .bold();
    }
    let tone = if remaining < CACHE_LAST_MINUTE {
        Tone::Danger
    } else if remaining < CACHE_WARN_SECONDS {
        Tone::Warning
    } else {
        Tone::Success
    };
    let prefix = if cache.estimated { "≈" } else { "⏱" };
    Piece::new(
        format!("{prefix} {} cache", cache_countdown(remaining)),
        tone,
    )
}

/// The local time of the session's newest record, `HH:MM DD Mon`.
pub fn last_activity(agent: &Agent) -> Option<String> {
    agent
        .last_activity
        .map(|secs| LocalTime::at(secs).clock_and_date())
}

/// The marker in front of a message or tool row.
pub fn marker(kind: ActivityKind) -> &'static str {
    match kind {
        ActivityKind::Command => "$",
        ActivityKind::Tool => "●",
        ActivityKind::Prompt => "»",
        ActivityKind::Message => "›",
        ActivityKind::Thinking => "…",
        ActivityKind::Question => "?",
        ActivityKind::Ready => "○",
    }
}

pub fn marker_tone(kind: ActivityKind) -> Tone {
    match kind {
        ActivityKind::Question => Tone::Warning,
        ActivityKind::Command | ActivityKind::Tool => Tone::Accent,
        ActivityKind::Thinking | ActivityKind::Ready => Tone::Muted,
        ActivityKind::Prompt | ActivityKind::Message => Tone::Text,
    }
}

/// A card's footer: the worker total, then each state present, the ones
/// tagged ready to merge right after the blocked.
pub fn worker_summary(counts: &WorkerCounts) -> Vec<Piece> {
    let workers = if counts.total == 1 {
        "worker"
    } else {
        "workers"
    };
    let mut pieces = vec![Piece::new(format!("{} {workers}", counts.total), Tone::Text).bold()];
    let states = [
        (counts.blocked, "▲", "blocked", Tone::Warning),
        (counts.merge, "⇡", "merge", Tone::Merge),
        (counts.working, "●", "working", Tone::Accent),
        (counts.done, "✓", "done", Tone::Success),
        (counts.idle, "○", "idle", Tone::Muted),
        (counts.unknown, "?", "unknown", Tone::Muted),
    ];
    for (count, mark, word, tone) in states {
        if count > 0 {
            pieces.push(Piece::new(format!("{mark} {count} {word}"), tone));
        }
    }
    pieces
}

/// A project's inbox items held back because Herdr never confirmed their
/// delivery, `None` when there are none: nothing more is typed into its
/// supervisor until they are read with `corgi inbox <root> --take`.
pub fn held(count: usize) -> Option<Piece> {
    (count > 0).then(|| {
        let items = if count == 1 { "item" } else { "items" };
        Piece::new(format!("⏸ {count} inbox {items} held"), Tone::Warning).bold()
    })
}

/// The HERD counts over every agent: working, blocked, merge, done, idle.
pub fn herd(agents: &[Agent]) -> Vec<Piece> {
    let count = |wanted: AgentState| {
        agents
            .iter()
            .filter(|agent| !agent.ready_to_merge && agent.state == wanted)
            .count()
    };
    let merge = agents.iter().filter(|agent| agent.ready_to_merge).count();
    vec![
        Piece::new(
            format!("● {} WORKING", count(AgentState::Working)),
            Tone::Accent,
        ),
        Piece::new(
            format!("! {} BLOCKED", count(AgentState::Blocked)),
            Tone::Warning,
        ),
        Piece::new(format!("⇡ {merge} MERGE"), Tone::Merge),
        Piece::new(format!("✓ {} DONE", count(AgentState::Done)), Tone::Success),
        Piece::new(format!("· {} IDLE", count(AgentState::Idle)), Tone::Muted),
    ]
}

/// The HERD card's frame, as the dashboard colors it: the most pressing
/// state any agent is in, blocked before working, merge and done.
pub fn herd_tone(agents: &[Agent]) -> Tone {
    let any = |wanted: AgentState| {
        agents
            .iter()
            .any(|agent| !agent.ready_to_merge && agent.state == wanted)
    };
    if any(AgentState::Blocked) {
        Tone::Warning
    } else if any(AgentState::Working) {
        Tone::Accent
    } else if agents.iter().any(|agent| agent.ready_to_merge) {
        Tone::Merge
    } else if any(AgentState::Done) {
        Tone::Success
    } else {
        Tone::Muted
    }
}

/// One plan-usage card as lines: the plan and the reading's age, then each
/// window's use and when it resets.
pub fn usage_lines(card: &UsageCard, now: u64) -> Vec<Vec<Piece>> {
    let heading = match &card.usage {
        Some(usage) if !usage.plan_type.is_empty() => {
            Piece::new(&usage.plan_type, Tone::Text).bold()
        }
        Some(_) => Piece::new("—", Tone::Muted),
        None if card.error.is_some() => Piece::new("unavailable", Tone::Muted),
        None => Piece::new("loading…", Tone::Muted),
    };
    let mut first = vec![heading];
    if let (Some(_), Some(fetched_at)) = (&card.usage, card.fetched_at) {
        first.push(Piece::new(
            format!("{} ago", reading_age(now.saturating_sub(fetched_at))),
            Tone::Muted,
        ));
    }
    let window = |label: &str, window: Option<&UsageWindow>| -> Vec<Piece> {
        match window {
            None => vec![Piece::new(label, Tone::Muted), Piece::new("—", Tone::Muted)],
            Some(window) => vec![
                Piece::new(label, Tone::Muted),
                Piece::new(
                    format!("{}%", window.used_percent),
                    usage_tone(window.used_percent),
                )
                .bold(),
                Piece::new(reset_countdown(window.resets_at, now), Tone::Accent),
            ],
        }
    };
    let usage = card.usage.as_ref();
    vec![
        first,
        window("5h", usage.and_then(|usage| usage.five_hour.as_ref())),
        window("Weekly", usage.and_then(|usage| usage.week.as_ref())),
    ]
}

fn usage_tone(used_percent: u8) -> Tone {
    match used_percent {
        0..=59 => Tone::Success,
        60..=84 => Tone::Warning,
        _ => Tone::Danger,
    }
}

/// How long ago a reading was taken, in its largest whole unit.
fn reading_age(elapsed: u64) -> String {
    let minutes = elapsed / 60;
    match minutes {
        0 => "<1m".to_string(),
        1..60 => format!("{minutes}m"),
        60..1_440 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1_440),
    }
}

/// When a usage window resets: `<1m`, `42m`, `2h 05m`, `3d 04h`.
pub fn reset_countdown(resets_at: u64, now: u64) -> String {
    if resets_at == 0 {
        return "—".to_string();
    }
    let remaining = resets_at.saturating_sub(now);
    if remaining == 0 {
        return "now".to_string();
    }
    let minutes = remaining / 60;
    if minutes < 60 {
        return if minutes == 0 {
            "<1m".to_string()
        } else {
            format!("{minutes}m")
        };
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h {:02}m", minutes % 60);
    }
    format!("{}d {:02}h", hours / 24, hours % 24)
}

#[cfg(test)]
mod tests {
    use corgi_core::{protocol::Activity, usage::PlanUsage, usage::Provider};

    use super::*;

    fn texts(pieces: &[Piece]) -> Vec<&str> {
        pieces.iter().map(|piece| piece.text.as_str()).collect()
    }

    #[test]
    fn a_row_s_fields_read_as_the_dashboard_s() {
        let agent = Agent {
            harness: "claude".into(),
            model: Some("Opus 5".into()),
            effort: Some("high".into()),
            context_percent: Some(92),
            cache: Some(Cache {
                expires_at: 1_000 + 52 * 60,
                tokens: 84_000,
                estimated: false,
            }),
            checkout: Some("fix-2d5a".into()),
            ..Agent::default()
        };
        let pieces = fields(&agent, 1_000);
        assert_eq!(
            texts(&pieces),
            ["Opus 5 high", "92% ctx", "⏱ 52m cache", "⑂ fix-2d5a"]
        );
        assert!(pieces[1].bold, "a nearly full context stands out");
        assert_eq!(pieces[2].tone, Tone::Success);
        let bare = Agent {
            harness: "codex".into(),
            ..Agent::default()
        };
        assert_eq!(texts(&fields(&bare, 0)), ["codex"]);
        assert_eq!(badge(&bare), ("UNKNOWN", Tone::State(AgentState::Unknown)));
        let tagged = Agent {
            ready_to_merge: true,
            state: AgentState::Done,
            ..Agent::default()
        };
        assert_eq!(badge(&tagged), ("MERGE", Tone::Merge));
    }

    #[test]
    fn the_cache_counts_down_then_says_what_a_cold_one_costs() {
        let cache = |left: u64, estimated: bool| Cache {
            expires_at: 100 + left,
            tokens: 182_000,
            estimated,
        };
        assert_eq!(cache_piece(&cache(4 * 60, false), 100).tone, Tone::Warning);
        assert_eq!(cache_piece(&cache(30, false), 100).text, "⏱ 30s cache");
        assert_eq!(cache_piece(&cache(30, false), 100).tone, Tone::Danger);
        assert_eq!(cache_piece(&cache(0, false), 100).text, "⚠ 182k re-read");
        assert_eq!(cache_piece(&cache(18 * 60, true), 100).text, "≈ 18m cache");
        assert_eq!(
            cache_piece(&cache(0, true), 100).text,
            "⚠ cache may be cold"
        );
    }

    #[test]
    fn counts_show_only_the_states_present_with_merge_after_blocked() {
        let counts = WorkerCounts {
            total: 3,
            blocked: 1,
            merge: 1,
            working: 1,
            ..WorkerCounts::default()
        };
        assert_eq!(
            texts(&worker_summary(&counts)),
            ["3 workers", "▲ 1 blocked", "⇡ 1 merge", "● 1 working"]
        );
        let agents = [
            Agent {
                state: AgentState::Working,
                ..Agent::default()
            },
            Agent {
                state: AgentState::Done,
                ready_to_merge: true,
                ..Agent::default()
            },
        ];
        assert_eq!(
            texts(&herd(&agents)),
            [
                "● 1 WORKING",
                "! 0 BLOCKED",
                "⇡ 1 MERGE",
                "✓ 0 DONE",
                "· 0 IDLE"
            ]
        );
        assert_eq!(herd_tone(&agents), Tone::Accent);
        assert_eq!(herd_tone(&agents[1..]), Tone::Merge);
        assert_eq!(herd_tone(&[]), Tone::Muted);
        assert_eq!(marker(Activity::default().kind), "○");
    }

    #[test]
    fn held_inbox_items_show_only_when_there_are_some() {
        assert_eq!(held(0), None);
        assert_eq!(held(1).unwrap().text, "⏸ 1 inbox item held");
        assert_eq!(held(3).unwrap().tone, Tone::Warning);
    }

    #[test]
    fn usage_cards_show_use_and_resets_and_say_when_there_is_no_reading() {
        let card = UsageCard {
            provider: Provider::Claude,
            title: "CLAUDE".into(),
            usage: Some(PlanUsage {
                provider: Provider::Claude,
                plan_type: "max".into(),
                five_hour: Some(UsageWindow {
                    used_percent: 70,
                    resets_at: 10_000 + 2 * 3600 + 5 * 60,
                }),
                week: None,
                banked_resets: None,
            }),
            fetched_at: Some(10_000 - 180),
            error: None,
        };
        let lines = usage_lines(&card, 10_000);
        assert_eq!(texts(&lines[0]), ["max", "3m ago"]);
        assert_eq!(texts(&lines[1]), ["5h", "70%", "2h 05m"]);
        assert_eq!(lines[1][1].tone, Tone::Warning);
        assert_eq!(texts(&lines[2]), ["Weekly", "—"]);
        let failed = UsageCard {
            usage: None,
            error: Some("no token".into()),
            ..card
        };
        assert_eq!(texts(&usage_lines(&failed, 0)[0]), ["unavailable"]);
        assert_eq!(reset_countdown(0, 5), "—");
        assert_eq!(reset_countdown(5 + 3 * 86_400 + 4 * 3600, 5), "3d 04h");
    }
}
