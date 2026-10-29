//! Helpers several app test modules share: pressing a key and polling a
//! background job to completion.

use std::{
    thread,
    time::{Duration, Instant},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::App;

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
