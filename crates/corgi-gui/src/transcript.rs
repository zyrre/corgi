//! The expanded row's session: its latest turns, newest first, the newest in
//! full and older ones folded to two lines, as the dashboard's expanded
//! session has them. The turns scroll on their own (the keys, the wheel), and
//! their text is selected with the mouse and copied.
//!
//! GPUI's text cannot be selected, so the element lays out its own lines,
//! wrapped as the text fields wrap theirs (`text_input::wrap`), and keeps them
//! from one frame to the next: a press is hit-tested against the lines drawn
//! under it. A place in the text is a turn and a byte offset in its text, so a
//! selection stays on its words while the row scrolls, and while new turns
//! come in above it.

use std::{cell::RefCell, ops::Range, rc::Rc};

use gpui::{
    App, Bounds, ContentMask, CursorStyle, DispatchPhase, Element, ElementId, FocusHandle,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, IntoElement, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ScrollWheelEvent,
    ShapedLine, SharedString, Style, TextAlign, TextRun, Window, fill, point, px, relative,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::{terminal::selection::separates, text_input::wrap};

/// Lines an older turn is folded to: enough to recognize it without letting
/// it crowd out its neighbours (the dashboard's `TRANSCRIPT_ENTRY_ROWS`).
pub const FOLDED_LINES: usize = 2;
/// Fewer lines than this and the session is not worth reading, so the rows
/// above it scroll off instead (the dashboard's `MIN_TRANSCRIPT_ROWS`).
pub const MIN_LINES: f32 = 6.0;
/// What a folded turn's last line ends in.
const ELLIPSIS: &str = "…";
/// What copied turns are joined with: a blank line, which keeps them apart
/// in the copied text.
const TURN_BREAK: &str = "\n\n";

/// One turn as the row draws it.
#[derive(Clone)]
pub struct Turn {
    pub text: SharedString,
    pub marker: SharedString,
    pub marker_color: Hsla,
    pub color: Hsla,
    /// The lines the turn is folded to; `None` draws it whole.
    pub fold: Option<usize>,
}

/// A place in the transcript's text: a turn, counted from the newest, and a
/// byte offset in its text. Ordered as the text reads, top to bottom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Spot {
    pub turn: usize,
    pub offset: usize,
}

/// How much a press takes: a click a place, a double click a word, a triple
/// click the whole turn; a drag after it goes on in the same steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Char,
    Word,
    Turn,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Selection {
    unit: Unit,
    /// What the press took, and what the pointer is on now.
    anchor: Range<Spot>,
    head: Range<Spot>,
}

impl Selection {
    fn range(&self) -> Range<Spot> {
        self.anchor.start.min(self.head.start)..self.anchor.end.max(self.head.end)
    }
}

/// A turn as laid out.
#[derive(Clone, Debug, Default, PartialEq)]
struct Placed {
    text: SharedString,
    /// Where the text shown ends: all of it, unless the turn is folded.
    shown: usize,
    folded: bool,
    /// Its lines, in `Laid::lines`.
    lines: Range<usize>,
}

/// A line as laid out: its turn, its bytes in that turn's text, and its top
/// from the top of the turns. A folded turn's last line is shaped with the
/// ellipsis after its bytes.
#[derive(Clone, Debug, Default)]
struct Line {
    turn: usize,
    range: Range<usize>,
    top: Pixels,
    shaped: ShapedLine,
}

/// The turns as last drawn, for hit-testing a press against them.
#[derive(Default)]
struct Laid {
    turns: Vec<Placed>,
    lines: Vec<Line>,
    line_height: Pixels,
    /// Where the element was drawn, and how far in from its left edge the
    /// text starts, past the markers.
    bounds: Bounds<Pixels>,
    indent: Pixels,
}

#[derive(Default)]
struct State {
    /// How far the turns are scrolled up; the next layout clamps it.
    scroll: Pixels,
    /// What a page key scrolls: the view less a line.
    page: Pixels,
    laid: Laid,
    selection: Option<Selection>,
    /// The left button went down in the turns and is still down.
    selecting: bool,
}

/// The expanded row's scroll, selection and last layout, shared between the
/// window and the element that draws them.
#[derive(Clone, Default)]
pub struct Transcript(Rc<RefCell<State>>);

