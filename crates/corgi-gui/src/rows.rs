//! Where the overview's agent rows are, so that a selection made without
//! clicking its row (the keyboard, the side panel's "Select it") scrolls the
//! overview just enough to show that row, and a click on a row scrolls
//! nothing: the row stays under the mouse for a double click.
//!
//! The rows sit in project cards, not directly in the scrolled list, so
//! GPUI's `ScrollHandle::scroll_to_item`, which knows only the list's
//! children, can show only a whole card (and one taller than the window
//! not at all). Instead each row records its layout as it is laid out
//! ([`Rows::slot`]), and the list, wrapped in an [`Overview`], moves its
//! offset after the frame's layout and before the list is prepainted, so
//! the rows are drawn at the new offset in the same frame: a row that
//! grows, or one brought into view, never shows a frame at the old offset
//! first.
//!
//! The expanded row (Space) fills the overview down to its bottom: the list
//! is drawn down to that row and no further, and stretched to the view's
//! height at its current offset (`fill`), so the row grows where it is. Only
//! when the rows above leave it less than its least height does the overview
//! scroll, to its end, the rows above going off the top.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, ScrollHandle, Window, point, px,
};

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
    /// This frame's rows, by pane ID, as laid out, and the list's content.
    layouts: HashMap<String, LayoutId>,
    content: Option<LayoutId>,
    /// This frame's rows, by pane ID: their top and bottom, in window
    /// coordinates at the overview's current offset.
    drawn: HashMap<String, (Pixels, Pixels)>,
    /// The row to bring into view once the frame's rows are known.
    reveal: Option<String>,
    /// The row expanded into its session, which fills the view's bottom,
    /// and the height the content was stretched to for it this frame.
    expanded: Option<String>,
    filled: Option<Pixels>,
}

/// Which of the overview's elements a [`Slot`] records.
#[derive(Clone)]
enum Part {
    Row(String),
    Content,
}

impl Rows {
    /// The next frame brings `pane_id`'s row into view, or, for a click,
    /// leaves the overview where it is (dropping an earlier request).
    pub fn pick(&self, pane_id: &str, pick: Pick) {
        self.0.borrow_mut().reveal = (pick == Pick::Reveal).then(|| pane_id.to_string());
    }

    /// The row expanded into its session this frame, if any, and the
    /// height the overview's content is stretched to for it: down to the
    /// view's bottom at the offset it has, so the expanded row grows to it
    /// without the overview moving. It is the last frame's view; the
    /// [`Overview`] draws again when this frame's differs, as after a
    /// resize. Called as the overview is rendered, it starts the frame's
    /// record of where the rows are.
    pub fn expand(&self, pane_id: Option<&str>, scroll: &ScrollHandle) -> Option<Pixels> {
        let mut places = self.0.borrow_mut();
        places.layouts.clear();
        places.content = None;
        places.expanded = pane_id.map(str::to_string);
        places.filled = pane_id.map(|_| fill(scroll.bounds().size.height, scroll.offset().y));
        places.filled
    }

    /// `row`, `pane_id`'s, recording where it is laid out.
    pub fn slot(&self, pane_id: &str, row: AnyElement) -> AnyElement {
        Slot {
            child: row,
            part: Part::Row(pane_id.to_string()),
            rows: self.clone(),
        }
        .into_any_element()
    }

    /// The overview list's one child, recording where it is laid out.
    pub fn content(&self, content: AnyElement) -> AnyElement {
        Slot {
            child: content,
            part: Part::Content,
            rows: self.clone(),
        }
        .into_any_element()
    }

    /// The rows drawn this frame, `ids` with their bounds, in window
    /// coordinates at the overview's offset.
    pub fn record(&self, ids: &[String], bounds: &[Bounds<Pixels>]) {
        let mut places = self.0.borrow_mut();
        for (id, bounds) in ids.iter().zip(bounds) {
            places
                .drawn
                .insert(id.clone(), (bounds.top(), bounds.bottom()));
        }
    }

    /// The offset that shows the row asked for, or gives the expanded row
    /// its room, if it must change, once every row of the frame is
    /// recorded; `viewport` is the overview's bounds and `end` the offset
    /// of its end. Forgets this frame's rows.
    fn settle(&self, viewport: Bounds<Pixels>, offset: Pixels, end: Pixels) -> Option<Pixels> {
        let mut places = self.0.borrow_mut();
        let drawn = std::mem::take(&mut places.drawn);
        let reveal = places.reveal.take();
        let view = (viewport.top(), viewport.bottom());
        let moved = match places.expanded.as_deref() {
            Some(expanded) => {
                let row = *drawn.get(expanded)?;
                crate::latency::expansion(|| {
                    format!(
                        "overview offset {:.0}, expanded row y {:.0}..{:.0}, view {:.0}..{:.0}",
                        f32::from(offset),
                        f32::from(row.0),
                        f32::from(row.1),
                        f32::from(view.0),
                        f32::from(view.1),
                    )
                });
                grown(offset, end, view, row, reveal.as_deref() == Some(expanded))
            }
            None => revealed(offset, view, *drawn.get(reveal?.as_str())?),
        };
        (moved != offset).then_some(moved)
    }

