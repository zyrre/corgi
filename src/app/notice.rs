//! A one-shot message for the dashboard from a command run beside it.
//!
//! `corgi setup` ends by opening or focusing the dashboard, which takes the
//! user to its tab before they can read what setup printed. So setup leaves
//! its outcome in `setup-notice.json` in Corgi's state directory, and the
//! next full dashboard to refresh takes it and shows it in a popup. Herdr's
//! `plugin action invoke` passes no arguments, hence the file; it works the
//! same for a dashboard the open action starts and for one already running.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::paths::corgi_state_dir;

/// How long a notice waits for a dashboard to take it. Past this it is
/// thrown away unread, so one left by a setup whose dashboard never opened
/// does not pop up days later.
const NOTICE_TTL_SECS: u64 = 5 * 60;

/// What a notice file holds: the message, and when it was left (Unix
/// seconds).
#[derive(Debug, Serialize, Deserialize)]
struct Notice {
    message: String,
    at: u64,
}

/// Where setup leaves its notice: `setup-notice.json` in Corgi's state
/// directory.
pub(super) fn notice_path() -> Option<PathBuf> {
    corgi_state_dir().map(|dir| dir.join("setup-notice.json"))
}

/// Leaves `message` at `path`, written `at` (Unix seconds), through a
/// temporary file renamed over it so a dashboard never reads half of it.
pub(super) fn leave(path: &Path, message: &str, at: u64) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let notice = Notice {
        message: message.to_string(),
        at,
    };
    let text = serde_json::to_string(&notice).map_err(io::Error::other)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.tmp", std::process::id()));
    fs::write(&temporary, text)?;
    fs::rename(&temporary, path)
}

/// Takes the notice at `path`, if there is one and it is no older than
/// [`NOTICE_TTL_SECS`] at `now`. The file is removed either way, and only
/// the process whose removal succeeds gets the message, so two dashboards
/// never both show it.
pub(super) fn take(path: &Path, now: u64) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    fs::remove_file(path).ok()?;
    let notice: Notice = serde_json::from_str(&text).ok()?;
    (now.saturating_sub(notice.at) <= NOTICE_TTL_SECS).then_some(notice.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ScratchDir;

    #[test]
    fn a_notice_is_taken_once() {
        let dir = ScratchDir::new("notice-once");
        let path = dir.join("state").join("setup-notice.json");
        leave(&path, "prefix+d now opens Corgi", 1_000).unwrap();

        assert_eq!(
            take(&path, 1_010).as_deref(),
            Some("prefix+d now opens Corgi")
        );
        assert!(!path.exists());
        assert_eq!(take(&path, 1_010), None);
    }

    #[test]
    fn a_stale_or_unreadable_notice_is_removed_unread() {
        let dir = ScratchDir::new("notice-stale");
        let path = dir.join("setup-notice.json");
        leave(&path, "Corgi already opens with prefix+d", 1_000).unwrap();
        assert_eq!(take(&path, 1_000 + NOTICE_TTL_SECS + 1), None);
        assert!(!path.exists());

        fs::write(&path, "not json").unwrap();
        assert_eq!(take(&path, 1_000), None);
        assert!(!path.exists());
    }
}
