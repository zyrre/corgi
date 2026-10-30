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

/// `CORGI_GUI_APPEARANCE`: `light` or `dark` overrides the system's
/// scheme; anything else follows it.
pub fn forced_light(value: Option<&str>) -> Option<bool> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "light" => Some(true),
        "dark" => Some(false),
        _ => None,
    }
}

/// Whether to paint light, and whether the user chose it with the header's
/// toggle: `CORGI_GUI_APPEARANCE`'s scheme, else the toggle's choice in this
/// window, else the system's.
pub fn appearance(forced: Option<bool>, chosen: Option<bool>, system: bool) -> (bool, bool) {
    match (forced, chosen) {
        (Some(light), _) => (light, false),
        (None, Some(light)) => (light, true),
        (None, None) => (system, false),
    }
}

impl Colors {
    /// The palette's colors for `light`: the server's, or, before its first
    /// snapshot, Catppuccin Latte or Mocha as Corgi derives them itself.
    /// `chosen` is true when the user picked `light` with the header's
    /// toggle (see `sent_theme`).
    pub fn new(palette: &PaletteSet, light: bool, chosen: bool) -> Self {
        let fallback = Theme::catppuccin(light, true);
        let fallback_roles = fallback.rgb_roles();
        let (foreground, background, roles) = match sent_theme(palette, light, chosen) {
            None => (
                fallback.palette.foreground,
                fallback.palette.background,
                fallback_roles,
            ),
            Some(sent) => (
                parse_hex(&sent.foreground).unwrap_or(fallback.palette.foreground),
                parse_hex(&sent.background).unwrap_or(fallback.palette.background),
                roles_from(sent, fallback_roles),
            ),
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

    /// The tile behind the corgi in a light theme, where the background all
    /// but hides its white blaze: the selection bar's grey, lighter than
    /// the coat and darker than the blaze. A dark theme shows it bare.
    pub fn corgi_tile(&self) -> Option<Hsla> {
        self.light.then_some(self.roles.focus_bar)
    }

    /// The text on a badge filled with `fill`: the theme's badge text, or,
    /// where that vanishes (Mocha's black on its grey IDLE), whichever of
    /// body text and foreground reads best.
    pub fn badge_text(&self, fill: Hsla) -> Hsla {
        let roles = &self.roles;
        readable(roles.badge_text, fill, &[roles.text, roles.foreground])
    }

    /// A long read on a dialog, a question's text: the foreground on the
    /// background, the theme's own strongest pair, in a box off the
    /// dialog's fill. Body text, a grey in Mocha, all but sinks into that
    /// fill.
    pub fn reading(&self) -> (Hsla, Hsla) {
        (self.foreground, self.background)
    }

    /// Secondary text (times, separators, the checkout, a dialog's hints)
    /// on `fill`: muted, or quiet where muted vanishes. Mocha's muted is
    /// nearly the grey of its selection bar and its popups; Latte's reads.
    pub fn secondary(&self, fill: Hsla) -> Hsla {
        readable(self.roles.muted, fill, &[self.roles.quiet])
    }
}

/// A contrast below which text all but vanishes on its fill.
const VANISHES: f32 = 2.0;

/// `role` on `fill`, or, where it vanishes there, whichever of it and
/// `instead` reads best: a theme keeps its look wherever its roles read.
fn readable(role: Hsla, fill: Hsla, instead: &[Hsla]) -> Hsla {
    if contrast(role, fill) >= VANISHES {
        return role;
    }
    instead.iter().copied().fold(role, |best, next| {
        if contrast(next, fill) > contrast(best, fill) {
            next
        } else {
            best
        }
    })
}

/// WCAG's contrast ratio of two colors, 1 (none) to 21.
pub fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// WCAG's relative luminance.
fn luminance(color: Hsla) -> f32 {
    let rgba = color.to_rgb();
    let linear = |c: f32| {
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(rgba.r) + 0.7152 * linear(rgba.g) + 0.0722 * linear(rgba.b)
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

/// A brand color, fixed in every theme: the corgi's coat and details.
pub fn brand_color(color: palette::Color) -> Option<Hsla> {
    brand_rgb(color).map(hsla)
}

fn brand_rgb(color: palette::Color) -> Option<Rgb> {
    match color {
        palette::Color::Indexed(index) => Some(palette::indexed_rgb(index)),
        palette::Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

/// The theme the server sent for `light`: none before its first snapshot,
/// and none when the user `chosen` `light` with the header's toggle and the
/// theme is for the other appearance only (Omarchy sends one for both), so
/// Catppuccin paints the appearance asked for until the toggle goes back.
fn sent_theme(palette: &PaletteSet, light: bool, chosen: bool) -> Option<&ThemeColors> {
    let sent = if light { &palette.light } else { &palette.dark };
    let other = parse_hex(&sent.background).is_some_and(|rgb| palette::is_light(rgb) != light);
    (!sent.roles.is_empty() && !(chosen && other)).then_some(sent)
}

/// A terminal's colors for `light`, as `Colors::new` picks the theme: the
/// server's foreground, background and sixteen ANSI colors, or, before its
/// first snapshot or for a color it left out, Catppuccin Latte's or Mocha's.
pub fn terminal_palette(palette: &PaletteSet, light: bool, chosen: bool) -> palette::Palette {
    let mut colors = palette::Palette::catppuccin(light);
    let Some(sent) = sent_theme(palette, light, chosen) else {
        return colors;
    };
    if let Some(foreground) = parse_hex(&sent.foreground) {
        colors.foreground = foreground;
    }
    if let Some(background) = parse_hex(&sent.background) {
        colors.background = background;
    }
    for (slot, hex) in colors.ansi.iter_mut().zip(&sent.ansi) {
        if let Some(rgb) = parse_hex(hex) {
            *slot = rgb;
        }
    }
    colors
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

/// A color as RGB bytes.
pub fn rgb(color: Hsla) -> Rgb {
    let rgba = color.to_rgb();
    let byte = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    (byte(rgba.r), byte(rgba.g), byte(rgba.b))
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
        let light = Colors::new(&empty, true, false);
        let dark = Colors::new(&empty, false, false);
        assert!(light.light && !dark.light);
        assert_eq!(light.background, hsla((0xef, 0xf1, 0xf5)));
        assert_eq!(dark.background, hsla((0x1e, 0x1e, 0x2e)));
        assert!(is_light(WindowAppearance::VibrantLight));
        assert_eq!(forced_light(Some("Light")), Some(true));
        assert_eq!(forced_light(Some("dark")), Some(false));
        assert_eq!(forced_light(Some("system")), None);
        assert_eq!(forced_light(None), None);
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
        let light = Colors::new(&palette, true, false);
        assert_eq!(light.roles.accent, hsla((0, 0, 255)));
        assert_eq!(light.roles.context_ramp[0], hsla((0, 255, 0)));
        let dark = Colors::new(&palette, false, false);
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
        assert!(!Colors::new(&omarchy, true, false).light);
    }

    #[test]
    fn states_and_vendors_take_their_colors() {
        let colors = Colors::new(&PaletteSet::default(), false, false);
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

    #[test]
    fn the_corgi_stands_on_a_tile_between_its_coat_and_blaze_only_when_light() {
        let empty = PaletteSet::default();
        assert_eq!(Colors::new(&empty, false, false).corgi_tile(), None);
        let latte = Colors::new(&empty, true, false);
        let tile = latte.corgi_tile().expect("a tile in Latte");
        let coat = brand_color(brand::CORGI_COAT).unwrap();
        let blaze = brand_color(brand::CORGI_WHITE).unwrap();
        assert!(luminance(coat) < luminance(tile) && luminance(tile) < luminance(blaze));
        // The blaze all but vanished on the background; on the tile it reads.
        assert!(contrast(blaze, latte.background) < 1.15);
        let on_tile = contrast(blaze, tile);
        assert!(on_tile >= 1.4, "blaze on tile: {on_tile}");
    }

    #[test]
    fn the_idle_badge_and_the_selected_row_read_in_mocha_and_latte() {
        let empty = PaletteSet::default();
        for light in [false, true] {
            let colors = Colors::new(&empty, light, false);
            let flavor = if light { "Latte" } else { "Mocha" };
            for state in [
                AgentState::Working,
                AgentState::Blocked,
                AgentState::Done,
                AgentState::Idle,
                AgentState::Unknown,
            ] {
                let fill = colors.state(state, false);
                let ratio = contrast(colors.badge_text(fill), fill);
                assert!(ratio >= VANISHES, "{flavor} {state:?} badge: {ratio}");
            }
            for (fill, on) in [
                (colors.roles.focus_bar, "selected row"),
                (colors.roles.popup_fill, "dialog"),
            ] {
                let ratio = contrast(colors.secondary(fill), fill);
                assert!(ratio >= 2.5, "{flavor} {on}: {ratio}");
            }
            assert_eq!(colors.secondary(colors.background), colors.roles.muted);
            // Latte keeps its look: badge text and muted were readable.
            if light {
                for fill in [colors.roles.muted, colors.roles.accent] {
                    assert_eq!(colors.badge_text(fill), colors.roles.badge_text);
                }
                for fill in [colors.roles.focus_bar, colors.roles.popup_fill] {
                    assert_eq!(colors.secondary(fill), colors.roles.muted);
                }
            }
        }
        // Mocha's IDLE badge: black on surface2 before, light text now.
        let mocha = Colors::new(&empty, false, false);
        assert!(contrast(mocha.roles.badge_text, mocha.roles.muted) < VANISHES);
        assert!(contrast(mocha.badge_text(mocha.roles.muted), mocha.roles.muted) > 4.0);
        assert_eq!(mocha.badge_text(mocha.roles.accent), mocha.roles.badge_text);
    }

    #[test]
    fn the_toggles_choice_wins_over_the_system_and_the_variable_over_both() {
        // Not clicked in this window (each window starts so): the system's,
        // followed as it changes.
        assert_eq!(appearance(None, None, true), (true, false));
        assert_eq!(appearance(None, None, false), (false, false));
        // Clicked: the choice, whatever the system says, until the window
        // closes.
        assert_eq!(appearance(None, Some(false), true), (false, true));
        assert_eq!(appearance(None, Some(true), false), (true, true));
        // CORGI_GUI_APPEARANCE checks a theme whatever was chosen.
        assert_eq!(appearance(Some(true), Some(false), false), (true, false));
    }

    #[test]
    fn a_theme_sent_for_one_appearance_gives_way_only_to_a_chosen_other() {
        let omarchy_dark = ThemeColors {
            foreground: "#c0caf5".into(),
            background: "#1a1b26".into(),
            ansi: vec!["#15161e".into(), "#f7768e".into()],
            roles: BTreeMap::from([("accent".to_string(), "#7dcfff".to_string())]),
            context_ramp: Vec::new(),
        };
        let palette = PaletteSet {
            source: "omarchy".into(),
            light: omarchy_dark.clone(),
            dark: omarchy_dark,
        };
        let sent = hsla((0x1a, 0x1b, 0x26));
        // The system's light, not chosen: Omarchy as it is, as before.
        assert_eq!(Colors::new(&palette, true, false).background, sent);
        // Light chosen with the toggle: Latte, in the window and its
        // terminal.
        let latte = Colors::new(&palette, true, true);
        assert!(latte.light);
        assert_eq!(latte.background, hsla((0xef, 0xf1, 0xf5)));
        assert_eq!(
            terminal_palette(&palette, true, true),
            palette::Palette::catppuccin(true)
        );
        // Back to dark: Omarchy's again.
        let dark = Colors::new(&palette, false, true);
        assert_eq!(dark.background, sent);
        assert_eq!(dark.roles.accent, hsla((0x7d, 0xcf, 0xff)));
        assert_eq!(
            terminal_palette(&palette, false, true).ansi[1],
            (0xf7, 0x76, 0x8e)
        );
    }

    #[test]
    fn a_questions_text_reads_at_seven_to_one_in_latte_and_mocha() {
        let empty = PaletteSet::default();
        for (light, flavor) in [(true, "Latte"), (false, "Mocha")] {
            let colors = Colors::new(&empty, light, false);
            let (text, fill) = colors.reading();
            let ratio = contrast(text, fill);
            assert!(ratio >= 7.0, "{flavor}: {ratio}");
            // Body text on the dialog's fill, as it was, read worse.
            let before = contrast(colors.roles.text, colors.roles.popup_fill);
            assert!(before < 7.0 && before < ratio, "{flavor} before: {before}");
        }
    }
}
