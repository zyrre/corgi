//! Helpers the dashboard's test suites share: those of the core (scratch
//! directories, a fake Herdr socket), a dashboard that never touches the
//! developer's own state, and the ways a rendered screen is read back as
//! text.

use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, text::Line};

pub(crate) use corgi_core::test_support::*;

use crate::{
    app::{App, AppInputs},
    herdr::HerdrClient,
};

/// A dashboard for tests: its Herdr client points at a socket nobody listens
/// on, its project memory starts empty and is never saved, and it shows no
/// usage cards, so neither the developer's remembered projects nor the
/// provider CLIs on their PATH change what a test sees.
pub(crate) fn test_app() -> App {
    App::with_inputs(
        HerdrClient::from_socket_path(UNUSED_SOCKET),
        false,
        AppInputs::default(),
    )
}

/// The text of one line, its spans joined.
pub(crate) fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// The text of each line.
pub(crate) fn lines_text(lines: &[Line<'_>]) -> Vec<String> {
    lines.iter().map(line_text).collect()
}

/// Every row of a rendered buffer as text.
pub(crate) fn buffer_rows(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (area.top()..area.bottom())
        .map(|y| {
            (area.left()..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

/// A rendered buffer as one string, row after row with nothing between them,
/// for tests that only ask whether some text is on screen.
pub(crate) fn buffer_text(buffer: &Buffer) -> String {
    buffer.content().iter().map(|cell| cell.symbol()).collect()
}

/// A terminal of `width` by `height` cells for rendering into.
pub(crate) fn test_terminal(width: u16, height: u16) -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(width, height)).expect("build test terminal")
}
