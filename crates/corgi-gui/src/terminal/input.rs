//! Text from the platform's input handler: typed characters, dead keys (´
//! then e gives é) and input methods. Printable text arrives here rather
//! than as key events, so every keyboard layout types what it shows; a
//! composition in progress is drawn over the cursor and reaches the pane
//! only when the input method commits it. Adapted from Herdr UI's
//! `input.rs` (Apache-2.0), reduced to the terminal's part.

use std::ops::Range;

use gpui::{
    App, Bounds, Context, EntityInputHandler, InputHandler, Pixels, Point, UTF16Selection,
    WeakEntity, Window,
};

use super::{CELL_HEIGHT, TerminalView};
use crate::herdr_ui::terminal::{input_area, input_cursor_bounds};

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text: Vec<u16> = self.marked.encode_utf16().collect();
        let range = range.start.min(text.len())..range.end.min(text.len());
        *adjusted = Some(range.clone());
        Some(String::from_utf16_lossy(&text[range]))
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.marked.encode_utf16().count();
        let range = self
            .marked_selection
            .clone()
            .filter(|range| range.start <= range.end && range.end <= end)
            .unwrap_or(end..end);
        Some(UTF16Selection {
            range,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.marked.is_empty()).then(|| 0..self.marked.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked.clear();
        self.marked_selection = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked.clear();
        self.marked_selection = None;
        self.text(text);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.live() {
            return;
        }
        self.marked = text.into();
        // The IME's caret or the clause it converts, so its candidate
        // window opens there.
        self.marked_selection = selected;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let surface = self.shown().map(|surface| surface.as_ref());
        let cursor = input_cursor_bounds(surface, self.bounds.origin, self.cell_width, CELL_HEIGHT);
        let area = input_area(surface, self.bounds, self.cell_width, CELL_HEIGHT);
        Some(self.painter.borrow().composition_bounds(
            &self.marked,
            range,
            cursor,
            area,
            &self.font,
            window,
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

/// The terminal's platform input handler. It holds the view weakly (GPUI
/// 0.3.6 keeps Wayland's handler past its quit-time leak check), and opts out
/// of macOS press-and-hold: a terminal repeats a held key instead of opening
/// the accent picker.
pub struct TerminalInputHandler {
    view: WeakEntity<TerminalView>,
    bounds: Bounds<Pixels>,
}

impl TerminalInputHandler {
    pub fn new(bounds: Bounds<Pixels>, view: WeakEntity<TerminalView>) -> Self {
        Self { view, bounds }
    }

    fn update<R: Default>(
        &self,
        cx: &mut App,
        callback: impl FnOnce(&mut TerminalView, &mut Context<TerminalView>) -> R,
    ) -> R {
        self.view.update(cx, callback).unwrap_or_default()
    }
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        self.update(cx, |view, cx| {
            view.selected_text_range(ignore_disabled_input, window, cx)
        })
    }

    fn marked_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.update(cx, |view, cx| view.marked_text_range(window, cx))
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        self.update(cx, |view, cx| {
            view.text_for_range(range_utf16, adjusted_range, window, cx)
        })
    }

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.update(cx, |view, cx| {
            view.replace_text_in_range(replacement_range, text, window, cx)
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(
                range_utf16,
                new_text,
                new_selected_range,
                window,
                cx,
            )
        });
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        self.update(cx, |view, cx| view.unmark_text(window, cx));
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds;
        self.update(cx, |view, cx| {
            view.bounds_for_range(range_utf16, bounds, window, cx)
        })
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        self.update(cx, |view, cx| {
            view.character_index_for_point(point, window, cx)
        })
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }
}
