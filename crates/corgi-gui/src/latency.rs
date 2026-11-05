//! The latency log (`CORGI_GUI_LATENCY`) of a row's expansion: from the
//! Space that expands it, when its transcript arrives, each frame's layout
//! of it and each move of the overview, so a later change can be measured
//! the way this one was. It says nothing unless the variable is set, and
//! only for a few seconds after an expansion.

use std::{
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

/// How long after an expansion its frames are logged.
const FOLLOWED: Duration = Duration::from_secs(3);

static EXPANDED: Mutex<Option<Instant>> = Mutex::new(None);

/// Whether `CORGI_GUI_LATENCY` is set.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CORGI_GUI_LATENCY").is_some())
}

/// A row was expanded now.
pub fn expanded() {
    if !enabled() {
        return;
    }
    if let Ok(mut at) = EXPANDED.lock() {
        *at = Some(Instant::now());
    }
    eprintln!("corgi-gui: latency expand +0.0 ms Space");
}

/// Logs `what` with the time since the expansion, while it is followed.
pub fn expansion(what: impl FnOnce() -> String) {
    if !enabled() {
        return;
    }
    let Some(at) = EXPANDED.lock().ok().and_then(|at| *at) else {
        return;
    };
    let since = at.elapsed();
    if since <= FOLLOWED {
        eprintln!(
            "corgi-gui: latency expand +{:.1} ms {}",
            since.as_secs_f64() * 1000.0,
            what()
        );
    }
}
