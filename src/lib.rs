// The frontend-independent modules live in the `corgi-core` crate; they are
// re-exported here under their old paths, so `crate::herdr` and the like read
// the same in the dashboard's code.
pub use corgi_core::{
    activity, choices, defaults, digest, git, harness, herdr, inbox, job, model, paths, projects,
    recovery, restart, session, supervisor, tag, time, usage, usage_cache,
};

pub mod app;
#[cfg(test)]
mod brand_tile;
pub(crate) mod motion;
#[cfg(test)]
mod readme_shots;
pub mod serve;
#[cfg(test)]
pub(crate) mod test_support;
pub mod textfield;
pub mod theme;
pub mod ui;
pub mod version;
