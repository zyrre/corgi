//! The brand's pixel art: the supervisor's corgi and the CORGI wordmark.
//! Every frontend draws them from here, so they cannot drift apart: the
//! dashboard paints them with half blocks, two pixel rows to a terminal
//! cell, and the desktop window paints each pixel as a square.

use crate::palette::{Color, brand};

/// The mascot is pixel art: every character below is one pixel, and the
/// dashboard's half-block glyphs fit two pixel rows into one terminal cell,
/// so these fourteen rows fill seven terminal rows, the same as the header.
/// The first and last pixel rows are blank so the art can sit one *pixel*
/// below the top of the screen, which is half a terminal row. `o` is the
/// coat, `d` the inner ear, `w` the blaze, cheeks and muzzle, `k` the eyes,
/// nose and mouth corners, `h` the glint in each eye and `p` the tongue.
pub const CORGI_PIXELS: [&str; 14] = [
    "................",
    ".o............o.",
    ".oo..........oo.",
    ".odo........odo.",
    ".oddo......oddo.",
    ".oooooooooooooo.",
    ".ookkoowwookkoo.",
    ".ookhoowwoohkoo.",
    ".wwooowwwwoooww.",
    ".wwwwwwkkwwwwww.",
    "..wwwwkwwkwwww..",
    "...wwwwppwwww...",
    ".....wwwwww.....",
    "................",
];

/// The brand color of a pixel of [`CORGI_PIXELS`], `None` for a blank one.
/// The coat and its details are fixed in every theme (see [`brand`]).
pub const fn corgi_color(pixel: char) -> Option<Color> {
    match pixel {
        'o' => Some(brand::CORGI_COAT),
        'd' => Some(brand::CORGI_INNER_EAR),
        'w' => Some(brand::CORGI_WHITE),
        'k' => Some(brand::CORGI_DARK),
        'h' => Some(brand::CORGI_GLINT),
        'p' => Some(brand::CORGI_TONGUE),
        _ => None,
    }
}

/// Omarchy's own wordmark is Delta Corps Priest 1: solid three-cell stems,
/// corners chamfered with half blocks, and crossbars split into a `▄▄` over a
/// `▀▀`. This is a CORGI cut of that alphabet, shortened to five rows so the
/// mascot beside it stays the taller of the two, and set tight: a single
/// column between letters rather than the two Omarchy itself uses.
///
/// Every part of a letter is keyed to the two-column counter: the R's `▄▄`
/// and `▀▀`, the diagonal shoulder under its leg, and the G's spur. Narrowing
/// the counter means redrawing each of them, not shifting them, or the fill
/// overruns the stems and the leg's shoulder is left floating.
///
/// Each glyph is one pixel wide and two tall, as a terminal cell is: see
/// [`wordmark_pixels`].
pub const WORDMARK: [&str; 5] = [
    "▄██████ ▄██████▄ ▄███████ ▄██████▄ ▄██",
    "███ ▀██ ███  ███ ███  ███ ███      ███",
    "███     ███  ███ ███▄▄██▀ ███ ████ ███",
    "███ ▄██ ███  ███ ███▀▀██▄ ███  ███ ███",
    "██████▀ ▀██████▀ ███  ▀██ ▀██████▀ ██▀",
];

/// The pixels a wordmark glyph paints, top and bottom.
pub const fn glyph_pixels(glyph: char) -> (bool, bool) {
    match glyph {
        '█' => (true, true),
        '▀' => (true, false),
        '▄' => (false, true),
        _ => (false, false),
    }
}

/// [`WORDMARK`] as square pixels: two rows for each of its rows, `true`
/// where a letter is painted.
pub fn wordmark_pixels() -> Vec<Vec<bool>> {
    WORDMARK
        .iter()
        .flat_map(|row| {
            let (top, bottom) = row.chars().map(glyph_pixels).unzip();
            [top, bottom]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_corgi_is_a_rectangle_of_known_pixels_that_pairs_into_cells() {
        let width = CORGI_PIXELS[0].len();
        assert!(CORGI_PIXELS.iter().all(|row| row.len() == width));
        assert!(CORGI_PIXELS.iter().all(|row| {
            row.chars()
                .all(|pixel| pixel == '.' || corgi_color(pixel).is_some())
        }));
        assert_eq!(CORGI_PIXELS.len() % 2, 0, "pixel rows pair up into cells");
    }

    #[test]
    fn the_wordmark_becomes_two_square_pixel_rows_for_each_of_its_rows() {
        assert!(
            WORDMARK
                .iter()
                .flat_map(|row| row.chars())
                .all(|glyph| " ▀▄█".contains(glyph))
        );
        let pixels = wordmark_pixels();
        assert_eq!(pixels.len(), 2 * WORDMARK.len());
        assert!(pixels.iter().all(|row| row.len() == 38));
        // The C's chamfered top-left corner: its first glyph is a `▄`.
        assert_eq!((pixels[0][0], pixels[1][0]), (false, true));
        assert_eq!((pixels[0][1], pixels[1][1]), (true, true));
        assert!(
            pixels.len() < CORGI_PIXELS.len(),
            "the wordmark stays shorter than the corgi beside it"
        );
    }
}
