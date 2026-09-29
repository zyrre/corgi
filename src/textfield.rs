//! Word wrapping for the composer's growing text fields, and the caret
//! arithmetic that lets one be edited along the rows the reader sees.
//!
//! A field holds one plain string, so a caret is a byte offset into it. The
//! rows it is drawn on exist only at the width it was last drawn at, which is
//! why wrapping reports byte ranges: the renderer slices the text with them,
//! and vertical caret moves translate between an offset and the row and
//! column it landed on.

use std::ops::Range;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Greedy word wrap for a growing text field: the byte range of `text` that
/// each row draws, in order. A row that fills the field is closed immediately
/// and a break swallows the space it broke on, so a caret at the end of the
/// text always sits exactly where the next typed character will be drawn.
///
/// Ranges never overlap and only ever leave a swallowed space between them,
/// so every caret offset belongs to exactly one row.
pub fn wrap_rows(text: &str, width: usize) -> Vec<Range<usize>> {
    // Nothing can be drawn at no width, but the caret still needs one row to
    // sit on, and the whole text keeps that row's own offsets in reach.
    if width == 0 {
        let everything = 0..text.len();
        return vec![everything];
    }
    let mut rows: Vec<Range<usize>> = Vec::new();
    let mut start = 0usize;
    let mut offset = 0usize;
    let mut row_width = 0usize;
    for chunk in text.split_inclusive(' ') {
        let word = chunk.trim_end_matches(' ');
        let word_width = UnicodeWidthStr::width(word);
        // Move a word that would overflow down as a whole when it can fit.
        if row_width > 0 && row_width + word_width > width && word_width <= width {
            rows.push(start..offset);
            start = offset;
            row_width = 0;
        }
        for character in word.chars() {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if row_width + character_width > width {
                rows.push(start..offset);
                start = offset;
                row_width = 0;
            }
            offset += character.len_utf8();
            row_width += character_width;
        }
        // `split_inclusive` leaves at most one space on a chunk.
        if word.len() < chunk.len() {
            if row_width >= width {
                rows.push(start..offset);
                // The row broke on this space, so it is drawn on neither row.
                start = offset + 1;
                row_width = 0;
            } else {
                row_width += 1;
            }
            offset += 1;
        }
    }
    if row_width >= width {
        rows.push(start..offset);
        start = offset;
    }
    rows.push(start..offset);
    rows
}

/// The row and the display column a caret `caret` bytes into `text` is drawn
/// at. An offset that both ends one row and starts the next belongs to the
/// later one, because that is where the next typed character appears.
pub fn caret_row_column(text: &str, rows: &[Range<usize>], caret: usize) -> (usize, usize) {
    let row = rows
        .iter()
        .rposition(|row| row.start <= caret)
        .unwrap_or_default();
    let caret = caret.clamp(rows[row].start, rows[row].end);
    (row, UnicodeWidthStr::width(&text[rows[row].start..caret]))
}

/// The caret offset `column` display columns into row `row`, clamped to the
/// text that row draws, so a vertical move onto a shorter row lands at its
/// end and never lands on the row below. Pass `usize::MAX` for that end.
pub fn caret_in_row(text: &str, rows: &[Range<usize>], row: usize, column: usize) -> usize {
    let range = &rows[row];
    let mut used = 0usize;
    let mut caret = range.end;
    for (offset, character) in text[range.clone()].char_indices() {
        if used >= column {
            caret = range.start + offset;
            break;
        }
        used += UnicodeWidthChar::width(character).unwrap_or(0);
    }
    caret.min(row_caret_end(text, rows, row))
}

/// The last caret offset that still draws on `row`. An offset that also
/// starts the next row belongs to that row, so a caret held on this one has
/// to stop one character short of it.
fn row_caret_end(text: &str, rows: &[Range<usize>], row: usize) -> usize {
    let range = &rows[row];
    match rows.get(row + 1) {
        Some(next) if next.start == range.end => caret_left(text, range.end).max(range.start),
        _ => range.end,
    }
}

/// The caret one character to the left of `caret`, or `caret` at the start.
pub fn caret_left(text: &str, caret: usize) -> usize {
    text[..caret]
        .chars()
        .next_back()
        .map_or(caret, |character| caret - character.len_utf8())
}

/// The caret one character to the right of `caret`, or `caret` at the end.
pub fn caret_right(text: &str, caret: usize) -> usize {
    text[caret..]
        .chars()
        .next()
        .map_or(caret, |character| caret + character.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str, width: usize) -> Vec<&str> {
        wrap_rows(text, width)
            .into_iter()
            .map(|row| &text[row])
            .collect()
    }

    #[test]
    fn a_field_wraps_whole_words_onto_following_rows() {
        assert_eq!(rows("", 10), vec![""]);
        assert_eq!(rows("short task", 10), vec!["short task", ""]);
        assert_eq!(
            rows("fix the failing merge test", 10),
            vec!["fix the ", "failing ", "merge test", ""]
        );
        // A word longer than the field is split rather than clipped.
        assert_eq!(rows("aaaaaaaaaaaa", 5), vec!["aaaaa", "aaaaa", "aa"]);
        // The caret keeps a row of its own once the previous one is full.
        assert_eq!(rows("abcde ", 5), vec!["abcde", ""]);
    }

    #[test]
    fn every_caret_offset_maps_onto_one_row_and_back() {
        let text = "fix the failing merge test";
        let wrapped = wrap_rows(text, 10);
        for caret in (0..=text.len()).filter(|caret| text.is_char_boundary(*caret)) {
            let (row, column) = caret_row_column(text, &wrapped, caret);
            assert!(row < wrapped.len(), "caret {caret} fell outside the field");
            assert_eq!(
                caret_in_row(text, &wrapped, row, column),
                caret,
                "caret {caret} did not survive the round trip"
            );
        }
    }

    #[test]
    fn a_vertical_move_keeps_the_column_and_clamps_to_a_short_row() {
        let text = "fix the failing merge test";
        let wrapped = wrap_rows(text, 10);
        // "merge test" sits under "failing ", which is two columns shorter.
        let caret = text.find("test").unwrap() + 2;
        let (row, column) = caret_row_column(text, &wrapped, caret);
        assert_eq!((row, column), (2, 8));

        // Up stops at the end of the shorter row rather than spilling onto
        // the row the caret came from.
        let up = caret_in_row(text, &wrapped, row - 1, column);
        assert_eq!(caret_row_column(text, &wrapped, up), (1, 7));
        assert_eq!(&text[wrapped[1].start..up], "failing");

        // Down from there returns to the same column it left.
        let down = caret_in_row(text, &wrapped, 2, 7);
        assert_eq!(caret_row_column(text, &wrapped, down), (2, 7));
    }

    #[test]
    fn the_end_of_a_row_is_the_far_side_of_its_own_text() {
        let text = "fix the failing merge test";
        let wrapped = wrap_rows(text, 10);
        // Row 0 broke on a space it swallowed, so its end is that space.
        assert_eq!(caret_in_row(text, &wrapped, 0, usize::MAX), 7);
        assert_eq!(&text[..7], "fix the");
        // The last row ends at the end of the text.
        let last = wrapped.len() - 1;
        assert_eq!(caret_in_row(text, &wrapped, last, usize::MAX), text.len());
    }

    #[test]
    fn horizontal_moves_step_over_whole_characters() {
        let text = "ä b";
        assert_eq!(caret_right(text, 0), 2);
        assert_eq!(caret_left(text, 2), 0);
        assert_eq!(caret_left(text, 0), 0);
        assert_eq!(caret_right(text, text.len()), text.len());
    }
}
