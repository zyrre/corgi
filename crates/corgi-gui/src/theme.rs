//! The window's colors: Corgi's theme roles from `corgi serve`'s palette,
//! light or dark as the system's appearance is, and the brand's fixed
//! colors. Every color the window paints comes from here.

use corgi_core::{
    model::AgentState,
    palette::{self, Rgb, Roles, Theme, brand},
    protocol::{PaletteSet, ThemeColors},
};
use gpui::{Hsla, Rgba, WindowAppearance};

/// The colors one frame paints with.
#[derive(Debug, Clone, PartialEq)]
pub struct Colors {
    pub light: bool,
    pub background: Hsla,
    pub foreground: Hsla,
    pub roles: Roles<Hsla>,
    /// A panel just off the background: cards and the side panel.
    pub panel: Hsla,
}

/// Whether the system asks for a light window.
pub fn is_light(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Light | WindowAppearance::VibrantLight
    )
}

impl Colors {
    /// The palette's colors for `light`: the server's, or, before its first
    /// snapshot, Catppuccin Latte or Mocha as Corgi derives them itself.
    pub fn new(palette: &PaletteSet, light: bool) -> Self {
        let fallback = Theme::catppuccin(light, true);
        let fallback_roles = fallback.rgb_roles();
        let sent = if light { &palette.light } else { &palette.dark };
        let (foreground, background, roles) = if sent.roles.is_empty() {
            (
                fallback.palette.foreground,
                fallback.palette.background,
                fallback_roles,
            )
        } else {
            (
                parse_hex(&sent.foreground).unwrap_or(fallback.palette.foreground),
                parse_hex(&sent.background).unwrap_or(fallback.palette.background),
                roles_from(sent, fallback_roles),
            )
        };
        // A theme sent for one appearance only (Omarchy's, a forced one) is
        // painted as it is, whatever the system asks.
        let light = palette::is_light(background);
        Self {
            light,
            background: hsla(background),
            foreground: hsla(foreground),
            panel: hsla(palette::blend(background, foreground, 0.05)),
            roles: roles.map(hsla),
        }
    }

    /// The color of an agent's state, as the dashboard's badges have it.
    pub fn state(&self, state: AgentState, ready_to_merge: bool) -> Hsla {
        if ready_to_merge {
            return self.roles.merge;
        }
        match state {
            AgentState::Working => self.roles.accent,
            AgentState::Blocked => self.roles.warning,
            AgentState::Done => self.roles.success,
            AgentState::Idle => self.roles.muted,
            AgentState::Unknown => self.roles.unknown,
        }
    }

    /// How full a context window is, green to red.
    pub fn context(&self, percent: u8) -> Hsla {
        let steps = self.roles.context_ramp.len();
        self.roles.context_ramp[usize::from(percent.min(100)) * (steps - 1) / 100]
    }

    /// The supervisor's color, the CORGI wordmark's: the theme's success.
    pub fn supervisor(&self) -> Hsla {
        self.roles.success
    }
}

/// A model name's vendor color, fixed in every theme so a herd mixing agents
/// reads by hue; `None` for a model of no vendor Corgi knows.
pub fn vendor(model: &str) -> Option<Hsla> {
    let lower = model.to_lowercase();
    brand::VENDORS
        .iter()
        .find(|(names, _)| names.iter().any(|name| lower.contains(name)))
        .and_then(|(_, color)| brand_rgb(*color))
        .map(hsla)
}

