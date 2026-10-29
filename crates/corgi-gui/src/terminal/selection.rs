//! Selecting a pane's text with the pointer.
//!
//! Selection is the window's own: nothing is sent to the pane while it is
//! made. Rows are Herdr's content rows, counted from the top of the pane's
//! retained history (the rows `pane.selection.read` takes), so a selection
//! stays on its text while the pane scrolls; columns are the pane's own.
//! Copying asks Herdr for the text, which knows where lines wrapped and what
//! scrolled off screen; the painted cells are the fallback.
//!
//! As in terminal emulators (and Herdr UI, whose rules these follow), a
//! press anchors on the half of the cell it lands on, so a plain click
//! selects nothing and a drag across one cell selects it; a double click
//! takes the word and a triple click the line.

use std::ops::Range;

use unicode_width::UnicodeWidthStr;

use crate::endpoint::protocol::{CellData, FrameData, PaneSurfacePane};
use crate::herdr_ui::terminal::HIDDEN;

/// A cell in a pane's content: its content row and its column in the pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    pub row: u32,
    pub col: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Side {
    Left,
    Right,
}

/// A pointer position: a cell and the half of it. Ordered as text reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Edge {
    point: Point,
    side: Side,
}

/// Where a pane's cells are in its frame, and which content row its first
/// grid row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    pub pane_id: String,
    /// The frame's columns and rows the pane's text fills.
    pub columns: Range<u16>,
    pub rows: Range<u16>,
    /// The content row on the pane's first grid row.
    pub top: u32,
}

impl Region {
    pub fn of(pane: &PaneSurfacePane) -> Self {
        let rect = pane.inner_rect;
        let top = pane.scroll.map_or(0, |scroll| {
            u32::try_from(
                scroll
                    .max_offset_from_bottom
                    .saturating_sub(scroll.offset_from_bottom),
            )
            .unwrap_or(u32::MAX)
        });
        Self {
            pane_id: pane.pane_id.clone(),
            columns: rect.x..rect.x.saturating_add(rect.width),
            rows: rect.y..rect.y.saturating_add(rect.height),
            top,
        }
    }

    fn width(&self) -> u16 {
        self.columns.end - self.columns.start
    }

    /// The frame row a content row is painted on, when it is on screen.
    pub fn grid_row(&self, row: u32) -> Option<u16> {
        let offset = u16::try_from(row.checked_sub(self.top)?).ok()?;
        let grid = self.rows.start.checked_add(offset)?;
        self.rows.contains(&grid).then_some(grid)
    }

    /// The edge under a point in frame cells (fractional), clamped into the
    /// pane so a drag past its edge selects up to it.
    fn edge(&self, column: f32, row: f32) -> Option<Edge> {
        if self.columns.is_empty()
            || self.rows.is_empty()
            || !column.is_finite()
            || !row.is_finite()
        {
            return None;
        }
        let last_row = self.rows.end - 1;
        let last_col = self.width() - 1;
        let content = |grid: u16| self.top + u32::from(grid - self.rows.start);
        if row < f32::from(self.rows.start) {
            return Some(Edge {
                point: Point {
                    row: self.top,
                    col: 0,
                },
                side: Side::Left,
            });
        }
        if row >= f32::from(self.rows.end) {
            return Some(Edge {
                point: Point {
                    row: content(last_row),
                    col: last_col,
                },
                side: Side::Right,
            });
        }
        let grid_row = row.floor() as u16;
        let row = content(grid_row);
        if column < f32::from(self.columns.start) {
            return Some(Edge {
                point: Point { row, col: 0 },
                side: Side::Left,
            });
        }
        if column >= f32::from(self.columns.end) {
            return Some(Edge {
                point: Point { row, col: last_col },
                side: Side::Right,
            });
        }
        let col = column.floor() as u16 - self.columns.start;
        let side = if column.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        Some(Edge {
            point: Point { row, col },
            side,
        })
    }

    /// The pane's cells on frame row `grid` of `frame`.
    fn cells<'a>(&self, frame: &'a FrameData, grid: u16) -> Option<&'a [CellData]> {
        if grid >= frame.height || self.columns.end > frame.width {
            return None;
        }
        let start = usize::from(grid) * usize::from(frame.width);
        frame
            .cells
            .get(start + usize::from(self.columns.start)..start + usize::from(self.columns.end))
    }
}

/// How much a press takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Cell,
    Word,
    Line,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub pane_id: String,
    unit: Unit,
    /// What the press itself took, which a drag never gives up.
    origin: (Edge, Edge),
    anchor: Edge,
    head: Edge,
    pub dragging: bool,
}

impl Selection {
    /// Starts a selection at frame cell (`column`, `row`), fractional, with
    /// the press's click count.
    pub fn begin(
        region: &Region,
        frame: &FrameData,
        column: f32,
        row: f32,
        clicks: usize,
    ) -> Option<Self> {
        let edge = region.edge(column, row)?;
        let unit = match clicks {
            0 | 1 => Unit::Cell,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        let origin = span(unit, region, frame, edge);
        Some(Self {
            pane_id: region.pane_id.clone(),
            unit,
            origin,
            anchor: origin.0,
            head: origin.1,
            dragging: true,
        })
    }

    /// Moves the free end to (`column`, `row`), growing by the press's unit.
    pub fn extend(&mut self, region: &Region, frame: &FrameData, column: f32, row: f32) {
        let Some(edge) = region.edge(column, row) else {
            return;
        };
        let (start, end) = span(self.unit, region, frame, edge);
        if start < self.origin.0 {
            self.anchor = self.origin.1;
            self.head = start;
        } else {
            self.anchor = self.origin.0;
            self.head = end.max(self.origin.1);
        }
    }

    /// The first and last selected cells, inclusive; `None` when the
    /// selection holds no whole cell.
    pub fn range(&self, region: &Region) -> Option<(Point, Point)> {
        let (first, last) = if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        };
        let width = u64::from(region.width().max(1));
        let index = |point: Point| u64::from(point.row) * width + u64::from(point.col);
        let start = index(first.point) + u64::from(first.side == Side::Right);
        let end = index(last.point).checked_sub(u64::from(last.side == Side::Left))?;
        if start > end {
            return None;
        }
        let point = |index: u64| Point {
            row: (index / width) as u32,
            col: (index % width) as u16,
        };
        Some((point(start), point(end)))
    }

