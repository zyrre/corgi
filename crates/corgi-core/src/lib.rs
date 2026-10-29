//! Corgi's core: everything a frontend needs that is not drawing. It talks to
//! Herdr, reads the agents' session files, derives the dashboard's rows, keeps
//! the supervisors' memory and inboxes, wakes them, and runs the flows that
//! change Herdr (launch, merge, close, recovery). It depends on no terminal
//! UI library, so the ratatui dashboard, the command-line tools, Herdr's
//! plugin hooks and a desktop app can all build on it.

pub mod activity;
pub mod catalog;
pub mod choices;
pub mod close;
pub mod defaults;
pub mod digest;
pub mod draft;
pub mod first_prompt;
pub mod git;
pub mod harness;
pub mod herdr;
pub mod hook;
pub mod inbox;
mod inbox_privacy;
pub mod job;
pub mod launch;
pub mod markers;
pub mod merge;
pub mod model;
pub mod notice;
pub mod palette;
pub mod paths;
pub mod progress;
pub mod project_main;
pub mod projects;
pub mod protocol;
pub mod recovery;
pub mod refresh;
pub mod rows;
pub mod session;
pub mod setup;
pub mod supervisor;
pub mod sweep;
pub mod tag;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod time;
pub mod usage;
pub mod usage_cache;
pub mod waker;
