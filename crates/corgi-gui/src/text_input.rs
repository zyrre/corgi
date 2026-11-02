//! A text field on one line of text, wrapped: the prompt, the answer and the
//! new agent's task. GPUI has no text field of its own; this one is adapted
//! from `examples/input.rs` of `gpui-pre` 0.3.6, copyright Zed Industries,
//! Inc., licensed under the Apache License, Version 2.0 (see
//! `LICENSE-APACHE` in this crate). Changed for Corgi: theme colors, the
//! text wrapped at the field's width (between words where it can) with the
//! field as tall as its lines, up to what its box allows and scrolling
//! beyond that to keep the cursor in view, pasted line breaks kept as
//! spaces, and the example's window removed. Enter, Esc, Tab and the arrows
//! up and down are left to the dialog around it; Home and End go to the
//! ends of the text, not of a wrapped line.

use std::ops::Range;

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, Hsla,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, ScrollWheelEvent, ShapedLine, SharedString, Style, TextRun, UTF16Selection,
    UnderlineStyle, Window, actions, div, fill, point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        WordLeft,
        WordRight,
        DeleteWordLeft,
    ]
);

const CONTEXT: &str = "TextInput";

/// The cursor's width, kept free at the right so it shows after the
/// widest line.
const CURSOR_WIDTH: Pixels = px(2.);

/// The field's keys, the platform's own: ⌘ on macOS, Ctrl elsewhere.
pub fn bind_keys(cx: &mut App) {
    let primary = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    let word = if cfg!(target_os = "macos") {
        "alt"
    } else {
        "ctrl"
    };
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        KeyBinding::new("left", Left, Some(CONTEXT)),
        KeyBinding::new("right", Right, Some(CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("home", Home, Some(CONTEXT)),
        KeyBinding::new("end", End, Some(CONTEXT)),
        KeyBinding::new("ctrl-a", Home, Some(CONTEXT)),
        KeyBinding::new("ctrl-e", End, Some(CONTEXT)),
        KeyBinding::new(&format!("{word}-left"), WordLeft, Some(CONTEXT)),
        KeyBinding::new(&format!("{word}-right"), WordRight, Some(CONTEXT)),
        KeyBinding::new(&format!("{word}-backspace"), DeleteWordLeft, Some(CONTEXT)),
        KeyBinding::new(&format!("{primary}-a"), SelectAll, Some(CONTEXT)),
        KeyBinding::new(&format!("{primary}-v"), Paste, Some(CONTEXT)),
        KeyBinding::new(&format!("{primary}-c"), Copy, Some(CONTEXT)),
        KeyBinding::new(&format!("{primary}-x"), Cut, Some(CONTEXT)),
    ]);
}

/// The colors the field paints with, set from the theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldColors {
    pub text: Hsla,
    pub placeholder: Hsla,
    pub cursor: Hsla,
    pub selection: Hsla,
}

/// A wrapped line as painted: its bytes in the shown text, and their shape.
struct VisualLine {
    range: Range<usize>,
    shaped: ShapedLine,
}

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    pub colors: FieldColors,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_lines: Vec<VisualLine>,
    last_bounds: Option<Bounds<Pixels>>,
    last_line_height: Pixels,
    /// How far the lines are scrolled up when they are more than the
    /// field's box shows.
    scroll: Pixels,
    /// Whether the next paint scrolls the cursor into view: the text or the
    /// selection changed, rather than the wheel scrolling.
    reveal_cursor: bool,
    is_selecting: bool,
}

impl TextInput {
    pub fn new(
        placeholder: impl Into<SharedString>,
        colors: FieldColors,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::default(),
            placeholder: placeholder.into(),
            colors,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_lines: Vec::new(),
            last_bounds: None,
            last_line_height: px(0.),
            scroll: px(0.),
            reveal_cursor: false,
            is_selecting: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>) {
        self.placeholder = placeholder.into();
    }

