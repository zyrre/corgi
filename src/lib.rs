//! Corgi: the `corgi` command-line tools, the dashboard, and what both are
//! built on: Herdr and herdr-core (`hc`), the agents' session files, the
//! supervisors' memory and inboxes, their waker, and the flows that start,
//! merge and close agents.

pub mod activity;
pub mod app;
pub mod app_layout;
pub mod app_space;
pub mod art;
pub mod backend;
pub mod backlog;
#[cfg(test)]
mod brand_tile;
pub mod catalog;
pub mod choices;
pub mod close;
pub mod defaults;
pub mod digest;
pub mod draft;
pub mod first_prompt;
pub mod git;
pub mod harness;
pub mod hc;
pub mod herdr;
pub mod inbox;
mod inbox_privacy;
pub mod job;
pub mod last_harness;
pub mod launch;
pub mod markers;
pub mod merge;
pub mod model;
pub(crate) mod motion;
pub mod notice;
pub mod orphans;
pub mod palette;
pub mod paths;
pub mod pause;
pub mod progress;
pub mod project_main;
pub mod projects;
#[cfg(test)]
mod readme_shots;
pub mod recovery;
pub mod refresh;
pub mod repo;
pub mod restart;
pub mod rows;
pub mod session;
pub mod supervisor;
pub mod sweep;
pub mod tag;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod textfield;
pub mod theme;
pub mod time;
pub mod ui;
pub mod usage;
pub mod usage_cache;
pub mod version;
pub mod waker;