fn brand_rgb(color: palette::Color) -> Option<Rgb> {
    match color {
        palette::Color::Indexed(index) => Some(palette::indexed_rgb(index)),
        palette::Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

/// The roles `sent` names, the rest from `fallback`.
fn roles_from(sent: &ThemeColors, mut roles: Roles<Rgb>) -> Roles<Rgb> {
    for (name, hex) in &sent.roles {
        let Some(rgb) = parse_hex(hex) else { continue };
        let slot = match name.as_str() {
            "accent" => &mut roles.accent,
            "muted" => &mut roles.muted,
            "success" => &mut roles.success,
            "warning" => &mut roles.warning,
            "danger" => &mut roles.danger,
            "unknown" => &mut roles.unknown,
            "merge" => &mut roles.merge,
            "thinking" => &mut roles.thinking,
            "text" => &mut roles.text,
            "quiet" => &mut roles.quiet,
            "badge_text" => &mut roles.badge_text,
            "popup_fill" => &mut roles.popup_fill,
            "keycap" => &mut roles.keycap,
            "rule" => &mut roles.rule,
            "focus_bar" => &mut roles.focus_bar,
            "dimmed" => &mut roles.dimmed,
            "dimmed_background" => &mut roles.dimmed_background,
            "fade_from" => &mut roles.fade_from,
            "foreground" => &mut roles.foreground,
            "success_frame" => &mut roles.success_frame,
            "stamp" => &mut roles.stamp,
            "flash" => &mut roles.flash,
            _ => continue,
        };
        *slot = rgb;
    }
    for (step, hex) in roles.context_ramp.iter_mut().zip(&sent.context_ramp) {
        if let Some(rgb) = parse_hex(hex) {
            *step = rgb;
        }
    }
    roles
}

/// `#rrggbb` as RGB.
pub fn parse_hex(hex: &str) -> Option<Rgb> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 || !digits.is_ascii() {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some((channel(0)?, channel(2)?, channel(4)?))
}

pub fn hsla((r, g, b): Rgb) -> Hsla {
    Rgba {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: 1.0,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn hex_colors_parse_and_bad_ones_do_not() {
        assert_eq!(parse_hex("#1e1e2e"), Some((0x1e, 0x1e, 0x2e)));
        assert_eq!(parse_hex("1e1e2e"), None);
        assert_eq!(parse_hex("#1e1e2"), None);
        assert_eq!(parse_hex("#zzzzzz"), None);
    }

    #[test]
    fn before_a_palette_arrives_the_window_follows_the_system_with_catppuccin() {
        let empty = PaletteSet::default();
        let light = Colors::new(&empty, true);
        let dark = Colors::new(&empty, false);
        assert!(light.light && !dark.light);
        assert_eq!(light.background, hsla((0xef, 0xf1, 0xf5)));
        assert_eq!(dark.background, hsla((0x1e, 0x1e, 0x2e)));
        assert!(is_light(WindowAppearance::VibrantLight));
        assert!(!is_light(WindowAppearance::Dark));
    }

    #[test]
    fn the_sent_palette_s_roles_are_used_for_the_appearance_asked_for() {
        let theme = |background: &str, accent: &str| ThemeColors {
            foreground: "#808080".into(),
            background: background.into(),
            ansi: Vec::new(),
            roles: BTreeMap::from([("accent".to_string(), accent.to_string())]),
            context_ramp: vec!["#00ff00".into()],
        };
        let palette = PaletteSet {
            source: "catppuccin".into(),
            light: theme("#ffffff", "#0000ff"),
            dark: theme("#000000", "#ff0000"),
        };
        let light = Colors::new(&palette, true);
        assert_eq!(light.roles.accent, hsla((0, 0, 255)));
        assert_eq!(light.roles.context_ramp[0], hsla((0, 255, 0)));
        let dark = Colors::new(&palette, false);
        assert_eq!(dark.roles.accent, hsla((255, 0, 0)));
        // A role not sent comes from Corgi's own theme.
        let fallback = Theme::catppuccin(false, true).rgb_roles();
        assert_eq!(dark.roles.merge, hsla(fallback.merge));
        // Omarchy sends one theme for both: a dark one stays dark.
        let omarchy = PaletteSet {
            source: "omarchy".into(),
            light: theme("#000000", "#ff0000"),
            dark: theme("#000000", "#ff0000"),
        };
        assert!(!Colors::new(&omarchy, true).light);
    }

    #[test]
    fn states_and_vendors_take_their_colors() {
        let colors = Colors::new(&PaletteSet::default(), false);
        assert_eq!(colors.state(AgentState::Done, true), colors.roles.merge);
        assert_eq!(
            colors.state(AgentState::Blocked, false),
            colors.roles.warning
        );
        assert_eq!(colors.context(0), colors.roles.context_ramp[0]);
        assert_eq!(colors.context(255), colors.roles.context_ramp[6]);
        assert_eq!(vendor("Opus 5"), vendor("claude"));
        assert_ne!(vendor("gpt-5-codex"), vendor("claude"));
        assert_eq!(vendor("some-local-model"), None);
        assert_eq!(vendor("claude"), brand_rgb(brand::CORGI_COAT).map(hsla));
    }
}