    /// Settles the overview, laid out in `viewport`, before it is
    /// prepainted: records where this frame's rows are drawn at the offset
    /// `scroll` has, and moves it to show the row asked for. Draws again
    /// when the expanded row's stretch was made for another offset.
    fn apply(&self, viewport: Bounds<Pixels>, scroll: &ScrollHandle, window: &mut Window) {
        let offset = scroll.offset();
        let (layouts, content) = {
            let places = self.0.borrow();
            let layouts: Vec<(String, LayoutId)> = places
                .layouts
                .iter()
                .map(|(id, layout)| (id.clone(), *layout))
                .collect();
            (layouts, places.content)
        };
        // Laid out, a row's bounds do not count the scroll yet: the list
        // offsets its children by it as it prepaints them.
        let (ids, bounds): (Vec<String>, Vec<Bounds<Pixels>>) = layouts
            .into_iter()
            .map(|(id, layout)| {
                let mut bounds = window.layout_bounds(layout);
                bounds.origin.y += offset.y;
                (id, bounds)
            })
            .unzip();
        self.record(&ids, &bounds);
        let height = content.map_or(viewport.size.height, |content| {
            window.layout_bounds(content).size.height
        });
        let end = -(height - viewport.size.height).max(px(0.));
        let moved = self.settle(viewport, offset.y, end);
        if let Some(y) = moved {
            scroll.set_offset(point(offset.x, y));
        }
        let now = moved.unwrap_or(offset.y);
        let stale = self
            .0
            .borrow()
            .filled
            .is_some_and(|filled| (filled - fill(viewport.size.height, now)).abs() >= px(0.5));
        if moved.is_some() || stale {
            crate::latency::expansion(|| {
                format!("overview moves to {moved:?} before it is drawn (stale fill: {stale})")
            });
        }
        if stale {
            window.on_next_frame(|window, _| window.refresh());
        }
    }

    /// `list`, the overview's scrolled list tracking `scroll`, settled
    /// before it is prepainted.
    pub fn overview(&self, list: AnyElement, scroll: &ScrollHandle) -> Overview {
        Overview {
            list,
            scroll: scroll.clone(),
            rows: self.clone(),
        }
    }
}

