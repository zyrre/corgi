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
pub fn worktree_commits(cwd: &Path, target: &str, source: &str) -> Vec<String> {
    let range = format!("{target}..{source}");
    git_output(cwd, &["log", "--format=%h %s", "--no-merges", &range])
        .map(|log| log.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The upstream `branch` pulls from and pushes to, as `origin/main`, or
/// nothing when it has none.
pub fn git_upstream(cwd: &Path, branch: &str) -> Option<String> {
    let spec = format!("{branch}@{{upstream}}");
    git_output(cwd, &["rev-parse", "--abbrev-ref", &spec])
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

pub fn git_current_branch(cwd: &Path) -> Result<String> {
    let branch = git_output(cwd, &["branch", "--show-current"])?;
    let branch = branch.trim();
    if branch.is_empty() {
        bail!("{} is in detached HEAD state", cwd.display());
    }
    Ok(branch.to_string())
}

/// Refuses a checkout with uncommitted or untracked changes, naming the
/// first few as `git status --short` does, so the user and a supervisor
/// asked to fix it see which.
pub fn ensure_git_clean(cwd: &Path, label: &str) -> Result<()> {
    const NAMED: usize = 5;
    let changes = git_output(cwd, &["status", "--porcelain", "--untracked-files=normal"])?;
    let changes: Vec<&str> = changes
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if !changes.is_empty() {
        let mut named = changes[..changes.len().min(NAMED)].join(", ");
        if changes.len() > NAMED {
            named.push_str(&format!(" and {} more", changes.len() - NAMED));
        }
        bail!("{label} has uncommitted or untracked changes: {named}");
    }
    Ok(())
}

/// Whether a checkout has no uncommitted or untracked changes.
pub fn git_status_clean(cwd: &Path) -> Result<bool> {
    let changes = git_output(cwd, &["status", "--porcelain", "--untracked-files=normal"])?;
    Ok(changes.trim().is_empty())
}

pub fn git_output(cwd: &Path, args: &[&str]) -> Result<String> {
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

pub fn git_failure_detail(output: &std::process::Output) -> String {
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
pub fn trust_root(dir: &Path) -> PathBuf {
    git_output(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()
    .map(|common| PathBuf::from(common.trim()))
    .and_then(|common| common.parent().map(Path::to_path_buf))
    .unwrap_or_else(|| dir.to_path_buf())
}