    /// The frame cells to tint: each selected row on screen, with its
    /// frame columns.
    pub fn highlights(&self, region: &Region) -> Vec<(u16, Range<u16>)> {
        let Some((start, end)) = self.range(region) else {
            return Vec::new();
        };
        let width = region.width();
        (start.row..=end.row)
            .filter_map(|row| {
                let grid = region.grid_row(row)?;
                let first = if row == start.row { start.col } else { 0 };
                let last = if row == end.row { end.col } else { width - 1 };
                Some((
                    grid,
                    region.columns.start + first..region.columns.start + last + 1,
                ))
            })
            .collect()
    }

    /// The selected text as the painted cells show it, when every selected
    /// row is on screen. Each row's trailing blanks are dropped.
    pub fn text(&self, region: &Region, frame: &FrameData) -> Option<String> {
        let (start, end) = self.range(region)?;
        let mut lines = Vec::new();
        for row in start.row..=end.row {
            let cells = region.cells(frame, region.grid_row(row)?)?;
            let first = if row == start.row { start.col } else { 0 };
            let last = if row == end.row {
                end.col
            } else {
                region.width() - 1
            };
            lines.push(row_text(cells, first, last));
        }
        Some(lines.join("\n"))
    }
}

/// The first and last edge of the unit around `edge`.
fn span(unit: Unit, region: &Region, frame: &FrameData, edge: Edge) -> (Edge, Edge) {
    let row = |columns: Range<u16>| {
        (
            Edge {
                point: Point {
                    row: edge.point.row,
                    col: columns.start,
                },
                side: Side::Left,
            },
            Edge {
                point: Point {
                    row: edge.point.row,
                    col: columns.end - 1,
                },
                side: Side::Right,
            },
        )
    };
    match unit {
        Unit::Cell => (edge, edge),
        Unit::Line => row(0..region.width()),
        Unit::Word => region
            .grid_row(edge.point.row)
            .and_then(|grid| region.cells(frame, grid))
            .and_then(|cells| word(cells, edge.point.col))
            .map_or((edge, edge), row),
    }
}

/// What a cell reads as: concealed and empty cells as blanks.
pub fn shown(cell: &CellData) -> &str {
    if cell.modifier & HIDDEN != 0 || cell.symbol.is_empty() {
        " "
    } else {
        cell.symbol.as_str()
    }
}

/// The cells of a row that start a grapheme, each with its column: Herdr
/// sends the second half of a wide character as an ordinary blank, so a wide
/// grapheme covers the columns its width takes.
pub fn graphemes(cells: &[CellData]) -> impl Iterator<Item = (usize, usize, &CellData)> {
    let mut covered = 0;
    cells.iter().enumerate().filter_map(move |(column, cell)| {
        if column < covered || cell.skip {
            return None;
        }
        let span = cell.symbol.width().max(1).min(cells.len() - column);
        covered = column + span;
        Some((column, span, cell))
    })
}

/// The text of columns `first..=last` of a row, its trailing blanks dropped.
fn row_text(cells: &[CellData], first: u16, last: u16) -> String {
    let (first, last) = (usize::from(first), usize::from(last));
    let mut text = String::new();
    for (column, span, cell) in graphemes(cells) {
        if column + span <= first || column > last {
            continue;
        }
        text.push_str(shown(cell));
    }
    text.trim_end().to_string()
}

/// Characters that end a word though they are printed.
pub fn separates(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(
            c,
            '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '|' | ',' | ';'
        )
        || ('\u{2500}'..='\u{259f}').contains(&c)
}

/// The columns of the word at `column`, or `None` on a separator. Sentence
/// punctuation after a word is not part of it.
fn word(cells: &[CellData], column: u16) -> Option<Range<u16>> {
    let column = usize::from(column);
    if column >= cells.len() {
        return None;
    }
    // A wide character's second half belongs to the cell that drew it.
    let mut owner = vec![0; cells.len()];
    for (start, span, _) in graphemes(cells) {
        for slot in owner.iter_mut().skip(start).take(span) {
            *slot = start;
        }
    }
    let first_char = |index: usize| shown(&cells[owner[index]]).chars().next().unwrap_or(' ');
    if separates(first_char(column)) {
        return None;
    }
    let mut start = column;
    while start > 0 && !separates(first_char(start - 1)) {
        start -= 1;
    }
    let mut end = column + 1;
    while end < cells.len() && !separates(first_char(end)) {
        end += 1;
    }
    while end - 1 > column && matches!(first_char(end - 1), '.' | ':' | '!' | '?') {
        end -= 1;
    }
    Some(start as u16..end as u16)
}

#[cfg(test)]
pub mod tests;
