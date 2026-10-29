//! The dashboard's colors: the theme of [`corgi_core::palette`] as ratatui
//! draws it, and the one theme the process draws with.
//!
//! A core role is the terminal's default, one of its sixteen ANSI colors, one
//! of the fixed 256, or an RGB; here each is the ratatui [`Color`] of the
//! same name, so the terminal paints the ANSI ones from its own theme.
//!
//! No color literal belongs anywhere else in the crate, except in tests.
//! The brand colors in [`brand`] are fixed in every theme.

use std::{cell::RefCell, sync::OnceLock};

use corgi_core::palette;
pub use corgi_core::palette::{
    CONTEXT_STEPS, Palette, Rgb, Source, blend, indexed_rgb, is_light, nearest,
};
use ratatui::style::Color;

/// The roles the dashboard draws with, as ratatui colors.
pub type Roles = palette::Roles<Color>;

/// The terminal's default color, foreground or background: no color of
/// Corgi's own.
pub const TERMINAL_DEFAULT: Color = Color::Reset;

/// The sixteen ANSI colors by name, as a role that the terminal paints.
const ANSI_NAMES: [Color; 16] = [
    Color::Black,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    Color::Gray,
    Color::DarkGray,
    Color::LightRed,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightBlue,
    Color::LightMagenta,
    Color::LightCyan,
    Color::White,
];

/// The ratatui color that draws `color`.
pub const fn color(color: palette::Color) -> Color {
    match color {
        palette::Color::Default => Color::Reset,
        palette::Color::Ansi(index) if index < 16 => ANSI_NAMES[index as usize],
        palette::Color::Ansi(_) => Color::Reset,
        palette::Color::Indexed(index) => Color::Indexed(index),
        palette::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// The core color `color` draws, the inverse of [`color()`].
fn core_color(color: Color) -> palette::Color {
    if let Some(index) = ANSI_NAMES.iter().position(|name| *name == color) {
        return palette::Color::Ansi(index as u8);
    }
    match color {
        Color::Indexed(index) => palette::Color::Indexed(index),
        Color::Rgb(r, g, b) => palette::Color::Rgb(r, g, b),
        _ => palette::Color::Default,
    }
}

/// A theme of the core as the dashboard draws it: the core's theme, and its
/// roles as ratatui colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub palette: Palette,
    /// Whether the background is light.
    pub light: bool,
    pub roles: Roles,
    core: palette::Theme,
}

impl From<palette::Theme> for Theme {
    fn from(core: palette::Theme) -> Self {
        Self {
            palette: core.palette,
            light: core.light,
            roles: core.roles.map(color),
            core,
        }
    }
}

impl Theme {
    /// The theme of `palette`. With `terminal_colors`, roles are the
    /// terminal's ANSI colors and only shades come from the palette's RGB.
    pub fn new(palette: Palette, source: Source, terminal_colors: bool, truecolor: bool) -> Self {
        palette::Theme::new(palette, source, terminal_colors, truecolor).into()
    }

    /// xterm's defaults in the terminal's own colors, snapped to the 256
    /// colors: the theme before one is resolved, and in tests.
    pub fn assumed() -> Self {
        palette::Theme::assumed().into()
    }

    /// Catppuccin in its own RGB rather than the terminal's colors, for a
    /// fixed look whatever the terminal is themed with.
    pub fn catppuccin(light: bool, truecolor: bool) -> Self {
        palette::Theme::catppuccin(light, truecolor).into()
    }

    /// `rgb` as the terminal can draw it: exactly in truecolor, else the
    /// nearest of the fixed 256 colors.
    pub fn shade(&self, rgb: Rgb) -> Color {
        color(self.core.shade(rgb))
    }

    /// The RGB of `color` in this theme: the palette's for the sixteen ANSI
    /// colors, exact for RGB and the fixed 256, and `None` for the
    /// terminal's default, which is the foreground or the background
    /// depending on where it is drawn.
    pub fn rgb(&self, color: Color) -> Option<Rgb> {
        self.core.rgb(core_color(color))
    }

    /// `from` blended `t` of the way to `to`. The ends are the colors
    /// themselves, so an effect that has finished leaves a theme color
    /// exactly as it was. A color without RGB, the terminal's default, is
    /// left as it is and switches over half way.
    pub fn mix(&self, from: Color, to: Color, t: f32) -> Color {
        if t <= 0.0 {
            return from;
        }
        if t >= 1.0 {
            return to;
        }
        color(self.core.mix(core_color(from), core_color(to), t))
    }

    /// The name the Omarchy bar widget gives `color`, when it is a role the
    /// widget has a color of its own for.
    pub fn tone(&self, color: Color) -> Option<&'static str> {
        let roles = &self.roles;
        [
            (roles.accent, "cyan"),
            (roles.success, "green"),
            (roles.warning, "yellow"),
            (roles.danger, "red"),
            (roles.merge, "magenta"),
            (roles.muted, "muted"),
        ]
        .into_iter()
        .find(|(role, _)| *role == color)
        .map(|(_, tone)| tone)
    }
}