impl Transcript {
    /// Starts afresh, for another row or a new expansion: at the newest
    /// turn, nothing selected.
    pub fn reset(&self) {
        *self.0.borrow_mut() = State::default();
    }

    /// Drops the selection; true when there was one.
    pub fn clear_selection(&self) -> bool {
        let mut state = self.0.borrow_mut();
        state.selecting = false;
        state.selection.take().is_some()
    }

    /// Scrolls by whole views, down for positive `pages`.
    pub fn scroll_pages(&self, pages: f32) {
        let mut state = self.0.borrow_mut();
        let step = state.page.max(state.laid.line_height).max(px(1.));
        state.scroll = (state.scroll + step * pages).max(px(0.));
    }

    /// Jumps to the newest turn, or to the end of the oldest; the next
    /// layout clamps the end to the last line.
    pub fn scroll_to_end(&self, end: bool) {
        self.0.borrow_mut().scroll = if end { px(f32::MAX / 4.) } else { px(0.) };
    }

    /// The selected text as the copy keys put it on the clipboard; `None`
    /// with nothing selected.
    pub fn copied(&self) -> Option<String> {
        let state = self.0.borrow();
        let selection = state.selection.as_ref()?;
        copied(&state.laid.turns, selection.range())
    }

    /// A press of the left button at `position`, the `clicks`th in a row;
    /// with `extend` (Shift) a click moves the selection's far end there.
    fn press(&self, position: Point<Pixels>, clicks: usize, extend: bool) {
        let mut state = self.0.borrow_mut();
        let unit = match clicks {
            0 | 1 => Unit::Char,
            2 => Unit::Word,
            _ => Unit::Turn,
        };
        let Some(spot) = state.spot(position, unit != Unit::Char) else {
            state.selection = None;
            return;
        };
        let range = unit_range(&state.laid.turns, spot, unit);
        state.selecting = true;
        if extend
            && unit == Unit::Char
            && let Some(selection) = state.selection.as_mut()
        {
            selection.head = range;
            return;
        }
        state.selection = Some(Selection {
            unit,
            anchor: range.clone(),
            head: range,
        });
    }

    /// The pointer moved with the button down; true when the selection
    /// changed.
    fn drag_to(&self, position: Point<Pixels>) -> bool {
        let mut state = self.0.borrow_mut();
        let Some(unit) = state
            .selection
            .as_ref()
            .filter(|_| state.selecting)
            .map(|selection| selection.unit)
        else {
            return false;
        };
        let Some(spot) = state.spot(position, unit != Unit::Char) else {
            return false;
        };
        let head = unit_range(&state.laid.turns, spot, unit);
        let Some(selection) = state.selection.as_mut() else {
            return false;
        };
        let changed = selection.head != head;
        selection.head = head;
        changed
    }

    fn release(&self) {
        self.0.borrow_mut().selecting = false;
    }

    fn scroll_by(&self, delta: Pixels) {
        let mut state = self.0.borrow_mut();
        state.scroll = (state.scroll + delta).max(px(0.));
    }
}

impl State {
    /// The place under `position` in the window, held inside the view so a
    /// drag past its edge selects up to it. `under` takes the character
    /// under the pointer, for a word; else the caret place nearest it.
    fn spot(&self, position: Point<Pixels>, under: bool) -> Option<Spot> {
        let laid = &self.laid;
        let bounds = laid.bounds;
        let y = position
            .y
            .clamp(bounds.top(), (bounds.bottom() - px(1.)).max(bounds.top()));
        let x = position.x - bounds.left() - laid.indent;
        spot_at(
            &laid.lines,
            laid.line_height,
            x,
            y - bounds.top() + self.scroll,
            |line, x| {
                if under {
                    line.shaped.index_for_x(x).unwrap_or(line.shaped.text.len())
                } else {
                    line.shaped.closest_index_for_x(x)
                }
            },
        )
    }
}

