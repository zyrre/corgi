//! Helpers the test suites share: scratch directories that clean up after
//! themselves, and Git repositories made with a test identity. It uses std
//! alone, so `tests/open.rs`, which runs the real binary, includes this file
//! as a module of its own.

use std::{
    fs,
    ops::Deref,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// A directory of its own for one test, removed with everything in it when
/// the value is dropped. The process id, the clock and a counter go into its
/// name, so parallel tests and parallel test runs never share one.
pub struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    pub fn new(label: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "corgi-{label}-{}-{nanos}-{count}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create scratch dir");
        Self { path }
    }
}

/// A scratch directory is used as the path it is.
impl Deref for ScratchDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for ScratchDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).ok();
    }
}

/// Runs `git args` in `dir` with a test identity and no signing, whatever
/// the developer's own Git configuration says, and fails the test if Git
/// does.
pub fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_AUTHOR_NAME", "Corgi Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Corgi Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .stdout(std::process::Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} in {}", dir.display());
}

/// A repository at `dir` on branch `main` with one empty commit, created
/// with its parents; returns its canonical path, as Git reports paths.
pub fn git_repo(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).expect("create repository directory");
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "first"]);
    dir.canonicalize().expect("canonical repository path")
}

/// A plain linked worktree of `repo` at `dir` on a new `branch`, as
/// `git worktree add` (and Claude Code's `--worktree`) makes one; returns
/// its canonical path.
pub fn git_worktree(repo: &Path, dir: &Path, branch: &str) -> PathBuf {
    git(
        repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            branch,
            &dir.to_string_lossy(),
        ],
    );
    dir.canonicalize().expect("canonical worktree path")
}
