//! Every color Corgi draws, by what it means rather than by its value.
//!
//! A [`Theme`] is a [`Palette`] — the terminal's foreground, background and
//! sixteen ANSI colors, as RGB — and the roles the dashboard draws with,
//! worked out from it once. Most roles are the terminal's own ANSI colors
//! (`Color::Cyan` for the accent, and so on), so they look exactly as the
//! terminal's theme paints them. The rest are shades between them, for the
//! rules and keycaps of a popup and for the effects that fade, and those are
//! blended from the palette's RGB, so they sit right on a light terminal as
//! well as a dark one.
//!
//! The palette is resolved once at startup by [`resolve`]: the active
//! Omarchy theme, else the terminal's answer to an OSC color query, else
//! Catppuccin. Until then, and in tests, the theme is xterm's defaults, the
//! palette Corgi assumed before it could read one.
//!
//! No color literal belongs anywhere else in the crate, except in tests.
//! The brand colors in [`brand`] are fixed in every theme.

use std::{cell::RefCell, sync::OnceLock};

use ratatui::style::Color;

pub(crate) mod resolve;

pub use resolve::resolve;

/// A color as red, green and blue.
pub type Rgb = (u8, u8, u8);

/// The colors a terminal is themed with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub foreground: Rgb,
    pub background: Rgb,
    /// Black, red, green, yellow, blue, magenta, cyan and white, then their
    /// bright variants.
    pub ansi: [Rgb; 16],
}

impl Palette {
    /// xterm's defaults: what Corgi assumed of every terminal before it
    /// could read one, and what a dark xterm-like terminal still looks like.
    pub const XTERM: Self = Self {
        foreground: (229, 229, 229),
        background: (0, 0, 0),
        ansi: [
            (0, 0, 0),
            (205, 0, 0),
            (0, 205, 0),
            (205, 205, 0),
            (0, 0, 238),
            (205, 0, 205),
            (0, 205, 205),
            (229, 229, 229),
            (127, 127, 127),
            (255, 0, 0),
            (0, 255, 0),
            (255, 255, 0),
            (92, 92, 255),
            (255, 0, 255),
            (0, 255, 255),
            (255, 255, 255),
        ],
    };

    /// Catppuccin Mocha, the fallback on a dark terminal.
    pub const CATPPUCCIN_MOCHA: Self = Self {
        foreground: (0xcd, 0xd6, 0xf4),
        background: (0x1e, 0x1e, 0x2e),
        ansi: [
            (0x45, 0x47, 0x5a),
            (0xf3, 0x8b, 0xa8),
            (0xa6, 0xe3, 0xa1),
            (0xf9, 0xe2, 0xaf),
            (0x89, 0xb4, 0xfa),
            (0xf5, 0xc2, 0xe7),
            (0x94, 0xe2, 0xd5),
            (0xba, 0xc2, 0xde),
            (0x58, 0x5b, 0x70),
            (0xf3, 0x8b, 0xa8),
            (0xa6, 0xe3, 0xa1),
            (0xf9, 0xe2, 0xaf),
            (0x89, 0xb4, 0xfa),
            (0xf5, 0xc2, 0xe7),
            (0x94, 0xe2, 0xd5),
            (0xa6, 0xad, 0xc8),
        ],
    };

    /// Catppuccin Latte, the fallback on a light terminal.
    pub const CATPPUCCIN_LATTE: Self = Self {
        foreground: (0x4c, 0x4f, 0x69),
        background: (0xef, 0xf1, 0xf5),
        ansi: [
            (0x5c, 0x5f, 0x77),
            (0xd2, 0x0f, 0x39),
            (0x40, 0xa0, 0x2b),
            (0xdf, 0x8e, 0x1d),
            (0x1e, 0x66, 0xf5),
            (0xea, 0x76, 0xcb),
            (0x17, 0x92, 0x99),
            (0xac, 0xb0, 0xbe),
            (0x6c, 0x6f, 0x85),
            (0xd2, 0x0f, 0x39),
            (0x40, 0xa0, 0x2b),
            (0xdf, 0x8e, 0x1d),
            (0x1e, 0x66, 0xf5),
            (0xea, 0x76, 0xcb),
            (0x17, 0x92, 0x99),
            (0xbc, 0xc0, 0xcc),
        ],
    };

    /// Catppuccin in the flavor that suits a light or a dark terminal.
    pub fn catppuccin(light: bool) -> Self {
        if light {
            Self::CATPPUCCIN_LATTE
        } else {
            Self::CATPPUCCIN_MOCHA
        }
    }

