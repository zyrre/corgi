//! The popups' effects, drawn over what the renderers drew: the frame that
//! grows and shrinks, the content fading in, the dashboard dimming behind,
//! the outcome flash and the failure shake. Each works on the finished
//! buffer, so the popups draw their content as they always have and never
//! need to know an effect is playing.

use std::{collections::HashMap, time::Duration};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Clear, Widget},
};

use crate::motion::{FLASH, Outcome, progress, stamp_content, stamp_drawn, stamp_faded};

use super::{
    blend::{DIMMED, FADE_FROM, FLASH_WHITE, STAMP_GREEN, SUCCESS_FRAME, mix},
    dialog_block,
};

/// How far the dashboard's colors go towards [`DIMMED`] behind a popup.
const DIM_STRENGTH: f32 = 0.8;
/// The grey a dimmed background fades towards, just above black.
const DIMMED_BACKGROUND: u8 = 234;
/// What the terminal's default foreground stands in as while it fades.
const DEFAULT_FOREGROUND: u8 = 252;

/// Fades every color in `area` `amount` of the way to muted, for the
/// dashboard behind an open popup. Nothing changes at 0, so a closed popup
/// leaves the theme's colors exactly as they are.
pub(super) fn dim(buffer: &mut Buffer, area: Rect, amount: f32) {
    if amount <= 0.0 {
        return;
    }
    let t = amount.min(1.0) * DIM_STRENGTH;
    let area = area.intersection(buffer.area);
    // A screen holds only a few dozen colors, and a popup stays open for
    // many frames, so each is blended once per frame rather than per cell.
    let mut foregrounds = HashMap::new();
    let mut backgrounds = HashMap::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buffer[(x, y)];
            cell.fg = *foregrounds
                .entry(cell.fg)
                .or_insert_with(|| mix(foreground(cell.fg), Color::Indexed(DIMMED), t));
            if cell.bg != Color::Reset {
                cell.bg = *backgrounds
                    .entry(cell.bg)
                    .or_insert_with(|| mix(cell.bg, Color::Indexed(DIMMED_BACKGROUND), t));
            }
        }
    }
}

/// A popup's frame at `openness` of its full size `full`, grown from its
/// centre and holding only its fill. Returns where it was drawn.
pub(super) fn draw_frame(buffer: &mut Buffer, full: Rect, color: Color, openness: f32) -> Rect {
    let scale = |size: u16| {
        (f32::from(size) * openness)
            .round()
            .max(2.0)
            .min(f32::from(size))
    };
    let width = scale(full.width) as u16;
    let height = scale(full.height) as u16;
    let area = Rect {
        x: full.x + (full.width - width) / 2,
        y: full.y + (full.height - height) / 2,
        width,
        height,
    }
    .intersection(buffer.area);
    Clear.render(area, buffer);
    dialog_block("", color).render(area, buffer);
    area
}

/// The color of a popup's frame, `base` when no outcome effect plays. A
/// success turns green at once and eases back as its stamp fades; a failure
/// pulses bright and settles into `base`. Either ends on `base` itself.
pub(super) fn frame_color(base: Color, effect: Option<(Outcome, Duration)>) -> Color {
    match effect {
        None => base,
        Some((Outcome::Success, elapsed)) => {
            mix(Color::Indexed(SUCCESS_FRAME), base, stamp_faded(elapsed))
        }
        Some((Outcome::Failure, elapsed)) => {
            mix(Color::Indexed(FLASH_WHITE), base, progress(elapsed, FLASH))
        }
    }
}

/// A success's check stamp: a big ✓, stroke by stroke from the left.
pub(super) const CHECK: [&str; 4] = [
    "          ▄█▀",
    "        ▄█▀  ",
    "▀█▄   ▄█▀    ",
    "  ▀█▄█▀      ",
];
/// Columns kept clear either side of the stamp, so it reads as a mark
/// rather than one more glyph among the text.
const STAMP_MARGIN: u16 = 2;