/// The place `x` across from the text's left edge and `y` down from the top
/// of the turns; `index` finds the byte at `x` in a line, from its start.
/// Above the first line is the start of the first; between two turns, or
/// below the last, the end of the line above; left of the text, the start
/// of the line.
fn spot_at(
    lines: &[Line],
    line_height: Pixels,
    x: Pixels,
    y: Pixels,
    index: impl Fn(&Line, Pixels) -> usize,
) -> Option<Spot> {
    let first = lines.first()?;
    let line = lines
        .iter()
        .rev()
        .find(|line| line.top <= y)
        .unwrap_or(first);
    let offset = if y < first.top {
        line.range.start
    } else if y >= line.top + line_height {
        line.range.end
    } else if x <= px(0.) {
        line.range.start
    } else {
        (line.range.start + index(line, x)).min(line.range.end)
    };
    Some(Spot {
        turn: line.turn,
        offset,
    })
}

/// What a press at `spot` takes, as a range of places.
fn unit_range(turns: &[Placed], spot: Spot, unit: Unit) -> Range<Spot> {
    let Some(placed) = turns.get(spot.turn) else {
        return spot..spot;
    };
    let at = |offset| Spot {
        turn: spot.turn,
        offset,
    };
    match unit {
        Unit::Char => spot..spot,
        Unit::Word => {
            let word = word_at(&placed.text[..placed.shown], spot.offset);
            at(word.start)..at(word.end)
        }
        Unit::Turn => at(0)..at(placed.shown),
    }
}

/// The word the character at `offset` (past the end, the last one) is in,
/// as the focus view's terminal takes a word: everything between two
/// separators (`selection::separates`), so a path or a flag is one word,
/// without the sentence's punctuation after it. A separator is taken alone.
fn word_at(text: &str, offset: usize) -> Range<usize> {
    let offset = offset.min(text.len());
    let Some((at, c)) = text[offset..]
        .chars()
        .next()
        .map(|c| (offset, c))
        .or_else(|| text.char_indices().next_back())
    else {
        return 0..0;
    };
    if separates(c) {
        return at..at + c.len_utf8();
    }
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| !separates(*c))
        .last()
        .map_or(at, |(index, _)| index);
    let mut end = text[at..]
        .char_indices()
        .find(|(_, c)| separates(*c))
        .map_or(text.len(), |(index, _)| at + index);
    while end > at + c.len_utf8() && text[..end].ends_with(['.', ':', '!', '?']) {
        end -= 1;
    }
    start..end
}

/// The text of `range` across `turns`, each turn's part apart from the
/// next by a blank line. A folded turn the range covers, from its start to
/// the end of what shows, gives its whole text; part of one gives what
/// shows of that part.
fn copied(turns: &[Placed], range: Range<Spot>) -> Option<String> {
    if range.start >= range.end {
        return None;
    }
    let mut parts = Vec::new();
    for (turn, placed) in turns
        .iter()
        .enumerate()
        .take(range.end.turn + 1)
        .skip(range.start.turn)
    {
        let start = if turn == range.start.turn {
            range.start.offset
        } else {
            0
        };
        let end = if turn == range.end.turn {
            range.end.offset
        } else {
            placed.shown
        };
        let (start, end) = (start.min(placed.shown), end.min(placed.shown));
        if start >= end {
            continue;
        }
        parts.push(if placed.folded && start == 0 && end == placed.shown {
            placed.text.as_ref()
        } else {
            &placed.text[start..end]
        });
    }
    (!parts.is_empty()).then(|| parts.join(TURN_BREAK))
}

/// How many turns came in above those laid out before (`old`), the newest
/// first: the first shift at which `new` reads the same as `old` from the
/// newest down to turn `through`. `None` when no shift does, as when the
/// session was read afresh.
fn shift(old: &[SharedString], new: &[SharedString], through: usize) -> Option<usize> {
    if through >= old.len() {
        return None;
    }
    (0..new.len()).find(|&shift| {
        old[..=through]
            .iter()
            .enumerate()
            .all(|(turn, text)| new.get(shift + turn) == Some(text))
    })
}