/// The theme to draw with, as [`palette::resolve()`] finds it.
pub fn resolve() -> Theme {
    palette::resolve().into()
}

/// The colors of the brand, fixed in every theme: the supervisor's corgi
/// would not read as a corgi in the theme's accent, and a model vendor is an
/// identity rather than a state of the session, read by hue across a
/// dashboard mixing agents.
pub mod brand {
    use corgi_core::palette::brand;
    use ratatui::style::Color;

    use super::color;

    pub const CORGI_COAT: Color = color(brand::CORGI_COAT);
    pub const CORGI_INNER_EAR: Color = color(brand::CORGI_INNER_EAR);
    pub const CORGI_WHITE: Color = color(brand::CORGI_WHITE);
    pub const CORGI_DARK: Color = color(brand::CORGI_DARK);
    pub const CORGI_GLINT: Color = color(brand::CORGI_GLINT);
    pub const CORGI_TONGUE: Color = color(brand::CORGI_TONGUE);

    /// Model vendors, by the words their model names and agent kinds
    /// contain.
    pub const VENDORS: [(&[&str], Color); 8] = {
        let mut vendors: [(&[&str], Color); 8] = [(&[], Color::Reset); 8];
        let mut index = 0;
        while index < vendors.len() {
            let (words, vendor) = brand::VENDORS[index];
            vendors[index] = (words, color(vendor));
            index += 1;
        }
        vendors
    };
}

static RESOLVED: OnceLock<Theme> = OnceLock::new();
static ASSUMED: OnceLock<Theme> = OnceLock::new();

thread_local! {
    static OVERRIDE: RefCell<Option<Theme>> = const { RefCell::new(None) };
}

/// Makes `theme` the one Corgi draws with from now on. Only the first call
/// counts: the theme is resolved once, at startup.
pub fn install(theme: Theme) {
    let _ = RESOLVED.set(theme);
}

/// Runs `f` with the theme in force on this thread.
pub fn with<R>(f: impl FnOnce(&Theme) -> R) -> R {
    OVERRIDE.with(|cell| match &*cell.borrow() {
        Some(theme) => f(theme),
        None => f(RESOLVED
            .get()
            .unwrap_or_else(|| ASSUMED.get_or_init(Theme::assumed))),
    })
}

/// The roles in force on this thread.
pub fn roles() -> Roles {
    with(|theme| theme.roles)
}

