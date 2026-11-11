//! Tells the `corgi` binary what it was built from, for `corgi --version`
//! (`src/version.rs`): the commit, whether the files that go into it differed
//! from that commit, and the checkout. A build outside a Git checkout of
//! Corgi (a source archive, a container without git) gets none of the three
//! and still builds.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

/// What of the checkout goes into the binary. A change to one of them runs
/// this script again, and only changes to them make a build "modified": the
/// docs, the scripts and the Herdr app's mods are not in the binary.
const INPUTS: [&str; 5] = ["build.rs", "Cargo.toml", "Cargo.lock", "src", "supervisor"];

/// What `git ARGS` prints in `dir`, when it succeeds.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The commit `dir` has checked out and whether [`INPUTS`] differ from it,
/// when `dir` is the top of a Git checkout: inside another repository (a
/// vendored copy) the commit would be that repository's.
fn checkout(dir: &Path) -> Option<(String, bool)> {
    let top = git(dir, &["rev-parse", "--show-toplevel"])?;
    if fs::canonicalize(&top).ok()? != fs::canonicalize(dir).ok()? {
        return None;
    }
    let commit = git(dir, &["rev-parse", "HEAD"])?;
    // No optional locks: a status that refreshed the index would take its
    // lock from a Git command an agent runs in the checkout meanwhile.
    let mut status = vec![
        "--no-optional-locks",
        "status",
        "--porcelain",
        "--untracked-files=normal",
        "--",
    ];
    status.extend(INPUTS);
    let dirty = git(dir, &status).is_none_or(|changes| !changes.is_empty());
    Some((commit, dirty))
}

/// The files of Git's that change when the checkout moves to another
/// commit: `HEAD` (a checkout), its log (a commit, a merge, a reset, which
/// leave `HEAD` naming the same branch) and the branch's own file, for a
/// repository that keeps no logs.
fn head_files(dir: &Path) -> Vec<PathBuf> {
    let mut names = vec!["HEAD".to_string(), "logs/HEAD".to_string()];
    if let Some(branch) = git(dir, &["symbolic-ref", "-q", "HEAD"]) {
        names.push(branch);
    }
    names
        .iter()
        .filter_map(|name| git(dir, &["rev-parse", "--git-path", name]))
        .map(|path| dir.join(path))
        // A path that does not exist would run the script on every build.
        .filter(|path| path.exists())
        .collect()
}

fn main() {
    let dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    for input in INPUTS {
        println!("cargo:rerun-if-changed={input}");
    }
    let built = checkout(&dir);
    if built.is_some() {
        for file in head_files(&dir) {
            println!("cargo:rerun-if-changed={}", file.display());
        }
    }
    let (commit, dirty) = built.unwrap_or_default();
    println!("cargo:rustc-env=CORGI_BUILD_COMMIT={commit}");
    println!(
        "cargo:rustc-env=CORGI_BUILD_DIRTY={}",
        if dirty { "1" } else { "" }
    );
    println!(
        "cargo:rustc-env=CORGI_BUILD_SOURCE={}",
        if commit.is_empty() {
            String::new()
        } else {
            dir.display().to_string()
        }
    );
}
