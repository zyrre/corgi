//! Sweeping Corgi's state directory, as a dashboard starts, of the files
//! kept per Herdr session or per dashboard that nothing will read again:
//! the selection file of a dashboard that did not quit cleanly, and the
//! marks and wake record of a Herdr session whose socket is gone.

use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime},
};

use crate::paths::spelled;

/// How long a file of a Herdr session whose socket is gone stays untouched
/// before it goes, so a session that is only stopped for a while, or whose
/// socket is being made again, keeps its marks.
const DEAD_SOCKET_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The files kept per Herdr session, `<kind>/<socket>.<extension>`, that go
/// once their session's socket is gone. A marks record's spawn locks
/// (`.spawn-<digest>.lock`) are not among them: they are empty, and one per
/// project.
const SOCKET_FILES: [(&str, &[&str]); 2] = [
    ("markers", &["json", "lock"]),
    ("wake", &["lock", "seen.json"]),
];

/// Sweeps the state directory `state`, for a dashboard of the Herdr session
/// at `socket`, whose files always stay. A file that cannot be removed is
/// left.
pub fn sweep(state: &Path, socket: &Path) {
    sweep_selections(&state.join("selection"), process_runs);
    let current = spelled(&socket.to_string_lossy());
    let now = SystemTime::now();
    for (kind, extensions) in SOCKET_FILES {
        sweep_dead_sockets(&state.join(kind), extensions, &current, now, |name| {
            spelled_path_exists(Path::new("/"), name)
        });
    }
}

/// Removes from `dir` the selection files, `<socket>.<pid>` and the
/// `.tmp` one written before it, of the dashboards not `running`.
fn sweep_selections(dir: &Path, running: impl Fn(u32) -> bool) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some((socket, pid)) = name.to_str().and_then(|name| name.split_once('.')) else {
            continue;
        };
        let pid = pid.strip_suffix(".tmp").unwrap_or(pid);
        if is_spelled_socket(socket)
            && let Ok(pid) = pid.parse::<u32>()
            && !running(pid)
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Removes from `dir` the files `<socket>.<extension>`, of one of
/// `extensions`, whose socket is not `current` and no longer `exists`, and
/// that were not touched for [`DEAD_SOCKET_AFTER`] by `now`.
fn sweep_dead_sockets(
    dir: &Path,
    extensions: &[&str],
    current: &str,
    now: SystemTime,
    exists: impl Fn(&str) -> bool,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some((socket, extension)) = name.to_str().and_then(|name| name.split_once('.')) else {
            continue;
        };
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .is_ok_and(|touched| {
                now.duration_since(touched)
                    .is_ok_and(|age| age >= DEAD_SOCKET_AFTER)
            });
        if is_spelled_socket(socket)
            && socket != current
            && extensions.contains(&extension)
            && stale
            && !exists(socket)
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Whether `name` is how [`spelled`] spells an absolute path.
fn is_spelled_socket(name: &str) -> bool {
    name.len() > 1
        && name.starts_with('_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether some existing path under `dir` is spelled `rest`, the `_` that
/// stands for the separator after `dir` included. The spelling cannot be
/// undone, since every `_` may stand for any character but a letter or
/// digit, so the directories are walked down to whatever matches.
fn spelled_path_exists(dir: &Path, rest: &str) -> bool {
    let Some(rest) = rest.strip_prefix('_') else {
        return false;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        match rest.strip_prefix(spelled(&entry.file_name().to_string_lossy()).as_str()) {
            Some("") => true,
            Some(after) => after.starts_with('_') && spelled_path_exists(&entry.path(), after),
            None => false,
        }
    })
}

/// Whether the process `pid` is running, or may be: one that runs as
/// another user also counts.
fn process_runs(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 sends nothing; it only checks that the process exists.
    let sent = unsafe { libc::kill(pid, 0) } == 0;
    sent || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use super::*;
    use crate::test_support::ScratchDir;

    fn touch(path: &Path, age: Duration) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        File::create(path)
            .unwrap()
            .set_modified(SystemTime::now() - age)
            .unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_selection_files_of_dashboards_no_longer_running_are_swept() {
        let scratch = ScratchDir::new("sweep-selection");
        let dir = scratch.join("selection");
        for name in [
            "_herdr_sock.100",
            "_herdr_sock.200",
            "_herdr_sock.300.tmp",
            "_other_sock.400",
            "_herdr_sock.notes",
            "README",
        ] {
            touch(&dir.join(name), Duration::ZERO);
        }
        sweep_selections(&dir, |pid| pid == 200 || pid == 400);
        assert_eq!(
            names(&dir),
            [
                "README",
                "_herdr_sock.200",
                "_herdr_sock.notes",
                "_other_sock.400"
            ]
        );
    }

    #[test]
    fn this_process_runs_and_a_reaped_child_does_not() {
        assert!(process_runs(std::process::id()));
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!process_runs(pid));
    }

    #[test]
    fn a_path_is_found_by_its_spelling_however_its_underscores_were_spelled() {
        let scratch = ScratchDir::new("sweep-spelling");
        let socket = scratch.join(".config/herdr/sessions/a-b/herdr.sock");
        touch(&socket, Duration::ZERO);
        let exists =
            |path: &Path| spelled_path_exists(Path::new("/"), &spelled(&path.to_string_lossy()));
        assert!(exists(&socket));
        assert!(exists(&scratch.join(".config/herdr")));
        assert!(!exists(
            &scratch.join(".config/herdr/sessions/a-b/herdr.sock2")
        ));
        assert!(!exists(
            &scratch.join(".config/herdr/sessions/a-c/herdr.sock")
        ));
    }

    #[test]
    fn the_files_of_a_herdr_session_whose_socket_is_gone_are_swept_once_untouched() {
        let scratch = ScratchDir::new("sweep-sockets");
        let live = scratch.join("live/herdr.sock");
        touch(&live, Duration::ZERO);
        let spell = |path: &Path| spelled(&path.to_string_lossy());
        let (live, gone, current) = (
            spell(&live),
            spell(&scratch.join("gone/herdr.sock")),
            spell(&scratch.join("current/herdr.sock")),
        );
        let old = DEAD_SOCKET_AFTER + Duration::from_secs(60);
        let dir = scratch.join("markers");
        let files = [
            (format!("{live}.json"), old),
            (format!("{live}.lock"), old),
            (format!("{gone}.json"), old),
            (format!("{gone}.lock"), old),
            (format!("{gone}.spawn-0123456789abcdef.lock"), old),
            (format!("{gone}.seen.json"), old),
            (format!("{current}.json"), old),
            (format!("{gone}.json.tmp"), old),
            ("notes.json".to_string(), old),
        ];
        for (name, age) in &files {
            touch(&dir.join(name), *age);
        }
        let recent = scratch.join("recent");
        touch(
            &recent.join(format!("{gone}.json")),
            Duration::from_secs(60),
        );

        let sweep = |dir: &Path| {
            sweep_dead_sockets(
                dir,
                &["json", "lock"],
                &current,
                SystemTime::now(),
                |name| spelled_path_exists(Path::new("/"), name),
            )
        };
        sweep(&dir);
        sweep(&recent);
        let mut kept: Vec<String> = files
            .iter()
            .map(|(name, _)| name.clone())
            .filter(|name| name != &format!("{gone}.json") && name != &format!("{gone}.lock"))
            .collect();
        kept.sort();
        assert_eq!(names(&dir), kept);
        assert_eq!(names(&recent), [format!("{gone}.json")]);
    }
}