    /// Empties the field for its next use.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.content = SharedString::default();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.scroll = px(0.);
        self.reveal_cursor = true;
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word(self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word(self.cursor_offset()), cx);
    }

    fn delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_word(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let previous = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == previous {
                return;
            }
            self.select_to(previous, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                return;
            }
            self.select_to(next, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    /// Scrolls lines the field's box has no room for; a field that shows
    /// all of its lines leaves the wheel to what is around it.
    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bounds) = self.last_bounds else {
            return;
        };
        let hidden = self.last_line_height * self.last_lines.len() as f32 - bounds.size.height;
        if hidden <= px(0.) {
            return;
        }
        let delta = event.delta.pixel_delta(self.last_line_height).y;
        self.scroll = (self.scroll - delta).clamp(px(0.), hidden);
        self.reveal_cursor = false;
        cx.stop_propagation();
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &one_line(&text), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.reveal_cursor = true;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    /// The visual line at `y` in the window, counting those scrolled out of
    /// view; `None` above the first.
    fn row_at(&self, y: Pixels) -> Option<usize> {
        let bounds = self.last_bounds?;
        let row = (y - bounds.top() + self.scroll) / self.last_line_height;
        (row >= 0.).then_some(row as usize)
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() || self.last_lines.is_empty() {
            return 0;
        }
        let Some(bounds) = self.last_bounds else {
            return 0;
        };
        let Some(row) = self.row_at(position.y) else {
            return 0;
        };
        let Some(line) = self.last_lines.get(row) else {
            return self.content.len();
        };
        let index = line.range.start + line.shaped.closest_index_for_x(position.x - bounds.left());
        let ranges: Vec<_> = self.last_lines.iter().map(|l| l.range.clone()).collect();
        on_row(&self.content, &ranges, row, index)
    }

    /// Where `offset` is drawn: its visual line and its x in that line.
    fn position_for(&self, offset: usize) -> Option<(usize, Pixels)> {
        let ranges: Vec<_> = self.last_lines.iter().map(|l| l.range.clone()).collect();
        let row = row_of(&ranges, offset);
        let line = self.last_lines.get(row)?;
        Some((row, line.shaped.x_for_index(offset - line.range.start)))
    }

    /// The shown text, the placeholder while empty, and its runs, the
    /// marked text underlined.
    fn display(&self, window: &Window) -> (SharedString, Vec<TextRun>) {
        let style = window.text_style();
        let (display_text, text_color) = if self.content.is_empty() {
            (self.placeholder.clone(), self.colors.placeholder)
        } else {
            (self.content.clone(), self.colors.text)
        };
        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = self.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };
        (display_text, runs)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.reveal_cursor = true;
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    fn previous_word(&self, offset: usize) -> usize {
        previous_word(&self.content, offset)
    }

    fn next_word(&self, offset: usize) -> usize {
        self.content
            .split_word_bound_indices()
            .find_map(|(idx, word)| {
                let end = idx + word.len();
                (end > offset && !word.trim().is_empty()).then_some(end)
            })
            .unwrap_or(self.content.len())
    }
}

/// Where the word before `offset` starts, skipping spaces first.
fn previous_word(text: &str, offset: usize) -> usize {
    text.split_word_bound_indices()
        .rev()
        .find_map(|(idx, word)| (idx < offset && !word.trim().is_empty()).then_some(idx))
        .unwrap_or(0)
}

/// Pasted text on one line: line breaks become spaces.
fn one_line(text: &str) -> String {
    text.replace("\r\n", " ").replace(['\n', '\r'], " ")
}

/// `text` broken into visual lines no wider than `width`, as byte ranges
/// that together cover it; `x` is how far into the text, set on one line, a
/// byte offset is drawn. A line breaks after the spaces that follow a word,
/// those spaces hanging past the edge; a word wider than a line alone is
/// broken between characters. Empty text is one empty line.
pub(crate) fn wrap(text: &str, width: Pixels, x: impl Fn(usize) -> Pixels) -> Vec<Range<usize>> {
    let width = width.max(px(0.));
    let mut lines = Vec::new();
    let mut start = 0;
    while start < text.len() && x(start + text[start..].trim_end().len()) - x(start) > width {
        let mut word_end = None;
        let mut fitting_end = None;
        for (index, grapheme) in text[start..].grapheme_indices(true) {
            let end = start + index + grapheme.len();
            let spaces_follow = text[end..].starts_with(char::is_whitespace);
            let ink_end = start + text[start..end].trim_end().len();
            if x(ink_end) - x(start) > width {
                // Past the first character that does not fit, nothing
                // later fits either.
                break;
            }
            fitting_end = Some(end);
            if grapheme.trim().is_empty() && !spaces_follow {
                word_end = Some(end);
            }
        }
        let end = word_end.or(fitting_end).unwrap_or_else(|| {
            // Not even one character fits: it takes a line of its own.
            start + text[start..].graphemes(true).next().map_or(0, str::len)
        });
        lines.push(start..end);
        start = end;
    }
    if start < text.len() || lines.is_empty() {
        lines.push(start..text.len());
    }
    lines
}