/// A turn's text as visual lines: each paragraph wrapped by `wrap_paragraph`
/// (its ranges within the paragraph), as byte ranges of `text`, and whether
/// `fold` cut it. A cut turn keeps `fold` lines.
fn turn_lines(
    text: &str,
    fold: Option<usize>,
    mut wrap_paragraph: impl FnMut(&str) -> Vec<Range<usize>>,
) -> (Vec<Range<usize>>, bool) {
    let mut lines = Vec::new();
    let mut start = 0;
    for paragraph in text.split('\n') {
        let shown = paragraph.strip_suffix('\r').unwrap_or(paragraph);
        lines.extend(
            wrap_paragraph(shown)
                .into_iter()
                .map(|range| start + range.start..start + range.end),
        );
        start += paragraph.len() + 1;
        // One line past the fold is enough to know the turn is cut.
        if fold.is_some_and(|fold| lines.len() > fold) {
            break;
        }
    }
    let folded = fold.is_some_and(|fold| lines.len() > fold);
    if let Some(fold) = fold.filter(|_| folded) {
        lines.truncate(fold);
    }
    (lines, folded)
}

/// Where a folded turn's last line, `range` of `text`, is cut so the
/// ellipsis fits after it in `width`; `x` is how far into the line a byte
/// offset of `text` is drawn. Spaces before the cut go too.
fn ellipsis_cut(
    text: &str,
    range: Range<usize>,
    width: Pixels,
    ellipsis: Pixels,
    x: impl Fn(usize) -> Pixels,
) -> usize {
    let line = &text[range.clone()];
    let mut end = range.end;
    let mut boundaries = line.grapheme_indices(true).rev();
    while end > range.start && x(end) + ellipsis > width {
        end = boundaries
            .next()
            .map_or(range.start, |(index, _)| range.start + index);
    }
    range.start + text[range.start..end].trim_end().len()
}

/// Draws the turns of the expanded row, below its status line, filling the
/// row down to the bottom of the overview.
pub struct TranscriptElement {
    turns: Vec<Turn>,
    state: Transcript,
    /// What a press in the turns gives the keyboard to: the overview's.
    focus: FocusHandle,
    selection: Hsla,
    /// The markers' column and the gap after it.
    indent: Pixels,
    /// The space between two turns.
    gap: Pixels,
}

impl TranscriptElement {
    pub fn new(
        turns: Vec<Turn>,
        state: Transcript,
        focus: FocusHandle,
        selection: Hsla,
        indent: Pixels,
        gap: Pixels,
    ) -> Self {
        Self {
            turns,
            state,
            focus,
            selection,
            indent,
            gap,
        }
    }
}

pub struct Prepainted {
    hitbox: Hitbox,
    /// Each turn's marker, shaped.
    markers: Vec<ShapedLine>,
    highlights: Vec<PaintQuad>,
}

