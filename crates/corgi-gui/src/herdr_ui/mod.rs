//! Herdr UI's terminal painter (penso/herdr-gpui, Apache-2.0), lifted at
//! a5b2df2: it paints the cells of a Herdr endpoint surface, with box drawing
//! and powerline symbols as shapes, colour emoji, and images a pane places.
//! `terminal_painter` and its submodules are Herdr UI's code unchanged but for
//! their imports; `terminal` and `config` keep the parts of Herdr UI's files of
//! those names that the painter and the focus view's input need. Each file
//! says what it came from and what changed; `NOTICE` in this crate has the
//! attribution. Corgi's own code (the focus view, selection, links) is in
//! `crate::terminal`.
//!
//! Upgrading: copy the painter's files from a newer Herdr UI again and
//! reapply the import changes; the painter takes `crate::endpoint`'s protocol
//! types, so a Herdr UI that changes those changes the adapter too.

#[allow(dead_code)]
pub mod config;
#[allow(dead_code)]
pub mod terminal;
// Lifted whole: some of its items serve Herdr UI's other views, and it is
// left as Herdr UI wrote it where a newer clippy would phrase it otherwise.
#[allow(unused_imports, dead_code, unknown_lints)]
#[allow(clippy::chunks_exact_to_as_chunks)]
pub mod terminal_painter;

use std::fmt;

/// What the painter's image module can fail with; Herdr UI's crate error,
/// reduced to the two the painter uses.
#[derive(Debug)]
pub enum Error {
    PaneImageDecode(image::ImageError),
    PaneImageLimit,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PaneImageDecode(_) => write!(f, "Could not decode an image a pane placed."),
            Self::PaneImageLimit => {
                write!(f, "An image a pane placed exceeds the decoded size limit.")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PaneImageDecode(error) => Some(error),
            Self::PaneImageLimit => None,
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