/// Plays a success's check stamp `elapsed` into it, centred on `around`,
/// where its popup is or was: the ✓ is drawn column by column, holds, and
/// fades. While `open` is the popup still standing there, its content fades
/// almost out under the stamp and back in as it goes. The two columns
/// cleared either side are the popup's fill inside `frame`, the frame drawn
/// this frame if any, and the terminal's own background over the dashboard.
/// Whatever lies off the screen is not drawn.
pub(super) fn stamp(
    buffer: &mut Buffer,
    around: Rect,
    elapsed: Duration,
    open: Option<Rect>,
    frame: Option<Rect>,
) {
    if let Some(popup) = open {
        fade_inner(buffer, popup, stamp_content(elapsed));
    }
    let faded = stamp_faded(elapsed);
    if faded >= 1.0 {
        return;
    }
    let width = CHECK[0].chars().count() as i32;
    let height = CHECK.len() as i32;
    let x0 = i32::from(around.x) + (i32::from(around.width) - width) / 2;
    let y0 = i32::from(around.y) + (i32::from(around.height) - height) / 2;
    let bounds = buffer.area;
    let place = |x: i32, y: i32| -> Option<(u16, u16)> {
        let (x, y) = (u16::try_from(x).ok()?, u16::try_from(y).ok()?);
        bounds.contains((x, y).into()).then_some((x, y))
    };
    let clear = |x: u16, y: u16| {
        let mut cell = ratatui::buffer::Cell::default();
        if frame.is_some_and(|frame| frame.contains((x, y).into())) {
            cell.set_bg(Color::Black);
        }
        cell
    };
    if faded < 0.5 {
        let margin = i32::from(STAMP_MARGIN);
        for y in y0..y0 + height {
            for x in x0 - margin..x0 + width + margin {
                if let Some((x, y)) = place(x, y) {
                    buffer[(x, y)] = clear(x, y);
                }
            }
        }
    }
    let style = Style::default()
        .fg(mix(Color::Indexed(STAMP_GREEN), Color::Black, faded))
        .add_modifier(Modifier::BOLD);
    let shown = stamp_drawn(elapsed) * (width + 1) as f32;
    for (row, line) in CHECK.iter().enumerate() {
        for (column, symbol) in line.chars().enumerate() {
            if symbol == ' ' || column as f32 >= shown {
                continue;
            }
            let Some((x, y)) = place(x0 + column as i32, y0 + row as i32) else {
                continue;
            };
            let mut cell = clear(x, y);
            cell.set_char(symbol);
            cell.set_style(style);
            buffer[(x, y)] = cell;
        }
    }
}

/// Repaints the frame of the popup at `area`, and its title, from `from`
/// to `to`.
pub(super) fn recolor_frame(buffer: &mut Buffer, area: Rect, from: Color, to: Color) {
    let area = area.intersection(buffer.area);
    if area.is_empty() {
        return;
    }
    for (x, y) in perimeter(area) {
        let cell = &mut buffer[(x, y)];
        if cell.fg == from {
            cell.fg = to;
        }
    }
}

/// Fades the content of the popup at `area` in from just above its fill:
/// everything inside the frame, and the title in its top edge. The frame
/// itself already stands at its color.
pub(super) fn fade_content(buffer: &mut Buffer, area: Rect, fade: f32) {
    if fade >= 1.0 {
        return;
    }
    let area = area.intersection(buffer.area);
    if area.width < 2 || area.height < 2 {
        return;
    }
    fade_inner(buffer, area, fade);
    for x in area.left() + 1..area.right() - 1 {
        let cell = &mut buffer[(x, area.top())];
        if cell.symbol() != "─" {
            cell.fg = faded(cell.fg, fade);
        }
    }
}

/// Fades everything inside the frame of the popup at `area`, leaving the
/// frame and its title as they are.
fn fade_inner(buffer: &mut Buffer, area: Rect, fade: f32) {
    let area = area.intersection(buffer.area);
    if fade >= 1.0 || area.width < 2 || area.height < 2 {
        return;
    }
    for y in area.top() + 1..area.bottom() - 1 {
        for x in area.left() + 1..area.right() - 1 {
            let cell = &mut buffer[(x, y)];
            cell.fg = faded(cell.fg, fade);
        }
    }
}

/// `color` `fade` of the way in from just above the popup's fill.
fn faded(color: Color, fade: f32) -> Color {
    mix(Color::Indexed(FADE_FROM), foreground(color), fade)
}

