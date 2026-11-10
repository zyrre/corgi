//! The `corgi` commands: each submodule parses a command's arguments, runs
//! it and prints what it found.

mod app_layout;
mod app_space;
mod cli;
mod daemon;
mod hc_cli;
mod hc_pause;
mod open;
mod status;
mod usage;

pub use app_layout::{APP_LAYOUT_USAGE, app_layout_command};
pub use app_space::{APP_SPACE_USAGE, app_space_command};
pub use cli::{
    DIGEST_USAGE, FLEET_USAGE, INBOX_USAGE, NOTIFY_USAGE, REPORT_USAGE, ROLE_USAGE, SPAWN_USAGE,
    SUPERVISOR_USAGE, TAG_USAGE, digest, fleet, inbox_command, notify, report, role_command, spawn,
    supervisor_command, tag_command,
};
pub use daemon::{DAEMON_USAGE, daemon_command};
pub use hc_pause::{PAUSE_USAGE, RESUME_USAGE, pause_command, resume_command};
pub use open::{OPEN_USAGE, PROJECTS_USAGE, open_command, projects_command};
pub use status::{
    CLOSE_USAGE, MERGE_USAGE, STATUS_USAGE, close_command, merge_command, status_command,
};
pub use usage::{USAGE_USAGE, usage_command};
