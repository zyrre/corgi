// Adapted from penso/herdr-gpui (Herdr UI) crates/herdr-gpui/src/config/theme.rs
// and config/font_weight.rs at a5b2df2 (2026-10-28). Copyright 2026 Fabien
// Penso, licensed under the Apache License, Version 2.0: see LICENSE-APACHE and
// NOTICE in crates/corgi-gui. Modified for Corgi: only `Theme` (its fields, its
// default and `primary`) and the bold weight step are kept, with `Contrast`
// reduced to a stub; Corgi fills `Theme` from its own palette.

use gpui::FontWeight;

/// Stub of herdr-gpui's `contrast::Contrast` (only `Standard` is used here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contrast {
    Standard,
    High,
}

/// Colors are packed 24-bit RGB, without an alpha channel.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub background: u32,
    pub foreground: u32,
    pub cursor: u32,
    pub bold: Option<u32>,
    pub surface: u32,
    pub active: u32,
    pub muted: u32,
    /// Herdr's optional `sidebar_bg`, which colors only the sidebar. Unset,
    /// the sidebar stays on [`Self::surface`].
    pub sidebar: Option<u32>,
    /// A desktop theme's own accent, as Omarchy's `colors.toml` names one.
    /// Unset, [`Self::primary`] takes the palette's magenta.
    pub accent: Option<u32>,
    pub palette: [u32; 256],
    /// Applied by [`Theme::with_contrast`]; every theme loads as `Standard`.
    pub contrast: Contrast,
    /// The system accent standing in for the theme's, from
    /// [`Theme::with_accent`]; every theme loads without one.
    pub system_accent: Option<u32>,
}

impl Default for Theme {
    fn default() -> Self {
        let mut palette = [0; 256];
        palette[..16].copy_from_slice(&[
            0x000000, 0x800000, 0x008000, 0x808000, 0x000080, 0x800080, 0x008080, 0xc0c0c0,
            0x808080, 0xff0000, 0x00ff00, 0xffff00, 0x0000ff, 0xff00ff, 0x00ffff, 0xffffff,
        ]);
        for (index, color) in palette.iter_mut().enumerate().skip(16) {
            let n = index as u32;
            *color = if n < 232 {
                let n = n - 16;
                let level = |v| if v == 0 { 0 } else { 55 + v * 40 };
                (level(n / 36) << 16) | (level(n / 6 % 6) << 8) | level(n % 6)
            } else {
                (8 + (n - 232) * 10) * 0x010101
            };
        }
        Self {
            background: 0x101419,
            foreground: 0xd8dee9,
            cursor: 0xd8dee9,
            bold: None,
            surface: 0x1c1c22,
            active: 0x2b2933,
            muted: 0x827e91,
            sidebar: None,
            accent: None,
            palette,
            contrast: Contrast::Standard,
            system_accent: None,
        }
    }
}

impl Theme {
    /// The theme's primary accent, used for selection colors that must read as
    /// chosen rather than merely hovered: the system's when asked for, else
    /// the theme's own, else its palette's magenta.
    pub fn primary(&self) -> u32 {
        self.system_accent
            .or(self.accent)
            .unwrap_or(self.palette[5])
    }
}

/// How far bold cells sit above the configured weight: regular text's 400
/// becomes the conventional 700.
const BOLD_STEP: f32 = 300.0;

/// The weight bold cells draw at: a fixed step heavier than `weight`, never
/// lighter than bold, so bold stays visible over a medium or light base.
pub fn bold_font_weight(weight: FontWeight) -> FontWeight {
    FontWeight((weight.0 + BOLD_STEP).clamp(FontWeight::BOLD.0, FontWeight::BLACK.0))
}