impl IntoElement for TranscriptElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TranscriptElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepainted;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // It takes what the row has left, however long the turns: they
        // scroll inside it.
        let style = Style {
            flex_grow: 1.,
            size: gpui::Size {
                width: relative(1.).into(),
                height: gpui::Length::Auto,
            },
            min_size: gpui::Size {
                width: gpui::Length::Auto,
                height: (window.line_height() * MIN_LINES).into(),
            },
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let style = window.text_style();
        let font = style.font();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let width = (bounds.size.width - self.indent).max(px(1.));
        let text_system = window.text_system().clone();
        let run = |len: usize, color: Hsla| TextRun {
            len,
            font: font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shape = |text: SharedString, runs: &[TextRun]| {
            text_system.shape_line(text, font_size, runs, None)
        };
        let ellipsis = shape(ELLIPSIS.into(), &[run(ELLIPSIS.len(), self.selection)]).width;

        let mut turns = Vec::with_capacity(self.turns.len());
        let mut lines: Vec<Line> = Vec::new();
        let mut markers = Vec::with_capacity(self.turns.len());
        let mut top = px(0.);
        for (index, turn) in self.turns.iter().enumerate() {
            if index > 0 {
                top += self.gap;
            }
            let text = &turn.text;
            let (ranges, folded) = turn_lines(text, turn.fold, |paragraph| {
                let whole = shape(
                    paragraph.to_string().into(),
                    &[run(paragraph.len(), turn.color)],
                );
                wrap(paragraph, width, |index| whole.x_for_index(index))
            });
            let first = lines.len();
            let last = ranges.len().saturating_sub(1);
            let mut shown = text.len();
            for (row, range) in ranges.into_iter().enumerate() {
                let (range, shaped) = if folded && row == last {
                    let line = shape(
                        text[range.clone()].to_string().into(),
                        &[run(range.len(), turn.color)],
                    );
                    let start = range.start;
                    let cut = ellipsis_cut(text, range, width, ellipsis, |offset| {
                        line.x_for_index(offset - start)
                    });
                    shown = cut;
                    let painted = format!("{}{ELLIPSIS}", &text[start..cut]);
                    let runs = [
                        run(cut - start, turn.color),
                        run(ELLIPSIS.len(), turn.color),
                    ];
                    (start..cut, shape(painted.into(), &runs))
                } else {
                    let shaped = shape(
                        text[range.clone()].to_string().into(),
                        &[run(range.len(), turn.color)],
                    );
                    (range, shaped)
                };
                lines.push(Line {
                    turn: index,
                    range,
                    top,
                    shaped,
                });
                top += line_height;
            }
            turns.push(Placed {
                text: text.clone(),
                shown,
                folded,
                lines: first..lines.len(),
            });
            markers.push(shape(
                turn.marker.clone(),
                &[run(turn.marker.len(), turn.marker_color)],
            ));
        }
        let content = top;

        let mut state = self.state.0.borrow_mut();
        // New turns came in: the selection and the scroll stay on the text
        // they were on, which moved down.
        let old: Vec<SharedString> = state.laid.turns.iter().map(|t| t.text.clone()).collect();
        let new: Vec<SharedString> = turns.iter().map(|t| t.text.clone()).collect();
        if !old.is_empty() && old != new {
            let through = state
                .selection
                .as_ref()
                .map_or(0, |selection| selection.range().end.turn);
            match shift(&old, &new, through) {
                Some(moved) => {
                    if let Some(selection) = state.selection.as_mut() {
                        for spot in [
                            &mut selection.anchor.start,
                            &mut selection.anchor.end,
                            &mut selection.head.start,
                            &mut selection.head.end,
                        ] {
                            spot.turn += moved;
                            // The turn that was newest is folded now.
                            spot.offset = spot.offset.min(turns[spot.turn].shown);
                        }
                    }
                    if moved > 0 && state.scroll > px(0.) {
                        state.scroll += lines[turns[moved].lines.start].top;
                    }
                }
                None => state.selection = None,
            }
        }
        state.scroll = state
            .scroll
            .min((content - bounds.size.height).max(px(0.)))
            .max(px(0.));
        state.page = (bounds.size.height - line_height).max(line_height);
        let scroll = state.scroll;

        let text_left = bounds.left() + self.indent;
        let highlights = state
            .selection
            .as_ref()
            .map(Selection::range)
            .filter(|range| range.start < range.end)
            .map(|range| {
                lines
                    .iter()
                    .filter_map(|line| {
                        let start = Spot {
                            turn: line.turn,
                            offset: line.range.start,
                        }
                        .max(range.start);
                        let end = Spot {
                            turn: line.turn,
                            offset: line.range.end,
                        }
                        .min(range.end);
                        if start >= end || start.turn != line.turn || end.turn != line.turn {
                            return None;
                        }
                        let x = |offset: usize| {
                            text_left
                                + line
                                    .shaped
                                    .x_for_index(offset - line.range.start)
                                    .min(width)
                        };
                        let row_top = bounds.top() + line.top - scroll;
                        Some(fill(
                            Bounds::from_corners(
                                point(x(start.offset), row_top),
                                point(x(end.offset), row_top + line_height),
                            ),
                            self.selection,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();

        state.laid = Laid {
            turns,
            lines,
            line_height,
            bounds,
            indent: self.indent,
        };
        Prepainted {
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
            markers,
            highlights,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let hitbox = prepaint.hitbox.clone();
        window.set_cursor_style(CursorStyle::IBeam, &hitbox);
        {
            let state = self.state.0.borrow();
            let laid = &state.laid;
            let line_height = laid.line_height;
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                for highlight in prepaint.highlights.drain(..) {
                    window.paint_quad(highlight);
                }
                for (index, line) in laid.lines.iter().enumerate() {
                    let top = bounds.top() + line.top - state.scroll;
                    if top + line_height < bounds.top() || top > bounds.bottom() {
                        continue;
                    }
                    let placed = &laid.turns[line.turn];
                    if index == placed.lines.start
                        && let Some(marker) = prepaint.markers.get(line.turn)
                    {
                        let _ = marker.paint(
                            point(bounds.left(), top),
                            line_height,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    }
                    let _ = line.shaped.paint(
                        point(bounds.left() + laid.indent, top),
                        line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            });
        }

        // A press in the turns selects text: it neither reaches the row,
        // whose click would open the terminal, nor the card, whose right
        // click would collapse it.
        let (state, focus, pressed) = (self.state.clone(), self.focus.clone(), hitbox.clone());
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !pressed.is_hovered(window) {
                return;
            }
            cx.stop_propagation();
            if event.button == MouseButton::Left {
                window.focus(&focus, cx);
                state.press(event.position, event.click_count, event.modifiers.shift);
                window.refresh();
            }
        });
        let state = self.state.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _| {
            if phase == DispatchPhase::Bubble
                && event.pressed_button == Some(MouseButton::Left)
                && state.drag_to(event.position)
            {
                window.refresh();
            }
        });
        let state = self.state.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, _| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                state.release();
            }
        });
        let state = self.state.clone();
        let line_height = self.state.0.borrow().laid.line_height;
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                state.scroll_by(-event.delta.pixel_delta(line_height).y);
                cx.stop_propagation();
                window.refresh();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A monospace font of 10-pixel cells, lines 20 tall.
    const CELL: f32 = 10.0;
    const LINE: f32 = 20.0;

    fn placed(text: &str, shown: usize, folded: bool) -> Placed {
        Placed {
            text: text.to_string().into(),
            shown,
            folded,
            lines: 0..0,
        }
    }

    fn whole(text: &str) -> Placed {
        placed(text, text.len(), false)
    }

    fn spot(turn: usize, offset: usize) -> Spot {
        Spot { turn, offset }
    }

    /// Lines of `width` cells for each turn, with a gap of half a line
    /// between turns, as the element lays them out.
    fn laid_out(texts: &[&str], width: usize) -> Vec<Line> {
        let mut lines = Vec::new();
        let mut top = px(0.);
        for (turn, text) in texts.iter().enumerate() {
            if turn > 0 {
                top += px(LINE / 2.);
            }
            let (ranges, _) = turn_lines(text, None, |paragraph| {
                wrap(paragraph, px(width as f32 * CELL), |index| {
                    px(paragraph[..index].chars().count() as f32 * CELL)
                })
            });
            for range in ranges {
                lines.push(Line {
                    turn,
                    range,
                    top,
                    shaped: ShapedLine::default(),
                });
                top += px(LINE);
            }
        }
        lines
    }

    /// The caret place nearest `x` cells into a line of `texts`.
    fn hit(texts: &[&str], lines: &[Line], x: f32, y: f32) -> Option<Spot> {
        spot_at(lines, px(LINE), px(x * CELL), px(y), |line, x| {
            let chars = texts[line.turn][line.range.clone()].chars().count();
            let column = ((x / px(CELL)).round() as usize).min(chars);
            texts[line.turn][line.range.clone()]
                .char_indices()
                .nth(column)
                .map_or(line.range.len(), |(index, _)| index)
        })
    }

    #[test]
    fn a_point_on_a_wrapped_line_lands_in_its_turns_text() {
        let texts = ["the newest turn wraps here", "older"];
        let lines = laid_out(&texts, 12);
        // "the newest " | "turn wraps " | "here", then "older" after a gap.
        let ranges: Vec<_> = lines.iter().map(|line| line.range.clone()).collect();
        assert_eq!(ranges, [0..11, 11..22, 22..26, 0..5]);
        assert_eq!(hit(&texts, &lines, 4.0, 5.0), Some(spot(0, 4)));
        // The second line's fifth character is the text's sixteenth.
        assert_eq!(hit(&texts, &lines, 5.0, LINE + 5.0), Some(spot(0, 16)));
        // Past the end of a line, its end; left of the text, its start.
        assert_eq!(
            hit(&texts, &lines, 40.0, 2.0 * LINE + 1.0),
            Some(spot(0, 26))
        );
        assert_eq!(hit(&texts, &lines, -3.0, LINE + 1.0), Some(spot(0, 11)));
        // The gap between turns is the end of the turn above.
        assert_eq!(
            hit(&texts, &lines, 2.0, 3.0 * LINE + 2.0),
            Some(spot(0, 26))
        );
        assert_eq!(hit(&texts, &lines, 2.0, 3.5 * LINE + 2.0), Some(spot(1, 2)));
        // Above the first line, its start; below the last, its end.
        assert_eq!(hit(&texts, &lines, 3.0, -10.0), Some(spot(0, 0)));
        assert_eq!(hit(&texts, &lines, 3.0, 500.0), Some(spot(1, 5)));
        assert_eq!(hit(&texts, &[], 3.0, 5.0), None);
    }

    #[test]
    fn paragraphs_wrap_apart_and_an_older_turn_folds_to_two_lines() {
        let narrow = |paragraph: &str| {
            wrap(paragraph, px(6. * CELL), |index| {
                px(paragraph[..index].chars().count() as f32 * CELL)
            })
        };
        let text = "one two three\r\nfour\nfive";
        let (lines, folded) = turn_lines(text, None, narrow);
        assert!(!folded);
        // The line break and its \r are in no line.
        let shown: Vec<_> = lines.iter().map(|range| &text[range.clone()]).collect();
        assert_eq!(shown, ["one ", "two ", "three", "four", "five"]);
        let (lines, folded) = turn_lines(text, Some(FOLDED_LINES), narrow);
        assert!(folded);
        assert_eq!(lines, [0..4, 4..8]);
        // A turn that fits its fold is not folded.
        let (lines, folded) = turn_lines("short", Some(FOLDED_LINES), narrow);
        assert!(!folded);
        assert_eq!((lines.len(), &lines[0]), (1, &(0..5)));
    }

    #[test]
    fn a_folded_line_is_cut_where_its_ellipsis_fits() {
        let text = "alpha beta gamma";
        let x = |offset: usize| px(offset as f32 * CELL);
        // Ten cells wide, the ellipsis one: nine cells of text stay; a cut
        // after a word drops the space before it too.
        assert_eq!(ellipsis_cut(text, 0..11, px(100.), px(CELL), x), 9);
        assert_eq!(
            &text[..ellipsis_cut(text, 0..6, px(100.), px(CELL), x)],
            "alpha"
        );
        // A cut on a later line is counted from that line's start.
        let later = |offset: usize| px((offset - 6) as f32 * CELL);
        assert_eq!(ellipsis_cut(text, 6..16, px(100.), px(CELL), later), 15);
    }

    #[test]
    fn a_double_click_takes_a_word_and_a_triple_click_the_turn() {
        let turns = [
            whole("cargo test --all"),
            placed("folded long text", 6, true),
        ];
        assert_eq!(
            unit_range(&turns, spot(0, 7), Unit::Word),
            spot(0, 6)..spot(0, 10)
        );
        // A space is taken alone; past the end, the last word, a flag
        // whole.
        assert_eq!(
            unit_range(&turns, spot(0, 5), Unit::Word),
            spot(0, 5)..spot(0, 6)
        );
        assert_eq!(
            unit_range(&turns, spot(0, 16), Unit::Word),
            spot(0, 11)..spot(0, 16)
        );
        // A word in a folded turn ends where the text shown does.
        assert_eq!(
            unit_range(&turns, spot(1, 4), Unit::Word),
            spot(1, 0)..spot(1, 6)
        );
        assert_eq!(
            unit_range(&turns, spot(1, 2), Unit::Turn),
            spot(1, 0)..spot(1, 6)
        );
        assert_eq!(
            unit_range(&turns, spot(0, 3), Unit::Char),
            spot(0, 3)..spot(0, 3)
        );
    }

    #[test]
    fn a_word_is_what_the_terminal_takes_for_one() {
        let text = "Edit crates/corgi-gui/src/view.rs, then hit-testing (GPUI's).";
        let word = |offset: usize| &text[word_at(text, offset)];
        // A path and a hyphenated word whole, from any character in them;
        // the comma after the path is not part of it.
        assert_eq!(word(5), "crates/corgi-gui/src/view.rs");
        assert_eq!(word(18), "crates/corgi-gui/src/view.rs");
        assert_eq!(word(43), "hit-testing");
        // Brackets and quotes part words, as in the terminal.
        assert_eq!(word(55), "GPUI");
        assert_eq!(word(52), "(");
        assert_eq!(word(text.len()), ".");
        assert_eq!(word_at("", 0), 0..0);
    }

    #[test]
    fn a_selection_reads_the_same_dragged_either_way() {
        let forward = Selection {
            unit: Unit::Char,
            anchor: spot(0, 4)..spot(0, 4),
            head: spot(1, 2)..spot(1, 2),
        };
        let backward = Selection {
            unit: Unit::Char,
            anchor: spot(1, 2)..spot(1, 2),
            head: spot(0, 4)..spot(0, 4),
        };
        assert_eq!(forward.range(), backward.range());
        // A drag after a double click keeps the whole first word.
        let words = Selection {
            unit: Unit::Word,
            anchor: spot(0, 6)..spot(0, 10),
            head: spot(0, 0)..spot(0, 5),
        };
        assert_eq!(words.range(), spot(0, 0)..spot(0, 10));
    }

    #[test]
    fn copying_gives_the_text_selected_across_turns() {
        let turns = [
            whole("newest summary"),
            whole("$ cargo test"),
            whole("Fix it"),
        ];
        assert_eq!(copied(&turns, spot(0, 7)..spot(0, 14)).unwrap(), "summary");
        assert_eq!(
            copied(&turns, spot(0, 7)..spot(2, 3)).unwrap(),
            "summary\n\n$ cargo test\n\nFix"
        );
        // Nothing selected copies nothing, and a selection ending at the
        // very start of a turn takes nothing of it.
        assert_eq!(copied(&turns, spot(1, 3)..spot(1, 3)), None);
        assert_eq!(copied(&turns, spot(0, 7)..spot(1, 0)).unwrap(), "summary");
        // Line breaks inside a turn are kept.
        let turns = [whole("first\nsecond")];
        assert_eq!(copied(&turns, spot(0, 2)..spot(0, 9)).unwrap(), "rst\nsec");
    }

    #[test]
    fn a_folded_turn_covered_copies_whole_and_part_of_one_copies_what_shows() {
        let full = "a long older turn that the row folds to two lines";
        let turns = [whole("newest"), placed(full, 17, true), whole("oldest")];
        // Covered, by a drag across it or a triple click on it: all of it.
        assert_eq!(
            copied(&turns, spot(0, 0)..spot(2, 6)).unwrap(),
            format!("newest\n\n{full}\n\noldest")
        );
        assert_eq!(copied(&turns, spot(1, 0)..spot(1, 17)).unwrap(), full);
        // Inside it, or from it into the next: what shows.
        assert_eq!(copied(&turns, spot(1, 2)..spot(1, 6)).unwrap(), "long");
        assert_eq!(
            copied(&turns, spot(1, 13)..spot(2, 3)).unwrap(),
            "turn\n\nold"
        );
        assert_eq!(
            copied(&turns, spot(0, 3)..spot(1, 6)).unwrap(),
            "est\n\na long"
        );
    }

    #[test]
    fn new_turns_above_shift_a_selection_onto_its_text_and_a_new_read_drops_it() {
        let texts = |texts: &[&str]| -> Vec<SharedString> {
            texts.iter().map(|text| text.to_string().into()).collect()
        };
        let old = texts(&["c", "b", "a"]);
        // One turn came in; the oldest fell off the end.
        assert_eq!(shift(&old, &texts(&["d", "c", "b"]), 1), Some(1));
        assert_eq!(shift(&old, &texts(&["e", "d", "c", "b", "a"]), 2), Some(2));
        assert_eq!(shift(&old, &old, 2), Some(0));
        // The selected turn fell off, or the turns are another session's.
        assert_eq!(shift(&old, &texts(&["d", "c", "b"]), 2), None);
        assert_eq!(shift(&old, &texts(&["x", "y"]), 0), None);
        assert_eq!(shift(&[], &old, 0), None);
        // A repeated turn does not fool it: the newer ones must match too.
        let old = texts(&["ok", "run", "ok"]);
        assert_eq!(shift(&old, &texts(&["ok", "ok", "run", "ok"]), 2), Some(1));
    }
}
