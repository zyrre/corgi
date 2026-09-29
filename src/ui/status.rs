use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

use unicode_width::UnicodeWidthStr;

use crate::model::{DashboardAgent, PromptCache, PromptCacheKind};

use super::agents::FIELD_SEPARATOR;
use super::{ACCENT, DANGER, MUTED, SUCCESS, WARNING, bold};

// Green through yellow and orange to red, in even steps of the 216-color cube.
// The hues are the full-saturation ramp's, muted: the leading channel peaks at
// 215 rather than 255 and the third channel is lifted off zero to 95, which
// takes the saturation down to roughly 60% so the reading tints the row
// instead of shouting from it.
const CONTEXT_RAMP: [u8; 7] = [77, 113, 149, 185, 179, 173, 167];
/// A prompt cache with less than this left is worth prompting soon.
const CACHE_WARN_SECONDS: u64 = 5 * 60;
/// And with less than this, it is about to go cold.
const CACHE_LAST_MINUTE: u64 = 60;
// Model vendors, by the words their model names and agent kinds contain.
// Anthropic keeps its orange, OpenAI its blue, and everything unrecognized
// stays muted rather than borrowing another vendor's color.
const VENDOR_COLORS: [(&[&str], u8); 8] = [
    (&["claude", "opus", "sonnet", "haiku", "fable"], 208),
    (&["gpt", "codex"], 39),
    (&["gemini", "gemma"], 105),
    (&["llama"], 63),
    (&["mistral", "codestral", "devstral"], 203),
    (&["grok"], 250),
    (&["deepseek"], 43),
    (&["qwen", "kimi", "glm"], 135),
];

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
            spans: vec![Span::styled(effort.clone(), Style::default().fg(ACCENT))],
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
            Span::styled("⑂ ", Style::default().fg(ACCENT)),
            Span::styled(worktree.clone(), Style::default().fg(MUTED)),
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
            return Span::styled("⚠ cache may be cold", Style::default().fg(WARNING));
        }
        return Span::styled(
            format!("⚠ {} re-read", compact_tokens(cache.tokens)),
            bold(DANGER),
        );
    }
    let color = if remaining < CACHE_LAST_MINUTE {
        DANGER
    } else if remaining < CACHE_WARN_SECONDS {
        WARNING
    } else {
        SUCCESS
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

/// Seconds left on a prompt cache, at the precision that matters for it: hours
/// only when at least an hour is left, minutes otherwise, and seconds when the
/// cache is about to lapse, so the field visibly counts down at the end.
fn cache_countdown(remaining: u64) -> String {
    if remaining >= 3_600 {
        format!("{}h", remaining / 3_600)
    } else if remaining >= 60 {
        format!("{}m", remaining / 60)
    } else {
        format!("{remaining}s")
    }
}

/// A token count short enough for a row field: `40k`, `1.2M`.
fn compact_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}k", tokens.div_ceil(1_000))
    } else {
        tokens.to_string()
    }
}

/// How full a context window is, as a color: green while there is room,
/// through yellow and orange, to red when the session is about to run out.
///
/// A ramp needs more shades than the terminal's sixteen palette colors, and
/// this reading must not change meaning with the theme — a nearly full context
/// has to look alarming in every one — so it uses the fixed color cube.
fn context_style(percent: u8) -> Style {
    let step = usize::from(percent.min(100)) * (CONTEXT_RAMP.len() - 1) / 100;
    let style = Style::default().fg(Color::Indexed(CONTEXT_RAMP[step]));
    if percent >= 90 {
        return style.add_modifier(Modifier::BOLD);
    }
    style
}

/// The color of a model name: its vendor's, so a dashboard mixing agents can
/// be read by hue before the names are read at all. A vendor is an identity
/// rather than a state of this session, so these are cube colors that stay put
/// while the theme changes around them.
pub(super) fn model_color(model: &str) -> Color {
    VENDOR_COLORS
        .iter()
        .find(|(names, _)| names.iter().any(|name| contains_ignoring_case(model, name)))
        .map(|(_, color)| Color::Indexed(*color))
        .unwrap_or(MUTED)
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
        assert_eq!(model_color("some-local-model"), MUTED);
        // Vendors are recognized in any case, and in names that are not ASCII.
        assert_eq!(model_color("GPT-5.6-SOL"), openai);
        assert_eq!(model_color("Ünïcode Claude"), anthropic);
    }

    #[test]
    fn the_context_color_runs_from_green_to_red_without_going_back() {
        let color = |percent: u8| match context_style(percent).fg {
            Some(Color::Indexed(index)) => index,
            other => panic!("expected a ramp color, got {other:?}"),
        };
        assert_eq!(color(0), CONTEXT_RAMP[0]);
        assert_eq!(color(100), CONTEXT_RAMP[CONTEXT_RAMP.len() - 1]);
        // Percentages beyond the window are still the last step rather than
        // an index outside the ramp.
        assert_eq!(color(u8::MAX), CONTEXT_RAMP[CONTEXT_RAMP.len() - 1]);
        let steps: Vec<u8> = (0..=100).map(color).collect();
        assert!(
            steps.windows(2).all(|pair| {
                let (before, after) = (pair[0], pair[1]);
                position(before) <= position(after)
            }),
            "{steps:?}"
        );
        // A nearly full window is emphasized, an empty one is not.
        assert!(context_style(95).add_modifier.contains(Modifier::BOLD));
        assert!(!context_style(20).add_modifier.contains(Modifier::BOLD));
    }

    fn position(index: u8) -> usize {
        CONTEXT_RAMP
            .iter()
            .position(|step| *step == index)
            .expect("ramp color")
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
        assert_eq!(plenty.style.fg, Some(SUCCESS));
        assert_eq!(field(&cache(NOW + 3_600)).content, "⏱ 1h cache");

        let soon = field(&cache(NOW + 4 * 60));
        assert_eq!(soon.content, "⏱ 4m cache");
        assert_eq!(soon.style.fg, Some(WARNING));

        let last = field(&cache(NOW + 45));
        assert_eq!(last.content, "⏱ 45s cache");
        assert_eq!(last.style.fg, Some(DANGER));

        let cold = field(&cache(NOW - 1));
        assert_eq!(cold.content, "⚠ 41k re-read");
        assert_eq!(cold.style.fg, Some(DANGER));
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
        assert_eq!(warm.style.fg, Some(SUCCESS));

        let uncertain = cache_field(&cache(NOW - 1), NOW);
        assert_eq!(uncertain.content, "⚠ cache may be cold");
        assert_eq!(uncertain.style.fg, Some(WARNING));
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
