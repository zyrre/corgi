//! Helpers several app test modules share: pressing a key, polling a
//! background job to completion, and recording the status lines a launch or
//! merge step reports.

use std::{
    thread,
    time::{Duration, Instant},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, progress::Progress};

pub(super) fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.handle_key(KeyEvent::new(code, modifiers));
}

/// Runs `poll` until `done` holds, failing after a few seconds.
pub(super) fn poll_until(
    app: &mut App,
    mut poll: impl FnMut(&mut App),
    done: impl Fn(&App) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(app) {
        assert!(
            Instant::now() < deadline,
            "the background job never got there"
        );
        thread::sleep(Duration::from_millis(5));
        poll(app);
    }
}

/// Keeps the status lines a launch step reports.
#[derive(Default)]
pub(super) struct Recorded(pub(super) Vec<String>);

impl Progress for Recorded {
    fn report(&mut self, message: String) {
        self.0.push(message);
    }
}
