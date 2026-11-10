//! `corgi --version`: the release this binary is, and what it was built from
//! (`build.rs`): the commit, whether the files that go into the binary
//! differed from it, and the checkout. `corgi status --json` carries the
//! same, with whether that checkout has moved on since, and the daemon keeps
//! its own in its lock, so the Herdr app's Projects list can say which Corgi
//! runs and whether it is the newest one merged.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::git::{git_current_branch, git_output};

pub const VERSION_USAGE: &str = "\
Usage: corgi --version [--json]

Prints the release and, for a build from a Git checkout, the commit it was
built from: corgi 0.2.3 (5b50895), with \", modified\" after the commit when
the files that go into the binary differed from that commit. A build without
Git (a source archive) prints the release alone.

With --json it prints {\"version\", \"commit\", \"dirty\", \"source\"}: the whole
commit, and the checkout it was built in; both null without Git.";

/// How many characters of a commit a person is shown.
const SHORT: usize = 7;

/// What a `corgi` binary was built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    /// The release: `version` in `Cargo.toml`.
    pub version: String,
    /// The commit that was checked out, in full; none for a build outside a
    /// Git checkout of Corgi.
    pub commit: Option<String>,
    /// Whether the files that go into the binary differed from `commit`.
    pub dirty: bool,
    /// The checkout it was built in.
    pub source: Option<PathBuf>,
}

/// The checkout at `source` ahead of the build made in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Newer {
    /// The branch checked out there; none when its `HEAD` is detached.
    pub branch: Option<String>,
    /// How many commits it has that the build lacks.
    pub commits: u64,
    /// The commit checked out there, in full.
    pub commit: String,
}

impl Build {
    /// This binary's.
    pub fn this() -> Self {
        let commit = env!("CORGI_BUILD_COMMIT");
        let source = env!("CORGI_BUILD_SOURCE");
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: (!commit.is_empty()).then(|| commit.to_string()),
            dirty: !env!("CORGI_BUILD_DIRTY").is_empty(),
            source: (!source.is_empty()).then(|| PathBuf::from(source)),
        }
    }

    /// The line `corgi --version` prints. `scripts/install.sh` and the
    /// release workflow check that it starts with `corgi <version>`.
    pub fn line(&self) -> String {
        let version = &self.version;
        match &self.commit {
            None => format!("corgi {version}"),
            Some(commit) => {
                let short: String = commit.chars().take(SHORT).collect();
                let modified = if self.dirty { ", modified" } else { "" };
                format!("corgi {version} ({short}{modified})")
            }
        }
    }

    /// What tells one build from another: the release, the commit, and
    /// whether it was modified. The daemon keeps this in its lock.
    pub fn identity(&self) -> Value {
        json!({"version": self.version, "commit": self.commit, "dirty": self.dirty})
    }

    /// What `corgi --version --json` prints.
    pub fn json(&self) -> Value {
        let mut value = self.identity();
        value["source"] = json!(self.source);
        value
    }

    /// The checkout this was built in, when it has commits the build lacks:
    /// a branch merged there since. None for a build whose checkout is not
    /// on this machine (a downloaded release), or is gone.
    pub fn newer(&self) -> Option<Newer> {
        newer_in(self.source.as_deref()?, self.commit.as_deref()?)
    }

    /// The `corgi` object of `corgi status --json`: [`Build::json`] and
    /// `newer`, `{branch, commits, commit}` or null.
    pub fn status_value(&self) -> Value {
        let mut value = self.json();
        value["newer"] = self.newer().map_or(Value::Null, |newer| {
            json!({"branch": newer.branch, "commits": newer.commits, "commit": newer.commit})
        });
        value
    }
}

/// What the checkout at `source` has checked out, when that holds commits a
/// build from `commit` lacks. A checkout behind the build, or one that does
/// not know `commit`, is not newer.
fn newer_in(source: &Path, commit: &str) -> Option<Newer> {
    let head = git_output(source, &["rev-parse", "HEAD"]).ok()?;
    let head = head.trim();
    if head == commit {
        return None;
    }
    let ahead = git_output(source, &["rev-list", "--count", &format!("{commit}..HEAD")]).ok()?;
    let commits: u64 = ahead.trim().parse().ok()?;
    (commits > 0).then(|| Newer {
        branch: git_current_branch(source).ok(),
        commits,
        commit: head.to_string(),
    })
}

