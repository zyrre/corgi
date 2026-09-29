//! The user's home directory and Corgi's state directory, the `~`
//! shorthand people type and read paths with, and the directory name a path
//! is known by.

use std::{
    env,
    path::{Path, PathBuf},
};

/// The home directory, from `HOME`. `None` when it is unset or empty.
pub fn home() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Corgi's own directory under `$XDG_STATE_HOME` or `~/.local/state`.
pub fn corgi_state_dir() -> Option<PathBuf> {
    let state_home = env::var_os("XDG_STATE_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|home| home.join(".local/state")))?;
    Some(state_home.join("corgi"))
}

/// The home directory as text, for the string edits below.
fn home_text() -> Option<String> {
    home().and_then(|home| home.into_os_string().into_string().ok())
}

/// `typed` with a leading `~` spelled out as the home directory, so paths
/// can be typed the way people write them.
pub fn expand_home(typed: &str) -> String {
    let Some(home) = home_text() else {
        return typed.to_string();
    };
    match typed.strip_prefix('~') {
        Some("") => home,
        Some(rest) if rest.starts_with('/') => format!("{home}{rest}"),
        _ => typed.to_string(),
    }
}

/// `path` with the home directory shortened to `~`, as people write it.
pub fn tilde(path: &str) -> String {
    match home_text() {
        Some(home) if home != "/" => match path.strip_prefix(home.as_str()) {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
            _ => path.to_string(),
        },
        _ => path.to_string(),
    }
}

/// Whether `path` is the home directory itself, not one of its
/// subdirectories: `~`, `$HOME` and a spelling with a trailing `/` or through
/// a symlink all are. The home directory is where scratch agents run, and it
/// is never a project.
pub fn is_home<P: AsRef<Path> + ?Sized>(path: &P) -> bool {
    let Some(home) = home() else {
        return false;
    };
    let typed = path.as_ref().to_string_lossy();
    let path = PathBuf::from(expand_home(typed.trim()));
    if path.as_os_str().is_empty() {
        return false;
    }
    match (path.canonicalize(), home.canonicalize()) {
        (Ok(path), Ok(home)) => path == home,
        // A directory that does not exist is compared as written.
        _ => path.components().eq(home.components()),
    }
}

/// The last component of `path`, which is the name a project or checkout is
/// known by. `None` for a path without one, such as `/`, or one that is not
/// UTF-8; each caller says what stands in for it.
pub fn dir_name<P: AsRef<Path> + ?Sized>(path: &P) -> Option<&str> {
    path.as_ref()
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
}

/// A file of Corgi's state kept per Herdr session: `<kind>/<socket>.<extension>`
/// in the state directory, the socket path spelled with only ASCII letters,
/// digits and underscores, so that each Herdr session has its own.
pub fn socket_state_file(kind: &str, socket: &Path, extension: &str) -> Option<PathBuf> {
    let name: String = socket
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    corgi_state_dir().map(|dir| dir.join(kind).join(format!("{name}.{extension}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_home_directory_is_known_however_it_is_spelled_but_not_below_it() {
        let Some(home) = home() else {
            return;
        };
        let text = home.to_string_lossy().into_owned();
        assert!(is_home("~"));
        assert!(is_home(&home));
        assert!(is_home(&format!("{text}/")));
        assert!(is_home(&format!("{text}/.")));
        assert!(!is_home("~/repos"));
        assert!(!is_home(&home.join("repos")));
        assert!(!is_home(""));
        assert!(!is_home("/"));
    }
}
