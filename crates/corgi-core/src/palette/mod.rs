//! Every color Corgi draws, by what it means rather than by its value, for
//! any frontend.
//!
//! A [`Theme`] is a [`Palette`] — the terminal's foreground, background and
//! sixteen ANSI colors, as RGB — and the [`Roles`] a frontend draws with,
//! worked out from it once. Most roles are the terminal's own ANSI colors
//! ([`Color::Ansi`] 6 for the accent, and so on), so a terminal paints them
//! exactly as its theme does. The rest are shades between them, for the
//! rules and keycaps of a popup and for the effects that fade, and those are
//! blended from the palette's RGB, so they sit right on a light terminal as
//! well as a dark one. [`Theme::rgb`] gives any role's RGB, and
//! [`Theme::rgb_roles`] all of them, for a frontend that is not a terminal.
//!
//! The palette is resolved once at startup by [`resolve()`]: the active
//! Omarchy theme, else the terminal's answer to an OSC color query, else
//! Catppuccin. Until then, and in tests, the theme is xterm's defaults, the
//! palette Corgi assumed before it could read one.
//!
//! No color literal belongs anywhere else, except in tests. The brand colors
//! in [`brand`] are fixed in every theme.

pub mod resolve;

pub use resolve::resolve;

/// A color as red, green and blue.
pub type Rgb = (u8, u8, u8);

/// A color as a frontend draws it: the terminal's default, one of the
/// sixteen ANSI colors the terminal's theme paints, one of the fixed 256, or
/// an exact RGB. [`Theme::rgb`] says what any of them looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Color {
    /// The terminal's default color, foreground or background: no color of
    /// Corgi's own.
    Default,
    /// ANSI color 0 to 15: black, red, green, yellow, blue, magenta, cyan and
    /// white, then their bright variants.
    Ansi(u8),
    /// One of the 256 colors by index; 0 to 15 are the ANSI colors.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

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

/// What the dashboard draws with, in colors of type `C`: the core's
/// [`Color`], or a frontend's own (see [`Roles::map`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Roles<C = Color> {
    /// Working agents, focus, and what the user can act on.
    pub accent: C,
    /// Secondary text: separators, hints, idle states.
    pub muted: C,
    pub success: C,
    pub warning: C,
    pub danger: C,
    /// A state Corgi cannot tell.
    pub unknown: C,
    /// An agent a supervisor tagged ready for the user to merge: magenta,
    /// the one semantic color no state the user acts on uses.
    pub merge: C,
    /// An agent's thinking, apart from what it says.
    pub thinking: C,
    /// Body text.
    pub text: C,
    /// Text that is done with: quieter than body text, louder than muted.
    pub quiet: C,
    /// Text on a colored badge, such as an agent's state.
    pub badge_text: C,
    /// A popup's fill, a panel tone that lifts it off the dashboard.
    pub popup_fill: C,
    /// The cap a key is drawn on, the rule over a popup's keys, and the bar
    /// behind a form's focused row: quiet greys over the popup's fill.
    pub keycap: C,
    pub rule: C,
    pub focus_bar: C,
    /// What the dashboard's colors fade towards behind a popup, and its
    /// backgrounds.
    pub dimmed: C,
    pub dimmed_background: C,
    /// What a popup's content fades in from, just off its fill.
    pub fade_from: C,
    /// The terminal's default foreground, for an effect that fades it.
    pub foreground: C,
    /// The green a success turns a popup's frame, and the vivid green of its
    /// check stamp.
    pub success_frame: C,
    pub stamp: C,
    /// What a failure's frame pulses from: as far from the background as a
    /// color goes.
    pub flash: C,
    /// How full a context window is, from green through yellow to red.
    pub context_ramp: [C; CONTEXT_STEPS],
}

/// The terminal's default color, foreground or background: no color of
/// Corgi's own.
pub const TERMINAL_DEFAULT: Color = Color::Default;

/// The steps of [`Roles::context_ramp`].
pub const CONTEXT_STEPS: usize = 7;

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
                (true, true) => Color::Default,
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
            Color::Ansi(index as u8)
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
        match color {
            Color::Default => None,
            Color::Rgb(r, g, b) => Some((r, g, b)),
            Color::Ansi(index @ 0..=15) | Color::Indexed(index @ 0..=15) => {
                Some(self.palette.ansi[usize::from(index)])
            }
            Color::Ansi(_) => None,
            Color::Indexed(index) => Some(indexed_rgb(index)),
        }
    }

    /// Every role as RGB, for a frontend that paints its own pixels. The
    /// terminal's default, which only body text can be, is the palette's
    /// foreground.
    pub fn rgb_roles(&self) -> Roles<Rgb> {
        self.roles
            .map(|color| self.rgb(color).unwrap_or(self.palette.foreground))
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
}