    /// Whether the background is light, by its perceived brightness.
    pub fn is_light(&self) -> bool {
        is_light(self.background)
    }
}

/// Whether `rgb` is a light color, by its perceived brightness.
pub fn is_light((r, g, b): Rgb) -> bool {
    0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b) > 127.5
}

/// Where a theme's palette came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Nothing was read: xterm's defaults, as before any theme was resolved.
    Assumed,
    /// The active Omarchy theme.
    Omarchy,
    /// The terminal's answer to the OSC color queries.
    Terminal,
    /// Catppuccin, because nothing could be read, or because it was asked
    /// for with `CORGI_THEME`.
    Catppuccin,
}

/// The palette and the roles worked out from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub palette: Palette,
    pub source: Source,
    /// Whether the background is light.
    pub light: bool,
    /// Whether roles are the terminal's own ANSI colors, which the terminal
    /// paints from its theme, rather than the palette's RGB. They are
    /// whenever the palette is the terminal's own, read or presumed.
    pub terminal_colors: bool,
    /// Whether shades are drawn in 24-bit RGB rather than snapped to the
    /// fixed 256-color cube.
    pub truecolor: bool,
    pub roles: Roles,
}

/// What the dashboard draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Roles {
    /// Working agents, focus, and what the user can act on.
    pub accent: Color,
    /// Secondary text: separators, hints, idle states.
    pub muted: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    /// A state Corgi cannot tell.
    pub unknown: Color,
    /// An agent a supervisor tagged ready for the user to merge: magenta,
    /// the one semantic color no state the user acts on uses.
    pub merge: Color,
    /// An agent's thinking, apart from what it says.
    pub thinking: Color,
    /// Body text.
    pub text: Color,
    /// Text that is done with: quieter than body text, louder than muted.
    pub quiet: Color,
    /// Text on a colored badge, such as an agent's state.
    pub badge_text: Color,
    /// A popup's fill, a panel tone that lifts it off the dashboard.
    pub popup_fill: Color,
    /// The cap a key is drawn on, the rule over a popup's keys, and the bar
    /// behind a form's focused row: quiet greys over the popup's fill.
    pub keycap: Color,
    pub rule: Color,
    pub focus_bar: Color,
    /// What the dashboard's colors fade towards behind a popup, and its
    /// backgrounds.
    pub dimmed: Color,
    pub dimmed_background: Color,
    /// What a popup's content fades in from, just off its fill.
    pub fade_from: Color,
    /// The terminal's default foreground, for an effect that fades it.
    pub foreground: Color,
    /// The green a success turns a popup's frame, and the vivid green of its
    /// check stamp.
    pub success_frame: Color,
    pub stamp: Color,
    /// What a failure's frame pulses from: as far from the background as a
    /// color goes.
    pub flash: Color,
    /// How full a context window is, from green through yellow to red.
    pub context_ramp: [Color; CONTEXT_STEPS],
}

/// The terminal's default color, foreground or background: no color of
/// Corgi's own.
pub const TERMINAL_DEFAULT: Color = Color::Reset;

/// The steps of [`Roles::context_ramp`].
pub const CONTEXT_STEPS: usize = 7;

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
const BLACK: usize = 0;
const RED: usize = 1;
const GREEN: usize = 2;
const YELLOW: usize = 3;
const MAGENTA: usize = 5;
const CYAN: usize = 6;
const WHITE: usize = 7;
const BRIGHT_BLACK: usize = 8;
const BRIGHT_GREEN: usize = 10;
const BRIGHT_BLUE: usize = 12;
const BRIGHT_WHITE: usize = 15;

impl Theme {
    /// The theme of `palette`. With `terminal_colors`, roles are the
    /// terminal's ANSI colors and only shades come from the palette's RGB.
    pub fn new(palette: Palette, source: Source, terminal_colors: bool, truecolor: bool) -> Self {
        let mut theme = Self {
            palette,
            source,
            light: palette.is_light(),
            terminal_colors,
            truecolor,
            roles: Roles::placeholder(),
        };
        theme.roles = theme.derive();
        theme
    }

    /// xterm's defaults in the terminal's own colors, snapped to the 256
    /// colors: the theme before one is resolved, and in tests.
    pub fn assumed() -> Self {
        Self::new(Palette::XTERM, Source::Assumed, true, false)
    }

    /// Catppuccin in its own RGB rather than the terminal's colors, for a
    /// fixed look whatever the terminal is themed with.
    pub fn catppuccin(light: bool, truecolor: bool) -> Self {
        Self::new(
            Palette::catppuccin(light),
            Source::Catppuccin,
            false,
            truecolor,
        )
    }