/// The scroll offset (negative as the list scrolls down) that brings a row
/// spanning `row` into a `view`, both at `offset`: unchanged while the row
/// shows in full, else moved just far enough to show its far edge, or its
/// top if it is taller than the view.
pub(crate) fn revealed(offset: Pixels, view: (Pixels, Pixels), row: (Pixels, Pixels)) -> Pixels {
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

/// The height that stretches the overview's content down to the bottom of
/// a view `height` high at `offset`.
fn fill(height: Pixels, offset: Pixels) -> Pixels {
    height - offset
}

/// The offset for the expanded row spanning `row` in a `view`, both at
/// `offset`; `end` is the offset that shows the overview's end. The row
/// reaches the view's bottom, and hangs below it only when the rows above
/// leave it less than its least height: then the overview scrolls to its
/// end, as far as the row's top stays in view. A row picked from the
/// keyboard above the view shows its top.
fn grown(
    offset: Pixels,
    end: Pixels,
    view: (Pixels, Pixels),
    row: (Pixels, Pixels),
    reveal: bool,
) -> Pixels {
    let (view_top, view_bottom) = view;
    let (top, bottom) = row;
    let showing_top = offset + (view_top - top);
    if reveal && top < view_top {
        showing_top
    } else if bottom > view_bottom + px(0.5) {
        // Scrolling to the end moves every row up by `offset - end`.
        if top - (offset - end) < view_top {
            showing_top
        } else {
            end
        }
    } else {
        offset
    }
}

/// An element of the overview that records its layout in [`Rows`].
struct Slot {
    child: AnyElement,
    part: Part,
    rows: Rows,
}

impl IntoElement for Slot {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Slot {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let layout = self.child.request_layout(window, cx);
        let mut places = self.rows.0.borrow_mut();
        match &self.part {
            Part::Row(id) => {
                places.layouts.insert(id.clone(), layout);
            }
            Part::Content => places.content = Some(layout),
        }
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

/// The overview's scrolled list, its offset settled between the frame's
/// layout and the list's prepaint (see the module's notes).
pub struct Overview {
    list: AnyElement,
    scroll: ScrollHandle,
    rows: Rows,
}

impl IntoElement for Overview {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Overview {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.list.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.rows.apply(bounds, &self.scroll, window);
        self.list.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.list.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    /// The overview: 400 high, 100 down the window, scrolled 300 down, of
    /// 800 it could scroll.
    const OFFSET: f32 = -300.0;
    const END: f32 = -800.0;

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
        assert_eq!(rows.settle(viewport(), px(OFFSET), px(END)), None);
        // A click also drops a reveal asked for before the frame drew.
        let rows = Rows::default();
        rows.pick("below", Pick::Reveal);
        rows.pick("below", Pick::Click);
        rows.record(&["below".into()], &[row(470., 60.)]);
        assert_eq!(rows.settle(viewport(), px(OFFSET), px(END)), None);
    }

    #[test]
    fn a_row_already_in_full_view_leaves_the_overview_alone() {
        let rows = card();
        rows.pick("seen", Pick::Reveal);
        assert_eq!(rows.settle(viewport(), px(OFFSET), px(END)), None);
        // Touching the edges still counts as in view.
        let rows = Rows::default();
        rows.record(&["edge".into()], &[row(440., 60.)]);
        rows.pick("edge", Pick::Reveal);
        assert_eq!(rows.settle(viewport(), px(OFFSET), px(END)), None);
    }

    #[test]
    fn a_row_out_of_view_scrolls_just_enough_to_show_it() {
        // 30 hangs below the bottom edge at 500: the list moves up 30.
        let rows = card();
        rows.pick("below", Pick::Reveal);
        assert_eq!(
            rows.settle(viewport(), px(OFFSET), px(END)),
            Some(px(OFFSET - 30.))
        );
        // 20 hides above the top edge at 100: the list moves down 20.
        let rows = card();
        rows.pick("above", Pick::Reveal);
        assert_eq!(
            rows.settle(viewport(), px(OFFSET), px(END)),
            Some(px(OFFSET + 20.))
        );
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
        assert_eq!(rows.settle(viewport(), px(OFFSET), px(END)), None);
        // The rows recorded went with that frame.
        rows.pick("below", Pick::Reveal);
        assert_eq!(rows.settle(viewport(), px(OFFSET), px(END)), None);
    }

    #[test]
    fn an_expanded_row_with_room_grows_where_it_is() {
        // It fills the view from 200 down to its bottom at 500: nothing
        // moves, a click's or a key's.
        let rows = Rows::default();
        rows.record(&["open".into()], &[row(200., 300.)]);
        rows.expand(Some("open"), &ScrollHandle::new());
        rows.pick("open", Pick::Reveal);
        assert_eq!(rows.settle(viewport(), px(OFFSET), px(END)), None);
        rows.record(&["open".into()], &[row(200., 300.)]);
        rows.pick("open", Pick::Click);
        assert_eq!(rows.settle(viewport(), px(OFFSET), px(END)), None);
    }

    #[test]
    fn an_expanded_row_low_in_the_view_pulls_the_rows_above_off_the_top() {
        // Its least height hangs 150 below the view: the overview scrolls
        // to its end, which leaves the row's top in view.
        let rows = Rows::default();
        rows.record(&["open".into()], &[row(450., 200.)]);
        rows.expand(Some("open"), &ScrollHandle::new());
        assert_eq!(
            rows.settle(viewport(), px(OFFSET), px(-500.)),
            Some(px(-500.))
        );
        // In a view too short for it, its top stays in view.
        assert_eq!(
            grown(
                px(OFFSET),
                px(-800.),
                (px(100.), px(500.)),
                (px(450.), px(950.)),
                false
            ),
            px(OFFSET - 350.)
        );
    }

    #[test]
    fn a_key_onto_an_expanded_row_above_the_view_shows_its_top() {
        let rows = Rows::default();
        rows.record(&["open".into()], &[row(40., 460.)]);
        rows.expand(Some("open"), &ScrollHandle::new());
        rows.pick("open", Pick::Reveal);
        assert_eq!(
            rows.settle(viewport(), px(OFFSET), px(END)),
            Some(px(OFFSET + 60.))
        );
        // A click leaves it be.
        rows.record(&["open".into()], &[row(40., 460.)]);
        rows.pick("open", Pick::Click);
        assert_eq!(rows.settle(viewport(), px(OFFSET), px(END)), None);
    }
}
