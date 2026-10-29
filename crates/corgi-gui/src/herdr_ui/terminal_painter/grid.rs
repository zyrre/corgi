// Copied from penso/herdr-gpui (Herdr UI) crates/herdr-gpui/src/terminal_painter/grid.rs
// at a5b2df2 (2026-10-28). Copyright 2026 Fabien Penso, licensed under the
// Apache License, Version 2.0: see LICENSE-APACHE and NOTICE in crates/corgi-gui.
// Modified for Corgi: imports name Corgi's modules (`crate::endpoint` for the
// protocol types, `crate::herdr_ui` for the rest), and Herdr UI's tests, which
// need its app's test harness, are left out. The code is otherwise unchanged.
//! Cell-grid geometry shared by every rectangle and path the painter draws on
//! the grid, so neighboring primitives meet on the same device pixel.
use super::graphics::CellSeparator;
use crate::endpoint::protocol::FrameData;
use gpui::{Bounds, Pixels, Point, Size, Window, size};

/// What a frame's cells on the theme's default background paint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Ground {
    /// The theme's background, as in an opaque window, and always for a
    /// popup, whose cells must hide the pane beneath.
    #[default]
    Opaque,
    /// Nothing: the window's translucent backdrop is already that color, and
    /// painting it again would stack its alpha.
    Backdrop,
}

pub(super) fn background_extent(
    grid: Size<Pixels>,
    available: Size<Pixels>,
    cell: Size<Pixels>,
) -> Size<Pixels> {
    let extend = |grid, available, cell| {
        if available > grid && available - grid < cell {
            available
        } else {
            grid
        }
    };
    size(
        extend(grid.width, available.width, cell.width),
        extend(grid.height, available.height, cell.height),
    )
}

/// The remainder below the grid continues the last row's backgrounds, as a
/// full-screen app expects, unless that row holds a prompt separator: its cap
/// stays cell-high, so the colors beside it would show as strips beneath it.
pub(super) fn last_row_has_separator(frame: &FrameData) -> bool {
    let Some(last) = usize::from(frame.height).checked_sub(1) else {
        return false;
    };
    let width = usize::from(frame.width);
    let start = last * width;
    frame
        .cells
        .get(start..(start + width).min(frame.cells.len()))
        .unwrap_or_default()
        .iter()
        .any(|cell| CellSeparator::from_symbol(&cell.symbol).is_some())
}

/// Snaps a rectangle's absolute grid corners before `Bounds` forms its size.
/// Rebuilding a far edge as left + width can cross a half-device-pixel rounding
/// tie, leaving a seam or overlap against the neighboring span's near edge.
pub(super) fn grid_rect(
    window: &Window,
    origin: Point<Pixels>,
    near: Point<Pixels>,
    far: Point<Pixels>,
) -> Bounds<Pixels> {
    let (near, far) = grid_corners(window, origin, near, far);
    Bounds::from_corners(near, far)
}

/// The snapped corners themselves, for geometry that is not a `Bounds`.
pub(super) fn grid_corners(
    window: &Window,
    origin: Point<Pixels>,
    near: Point<Pixels>,
    far: Point<Pixels>,
) -> (Point<Pixels>, Point<Pixels>) {
    (
        window.pixel_snap_point(origin + near),
        window.pixel_snap_point(origin + far),
    )
}
