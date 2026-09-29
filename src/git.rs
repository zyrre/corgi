//! The plain Git commands Corgi runs in a checkout: its branch, whether it is
//! clean, the commits a merge would bring, and the main checkout trust keys on.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

/// The commits on `source` that `target` does not have, newest first, each
/// as `<short hash> <subject>`. Only informs the confirmation dialog, so a
/// failure to list them shows nothing rather than blocking the merge.
pub(crate) fn worktree_commits(cwd: &Path, target: &str, source: &str) -> Vec<String> {
    let range = format!("{target}..{source}");
    git_output(cwd, &["log", "--format=%h %s", "--no-merges", &range])
        .map(|log| log.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

pub(crate) fn git_current_branch(cwd: &Path) -> Result<String> {
    let branch = git_output(cwd, &["branch", "--show-current"])?;
    let branch = branch.trim();
    if branch.is_empty() {
        bail!("{} is in detached HEAD state", cwd.display());
    }
    Ok(branch.to_string())
}

pub(crate) fn ensure_git_clean(cwd: &Path, label: &str) -> Result<()> {
    let changes = git_output(cwd, &["status", "--porcelain", "--untracked-files=normal"])?;
    if !changes.trim().is_empty() {
        bail!("{label} has uncommitted or untracked changes");
    }
    Ok(())
}

pub(crate) fn git_output(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .with_context(|| format!("run git {} in {}", args.join(" "), cwd.display()))?;
    if !output.status.success() {
        bail!("git {}: {}", args.join(" "), git_failure_detail(&output));
    }
    String::from_utf8(output.stdout).context("git returned non-UTF-8 output")
}

pub(crate) fn git_failure_detail(output: &std::process::Output) -> String {
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        format!("exited with {}", output.status)
    } else {
        detail.chars().take(240).collect()
    }
}

/// The directory whose trust Claude Code consults for `dir`: the main
/// checkout of the repository it belongs to, since a worktree's trust is its
/// main repository's, or else the directory itself.
pub(crate) fn trust_root(dir: &Path) -> PathBuf {
    git_output(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()
    .map(|common| PathBuf::from(common.trim()))
    .and_then(|common| common.parent().map(Path::to_path_buf))
    .unwrap_or_else(|| dir.to_path_buf())
}