/// The visual line `offset` is drawn on: a break between two lines puts it
/// at the start of the later one.
fn row_of(lines: &[Range<usize>], offset: usize) -> usize {
    lines
        .iter()
        .rposition(|line| line.start <= offset)
        .unwrap_or(0)
}

/// `index`, hit on visual line `row`, kept on that line: the end of a line
/// that wraps is the start of the next, so a hit there lands just before it.
fn on_row(text: &str, lines: &[Range<usize>], row: usize, index: usize) -> usize {
    match lines.get(row) {
        Some(line) if row + 1 < lines.len() && index >= line.end => text[line.clone()]
            .grapheme_indices(true)
            .next_back()
            .map_or(line.start, |(start, _)| line.start + start),
        _ => index,
    }
}

/// The part of `selected` on each visual line it covers, by row.
fn spans(lines: &[Range<usize>], selected: &Range<usize>) -> Vec<(usize, Range<usize>)> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(row, line)| {
            let start = line.start.max(selected.start);
            let end = line.end.min(selected.end);
            (start < end).then_some((row, start..end))
        })
        .collect()
}

/// The scroll that shows visual line `row` in a box `shown` tall, moving no
/// further than needed.
fn reveal(scroll: Pixels, row: usize, line_height: Pixels, shown: Pixels) -> Pixels {
    let top = line_height * row as f32;
    let bottom = top + line_height;
    if top < scroll {
        top
    } else if bottom > scroll + shown {
        bottom - shown
    } else {
        scroll
    }
}

