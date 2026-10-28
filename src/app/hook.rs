//! `corgi hook`, which Herdr runs at its start and on the events the plugin's
//! manifest names. The work is in the core's `hook` module; this is the
//! command around it.

use anyhow::Result;

use crate::herdr::HerdrClient;
use corgi_core::hook::{self, Event, Limits, Watchdog};

pub const HOOK_USAGE: &str = "\
Usage: corgi hook

Herdr runs this when its server starts or hands over, and when an agent is
detected, changes status, or a pane or workspace goes (the plugin's manifest
names the events). Each run makes one pass at what an open dashboard does:
puts back the marks a Herdr restart lost, records new agents' sessions, adds
stopped agents to their supervisor's inbox and delivers it once the
supervisor is between turns, and hands a supervisor whose context is full
over to a fresh session. A run that finds another one at work leaves it to
that one and exits. Every run ends within its time limit.

Run by hand inside Herdr, it makes the same pass.";

/// Runs the hook Herdr's variables describe, and prints its line for
/// Herdr's plugin log (`herdr plugin log list`).
pub fn hook_command(_args: &[String]) -> Result<()> {
    let event = Event::from_env();
    let client = HerdrClient::from_env()?;
    let limits = Limits::from_env();
    // Herdr gives a hook no time limit and keeps a hung one's slot for good,
    // so the hook ends itself; the wake lock goes with the process.
    let watchdog = Watchdog::start(limits.hard, move || {
        eprintln!(
            "corgi hook: past its time limit of {} ms; ending",
            limits.hard.as_millis()
        );
        std::process::exit(124);
    });
    let line = hook::run(client, &event, limits, &watchdog);
    watchdog.stop();
    println!("{line}");
    Ok(())
}
