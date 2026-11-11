//! The `corgi` commands: each submodule parses its commands' arguments,
//! runs them and prints what they found.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::{
    digest::memory::corgi_bin_text, git::trust_root, hc, inbox, paths::expand_home, time::unix_now,
};

mod app_layout;
mod daemon;
mod fleet;
mod launch;
mod memory;
mod open;
mod pause;
mod status;
mod usage;

pub use app_layout::{APP_LAYOUT_USAGE, app_layout_command};
pub use daemon::{DAEMON_USAGE, daemon_command};
pub use fleet::{FLEET_USAGE, REPORT_USAGE, TAG_USAGE, fleet_command, report_command, tag_command};
pub use launch::{SPAWN_USAGE, SUPERVISOR_USAGE, spawn_command, supervisor_command};
pub use memory::{
    DIGEST_USAGE, INBOX_USAGE, NOTIFY_USAGE, ROLE_USAGE, digest_command, inbox_command,
    notify_command, role_command,
};
pub use open::{OPEN_USAGE, PROJECTS_USAGE, open_command, projects_command};
pub use pause::{PAUSE_USAGE, RESUME_USAGE, pause_command, resume_command};
pub use status::{
    CLOSE_USAGE, MERGE_USAGE, STATUS_USAGE, close_command, merge_command, status_command,
};
pub use usage::{USAGE_USAGE, usage_command};

/// The project root of `project` on hc, as its agents' `corgi.project`
/// label holds it: the primary checkout of its repository, as Git says, or
/// the directory itself outside Git, without symlinks so that labels
/// compare as text.
fn project_root_of(project: &Path) -> String {
    let root = trust_root(project);
    fs::canonicalize(&root)
        .unwrap_or(root)
        .to_string_lossy()
        .into_owned()
}

/// The root of `project` (default: the current directory): its
/// repository's primary checkout, as hc's labels hold it.
fn project_root(project: Option<&str>) -> Result<String> {
    Ok(project_root_of(&existing_project_dir(project)?))
}

/// `project`, or the current directory, as an existing absolute directory.
fn existing_project_dir(project: Option<&str>) -> Result<PathBuf> {
    let project = match project {
        Some(path) => PathBuf::from(expand_home(path.trim())),
        None => env::current_dir().context("read the current directory")?,
    };
    let project = std::path::absolute(&project)
        .with_context(|| format!("resolve project {}", project.display()))?;
    anyhow::ensure!(
        project.is_dir(),
        "project {} is not an existing directory",
        project.display()
    );
    Ok(project)
}

/// Prints, on stderr, that the supervisor of the project at `root` has missed
/// inbox items, if it does: any undelivered while nothing delivers them (no
/// `corgi daemon` runs for the core of the terminal this runs in), else
/// those that have waited minutes, not just for the supervisor's turn.
fn print_inbox_footer(root: &str) {
    let delivering = hc::in_terminal()
        && hc::expected_socket().is_some_and(|socket| hc::daemon::running(&socket).running);
    if let Some(footer) = crate::supervisor::state_dir(root)
        .ok()
        .and_then(|dir| inbox::footer(&dir, &corgi_bin_text(), root, delivering, unix_now()))
    {
        eprintln!("{footer}");
    }
}