/// The runs of `runs` that fall in `range`, cut to it, for shaping one
/// visual line; at least one, so an empty line still has a font.
fn runs_in(runs: &[TextRun], range: &Range<usize>) -> Vec<TextRun> {
    let mut cut = Vec::new();
    let mut start = 0;
    for run in runs {
        let end = start + run.len;
        let len = end.min(range.end).saturating_sub(start.max(range.start));
        if len > 0 {
            cut.push(TextRun { len, ..run.clone() });
        }
        start = end;
    }
    if cut.is_empty()
        && let Some(run) = runs.first()
    {
        cut.push(TextRun {
            len: 0,
            ..run.clone()
        });
    }
    cut
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_text = one_line(new_text);
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        self.reveal_cursor = true;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.reveal_cursor = true;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // From the range's start to its end, or to the end of the start's
        // line when it wraps.
        let range = self.range_from_utf16(&range_utf16);
        let (row, start_x) = self.position_for(range.start)?;
        let end_x = match self.position_for(range.end)? {
            (end_row, end_x) if end_row == row => end_x,
            _ => self.last_lines[row].shaped.width,
        };
        let top = bounds.top() + self.last_line_height * row as f32 - self.scroll;
        Some(Bounds::from_corners(
            point(bounds.left() + start_x, top),
            point(bounds.left() + end_x, top + self.last_line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let line = self.last_lines.get(self.row_at(point.y)?)?;
        let utf8_index = line.range.start + line.shaped.index_for_x(line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    lines: Vec<VisualLine>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
    scroll: Pixels,
    line_height: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

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
        // As tall as the text's lines at the width the field gets; a box
        // with less room shrinks it, down to one line, and it scrolls.
        let (text, runs) = self.input.read(cx).display(window);
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let line = window
            .text_system()
            .shape_line(text.clone(), font_size, &runs, None);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.min_size.height = line_height.into();
        let layout_id =
            window.request_measured_layout(style, move |known, available, _window, _cx| {
                let width = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let rows = width.map_or(1, |width| {
                    wrap(&text, width - CURSOR_WIDTH, |index| line.x_for_index(index)).len()
                });
                size(
                    width.unwrap_or(line.width + CURSOR_WIDTH),
                    known.height.unwrap_or(line_height * rows as f32),
                )
            });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let selected_range = input.selected_range.clone();
        let cursor = if input.content.is_empty() {
            0
        } else {
            input.cursor_offset()
        };
        let colors = input.colors;
        let (text, runs) = input.display(window);
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let text_system = window.text_system();
        let whole = text_system.shape_line(text.clone(), font_size, &runs, None);
        let width = bounds.size.width - CURSOR_WIDTH;
        let ranges = wrap(&text, width, |index| whole.x_for_index(index));
        let lines: Vec<VisualLine> = ranges
            .iter()
            .map(|range| VisualLine {
                range: range.clone(),
                shaped: text_system.shape_line(
                    text[range.clone()].to_string().into(),
                    font_size,
                    &runs_in(&runs, range),
                    None,
                ),
            })
            .collect();

        let cursor_row = row_of(&ranges, cursor);
        let hidden = (line_height * lines.len() as f32 - bounds.size.height).max(px(0.));
        let mut scroll = input.scroll;
        if input.reveal_cursor {
            scroll = reveal(scroll, cursor_row, line_height, bounds.size.height);
        }
        let scroll = scroll.clamp(px(0.), hidden);
        let row_top = |row: usize| bounds.top() + line_height * row as f32 - scroll;
        // Spaces hanging past the edge keep the cursor at the edge.
        let x = |row: usize, offset: usize| {
            let line = &lines[row];
            bounds.left()
                + line
                    .shaped
                    .x_for_index(offset - line.range.start)
                    .min(width)
        };

        let (selections, cursor) = if selected_range.is_empty() {
            (
                Vec::new(),
                Some(fill(
                    Bounds::new(
                        point(x(cursor_row, cursor), row_top(cursor_row)),
                        size(CURSOR_WIDTH, line_height),
                    ),
                    colors.cursor,
                )),
            )
        } else {
            (
                spans(&ranges, &selected_range)
                    .into_iter()
                    .map(|(row, span)| {
                        fill(
                            Bounds::from_corners(
                                point(x(row, span.start), row_top(row)),
                                point(x(row, span.end), row_top(row) + line_height),
                            ),
                            colors.selection,
                        )
                    })
                    .collect(),
                None,
            )
        };
        PrepaintState {
            lines,
            cursor,
            selections,
            scroll,
            line_height,
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
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let lines = std::mem::take(&mut prepaint.lines);
        let scroll = prepaint.scroll;
        let line_height = prepaint.line_height;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for selection in prepaint.selections.drain(..) {
                window.paint_quad(selection)
            }
            for (row, line) in lines.iter().enumerate() {
                let top = bounds.top() + line_height * row as f32 - scroll;
                if top + line_height < bounds.top() || top > bounds.bottom() {
                    continue;
                }
                let _ = line.shaped.paint(
                    point(bounds.left(), top),
                    line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            if focus_handle.is_focused(window)
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
        });
        self.input.update(cx, |input, _cx| {
            input.last_lines = lines;
            input.last_bounds = Some(bounds);
            input.last_line_height = line_height;
            input.scroll = scroll;
            input.reveal_cursor = false;
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A column that can shrink, so a field box with a height cap
        // shrinks the text to it.
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_h_0()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_lines_join_and_words_are_found_backwards() {
        assert_eq!(one_line("run\nthe tests\r\nagain"), "run the tests again");
        assert_eq!(previous_word("go on  now", 10), 7);
        assert_eq!(previous_word("go on  now", 7), 3);
        assert_eq!(previous_word("go", 0), 0);
    }

    /// Ten pixels a character, as a monospaced font sets them.
    fn wrapped(text: &str, columns: f32) -> Vec<Range<usize>> {
        wrap(text, px(columns * 10.), |index| {
            px(text[..index].chars().count() as f32 * 10.)
        })
    }

    #[test]
    fn lines_break_after_the_spaces_between_words() {
        assert_eq!(wrapped("", 5.), vec![0..0]);
        assert_eq!(wrapped("hello", 5.), vec![0..5]);
        assert_eq!(wrapped("hello world", 10.), vec![0..6, 6..11]);
        assert_eq!(wrapped("one two three four", 9.), vec![0..8, 8..14, 14..18]);
        // The latest break that fits, not the first.
        assert_eq!(wrapped("a b c d e f", 7.), vec![0..8, 8..11]);
    }

    #[test]
    fn spaces_hang_past_the_edge_and_add_no_line() {
        assert_eq!(wrapped("hello     ", 5.), vec![0..10]);
        assert_eq!(wrapped("hello   world", 5.), vec![0..8, 8..13]);
    }

    #[test]
    fn a_word_wider_than_a_line_is_broken_between_characters() {
        assert_eq!(wrapped("abcdefghij", 4.), vec![0..4, 4..8, 8..10]);
        // A short word before it keeps its own line.
        assert_eq!(
            wrapped("go abcdefghij", 4.),
            vec![0..3, 3..7, 7..11, 11..13]
        );
        // Characters are not split, however narrow the field.
        assert_eq!(wrapped("åäö", 0.5), vec![0..2, 2..4, 4..6]);
        assert_eq!(wrapped("åä öå", 3.), vec![0..5, 5..9]);
    }

    #[test]
    fn the_cursor_at_a_break_is_on_the_later_line() {
        let lines = wrapped("hello world", 10.);
        assert_eq!(row_of(&lines, 0), 0);
        assert_eq!(row_of(&lines, 5), 0);
        assert_eq!(row_of(&lines, 6), 1);
        assert_eq!(row_of(&lines, 11), 1);
        assert_eq!(row_of(&wrapped("", 5.), 0), 0);
    }

    #[test]
    fn a_click_past_a_wrapped_line_stays_on_it() {
        let text = "hello world";
        let lines = wrapped(text, 10.);
        // Past "hello ": before its space, not at the next line's start.
        assert_eq!(on_row(text, &lines, 0, 6), 5);
        assert_eq!(row_of(&lines, on_row(text, &lines, 0, 6)), 0);
        assert_eq!(on_row(text, &lines, 0, 3), 3);
        // The last line's end is the text's end.
        assert_eq!(on_row(text, &lines, 1, 11), 11);
        // A broken word's line keeps its last character after the cursor.
        let lines = wrapped("abcdefgh", 4.);
        assert_eq!(on_row("abcdefgh", &lines, 0, 4), 3);
    }

    #[test]
    fn a_selection_is_cut_into_its_lines() {
        let lines = wrapped("one two three four", 9.);
        assert_eq!(spans(&lines, &(2..10)), vec![(0, 2..8), (1, 8..10)]);
        assert_eq!(
            spans(&lines, &(0..18)),
            vec![(0, 0..8), (1, 8..14), (2, 14..18)]
        );
        // Ending at a break, it covers nothing of the next line.
        assert_eq!(spans(&lines, &(3..8)), vec![(0, 3..8)]);
    }

    #[test]
    fn scrolling_shows_the_cursor_line_moving_no_further_than_needed() {
        let line = px(20.);
        let shown = px(60.);
        // Below the view: its bottom at the view's bottom.
        assert_eq!(reveal(px(0.), 5, line, shown), px(60.));
        // Above it: its top at the view's top.
        assert_eq!(reveal(px(60.), 1, line, shown), px(20.));
        // In view: no move.
        assert_eq!(reveal(px(20.), 2, line, shown), px(20.));
        assert_eq!(reveal(px(20.), 3, line, shown), px(20.));
    }

    #[test]
    fn a_line_gets_the_runs_over_it() {
        let run = |len, underlined: bool| TextRun {
            len,
            font: gpui::font("Menlo"),
            color: gpui::black(),
            background_color: None,
            underline: underlined.then(UnderlineStyle::default),
            strikethrough: None,
        };
        let runs = [run(3, false), run(2, true), run(4, false)];
        let cut = runs_in(&runs, &(2..7));
        assert_eq!(
            cut.iter()
                .map(|run| (run.len, run.underline.is_some()))
                .collect::<Vec<_>>(),
            vec![(1, false), (2, true), (2, false)]
        );
        // An empty line keeps the font.
        assert_eq!(
            runs_in(&runs, &(9..9))
                .iter()
                .map(|run| run.len)
                .collect::<Vec<_>>(),
            vec![0]
        );
    }
}
