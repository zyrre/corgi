use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

use unicode_width::UnicodeWidthStr;

use crate::model::{DashboardAgent, PromptCache, PromptCacheKind, cache_countdown, compact_tokens};

use super::agents::FIELD_SEPARATOR;
use super::bold;
use crate::theme::{CONTEXT_STEPS, brand, roles};

/// A prompt cache with less than this left is worth prompting soon.
const CACHE_WARN_SECONDS: u64 = 5 * 60;
/// And with less than this, it is about to go cold.
const CACHE_LAST_MINUTE: u64 = 60;

/// One field after the task summary, with the separator drawn in front of
/// it: the dot, except for the effort, which follows the model with
/// a single space so the two read as one phrase, `Opus 5 high`.
pub(crate) struct StatusField {
    pub separator: &'static str,
    pub spans: Vec<Span<'static>>,
}

impl StatusField {
    fn new(spans: Vec<Span<'static>>) -> Self {
        Self {
            separator: FIELD_SEPARATOR,
            spans,
        }
    }

    /// Columns the field takes, its separator included.
    pub(super) fn width(&self) -> usize {
        self.separator.width()
            + self
                .spans
                .iter()
                .map(|span| span.content.width())
                .sum::<usize>()
    }
}

/// The fields shown after the task summary, in the order they are dropped from
/// a narrow row: the worktree goes first because the project heading above the
/// row already names the repository, the cache countdown next because it is
/// advice rather than identity, and the model stays longest because it gives
/// the context percentage its meaning.
pub(crate) fn status_fields(agent: &DashboardAgent, now: u64) -> Vec<StatusField> {
    let model = agent.model.as_deref().unwrap_or_else(|| agent.info.kind());
    let mut fields = vec![StatusField::new(vec![Span::styled(
        model.to_string(),
        Style::default().fg(model_color(model)),
    )])];
    if let Some(effort) = &agent.effort {
        fields.push(StatusField {
            separator: " ",
            spans: vec![Span::styled(
                effort.clone(),
                Style::default().fg(roles().accent),
            )],
        });
    }
    if let Some(percent) = agent.context_percent {
        fields.push(StatusField::new(vec![Span::styled(
            format!("{percent}% ctx"),
            context_style(percent),
        )]));
    }
    if let Some(cache) = &agent.cache {
        fields.push(StatusField::new(vec![cache_field(cache, now)]));
    }
    if let Some(worktree) = &agent.worktree_label {
        fields.push(StatusField::new(vec![
            Span::styled("⑂ ", Style::default().fg(roles().accent)),
            Span::styled(worktree.clone(), Style::default().fg(roles().muted)),
        ]));
    }
    fields
}

/// The prompt cache of a session as one field. Exact cache lifetimes count down
/// to the full re-read warning; Codex's documented minimum lifetime counts down
/// separately, then says that its cache may be cold rather than asserting it.
fn cache_field(cache: &PromptCache, now: u64) -> Span<'static> {
    let remaining = cache.remaining(now);
    if remaining == 0 {
        if cache.kind == PromptCacheKind::Estimated {
            return Span::styled("⚠ cache may be cold", Style::default().fg(roles().warning));
        }
        return Span::styled(
            format!("⚠ {} re-read", compact_tokens(cache.tokens)),
            bold(roles().danger),
        );
    }
    let color = if remaining < CACHE_LAST_MINUTE {
        roles().danger
    } else if remaining < CACHE_WARN_SECONDS {
        roles().warning
    } else {
        roles().success
    };
    let prefix = if cache.kind == PromptCacheKind::Estimated {
        "≈ "
    } else {
        "⏱ "
    };
    Span::styled(
        format!("{prefix}{} cache", cache_countdown(remaining)),
        Style::default().fg(color),
    )
}

/// How full a context window is, as a color: green while there is room,
/// through yellow and orange, to red when the session is about to run out.
///
/// A ramp needs more shades than the terminal's sixteen palette colors, so
/// the theme blends it from its own green, yellow and red, softened so the
/// reading tints the row instead of shouting from it; a nearly full context
/// still ends on the theme's red, alarming in every theme.
fn context_style(percent: u8) -> Style {
    let step = usize::from(percent.min(100)) * (CONTEXT_STEPS - 1) / 100;
    let style = Style::default().fg(roles().context_ramp[step]);
    if percent >= 90 {
        return style.add_modifier(Modifier::BOLD);
    }
    style
}

/// The color of a model name: its vendor's, so a dashboard mixing agents can
/// be read by hue before the names are read at all. A vendor is an identity
/// rather than a state of this session, so these are brand colors that stay
/// put while the theme changes around them; everything unrecognized stays
/// muted rather than borrowing another vendor's color.
pub(super) fn model_color(model: &str) -> Color {
    brand::VENDORS
        .iter()
        .find(|(names, _)| names.iter().any(|name| contains_ignoring_case(model, name)))
        .map(|(_, color)| *color)
        .unwrap_or(roles().muted)
}

