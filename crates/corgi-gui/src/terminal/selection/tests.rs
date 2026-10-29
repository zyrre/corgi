use super::*;
use crate::endpoint::protocol::{PaneSurfaceScrollMetrics, SurfaceRect};

/// A frame of `rows`, each padded with blanks to the widest. A wide
/// character takes two cells, its second an ordinary blank, as Herdr sends it.
pub fn frame(rows: &[&str]) -> FrameData {
    let row_cells = |row: &str| {
        let mut cells = Vec::new();
        for c in row.chars() {
            let symbol = c.to_string();
            let wide = symbol.width() > 1;
            cells.push(CellData {
                symbol,
                fg: 0,
                bg: 0,
                modifier: 0,
                skip: false,
                hyperlink: None,
            });
            if wide {
                cells.push(CellData {
                    symbol: " ".into(),
                    fg: 0,
                    bg: 0,
                    modifier: 0,
                    skip: false,
                    hyperlink: None,
                });
            }
        }
        cells
    };
    let width = rows.iter().map(|r| row_cells(r).len()).max().unwrap_or(0) as u16;
    let mut cells = Vec::new();
    for row in rows {
        let mut row = row_cells(row);
        while row.len() < usize::from(width) {
            row.push(CellData {
                symbol: " ".into(),
                fg: 0,
                bg: 0,
                modifier: 0,
                skip: false,
                hyperlink: None,
            });
        }
        cells.extend(row);
    }
    FrameData {
        cells,
        width,
        height: rows.len() as u16,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    }
}

/// A pane over the whole of `frame`, scrolled `offset` rows up into a
/// history of `history` rows.
pub fn pane(frame: &FrameData, history: u64, offset: u64) -> PaneSurfacePane {
    let rect = SurfaceRect {
        x: 0,
        y: 0,
        width: frame.width,
        height: frame.height,
    };
    PaneSurfacePane {
        pane_id: "w2:p1".into(),
        content_revision: 1,
        rect,
        inner_rect: rect,
        scrollbar_rect: None,
        scroll: Some(PaneSurfaceScrollMetrics {
            offset_from_bottom: offset,
            max_offset_from_bottom: history,
            viewport_rows: u64::from(frame.height),
        }),
        focused: true,
        mouse_reporting: false,
        sgr_pixel_mouse: false,
        alternate_screen_active: false,
        pixel_width: 0,
        pixel_height: 0,
    }
}

const ROWS: &[&str] = &[
    "Select this sentence.  ",
    "path src/main.rs:12 ok ",
    "日本 wide            ",
];

#[test]
fn a_press_anchors_on_a_half_cell_so_a_click_selects_nothing() {
    let frame = frame(ROWS);
    let region = Region::of(&pane(&frame, 0, 0));
    // A click, and a drag inside one half, select nothing.
    let mut click = Selection::begin(&region, &frame, 2.2, 0.5, 1).unwrap();
    assert_eq!(click.range(&region), None);
    click.extend(&region, &frame, 2.4, 0.5);
    assert_eq!(click.range(&region), None);
    // From the left half of S across to the right half of the first e.
    let mut drag = Selection::begin(&region, &frame, 0.1, 0.5, 1).unwrap();
    drag.extend(&region, &frame, 1.9, 0.5);
    assert_eq!(drag.text(&region, &frame).as_deref(), Some("Se"));
    // Backwards, the same cells.
    let mut back = Selection::begin(&region, &frame, 1.9, 0.5, 1).unwrap();
    back.extend(&region, &frame, 0.1, 0.5);
    assert_eq!(back.range(&region), drag.range(&region));
}