    fn derive(&self) -> Roles {
        let palette = &self.palette;
        let (foreground, background) = (palette.foreground, palette.background);
        let named = |index: usize| self.named(index);
        let shade = |rgb: Rgb| self.shade(rgb);
        // A dark theme's black is a panel tone just off its background, as
        // popups have always been filled; a light theme's black is dark,
        // so its popups are filled a step from the background instead.
        let (popup_fill, fill) = if self.light {
            let fill = blend(background, foreground, 0.06);
            (shade(fill), fill)
        } else {
            (named(BLACK), palette.ansi[BLACK])
        };
        // A theme color taken towards the foreground as far as it is
        // saturated: xterm's pure green loses its edge, a pastel theme's
        // stays nearly as it is, and on a light theme the color deepens, so
        // either way it reads without shouting.
        let soften = |rgb: Rgb| blend(rgb, foreground, 0.3 * saturation(rgb));
        let context_ramp = std::array::from_fn(|step| {
            let half = (CONTEXT_STEPS - 1) / 2;
            let rgb = if step <= half {
                blend(
                    palette.ansi[GREEN],
                    palette.ansi[YELLOW],
                    step as f32 / half as f32,
                )
            } else {
                blend(
                    palette.ansi[YELLOW],
                    palette.ansi[RED],
                    (step - half) as f32 / half as f32,
                )
            };
            shade(soften(rgb))
        });
        Roles {
            accent: named(CYAN),
            muted: named(BRIGHT_BLACK),
            success: named(GREEN),
            warning: named(YELLOW),
            danger: named(RED),
            unknown: named(MAGENTA),
            merge: named(MAGENTA),
            thinking: named(BRIGHT_BLUE),
            // A light theme's whites are pale, so its text is the
            // terminal's own foreground.
            text: match (self.light, self.terminal_colors) {
                (false, _) => named(BRIGHT_WHITE),
                (true, true) => Color::Reset,
                (true, false) => shade(foreground),
            },
            quiet: if self.light {
                shade(blend(foreground, background, 0.25))
            } else {
                named(WHITE)
            },
            badge_text: if self.light {
                shade(background)
            } else {
                named(BLACK)
            },
            popup_fill,
            keycap: shade(blend(fill, foreground, 0.25)),
            rule: shade(blend(fill, foreground, 0.25)),
            focus_bar: shade(blend(fill, foreground, 0.2)),
            dimmed: shade(blend(background, foreground, 0.3)),
            dimmed_background: shade(blend(background, foreground, 0.12)),
            fade_from: shade(blend(fill, foreground, 0.16)),
            foreground: shade(foreground),
            success_frame: shade(soften(palette.ansi[GREEN])),
            stamp: named(BRIGHT_GREEN),
            flash: shade(if self.light {
                (0, 0, 0)
            } else {
                (255, 255, 255)
            }),
            context_ramp,
        }
    }

    /// ANSI color `index` as a role: the terminal's own when it paints the
    /// palette, else the palette's RGB.
    fn named(&self, index: usize) -> Color {
        if self.terminal_colors {
            ANSI_NAMES[index]
        } else {
            self.shade(self.palette.ansi[index])
        }
    }

    /// `rgb` as the terminal can draw it: exactly in truecolor, else the
    /// nearest of the fixed 256 colors.
    pub fn shade(&self, rgb: Rgb) -> Color {
        if self.truecolor {
            Color::Rgb(rgb.0, rgb.1, rgb.2)
        } else {
            Color::Indexed(nearest(rgb))
        }
    }

