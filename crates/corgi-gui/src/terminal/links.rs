//! Links in a pane: an explicit hyperlink a program set (OSC 8), else a web
//! address printed as text. Only web and mail addresses open, and only on a
//! click with the link modifier (Cmd on macOS, Ctrl elsewhere) held, as in
//! terminal emulators; the system's browser opens them.
//!
//! A long address wraps where the pane is narrower than it. When a row's text
//! runs to the pane's last column, the address goes on at the start of the
//! next row, as Claude Code prints them.

use std::ops::Range;

use crate::endpoint::protocol::FrameData;

use super::selection::{Region, graphemes, shown};

/// A link under the pointer: where it goes and the frame cells it covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    /// Frame rows and columns, for the underline.
    pub cells: Vec<(u16, Range<u16>)>,
}

/// How many rows one wrapped address may take.
const MAX_ROWS: u16 = 8;

/// The link at frame cell (`column`, `row`) of `region`'s pane.
pub fn at(frame: &FrameData, region: &Region, column: u16, row: u16) -> Option<Link> {
    if !region.columns.contains(&column) || !region.rows.contains(&row) {
        return None;
    }
    hyperlink(frame, region, column, row).or_else(|| plain(frame, region, column, row))
}

/// Whether `url` may be opened: web and mail addresses, with something
/// after the scheme.
pub fn openable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let rest = ["https://", "http://", "mailto:"]
        .iter()
        .find_map(|scheme| lower.strip_prefix(scheme));
    rest.is_some_and(|rest| !rest.is_empty())
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

fn index(frame: &FrameData, column: u16, row: u16) -> usize {
    usize::from(row) * usize::from(frame.width) + usize::from(column)
}

/// The OSC 8 hyperlink under the cell, over every cell of the pane's row
/// that carries the same one.
fn hyperlink(frame: &FrameData, region: &Region, column: u16, row: u16) -> Option<Link> {
    let id = frame.cells.get(index(frame, column, row))?.hyperlink?;
    let url = frame.hyperlinks.get(id as usize)?.clone();
    let same = |c: u16| {
        frame
            .cells
            .get(index(frame, c, row))
            .and_then(|cell| cell.hyperlink)
            == Some(id)
    };
    let mut start = column;
    while start > region.columns.start && same(start - 1) {
        start -= 1;
    }
    let mut end = column + 1;
    while end < region.columns.end && same(end) {
        end += 1;
    }
    Some(Link {
        url,
        cells: vec![(row, start..end)],
    })
}

/// One row of the pane as text, with the frame column each byte came from.
struct Line {
    row: u16,
    text: String,
    columns: Vec<u16>,
    /// Its text reaches the pane's last column.
    full: bool,
}

fn line(frame: &FrameData, region: &Region, row: u16) -> Option<Line> {
    if row >= frame.height || region.columns.end > frame.width {
        return None;
    }
    let start = index(frame, region.columns.start, row);
    let cells = frame
        .cells
        .get(start..start + usize::from(region.columns.end - region.columns.start))?;
    let mut text = String::new();
    let mut columns = Vec::new();
    for (column, _, cell) in graphemes(cells) {
        let symbol = shown(cell);
        text.push_str(symbol);
        columns.extend(std::iter::repeat_n(
            region.columns.start + column as u16,
            symbol.len(),
        ));
    }
    let full = !text.ends_with(' ');
    Some(Line {
        row,
        text,
        columns,
        full,
    })
}

/// A web address printed as text, across rows when it wraps.
fn plain(frame: &FrameData, region: &Region, column: u16, row: u16) -> Option<Link> {
    // The rows this one may be part of: back while the row above ran to the
    // edge, forward while each row does.
    let mut first = row;
    while first > region.rows.start
        && row - first < MAX_ROWS
        && line(frame, region, first - 1).is_some_and(|line| line.full)
    {
        first -= 1;
    }
    let mut lines = Vec::new();
    let mut at = first;
    while at < region.rows.end && at - first < 2 * MAX_ROWS {
        let next = line(frame, region, at)?;
        let full = next.full;
        lines.push(next);
        if at >= row && !full {
            break;
        }
        at += 1;
    }
    // The joined text, and the frame cell of each byte.
    let mut text = String::new();
    let mut cells = Vec::new();
    for line in &lines {
        text.push_str(&line.text);
        cells.extend(line.columns.iter().map(|&column| (line.row, column)));
    }
    let hit = cells.iter().position(|&cell| cell == (row, column))?;
    let range = address_around(&text, hit)?;
    let mut covered: Vec<(u16, Range<u16>)> = Vec::new();
    for &(row, column) in &cells[range.clone()] {
        match covered.last_mut() {
            Some((last, columns)) if *last == row => columns.end = columns.end.max(column + 1),
            _ => covered.push((row, column..column + 1)),
        }
    }
    Some(Link {
        url: text[range].to_string(),
        cells: covered,
    })
}

/// The byte range of the web address in `text` covering byte `hit`.
fn address_around(text: &str, hit: usize) -> Option<Range<usize>> {
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = ["https://", "http://"]
        .iter()
        .filter_map(|scheme| lower[from..].find(scheme).map(|at| from + at))
        .min()
    {
        let end = found
            + text[found..]
                .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`'))
                .unwrap_or(text.len() - found);
        let end = trim_end(&text[found..end]) + found;
        if (found..end).contains(&hit) {
            return Some(found..end);
        }
        from = end.max(found + 1);
    }
    None
}

/// Where an address ends once sentence punctuation after it, and a closing
/// bracket it did not open, are taken off.
fn trim_end(address: &str) -> usize {
    let mut end = address.len();
    loop {
        let tail = &address[..end];
        let Some(last) = tail.chars().last() else {
            return end;
        };
        let unmatched = |open: char, close: char| {
            last == close && tail.matches(open).count() < tail.matches(close).count()
        };
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?')
            || unmatched('(', ')')
            || unmatched('[', ']')
            || unmatched('{', '}')
        {
            end -= last.len_utf8();
        } else {
            return end;
        }
    }
}

#[cfg(test)]
mod tests;