/// Moves the popup at `area` `columns` sideways, putting back what was
/// under it from `under` wherever it no longer covers.
pub(super) fn shift(buffer: &mut Buffer, under: &Buffer, area: Rect, columns: i16) {
    let area = area.intersection(buffer.area);
    if columns == 0 || area.is_empty() {
        return;
    }
    for y in area.top()..area.bottom() {
        let row: Vec<_> = (area.left()..area.right())
            .map(|x| buffer[(x, y)].clone())
            .collect();
        for x in area.left()..area.right() {
            buffer[(x, y)] = under[(x, y)].clone();
        }
        for (offset, cell) in row.into_iter().enumerate() {
            let x = i32::from(area.left()) + offset as i32 + i32::from(columns);
            let Ok(x) = u16::try_from(x) else { continue };
            if x >= buffer.area.left() && x < buffer.area.right() {
                buffer[(x, y)] = cell;
            }
        }
    }
}

/// The edge cells of `area`, clockwise from its top-left corner.
fn perimeter(area: Rect) -> impl Iterator<Item = (u16, u16)> {
    let top = (area.left()..area.right()).map(move |x| (x, area.top()));
    let bottom = (area.left()..area.right()).map(move |x| (x, area.bottom() - 1));
    let sides = (area.top() + 1..area.bottom().saturating_sub(1))
        .flat_map(move |y| [(area.left(), y), (area.right() - 1, y)]);
    top.chain(sides).chain(bottom)
}