    /// The RGB of `color` in this theme: the palette's for the sixteen ANSI
    /// colors, exact for RGB and the fixed 256, and `None` for the
    /// terminal's default, which is the foreground or the background
    /// depending on where it is drawn.
    pub fn rgb(&self, color: Color) -> Option<Rgb> {
        if let Some(index) = ANSI_NAMES.iter().position(|name| *name == color) {
            return Some(self.palette.ansi[index]);
        }
        match color {
            Color::Reset => None,
            Color::Rgb(r, g, b) => Some((r, g, b)),
            Color::Indexed(index @ 0..=15) => Some(self.palette.ansi[usize::from(index)]),
            Color::Indexed(index) => Some(indexed_rgb(index)),
            _ => None,
        }
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
        let (Some(a), Some(b)) = (self.rgb(from), self.rgb(to)) else {
            return if t < 0.5 { from } else { to };
        };
        self.shade(blend(a, b, t))
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

impl Roles {
    fn placeholder() -> Self {
        Self {
            accent: Color::Reset,
            muted: Color::Reset,
            success: Color::Reset,
            warning: Color::Reset,
            danger: Color::Reset,
            unknown: Color::Reset,
            merge: Color::Reset,
            thinking: Color::Reset,
            text: Color::Reset,
            quiet: Color::Reset,
            badge_text: Color::Reset,
            popup_fill: Color::Reset,
            keycap: Color::Reset,
            rule: Color::Reset,
            focus_bar: Color::Reset,
            dimmed: Color::Reset,
            dimmed_background: Color::Reset,
            fade_from: Color::Reset,
            foreground: Color::Reset,
            success_frame: Color::Reset,
            stamp: Color::Reset,
            flash: Color::Reset,
            context_ramp: [Color::Reset; CONTEXT_STEPS],
        }
    }
}

/// The colors of the brand, fixed in every theme: the supervisor's corgi
/// would not read as a corgi in the theme's accent, and a model vendor is an
/// identity rather than a state of the session, read by hue across a
/// dashboard mixing agents.
pub mod brand {
    use ratatui::style::Color;

    pub const CORGI_COAT: Color = Color::Indexed(208);
    pub const CORGI_INNER_EAR: Color = Color::Indexed(166);
    pub const CORGI_WHITE: Color = Color::Indexed(255);
    pub const CORGI_DARK: Color = Color::Indexed(16);
    // Brighter than the muzzle so the glint reads as a reflection, not fur.
    pub const CORGI_GLINT: Color = Color::Indexed(231);
    pub const CORGI_TONGUE: Color = Color::Indexed(210);

    /// Model vendors, by the words their model names and agent kinds
    /// contain. Anthropic keeps its orange, OpenAI its blue.
    pub const VENDORS: [(&[&str], Color); 8] = [
        (
            &["claude", "opus", "sonnet", "haiku", "fable"],
            Color::Indexed(208),
        ),
        (&["gpt", "codex"], Color::Indexed(39)),
        (&["gemini", "gemma"], Color::Indexed(105)),
        (&["llama"], Color::Indexed(63)),
        (&["mistral", "codestral", "devstral"], Color::Indexed(203)),
        (&["grok"], Color::Indexed(250)),
        (&["deepseek"], Color::Indexed(43)),
        (&["qwen", "kimi", "glm"], Color::Indexed(135)),
    ];
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

/// How saturated `rgb` is, from 0 for a grey to 1 for a pure hue.
fn saturation((r, g, b): Rgb) -> f32 {
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    if max == 0 {
        0.0
    } else {
        f32::from(max - min) / f32::from(max)
    }
}

/// `a` blended `t` of the way to `b`.
pub fn blend(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    (channel(a.0, b.0), channel(a.1, b.1), channel(a.2, b.2))
}

/// The channel levels of the 6×6×6 color cube.
const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The RGB of one of the fixed 240 colors past the sixteen ANSI ones: the
/// color cube and the grey ramp, the same in every terminal.
pub fn indexed_rgb(index: u8) -> Rgb {
    match index {
        0..=15 => Palette::XTERM.ansi[usize::from(index)],
        16..=231 => {
            let cube = index - 16;
            (
                CUBE_LEVELS[usize::from(cube / 36)],
                CUBE_LEVELS[usize::from(cube / 6 % 6)],
                CUBE_LEVELS[usize::from(cube % 6)],
            )
        }
        _ => {
            let grey = 8 + 10 * (index - 232);
            (grey, grey, grey)
        }
    }
}

/// The fixed color nearest `rgb`: the nearer of the cube and the grey ramp.
/// The cube is a grid, so its nearest point is each channel's nearest level;
/// the ramp's nearest step is the one nearest the channels' mean.
pub fn nearest((r, g, b): Rgb) -> u8 {
    let level = |channel: u8| {
        (0..CUBE_LEVELS.len())
            .min_by_key(|&index| CUBE_LEVELS[index].abs_diff(channel))
            .unwrap_or(0) as u8
    };
    let cube = 16 + 36 * level(r) + 6 * level(g) + level(b);
    let mean = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    let grey = 232 + (mean.saturating_sub(3) / 10).min(23) as u8;
    let distance = |index: u8| {
        let (cr, cg, cb) = indexed_rgb(index);
        let square = |a: u8, b: u8| u32::from(a.abs_diff(b)).pow(2);
        square(r, cr) + square(g, cg) + square(b, cb)
    };
    if distance(grey) < distance(cube) {
        grey
    } else {
        cube
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