impl<C: Copy> Roles<C> {
    /// The same roles in another kind of color, each mapped by `f`.
    pub fn map<D>(&self, mut f: impl FnMut(C) -> D) -> Roles<D> {
        Roles {
            accent: f(self.accent),
            muted: f(self.muted),
            success: f(self.success),
            warning: f(self.warning),
            danger: f(self.danger),
            unknown: f(self.unknown),
            merge: f(self.merge),
            thinking: f(self.thinking),
            text: f(self.text),
            quiet: f(self.quiet),
            badge_text: f(self.badge_text),
            popup_fill: f(self.popup_fill),
            keycap: f(self.keycap),
            rule: f(self.rule),
            focus_bar: f(self.focus_bar),
            dimmed: f(self.dimmed),
            dimmed_background: f(self.dimmed_background),
            fade_from: f(self.fade_from),
            foreground: f(self.foreground),
            success_frame: f(self.success_frame),
            stamp: f(self.stamp),
            flash: f(self.flash),
            context_ramp: self.context_ramp.map(f),
        }
    }

    /// Every role but the context ramp, by its field name, for a frontend
    /// that receives them as data (`corgi serve`).
    pub fn named(&self) -> Vec<(&'static str, C)> {
        vec![
            ("accent", self.accent),
            ("muted", self.muted),
            ("success", self.success),
            ("warning", self.warning),
            ("danger", self.danger),
            ("unknown", self.unknown),
            ("merge", self.merge),
            ("thinking", self.thinking),
            ("text", self.text),
            ("quiet", self.quiet),
            ("badge_text", self.badge_text),
            ("popup_fill", self.popup_fill),
            ("keycap", self.keycap),
            ("rule", self.rule),
            ("focus_bar", self.focus_bar),
            ("dimmed", self.dimmed),
            ("dimmed_background", self.dimmed_background),
            ("fade_from", self.fade_from),
            ("foreground", self.foreground),
            ("success_frame", self.success_frame),
            ("stamp", self.stamp),
            ("flash", self.flash),
        ]
    }
}

impl Roles {
    fn placeholder() -> Self {
        Self {
            accent: Color::Default,
            muted: Color::Default,
            success: Color::Default,
            warning: Color::Default,
            danger: Color::Default,
            unknown: Color::Default,
            merge: Color::Default,
            thinking: Color::Default,
            text: Color::Default,
            quiet: Color::Default,
            badge_text: Color::Default,
            popup_fill: Color::Default,
            keycap: Color::Default,
            rule: Color::Default,
            focus_bar: Color::Default,
            dimmed: Color::Default,
            dimmed_background: Color::Default,
            fade_from: Color::Default,
            foreground: Color::Default,
            success_frame: Color::Default,
            stamp: Color::Default,
            flash: Color::Default,
            context_ramp: [Color::Default; CONTEXT_STEPS],
        }
    }
}

/// The colors of the brand, fixed in every theme: the supervisor's corgi
/// would not read as a corgi in the theme's accent, and a model vendor is an
/// identity rather than a state of the session, read by hue across a
/// dashboard mixing agents.
pub mod brand {
    use super::Color;

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
    fn roles_are_the_terminals_colors_or_the_palettes_rgb_and_always_have_an_rgb() {
        let theme = Theme::new(Palette::CATPPUCCIN_MOCHA, Source::Terminal, true, true);
        assert_eq!(theme.roles.accent, Color::Ansi(6));
        assert_eq!(theme.rgb(theme.roles.accent), Some((0x94, 0xe2, 0xd5)));
        let fixed = Theme::catppuccin(false, true);
        assert_eq!(fixed.roles.accent, Color::Rgb(0x94, 0xe2, 0xd5));
        // A light terminal's body text is its default foreground, which as
        // RGB is the palette's.
        let latte = Theme::new(Palette::CATPPUCCIN_LATTE, Source::Terminal, true, true);
        assert_eq!(latte.roles.text, Color::Default);
        let rgb = latte.rgb_roles();
        assert_eq!(rgb.text, Palette::CATPPUCCIN_LATTE.foreground);
        assert_eq!(rgb.accent, Palette::CATPPUCCIN_LATTE.ansi[6]);
        assert_eq!(
            rgb.context_ramp,
            latte
                .roles
                .context_ramp
                .map(|color| latte.rgb(color).unwrap())
        );
    }
}
