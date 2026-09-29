//! Colors between colors, for effects that fade. A blend is worked out in
//! RGB and snapped back into the fixed 256-color cube or grey ramp, so it
//! looks the same in every theme. A theme color has no RGB Corgi can know,
//! so while an effect runs it stands in as the cube color nearest xterm's
//! default for it; an effect that ends lands on the theme color itself.

use ratatui::style::Color;

/// The grey the dashboard fades towards behind a popup.
pub(super) const DIMMED: u8 = 238;
/// The grey a popup's content fades in from, just above its black fill.
pub(super) const FADE_FROM: u8 = 235;
/// The green a success turns the frame, the vivid green of its check
/// stamp, and the bright white a failure pulses from.
pub(super) const SUCCESS_FRAME: u8 = 78;
pub(super) const STAMP_GREEN: u8 = 46;
pub(super) const FLASH_WHITE: u8 = 231;

/// The channel levels of the 6×6×6 color cube.
const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The RGB an effect works with for `color`: exact for RGB and cube colors,
/// xterm's defaults for the sixteen theme colors, and `None` for the
/// terminal's own default, which has no color to blend.
pub(super) fn rgb(color: Color) -> Option<(u8, u8, u8)> {
    Some(match color {
        Color::Reset => return None,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(index) => indexed_rgb(index),
        Color::Black => indexed_rgb(0),
        Color::Red => indexed_rgb(1),
        Color::Green => indexed_rgb(2),
        Color::Yellow => indexed_rgb(3),
        Color::Blue => indexed_rgb(4),
        Color::Magenta => indexed_rgb(5),
        Color::Cyan => indexed_rgb(6),
        Color::Gray => indexed_rgb(7),
        Color::DarkGray => indexed_rgb(8),
        Color::LightRed => indexed_rgb(9),
        Color::LightGreen => indexed_rgb(10),
        Color::LightYellow => indexed_rgb(11),
        Color::LightBlue => indexed_rgb(12),
        Color::LightMagenta => indexed_rgb(13),
        Color::LightCyan => indexed_rgb(14),
        Color::White => indexed_rgb(15),
    })
}

/// The RGB of one of the 256 indexed colors, the sixteen theme colors by
/// xterm's defaults.
fn indexed_rgb(index: u8) -> (u8, u8, u8) {
    const BASE: [(u8, u8, u8); 16] = [
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
    ];
    match index {
        0..=15 => BASE[usize::from(index)],
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
pub(super) fn nearest((r, g, b): (u8, u8, u8)) -> u8 {
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

/// `from` blended `t` of the way to `to`, snapped to the fixed palette. The
/// ends are the colors themselves, so an effect that has finished leaves a
/// theme color exactly as it was. A color without RGB, the terminal's
/// default, is left as it is.
pub(super) fn mix(from: Color, to: Color, t: f32) -> Color {
    if t <= 0.0 {
        return from;
    }
    if t >= 1.0 {
        return to;
    }
    let (Some(a), Some(b)) = (rgb(from), rgb(to)) else {
        return if t < 0.5 { from } else { to };
    };
    let channel = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color::Indexed(nearest((
        channel(a.0, b.0),
        channel(a.1, b.1),
        channel(a.2, b.2),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_has_the_rgb_of_the_cube_and_ramp() {
        assert_eq!(rgb(Color::Indexed(16)), Some((0, 0, 0)));
        assert_eq!(rgb(Color::Indexed(231)), Some((255, 255, 255)));
        assert_eq!(rgb(Color::Indexed(78)), Some((95, 215, 135)));
        assert_eq!(rgb(Color::Indexed(232)), Some((8, 8, 8)));
        assert_eq!(rgb(Color::Indexed(255)), Some((238, 238, 238)));
        assert_eq!(rgb(Color::Cyan), Some((0, 205, 205)));
        assert_eq!(rgb(Color::Reset), None);
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
    fn a_blend_ends_on_its_colors_and_stays_in_the_fixed_palette_between() {
        assert_eq!(mix(Color::Cyan, Color::Red, 0.0), Color::Cyan);
        assert_eq!(mix(Color::Cyan, Color::Red, 1.0), Color::Red);
        assert_eq!(mix(Color::Cyan, Color::Red, 7.0), Color::Red);
        for step in 1..10 {
            let blended = mix(
                Color::Indexed(SUCCESS_FRAME),
                Color::Cyan,
                step as f32 / 10.0,
            );
            assert!(
                matches!(blended, Color::Indexed(16..=255)),
                "{step}: {blended:?}"
            );
        }
        // Half way from black to white is a mid grey.
        assert_eq!(
            mix(Color::Indexed(16), Color::Indexed(231), 0.5),
            Color::Indexed(244)
        );
        // The terminal's default has nothing to blend, so it switches over.
        assert_eq!(mix(Color::Reset, Color::Red, 0.2), Color::Reset);
        assert_eq!(mix(Color::Reset, Color::Red, 0.8), Color::Red);
    }
}
