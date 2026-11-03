//! TEMPORARY: three mocked-up looks for the overview's selected row, an
//! outline in place of today's shaded bar, picked by `CORGI_GUI_ROW_LOOK`
//! (`a`, `b` or `c`; anything else, or nothing, draws today's look) so one
//! build renders them all. Removed once the user picks one.
//!
//! Every look draws its outline on a frame around the row that every row
//! has, transparent until selected, so selecting a row moves no text and
//! changes no row's height.

use std::sync::OnceLock;

use gpui::Hsla;

use crate::theme::{self, Colors};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowLook {
    /// Today's: the row shaded with the selection bar's grey.
    Baseline,
    /// A 1 px accent outline with rounded corners, no fill.
    Hairline,
    /// A 2 px accent outline over a faint accent tint.
    Tinted,
    /// A 1 px outline in the row's own color (the supervisor's for a
    /// supervisor) and today's 4 px bar at its left, no fill.
    Barred,
}

impl RowLook {
    pub fn get() -> Self {
        static LOOK: OnceLock<RowLook> = OnceLock::new();
        *LOOK.get_or_init(|| {
            match std::env::var("CORGI_GUI_ROW_LOOK")
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str()
            {
                "a" => RowLook::Hairline,
                "b" => RowLook::Tinted,
                "c" => RowLook::Barred,
                _ => RowLook::Baseline,
            }
        })
    }

    /// The fill a selected row's text sits on.
    pub fn fill(self, colors: &Colors) -> Hsla {
        match self {
            RowLook::Baseline => colors.roles.focus_bar,
            RowLook::Hairline | RowLook::Barred => colors.background,
            RowLook::Tinted => tint(colors),
        }
    }

    /// Whether the outline is 2 px rather than 1.
    pub fn thick(self) -> bool {
        self == RowLook::Tinted
    }

    /// The outline's color on a selected row.
    pub fn outline(self, colors: &Colors, supervisor: bool) -> Hsla {
        match self {
            RowLook::Barred if supervisor => colors.supervisor(),
            _ => colors.roles.accent,
        }
    }

    /// Whether a selected worker keeps today's accent bar at its left.
    pub fn worker_bar(self) -> bool {
        matches!(self, RowLook::Baseline | RowLook::Barred)
    }
}

/// The accent laid faintly over the background, opaque, so secondary text
/// can be checked against it.
fn tint(colors: &Colors) -> Hsla {
    let amount = if colors.light { 0.08 } else { 0.12 };
    let (bg, accent) = (
        theme::rgb(colors.background),
        theme::rgb(colors.roles.accent),
    );
    let mix = |b: u8, a: u8| (f32::from(b) + (f32::from(a) - f32::from(b)) * amount).round() as u8;
    theme::hsla((
        mix(bg.0, accent.0),
        mix(bg.1, accent.1),
        mix(bg.2, accent.2),
    ))
}

#[cfg(test)]
mod tests {
    use corgi_core::protocol::PaletteSet;

    use super::*;

    #[test]
    fn every_looks_selected_row_keeps_its_text_readable() {
        for light in [false, true] {
            let colors = Colors::new(&PaletteSet::default(), light, false);
            // Today's rows: the selected one on the selection grey, the
            // others on the background. Mocha's grey turns secondary text
            // to quiet; an unfilled row keeps muted, as unselected rows do.
            let background = colors.background;
            let today = [RowLook::Baseline.fill(&colors), background];
            let least = |color: fn(&Colors, Hsla) -> Hsla| {
                today
                    .iter()
                    .map(|&fill| theme::contrast(color(&colors, fill), fill))
                    .fold(f32::MAX, f32::min)
            };
            let today_text = least(|colors, _| colors.roles.text);
            let today_muted = least(|colors, fill| colors.secondary(fill));
            for look in [RowLook::Hairline, RowLook::Tinted, RowLook::Barred] {
                let fill = look.fill(&colors);
                let text = theme::contrast(colors.roles.text, fill);
                let muted = theme::contrast(colors.secondary(fill), fill);
                let outline = theme::contrast(look.outline(&colors, false), background);
                assert!(text >= today_text, "{light} {look:?} text {text}");
                assert!(muted >= today_muted, "{light} {look:?} secondary {muted}");
                assert!(outline >= 2.0, "{light} {look:?} outline {outline}");
            }
        }
    }
}
