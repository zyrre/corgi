//! Where the overview's agent rows were drawn, so that a selection made
//! without clicking its row (the keyboard, the side panel's "Select it")
//! scrolls the overview just enough to show that row, and a click on a row
//! scrolls nothing: the row stays under the mouse for a double click.
//!
//! The rows sit in project cards, not directly in the scrolled list, so
//! GPUI's `ScrollHandle::scroll_to_item`, which knows only the list's
//! children, can show only a whole card (and one taller than the window
//! not at all). Instead each card records its rows' bounds as it is
//! prepainted, and the list, prepainted after its cards, moves its offset
//! once they are known.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui::{Bounds, Pixels, ScrollHandle, Window, point};

/// How a selection came about, which decides whether the overview scrolls
/// to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// A click on the row itself: the overview stays where it is.
    Click,
    /// Anything else: the overview scrolls, as little as it can, until the
    /// row shows in full.
    Reveal,
}

/// Shared between the window and the elements it draws.
#[derive(Clone, Default)]
pub struct Rows(Rc<RefCell<Places>>);

#[derive(Default)]
struct Places {
    /// This frame's rows, by pane ID: their top and bottom, in window
    /// coordinates at the overview's current offset.
    drawn: HashMap<String, (Pixels, Pixels)>,
    /// The row to bring into view once the frame's rows are known.
    reveal: Option<String>,
}

impl Rows {
    /// The next frame brings `pane_id`'s row into view, or, for a click,
    /// leaves the overview where it is (dropping an earlier request).
    pub fn pick(&self, pane_id: &str, pick: Pick) {
        self.0.borrow_mut().reveal = (pick == Pick::Reveal).then(|| pane_id.to_string());
    }

    /// A card's rows, `ids` in the order drawn, and the bounds of its row
    /// list's children (which may end with a footer that is not a row).
    pub fn record(&self, ids: &[String], bounds: &[Bounds<Pixels>]) {
        let mut places = self.0.borrow_mut();
        for (id, bounds) in ids.iter().zip(bounds) {
            places
                .drawn
                .insert(id.clone(), (bounds.top(), bounds.bottom()));
        }
    }

    /// The offset that shows the row asked for, if it must change, once
    /// every row of the frame is recorded; `viewport` is the overview's
    /// bounds. Forgets this frame's rows.
    fn settle(&self, viewport: Bounds<Pixels>, offset: Pixels) -> Option<Pixels> {
        let mut places = self.0.borrow_mut();
        let drawn = std::mem::take(&mut places.drawn);
        let (top, bottom) = *drawn.get(places.reveal.take()?.as_str())?;
        let moved = revealed(offset, (viewport.top(), viewport.bottom()), (top, bottom));
        (moved != offset).then_some(moved)
    }

    /// The overview's part, once its cards are prepainted: scrolls to the
    /// row asked for, and draws again at the new offset.
    pub fn apply(&self, scroll: &ScrollHandle, window: &mut Window) {
        let offset = scroll.offset();
        if let Some(y) = self.settle(scroll.bounds(), offset.y) {
            scroll.set_offset(point(offset.x, y));
            window.on_next_frame(|window, _| window.refresh());
        }
    }
}

/// The scroll offset (negative as the list scrolls down) that brings a row
/// spanning `row` into a `view`, both at `offset`: unchanged while the row
/// shows in full, else moved just far enough to show its far edge, or its
/// top if it is taller than the view.
fn revealed(offset: Pixels, view: (Pixels, Pixels), row: (Pixels, Pixels)) -> Pixels {
    let (view_top, view_bottom) = view;
    let (top, bottom) = row;
    if top < view_top || bottom - top > view_bottom - view_top {
        offset + (view_top - top)
    } else if bottom > view_bottom {
        offset - (bottom - view_bottom)
    } else {
        offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{px, size};

    /// The overview: 400 high, 100 down the window, scrolled 300 down.
    const OFFSET: f32 = -300.0;

    fn viewport() -> Bounds<Pixels> {
        Bounds::new(point(px(0.), px(100.)), size(px(600.), px(400.)))
    }

    /// A row of `height` whose top is `top` down the window.
    fn row(top: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(10.), px(top)), size(px(580.), px(height)))
    }

    /// A card of three rows: one in view, one below it, one above it.
    fn card() -> Rows {
        let rows = Rows::default();
        let ids = ["seen", "below", "above"].map(String::from);
        rows.record(
            &ids,
            &[
                row(200., 60.),
                row(470., 60.),
                row(80., 60.),
                row(600., 30.),
            ],
        );
        rows
    }

    #[test]
    fn a_click_never_scrolls_the_overview() {
        let rows = card();
        rows.pick("below", Pick::Click);
        assert_eq!(rows.settle(viewport(), px(OFFSET)), None);
        // A click also drops a reveal asked for before the frame drew.
        let rows = Rows::default();
        rows.pick("below", Pick::Reveal);
        rows.pick("below", Pick::Click);
        rows.record(&["below".into()], &[row(470., 60.)]);
        assert_eq!(rows.settle(viewport(), px(OFFSET)), None);
    }

    #[test]
    fn a_row_already_in_full_view_leaves_the_overview_alone() {
        let rows = card();
        rows.pick("seen", Pick::Reveal);
        assert_eq!(rows.settle(viewport(), px(OFFSET)), None);
        // Touching the edges still counts as in view.
        let rows = Rows::default();
        rows.record(&["edge".into()], &[row(440., 60.)]);
        rows.pick("edge", Pick::Reveal);
        assert_eq!(rows.settle(viewport(), px(OFFSET)), None);
    }

    #[test]
    fn a_row_out_of_view_scrolls_just_enough_to_show_it() {
        // 30 hangs below the bottom edge at 500: the list moves up 30.
        let rows = card();
        rows.pick("below", Pick::Reveal);
        assert_eq!(rows.settle(viewport(), px(OFFSET)), Some(px(OFFSET - 30.)));
        // 20 hides above the top edge at 100: the list moves down 20.
        let rows = card();
        rows.pick("above", Pick::Reveal);
        assert_eq!(rows.settle(viewport(), px(OFFSET)), Some(px(OFFSET + 20.)));
    }

    #[test]
    fn a_row_taller_than_the_view_shows_its_top() {
        assert_eq!(
            revealed(px(OFFSET), (px(100.), px(500.)), (px(300.), px(900.))),
            px(OFFSET - 200.)
        );
    }

    #[test]
    fn a_row_not_drawn_this_frame_moves_nothing_and_each_frame_starts_afresh() {
        let rows = card();
        rows.pick("gone", Pick::Reveal);
        assert_eq!(rows.settle(viewport(), px(OFFSET)), None);
        // The rows recorded went with that frame.
        rows.pick("below", Pick::Reveal);
        assert_eq!(rows.settle(viewport(), px(OFFSET)), None);
    }
}