/// Runs `f` with `theme` in force on this thread instead, for drawing with a
/// fixed theme whatever the machine's is.
#[cfg(test)]
pub(crate) fn scoped<R>(theme: Theme, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Theme>);
    impl Drop for Restore {
        fn drop(&mut self) {
            OVERRIDE.with(|cell| *cell.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(OVERRIDE.with(|cell| cell.borrow_mut().replace(theme)));
    f()
}

/// [`Theme::mix`] in the theme in force.
pub fn mix(from: Color, to: Color, t: f32) -> Color {
    with(|theme| theme.mix(from, to, t))
}

/// The index of `color` among the 256 colors, an RGB one snapped to the
/// nearest, for a reader that only knows indexes.
pub fn indexed(color: Color) -> Option<u8> {
    match color {
        Color::Indexed(index) => Some(index),
        Color::Rgb(r, g, b) => Some(nearest((r, g, b))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_has_the_rgb_of_the_cube_and_ramp() {
        let theme = Theme::assumed();
        assert_eq!(theme.rgb(Color::Indexed(16)), Some((0, 0, 0)));
        assert_eq!(theme.rgb(Color::Indexed(231)), Some((255, 255, 255)));
        assert_eq!(theme.rgb(Color::Indexed(78)), Some((95, 215, 135)));
        assert_eq!(theme.rgb(Color::Indexed(232)), Some((8, 8, 8)));
        assert_eq!(theme.rgb(Color::Indexed(255)), Some((238, 238, 238)));
        assert_eq!(theme.rgb(Color::Cyan), Some((0, 205, 205)));
        assert_eq!(theme.rgb(Color::Reset), None);
        // The sixteen are the palette's, whatever it is.
        let mocha = Theme::new(Palette::CATPPUCCIN_MOCHA, Source::Terminal, true, true);
        assert_eq!(mocha.rgb(Color::Cyan), Some((0x94, 0xe2, 0xd5)));
        assert_eq!(mocha.rgb(Color::Indexed(1)), Some((0xf3, 0x8b, 0xa8)));
    }

    #[test]
    fn a_color_snaps_to_the_nearest_fixed_one() {
        // Every fixed color is its own nearest.
        for index in 16..=255u8 {
            let snapped = nearest(indexed_rgb(index));
            assert_eq!(
                indexed_rgb(snapped),
                indexed_rgb(index),
                "{index} snapped to {snapped}"
            );
        }
        // The same answer as searching the whole palette.
        for rgb in [(1, 2, 3), (200, 10, 90), (128, 128, 130), (0, 205, 205)] {
            let searched = (16..=255u8)
                .min_by_key(|&index| {
                    let (r, g, b) = indexed_rgb(index);
                    let square = |a: u8, b: u8| u32::from(a.abs_diff(b)).pow(2);
                    square(rgb.0, r) + square(rgb.1, g) + square(rgb.2, b)
                })
                .unwrap();
            assert_eq!(indexed_rgb(nearest(rgb)), indexed_rgb(searched), "{rgb:?}");
        }
    }

    #[test]
    fn a_blend_ends_on_its_colors_and_is_a_shade_between() {
        let theme = Theme::assumed();
        assert_eq!(theme.mix(Color::Cyan, Color::Red, 0.0), Color::Cyan);
        assert_eq!(theme.mix(Color::Cyan, Color::Red, 1.0), Color::Red);
        assert_eq!(theme.mix(Color::Cyan, Color::Red, 7.0), Color::Red);
        for step in 1..10 {
            let blended = theme.mix(Color::Green, Color::Cyan, step as f32 / 10.0);
            assert!(
                matches!(blended, Color::Indexed(16..=255)),
                "{step}: {blended:?}"
            );
        }
        // Half way from black to white is a mid grey.
        assert_eq!(
            theme.mix(Color::Indexed(16), Color::Indexed(231), 0.5),
            Color::Indexed(244)
        );
        // The terminal's default has nothing to blend, so it switches over.
        assert_eq!(theme.mix(Color::Reset, Color::Red, 0.2), Color::Reset);
        assert_eq!(theme.mix(Color::Reset, Color::Red, 0.8), Color::Red);
        // In truecolor a blend is exact, from the palette's own colors.
        let mocha = Theme::new(Palette::CATPPUCCIN_MOCHA, Source::Terminal, true, true);
        assert_eq!(
            mocha.mix(Color::Black, Color::White, 0.5),
            Color::Rgb(0x76, 0x7a, 0x91)
        );
    }

    #[test]
    fn on_a_dark_xterm_the_shades_are_the_fixed_greys_corgi_always_drew() {
        let roles = Theme::assumed().roles;
        assert_eq!(roles.accent, Color::Cyan);
        assert_eq!(roles.text, Color::White);
        assert_eq!(roles.popup_fill, Color::Black);
        assert_eq!(roles.badge_text, Color::Black);
        assert_eq!(roles.quiet, Color::Gray);
        assert_eq!(roles.keycap, Color::Indexed(237));
        assert_eq!(roles.rule, Color::Indexed(237));
        assert_eq!(roles.focus_bar, Color::Indexed(236));
        assert_eq!(roles.dimmed, Color::Indexed(238));
        assert_eq!(roles.dimmed_background, Color::Indexed(234));
        assert_eq!(roles.fade_from, Color::Indexed(235));
        assert_eq!(roles.flash, Color::Indexed(231));
        assert_eq!(roles.stamp, Color::LightGreen);
        assert_eq!(roles.success_frame, Color::Indexed(77));
        assert_eq!(
            roles.context_ramp.map(|color| indexed(color).unwrap()),
            [77, 113, 149, 185, 179, 173, 167]
        );
    }

    #[test]
    fn a_light_theme_draws_dark_text_on_a_light_popup() {
        let theme = Theme::new(Palette::CATPPUCCIN_LATTE, Source::Terminal, true, true);
        assert!(theme.light);
        let roles = theme.roles;
        // Roles are still the terminal's, but body text is its foreground,
        // since a light theme's white is pale.
        assert_eq!(roles.accent, Color::Cyan);
        assert_eq!(roles.text, Color::Reset);
        let fill = theme.rgb(roles.popup_fill).unwrap();
        assert!(is_light(fill), "{fill:?}");
        assert!(fill < theme.palette.background, "a step off the background");
        // The shades over the fill are darker than it, and a failure
        // flashes dark rather than white.
        let rule = theme.rgb(roles.rule).unwrap();
        assert!(rule.0 < fill.0 && rule.1 < fill.1, "{rule:?}");
        assert_eq!(roles.flash, Color::Rgb(0, 0, 0));
        assert!(!is_light(theme.rgb(roles.quiet).unwrap()));
        assert_eq!(roles.badge_text, Color::Rgb(0xef, 0xf1, 0xf5));
    }

    #[test]
    fn a_fixed_theme_draws_every_role_in_its_own_rgb() {
        let theme = Theme::catppuccin(false, true);
        let roles = theme.roles;
        assert_eq!(roles.accent, Color::Rgb(0x94, 0xe2, 0xd5));
        assert_eq!(roles.text, Color::Rgb(0xa6, 0xad, 0xc8));
        assert_eq!(roles.popup_fill, Color::Rgb(0x45, 0x47, 0x5a));
        let latte = Theme::catppuccin(true, true).roles;
        assert_eq!(latte.text, Color::Rgb(0x4c, 0x4f, 0x69));
        // Without truecolor the same roles snap to the 256 colors.
        let snapped = Theme::catppuccin(false, false).roles;
        assert!(matches!(snapped.accent, Color::Indexed(16..=255)));
    }

    #[test]
    fn the_context_ramp_runs_from_the_themes_green_to_its_red() {
        for theme in [
            Theme::new(Palette::CATPPUCCIN_MOCHA, Source::Terminal, true, true),
            Theme::new(Palette::CATPPUCCIN_LATTE, Source::Terminal, true, true),
        ] {
            let ramp = theme
                .roles
                .context_ramp
                .map(|color| theme.rgb(color).unwrap());
            let (first, last) = (ramp[0], ramp[CONTEXT_STEPS - 1]);
            assert!(first.1 > first.0, "the first step is green: {first:?}");
            assert!(last.0 > last.1, "the last step is red: {last:?}");
            // Each step is redder than the last.
            let lead = |(r, g, _): Rgb| i32::from(g) - i32::from(r);
            assert!(
                ramp.windows(2).all(|pair| lead(pair[0]) > lead(pair[1])),
                "{ramp:?}"
            );
        }
    }

    #[test]
    fn a_role_has_the_bar_widgets_tone_and_an_rgb_color_an_index() {
        let theme = Theme::assumed();
        assert_eq!(theme.tone(Color::Cyan), Some("cyan"));
        assert_eq!(theme.tone(Color::DarkGray), Some("muted"));
        assert_eq!(theme.tone(theme.roles.merge), Some("magenta"));
        assert_eq!(indexed(Color::Rgb(95, 215, 135)), Some(78));
        assert_eq!(indexed(Color::Indexed(208)), Some(208));
        assert_eq!(indexed(Color::Cyan), None);
    }

    #[test]
    fn a_scoped_theme_holds_only_while_it_runs() {
        let mocha = Theme::catppuccin(false, true);
        assert_eq!(roles().accent, Color::Cyan);
        scoped(mocha, || assert_eq!(roles().accent, mocha.roles.accent));
        assert_eq!(roles().accent, Color::Cyan);
    }
}