/// `corgi --version`.
pub fn version_command(args: &[String]) -> Result<()> {
    let build = Build::this();
    match args {
        [] => println!("{}", build.line()),
        [json] if json == "--json" => println!("{}", build.json()),
        [help] if help == "--help" || help == "-h" => println!("{VERSION_USAGE}"),
        _ => bail!("unknown option {:?}\n\n{VERSION_USAGE}", args.join(" ")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_support::{ScratchDir, git, git_repo, git_worktree};

    fn build(commit: Option<&str>, dirty: bool) -> Build {
        Build {
            version: "0.2.3".into(),
            commit: commit.map(str::to_string),
            dirty,
            source: commit.map(|_| PathBuf::from("/repos/corgi")),
        }
    }

    const COMMIT: &str = "5b508951c0ffee5b508951c0ffee5b508951c0ff";

    #[test]
    fn the_version_line_names_the_commit_and_says_when_the_build_was_modified() {
        assert_eq!(build(Some(COMMIT), false).line(), "corgi 0.2.3 (5b50895)");
        assert_eq!(
            build(Some(COMMIT), true).line(),
            "corgi 0.2.3 (5b50895, modified)"
        );
        // A build without Git: the release alone, as before there was a commit.
        assert_eq!(build(None, false).line(), "corgi 0.2.3");
    }

    #[test]
    fn the_json_form_gives_the_whole_commit_and_the_checkout() {
        assert_eq!(
            build(Some(COMMIT), true).json(),
            json!({"version": "0.2.3", "commit": COMMIT, "dirty": true, "source": "/repos/corgi"})
        );
        assert_eq!(
            build(None, false).json(),
            json!({"version": "0.2.3", "commit": null, "dirty": false, "source": null})
        );
        assert_eq!(
            build(Some(COMMIT), false).identity(),
            json!({"version": "0.2.3", "commit": COMMIT, "dirty": false})
        );
    }

    #[test]
    fn this_binary_knows_the_commit_of_the_checkout_it_was_built_in() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let this = Build::this();
        assert_eq!(this.version, env!("CARGO_PKG_VERSION"));
        // Without Git, or built inside another repository, it knows none.
        let top = git_output(dir, &["rev-parse", "--show-toplevel"])
            .ok()
            .and_then(|top| fs::canonicalize(top.trim()).ok());
        if top != fs::canonicalize(dir).ok() {
            assert_eq!((this.commit, this.source), (None, None));
            return;
        }
        let head = git_output(dir, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(this.commit.as_deref(), Some(head.trim()));
        assert_eq!(this.source.as_deref(), Some(dir));
        assert_eq!(this.newer(), None, "it was built from what is checked out");
        assert!(
            this.line()
                .starts_with(&format!("corgi {} ({}", this.version, &head[..SHORT]))
        );
    }

    fn commit(repo: &Path, file: &str) -> String {
        fs::write(repo.join(file), file).unwrap();
        git(repo, &["add", file]);
        git(repo, &["commit", "-q", "-m", file]);
        git_output(repo, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string()
    }

    #[test]
    fn a_checkout_that_moved_on_since_the_build_is_newer() {
        let scratch = ScratchDir::new("version-newer");
        let repo = git_repo(&scratch.join("corgi"));
        let built = commit(&repo, "built.txt");
        assert_eq!(newer_in(&repo, &built), None, "built from its HEAD");

        // A worker's branch, still unmerged, is not the checkout's.
        let worktree = git_worktree(&repo, &scratch.join("worktree"), "worktree/fix");
        let fixed = commit(&worktree, "fix.txt");
        assert_eq!(newer_in(&repo, &built), None);

        git(
            &repo,
            &["merge", "-q", "--no-ff", "-m", "merge", "worktree/fix"],
        );
        let head = git_output(&repo, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(
            newer_in(&repo, &built),
            Some(Newer {
                branch: Some("main".into()),
                commits: 2,
                commit: head.trim().to_string(),
            })
        );
        let mut build = build(Some(&built), false);
        build.source = Some(repo.clone());
        assert_eq!(
            build.status_value()["newer"],
            json!({"branch": "main", "commits": 2, "commit": head.trim()})
        );

        // A build ahead of its checkout (the checkout was reset, or the
        // build came from the worker's branch) is not older than it.
        git(&repo, &["checkout", "-q", "--detach", &built]);
        assert_eq!(newer_in(&repo, &fixed), None);
        // Detached and ahead: newer, on no branch.
        git(&repo, &["checkout", "-q", "--detach", "main"]);
        assert_eq!(
            newer_in(&repo, &built).map(|newer| (newer.branch, newer.commits)),
            Some((None, 2))
        );
    }

    #[test]
    fn a_build_whose_checkout_is_not_here_or_does_not_know_it_is_never_older() {
        let scratch = ScratchDir::new("version-elsewhere");
        let repo = git_repo(&scratch.join("corgi"));
        commit(&repo, "one.txt");
        // A release built elsewhere: its commit is not in this repository.
        assert_eq!(newer_in(&repo, COMMIT), None);
        // Its checkout is gone, or was never on this machine.
        assert_eq!(newer_in(&scratch.join("gone"), COMMIT), None);
        // Not a repository at all.
        assert_eq!(newer_in(&scratch, COMMIT), None);
        for build in [build(None, false), build(Some(COMMIT), false)] {
            assert_eq!(build.newer(), None);
            assert_eq!(build.status_value()["newer"], Value::Null);
        }
    }
}