/// A foreground with a color to fade from: the terminal's own default
/// stands in as a light grey.
fn foreground(color: Color) -> Color {
    if color == Color::Reset {
        Color::Indexed(DEFAULT_FOREGROUND)
    } else {
        color
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::SHAKE;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn an_outcome_flash_starts_bright_and_ends_on_the_frames_own_color() {
        let accent = Color::Cyan;
        assert_eq!(frame_color(accent, None), accent);
        assert_eq!(
            frame_color(accent, Some((Outcome::Success, ms(0)))),
            Color::Indexed(SUCCESS_FRAME)
        );
        assert_eq!(
            frame_color(accent, Some((Outcome::Success, ms(579)))),
            Color::Indexed(SUCCESS_FRAME)
        );
        assert!(matches!(
            frame_color(accent, Some((Outcome::Success, ms(700)))),
            Color::Indexed(_)
        ));
        assert_eq!(
            frame_color(accent, Some((Outcome::Success, ms(880)))),
            accent
        );
        assert_eq!(
            frame_color(Color::Red, Some((Outcome::Failure, ms(0)))),
            Color::Indexed(FLASH_WHITE)
        );
        assert_eq!(
            frame_color(Color::Red, Some((Outcome::Failure, SHAKE.max(FLASH)))),
            Color::Red
        );
    }

    #[test]
    fn dimming_mutes_every_color_and_undimmed_leaves_them_alone() {
        let area = Rect::new(0, 0, 4, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(
            0,
            0,
            "ab",
            Style::default().fg(Color::Green).bg(Color::Blue),
        );
        let original = buffer.clone();
        dim(&mut buffer, area, 0.0);
        assert_eq!(buffer, original);

        dim(&mut buffer, area, 1.0);
        let cell = &buffer[(0, 0)];
        assert!(
            matches!(cell.fg, Color::Indexed(232..=255)),
            "{:?}",
            cell.fg
        );
        assert!(matches!(cell.bg, Color::Indexed(_)), "{:?}", cell.bg);
        // A cell with the terminal's background keeps it.
        assert_eq!(buffer[(3, 0)].bg, Color::Reset);
    }

    #[test]
    fn a_growing_frame_starts_small_in_the_middle_and_reaches_full_size() {
        let full = Rect::new(10, 4, 20, 8);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 16));
        draw_frame(&mut buffer, full, Color::Cyan, 0.0);
        assert_eq!(buffer[(19, 7)].symbol(), "╭");
        assert_eq!(buffer[(20, 8)].symbol(), "╯");
        assert_eq!(buffer[(10, 4)].symbol(), " ");

        let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 16));
        draw_frame(&mut buffer, full, Color::Cyan, 1.0);
        assert_eq!(buffer[(10, 4)].symbol(), "╭");
        assert_eq!(buffer[(29, 11)].symbol(), "╯");
        assert_eq!(buffer[(15, 6)].bg, Color::Black);
    }

    #[test]
    fn a_check_stamp_is_drawn_from_the_left_holds_and_fades_away() {
        let area = Rect::new(0, 0, 30, 8);
        let panel = || {
            let mut buffer = Buffer::empty(area);
            dialog_block(" ✓ Done ", Color::Cyan).render(area, &mut buffer);
            for y in 1..7 {
                buffer.set_string(1, y, "x".repeat(28), Style::default().fg(Color::White));
            }
            buffer
        };
        let row = |buffer: &Buffer, y: u16| -> String {
            (0..30)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        };
        // Early on, only the short arm is drawn, bold in the vivid green,
        // its margin cleared and the content fading out round it.
        let mut buffer = panel();
        stamp(&mut buffer, area, ms(40), Some(area), Some(area));
        assert_eq!(row(&buffer, 4), format!("│xxxxx  {:<13}  xxxxxx│", "▀█▄"));
        assert_eq!(row(&buffer, 2), format!("│xxxxx  {:13}  xxxxxx│", ""));
        assert_eq!(buffer[(8, 4)].fg, Color::Indexed(STAMP_GREEN));
        assert!(buffer[(8, 4)].modifier.contains(Modifier::BOLD));
        assert_eq!(buffer[(7, 4)].bg, Color::Black, "the margin is the fill");
        assert_ne!(buffer[(1, 1)].fg, Color::White);

        // Drawn and holding: the whole ✓ in the middle, over content almost
        // gone. The frame and its title are left to the frame's color.
        let mut buffer = panel();
        stamp(&mut buffer, area, ms(400), Some(area), Some(area));
        for (offset, line) in CHECK.iter().enumerate() {
            assert_eq!(
                row(&buffer, 2 + offset as u16),
                format!("│xxxxx  {line}  xxxxxx│")
            );
        }
        assert_eq!(buffer[(1, 1)].fg, Color::Indexed(FADE_FROM));
        assert_eq!(buffer[(3, 0)].fg, Color::Cyan);

        // Faded out, the panel is exactly as it was.
        let mut buffer = panel();
        stamp(
            &mut buffer,
            area,
            crate::motion::STAMP_LENGTH,
            Some(area),
            Some(area),
        );
        assert_eq!(buffer, panel());
    }

    #[test]
    fn a_stamp_over_the_dashboard_clears_to_its_background_and_clips_at_the_edge() {
        let screen = Rect::new(0, 0, 12, 6);
        let mut buffer = Buffer::empty(screen);
        buffer.set_string(0, 1, "dashboard!!!", Style::default().fg(Color::Gray));
        // Where a small popup was, in the corner: the stamp is centred there
        // and simply cut off by the screen.
        stamp(&mut buffer, Rect::new(0, 0, 6, 4), ms(400), None, None);
        let row = |y: u16| -> String {
            (0..12)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        };
        assert_eq!(row(0), "       ▄█▀  ");
        assert_eq!(row(1), "     ▄█▀    ", "the dashboard's text is cleared");
        assert_eq!(row(2), "   ▄█▀      ");
        assert_eq!(row(3), "█▄█▀        ", "the short arm is off the screen");
        assert_eq!(row(4), format!("{:12}", ""));
        assert_eq!(buffer[(0, 1)].bg, Color::Reset);
        assert_eq!(buffer[(5, 1)].fg, Color::Indexed(STAMP_GREEN));
    }

    #[test]
    fn a_shake_moves_the_popup_and_uncovers_what_was_under_it() {
        let area = Rect::new(0, 0, 8, 1);
        let mut under = Buffer::empty(area);
        under.set_string(0, 0, "........", Style::default());
        let mut buffer = under.clone();
        buffer.set_string(2, 0, "ab", Style::default());
        shift(&mut buffer, &under, Rect::new(2, 0, 2, 1), 2);
        let text: String = (0..8)
            .map(|x| buffer[(x, 0)].symbol().to_string())
            .collect();
        assert_eq!(text, "....ab..");
    }

    #[test]
    fn fading_content_leaves_the_frame_and_ends_on_the_real_colors() {
        let area = Rect::new(0, 0, 10, 4);
        let mut buffer = Buffer::empty(area);
        dialog_block(" ◆ T ", Color::Cyan).render(area, &mut buffer);
        buffer.set_string(2, 1, "hi", Style::default().fg(Color::White));
        let settled = buffer.clone();
        fade_content(&mut buffer, area, 1.0);
        assert_eq!(buffer, settled);

        fade_content(&mut buffer, area, 0.0);
        assert_eq!(buffer[(2, 1)].fg, Color::Indexed(FADE_FROM));
        assert_eq!(buffer[(0, 1)].fg, Color::Cyan, "the frame keeps its color");
        assert_eq!(buffer[(7, 0)].fg, Color::Cyan, "the rule keeps its color");
    }
}