/// Whether `text` contains the lower-case ASCII `word` in any case, without
/// lower-casing a copy of `text` on every row of every frame.
fn contains_ignoring_case(text: &str, word: &str) -> bool {
    text.as_bytes()
        .windows(word.len())
        .any(|window| window.eq_ignore_ascii_case(word.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{DashboardAgent, PromptCache, PromptCacheKind},
        test_support::line_text,
        ui::{
            agents::agent_status_line,
            tests::{NOW, dashboard_agent},
        },
    };

    #[test]
    fn a_model_takes_its_vendors_color_and_an_unknown_one_stays_muted() {
        let anthropic = model_color("Opus 5 1M");
        let openai = model_color("gpt-5-codex");
        assert_eq!(anthropic, model_color("claude"));
        assert_eq!(openai, model_color("codex"));
        assert_ne!(anthropic, openai);
        assert_eq!(model_color("some-local-model"), roles().muted);
        // Vendors are recognized in any case, and in names that are not ASCII.
        assert_eq!(model_color("GPT-5.6-SOL"), openai);
        assert_eq!(model_color("Ünïcode Claude"), anthropic);
    }

    #[test]
    fn the_context_color_runs_from_green_to_red_without_going_back() {
        let ramp = roles().context_ramp;
        let color = |percent: u8| context_style(percent).fg.expect("a ramp color");
        assert_eq!(color(0), ramp[0]);
        assert_eq!(color(100), ramp[CONTEXT_STEPS - 1]);
        // Percentages beyond the window are still the last step rather than
        // an index outside the ramp.
        assert_eq!(color(u8::MAX), ramp[CONTEXT_STEPS - 1]);
        let position = |color: Color| ramp.iter().position(|step| *step == color);
        let steps: Vec<_> = (0..=100).map(|percent| position(color(percent))).collect();
        assert!(steps.iter().all(Option::is_some), "{steps:?}");
        assert!(steps.windows(2).all(|pair| pair[0] <= pair[1]), "{steps:?}");
        // A nearly full window is emphasized, an empty one is not.
        assert!(context_style(95).add_modifier.contains(Modifier::BOLD));
        assert!(!context_style(20).add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn a_warm_prompt_cache_counts_down_and_a_cold_one_warns_with_its_size() {
        let cache = |expires_at: u64| DashboardAgent {
            cache: Some(PromptCache {
                expires_at,
                tokens: 40_355,
                kind: PromptCacheKind::Exact,
            }),
            ..dashboard_agent()
        };
        let field = |agent: &DashboardAgent| {
            status_fields(agent, NOW)
                .into_iter()
                .map(|field| field.spans)
                .find(|spans| {
                    spans[0].content.contains("cache") || spans[0].content.contains("re-read")
                })
                .expect("cache field")
                .remove(0)
        };

        let plenty = field(&cache(NOW + 42 * 60 + 30));
        assert_eq!(plenty.content, "⏱ 42m cache");
        assert_eq!(plenty.style.fg, Some(roles().success));
        assert_eq!(field(&cache(NOW + 3_600)).content, "⏱ 1h cache");

        let soon = field(&cache(NOW + 4 * 60));
        assert_eq!(soon.content, "⏱ 4m cache");
        assert_eq!(soon.style.fg, Some(roles().warning));

        let last = field(&cache(NOW + 45));
        assert_eq!(last.content, "⏱ 45s cache");
        assert_eq!(last.style.fg, Some(roles().danger));

        let cold = field(&cache(NOW - 1));
        assert_eq!(cold.content, "⚠ 41k re-read");
        assert_eq!(cold.style.fg, Some(roles().danger));
        assert!(cold.style.add_modifier.contains(Modifier::BOLD));

        // The field sits between the context share and the worktree, and a
        // session without a recorded cache has no field.
        let line = line_text(&agent_status_line(&cache(NOW + 600), false, 120, NOW));
        let context = line.find("13% ctx").expect("context");
        let cache_at = line.find("⏱ 10m cache").expect("cache");
        let worktree = line.find("silver-cloud-028f").expect("worktree");
        assert!(context < cache_at && cache_at < worktree, "{line}");
        assert!(
            !line_text(&agent_status_line(&dashboard_agent(), false, 120, NOW)).contains("cache")
        );
    }

    #[test]
    fn an_estimated_prompt_cache_never_claims_that_a_re_read_is_certain() {
        let cache = |expires_at: u64| PromptCache {
            expires_at,
            tokens: 40_355,
            kind: PromptCacheKind::Estimated,
        };

        let warm = cache_field(&cache(NOW + 10 * 60), NOW);
        assert_eq!(warm.content, "≈ 10m cache");
        assert_eq!(warm.style.fg, Some(roles().success));

        let uncertain = cache_field(&cache(NOW - 1), NOW);
        assert_eq!(uncertain.content, "⚠ cache may be cold");
        assert_eq!(uncertain.style.fg, Some(roles().warning));
        assert!(!uncertain.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn token_counts_are_compact_in_a_row() {
        assert_eq!(compact_tokens(950), "950");
        assert_eq!(compact_tokens(1_000), "1k");
        assert_eq!(compact_tokens(40_355), "41k");
        assert_eq!(compact_tokens(1_234_567), "1.2M");
    }
}