#[test]
fn a_double_click_takes_the_word_and_a_triple_click_the_line() {
    let frame = frame(ROWS);
    let region = Region::of(&pane(&frame, 0, 0));
    let word = Selection::begin(&region, &frame, 15.5, 0.5, 2).unwrap();
    // The sentence's full stop is not part of its last word.
    assert_eq!(word.text(&region, &frame).as_deref(), Some("sentence"));
    // A path keeps its colon and line number.
    let path = Selection::begin(&region, &frame, 8.5, 1.5, 2).unwrap();
    assert_eq!(
        path.text(&region, &frame).as_deref(),
        Some("src/main.rs:12")
    );
    // On a blank, a double click takes nothing more than the cell.
    let blank = Selection::begin(&region, &frame, 21.5, 0.5, 2).unwrap();
    assert_eq!(blank.range(&region), None);
    let line = Selection::begin(&region, &frame, 3.5, 1.5, 3).unwrap();
    assert_eq!(
        line.text(&region, &frame).as_deref(),
        Some("path src/main.rs:12 ok")
    );
    // Dragging from a word grows by words.
    let mut words = Selection::begin(&region, &frame, 8.5, 1.5, 2).unwrap();
    words.extend(&region, &frame, 20.5, 1.5);
    assert_eq!(
        words.text(&region, &frame).as_deref(),
        Some("src/main.rs:12 ok")
    );
}

#[test]
fn rows_span_lines_and_drop_trailing_blanks_and_wide_halves() {
    let frame = frame(ROWS);
    let region = Region::of(&pane(&frame, 0, 0));
    // From the left half of the t before "ence." to the right half of the
    // e of "wide" (the wide characters take two columns each).
    let mut selection = Selection::begin(&region, &frame, 15.1, 0.5, 1).unwrap();
    selection.extend(&region, &frame, 8.9, 2.5);
    assert_eq!(
        selection.text(&region, &frame).as_deref(),
        Some("tence.\npath src/main.rs:12 ok\n日本 wide")
    );
    // A drag past the pane's edge selects up to it.
    let mut past = Selection::begin(&region, &frame, 5.1, 2.5, 1).unwrap();
    // (Column 5 is the w: 日 and 本 take two columns each, then a blank.)
    past.extend(&region, &frame, 99.0, 99.0);
    assert_eq!(past.text(&region, &frame).as_deref(), Some("wide"));
}

#[test]
fn the_selection_keeps_to_its_text_while_the_pane_scrolls() {
    let frame = frame(ROWS);
    // At the bottom of 10 rows of history, the first grid row is content
    // row 10.
    let bottom = Region::of(&pane(&frame, 10, 0));
    let mut selection = Selection::begin(&bottom, &frame, 0.1, 1.5, 1).unwrap();
    selection.extend(&bottom, &frame, 3.9, 1.5);
    assert_eq!(
        selection.range(&bottom),
        Some((Point { row: 11, col: 0 }, Point { row: 11, col: 3 }))
    );
    assert_eq!(selection.highlights(&bottom), vec![(1, 0..4)]);
    // Scrolled one row up, the same text is a row lower on screen.
    let up = Region::of(&pane(&frame, 10, 1));
    assert_eq!(selection.highlights(&up), vec![(2, 0..4)]);
    // Scrolled further, it is off screen: nothing tints, and the painted
    // text is not enough to copy it.
    let away = Region::of(&pane(&frame, 10, 5));
    assert!(selection.highlights(&away).is_empty());
    assert_eq!(selection.text(&away, &frame), None);
}

#[test]
fn a_pane_beside_another_keeps_the_selection_inside_it() {
    let frame = frame(&["left |right side"]);
    let mut right = pane(&frame, 0, 0);
    right.inner_rect = SurfaceRect {
        x: 6,
        y: 0,
        width: 10,
        height: 1,
    };
    let region = Region::of(&right);
    // A press left of the pane clamps to its first column.
    let mut selection = Selection::begin(&region, &frame, 1.0, 0.5, 1).unwrap();
    selection.extend(&region, &frame, 10.9, 0.5);
    assert_eq!(selection.text(&region, &frame).as_deref(), Some("right"));
    // Columns are the pane's own, as Herdr reads them; tints are the frame's.
    assert_eq!(
        selection.range(&region),
        Some((Point { row: 0, col: 0 }, Point { row: 0, col: 4 }))
    );
    assert_eq!(selection.highlights(&region), vec![(0, 6..11)]);
}
