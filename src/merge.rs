//! Merging an agent's clean worktree branch into the branch checked out in
//! its project's primary checkout, then pushing it, once the user confirms
//! (`corgi merge`, which the Herdr app's Projects list runs). The base
//! branch's upstream is fetched first, so a merge does not end with a push
//! the remote rejects for commits pushed from elsewhere.
//!
//! This is the Git work and the lines for the supervisor's inbox.

use std::{
    fmt,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use crate::{
    git::{ensure_git_clean, git_current_branch, git_failure_detail, git_output, git_upstream},
    inbox::{Item, Kind},
    model::Agent,
    progress::Progress,
    supervisor::{self, MergeConflict, MergeFailed, MergeLanded},
    waker::Inboxes,
};

/// A reviewed agent branch to merge from its isolated worktree into the
/// branch currently checked out in the repository's primary checkout.
#[derive(Debug, Clone)]
pub struct MergeWorktreeForm {
    /// The agent's name, by which its supervisor knows it.
    pub agent: String,
    pub project_root: PathBuf,
    pub worktree_checkout: PathBuf,
    pub source_branch: String,
    pub target_branch: String,
    /// What the agent was asked to do, so a line for the supervisor names
    /// the work and not only the branch.
    pub task: String,
    /// What the merge does about the target branch's upstream first.
    pub upstream: Upstream,
}

/// What a merge does about the target branch's upstream before it merges.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Upstream {
    /// The target branch has none, so nothing is fetched.
    #[default]
    None,
    /// Fetch this upstream (`origin/main`), and stop when it has commits the
    /// target branch lacks.
    Check(String),
    /// Fetch it and bring its commits into the target branch first.
    Pull(String),
    /// Merge without it, as the user chose after its fetch failed.
    Skip(String),
}

impl Upstream {
    pub fn of(name: Option<String>) -> Self {
        name.map_or(Self::None, Self::Check)
    }

    /// The upstream's name, when the target branch has one.
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Check(name) | Self::Pull(name) | Self::Skip(name) => Some(name),
        }
    }

    /// Checked again, as a new attempt at the merge starts.
    pub fn checked(&self) -> Self {
        Self::of(self.name().map(str::to_string))
    }

    pub fn pulled(&self) -> Self {
        self.name()
            .map_or(Self::None, |name| Self::Pull(name.to_string()))
    }

    pub fn skipped(&self) -> Self {
        self.name()
            .map_or(Self::None, |name| Self::Skip(name.to_string()))
    }
}

/// How a merge ends when it does not succeed.
#[derive(Debug, Clone)]
pub enum MergeError {
    Failed(String),
    /// The merge stopped on conflicts in these files.
    Conflicts(Vec<String>),
    /// The upstream has these commits the target branch lacks.
    Behind(Vec<String>),
    /// Fetching the upstream failed.
    FetchFailed(String),
    /// The merge landed at `commit`, but the push failed.
    PushFailed {
        commit: String,
        error: String,
    },
}

impl fmt::Display for MergeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(error) => f.write_str(error),
            Self::Conflicts(files) => write!(f, "conflicts in {}", files.join(", ")),
            Self::Behind(commits) => write!(f, "the upstream has {} new commit(s)", commits.len()),
            Self::FetchFailed(error) => write!(f, "fetch failed: {error}"),
            Self::PushFailed { error, .. } => {
                write!(f, "merge completed, but git push failed: {error}")
            }
        }
    }
}

impl std::error::Error for MergeError {}

/// A merge that stopped on conflicts, carried out of
/// [`merge_worktree_branch`] as its error.
#[derive(Debug)]
struct Conflicts {
    project_root: PathBuf,
    files: Vec<String>,
}

impl fmt::Display for Conflicts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "conflicts in {}; resolve them there and run git merge --continue, or git merge --abort",
            self.project_root.display()
        )
    }
}

impl std::error::Error for Conflicts {}

/// The merge of `agent`'s worktree branch: its branch and the one checked
/// out in the project's primary checkout, and the upstream to check.
/// Refused for an agent that is not in a linked worktree, or whose
/// checkouts are not ready to merge.
pub fn merge_form(agent: &Agent) -> Result<MergeWorktreeForm> {
    let Some(worktree_checkout) = agent.worktree_checkout.as_deref() else {
        bail!("Merge is available only for an agent in a linked worktree");
    };
    let project_root = PathBuf::from(&agent.project_root);
    let worktree_checkout = PathBuf::from(worktree_checkout);
    let (source_branch, target_branch) = prepare_worktree_merge(&project_root, &worktree_checkout)?;
    let upstream = Upstream::of(git_upstream(&project_root, &target_branch));
    Ok(MergeWorktreeForm {
        agent: agent
            .info
            .name
            .clone()
            .unwrap_or_else(|| agent.info.address.clone()),
        project_root,
        worktree_checkout,
        source_branch,
        target_branch,
        task: agent.task.clone(),
        upstream,
    })
}

/// The merge of `agent`'s worktree branch that failed or was refused, as
/// far as Git can still tell its branches, for the line that hands it to
/// the supervisor. `None` for an agent not in a linked worktree, which has
/// nothing to merge and so nothing to fix.
pub fn failed_form(agent: &Agent) -> Option<MergeWorktreeForm> {
    let worktree_checkout = PathBuf::from(agent.worktree_checkout.as_deref()?);
    let project_root = PathBuf::from(&agent.project_root);
    let branch = |cwd: &Path| git_current_branch(cwd).unwrap_or_default();
    Some(MergeWorktreeForm {
        agent: agent
            .info
            .name
            .clone()
            .unwrap_or_else(|| agent.info.address.clone()),
        source_branch: branch(&worktree_checkout),
        target_branch: branch(&project_root),
        project_root,
        worktree_checkout,
        task: agent.task.clone(),
        upstream: Upstream::None,
    })
}

/// Adds a line to the inbox of the project's supervisor saying that `form`'s
/// branch was merged into its base at `commit`, and that the push failed
/// with `push_error` when it did, so the supervisor records it and goes on
/// with what waited on it. It is kept for the supervisor's next turn when
/// none runs now, and not written for a project that never had one.
pub fn tell_supervisor_about_merge(
    inboxes: &Inboxes,
    agents: &[Agent],
    form: &MergeWorktreeForm,
    commit: &str,
    push_error: Option<&str>,
) -> Result<()> {
    let project = form.project_root.to_string_lossy().into_owned();
    if project_supervisor(agents, &form.project_root).is_none() && !inboxes.has_supervisor(&project)
    {
        return Ok(());
    }
    let item = merge_item(
        form,
        Kind::Merge,
        merged_key(form),
        supervisor::merge_landed_message(&landed(form, commit, push_error)),
    );
    inboxes.notify(item)
}

/// The inbox line that hands `form`'s conflict in `files` to the project's
/// supervisor, to have the worker merge the base branch into its own branch
/// and resolve it there. The conflicted merge is aborted first
/// ([`abort_merge`]).
pub fn conflict_item(form: &MergeWorktreeForm, files: &[String]) -> Item {
    let message = supervisor::merge_conflict_message(&MergeConflict {
        worker: &form.agent,
        task: &form.task,
        branch: &form.source_branch,
        worktree: &form.worktree_checkout,
        base: &form.target_branch,
        files,
    });
    merge_item(
        form,
        Kind::Conflict,
        format!("{} merge conflict", form.agent),
        message,
    )
}

/// The inbox line that hands the push that failed with `error` after
/// `form`'s merge landed at `commit` to the project's supervisor, so it
/// tells the user what the error means and what to run. It shares the
/// merged line's key: it says the merge landed too, and stands in for that
/// line when it has not gone out yet.
pub fn push_failed_item(form: &MergeWorktreeForm, commit: &str, error: &str) -> Item {
    merge_item(
        form,
        Kind::Merge,
        merged_key(form),
        supervisor::push_failed_message(&landed(form, commit, Some(error))),
    )
}

/// The inbox line that asks the project's supervisor, at the user's
/// request, to fix `error`, which failed `form`'s merge or refused it, and
/// report back. A newer one for the same agent replaces it while it waits.
pub fn merge_failed_item(form: &MergeWorktreeForm, error: &str) -> Item {
    let message = supervisor::merge_failed_message(&MergeFailed {
        worker: &form.agent,
        task: &form.task,
        branch: &form.source_branch,
        worktree: &form.worktree_checkout,
        base: &form.target_branch,
        primary: &form.project_root,
        error,
    });
    merge_item(
        form,
        Kind::Conflict,
        format!("{} merge failed", form.agent),
        message,
    )
}

/// An inbox item about the merge in `form`, for its project's supervisor.
fn merge_item(form: &MergeWorktreeForm, kind: Kind, key: String, text: String) -> Item {
    Item {
        project: form.project_root.to_string_lossy().into_owned(),
        source: "merge".into(),
        kind,
        agent: Some(form.agent.clone()),
        key,
        text,
        ..Default::default()
    }
}

/// The key of the line saying `form`'s branch was merged, which a newer
/// one about the same merge replaces while it waits.
fn merged_key(form: &MergeWorktreeForm) -> String {
    format!("{} merged", form.agent)
}

/// `form`'s merge, landed at `commit`, for a supervisor's line.
fn landed<'a>(
    form: &'a MergeWorktreeForm,
    commit: &'a str,
    push_error: Option<&'a str>,
) -> MergeLanded<'a> {
    MergeLanded {
        worker: &form.agent,
        task: &form.task,
        branch: &form.source_branch,
        base: &form.target_branch,
        commit,
        push_error,
    }
}

/// The name of the running supervisor of the project at `project_root`.
pub fn project_supervisor(agents: &[Agent], project_root: &Path) -> Option<String> {
    agents
        .iter()
        .filter(|agent| agent.supervisor && Path::new(&agent.project_root) == project_root)
        .find_map(|agent| agent.info.name.clone())
}

/// Aborts the merge in progress in `project_root`, and checks that the
/// checkout is back to clean.
pub fn abort_merge(project_root: &Path) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["merge", "--abort"])
        .output()
        .with_context(|| format!("run git merge --abort in {}", project_root.display()))?;
    if !output.status.success() {
        bail!("git merge --abort: {}", git_failure_detail(&output));
    }
    ensure_git_clean(project_root, "primary checkout after git merge --abort")
}

/// Checks both checkouts before showing the confirmation dialog. The merge
/// target is deliberately the branch already checked out in the repository's
/// primary checkout; Corgi never switches branches on the user's behalf.
fn prepare_worktree_merge(
    project_root: &Path,
    worktree_checkout: &Path,
) -> Result<(String, String)> {
    let source_branch = git_current_branch(worktree_checkout)?;
    let target_branch = git_current_branch(project_root)?;
    if source_branch == target_branch {
        bail!("the agent and primary checkout are both on {source_branch}");
    }
    ensure_git_clean(worktree_checkout, "agent worktree")?;
    ensure_git_clean(project_root, "primary checkout")?;
    Ok((source_branch, target_branch))
}

/// Runs the merge job for `form`, telling conflicts, the upstream and the
/// push from other failures.
pub fn run_merge(
    form: &MergeWorktreeForm,
    progress: &mut dyn Progress,
) -> Result<String, MergeError> {
    merge_worktree_branch(form, progress).map_err(|error| {
        let error = match error.downcast::<Conflicts>() {
            Ok(conflicts) => return MergeError::Conflicts(conflicts.files),
            Err(error) => error,
        };
        match error.downcast::<MergeError>() {
            Ok(error) => error,
            Err(error) => MergeError::Failed(format!("{error:#}")),
        }
    })
}

/// Revalidates the reviewed merge immediately before changing Git state. This
/// catches an agent resuming work, a user switching branches, or local edits
/// made while the confirmation dialog was open. Then it fetches the target
/// branch's upstream, as [`MergeWorktreeForm::upstream`] says, and stops
/// when it has commits the target lacks, or pulls them in first. Returns the
/// short hash of the commit the target branch points at after the merge, or
/// nothing when Git could not tell it.
fn merge_worktree_branch(form: &MergeWorktreeForm, progress: &mut dyn Progress) -> Result<String> {
    progress.report("Checking both worktrees…".into());
    let (source_branch, target_branch) =
        prepare_worktree_merge(&form.project_root, &form.worktree_checkout)?;
    if source_branch != form.source_branch || target_branch != form.target_branch {
        bail!(
            "branches changed while waiting for confirmation (now {source_branch} → {target_branch})"
        );
    }

    if let Upstream::Check(upstream) | Upstream::Pull(upstream) = &form.upstream {
        progress.report(format!("Fetching {upstream}…"));
        if let Err(error) = fetch_upstream(&form.project_root, &form.target_branch) {
            return Err(MergeError::FetchFailed(format!("{error:#}")).into());
        }
        let behind = upstream_commits(&form.project_root, &form.target_branch)?;
        if !behind.is_empty() {
            if let Upstream::Check(_) = form.upstream {
                return Err(MergeError::Behind(behind).into());
            }
            progress.report(format!("Pulling {upstream} into {}…", form.target_branch));
            pull_upstream(&form.project_root, &form.target_branch, upstream)?;
        }
    }

    progress.report(format!(
        "Merging {} into {}…",
        form.source_branch, form.target_branch
    ));
    let output = Command::new("git")
        .arg("-C")
        .arg(&form.project_root)
        .args(["merge", "--no-ff", "--no-edit", &form.source_branch])
        .output()
        .with_context(|| format!("run git merge in {}", form.project_root.display()))?;
    if !output.status.success() {
        let files: Vec<String> = git_output(
            &form.project_root,
            &["diff", "--name-only", "--diff-filter=U"],
        )
        .map(|paths| paths.lines().map(str::to_string).collect())
        .unwrap_or_default();
        if !files.is_empty() {
            return Err(Conflicts {
                project_root: form.project_root.clone(),
                files,
            }
            .into());
        }
        bail!(
            "git merge {}: {}",
            form.source_branch,
            git_failure_detail(&output)
        );
    }

    let commit = git_output(&form.project_root, &["rev-parse", "--short", "HEAD"])
        .map(|hash| hash.trim().to_string())
        .unwrap_or_default();

    progress.report(format!("Pushing {}…", form.target_branch));
    let output = Command::new("git")
        .arg("-C")
        .arg(&form.project_root)
        .arg("push")
        .output()
        .with_context(|| format!("run git push in {}", form.project_root.display()))?;
    if !output.status.success() {
        return Err(MergeError::PushFailed {
            commit,
            error: push_failure_detail(&output),
        }
        .into());
    }
    Ok(commit)
}

/// Git's reasons for a failed push, without its hints (which a cut-off
/// detail would end mid-sentence in), one line of its output per line.
fn push_failure_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("hint:"))
        .collect::<Vec<_>>()
        .join("\n");
    if detail.is_empty() {
        git_failure_detail(output)
    } else {
        detail.chars().take(480).collect()
    }
}

/// Fetches `branch`'s upstream from its remote, without asking for
/// credentials on a terminal nobody watches. An upstream in the
/// same repository (remote `.`) has nothing to fetch.
fn fetch_upstream(project_root: &Path, branch: &str) -> Result<()> {
    let remote = git_output(
        project_root,
        &["config", &format!("branch.{branch}.remote")],
    )?;
    let merge = git_output(project_root, &["config", &format!("branch.{branch}.merge")])?;
    let (remote, merge) = (remote.trim(), merge.trim());
    if remote == "." {
        return Ok(());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["fetch", "--quiet", remote, merge])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .with_context(|| format!("run git fetch in {}", project_root.display()))?;
    if !output.status.success() {
        bail!("git fetch {remote}: {}", git_failure_detail(&output));
    }
    Ok(())
}

/// The commits `branch`'s upstream has that `branch` lacks, newest first,
/// each as `<short hash> <subject>`.
fn upstream_commits(project_root: &Path, branch: &str) -> Result<Vec<String>> {
    let range = format!("{branch}..{branch}@{{upstream}}");
    let log = git_output(project_root, &["log", "--format=%h %s", &range])?;
    Ok(log.lines().map(str::to_string).collect())
}

/// Brings `upstream`'s commits into `branch`, checked out in
/// `project_root`, as `git pull` would: a fast-forward when `branch` has
/// nothing of its own, else a merge. A merge that conflicts is aborted, so
/// the checkout is clean again, and the user is told to pull by hand.
fn pull_upstream(project_root: &Path, branch: &str, upstream: &str) -> Result<()> {
    let ahead = git_output(
        project_root,
        &[
            "rev-list",
            "--count",
            &format!("{branch}@{{upstream}}..{branch}"),
        ],
    )?;
    let how = if ahead.trim() == "0" {
        "--ff-only"
    } else {
        "--no-edit"
    };
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["merge", how, upstream])
        .output()
        .with_context(|| format!("run git merge in {}", project_root.display()))?;
    if output.status.success() {
        return Ok(());
    }
    let files = git_output(project_root, &["diff", "--name-only", "--diff-filter=U"])
        .map(|paths| paths.lines().map(str::to_string).collect::<Vec<_>>())
        .unwrap_or_default();
    if files.is_empty() {
        bail!(
            "pulling {upstream} into {branch}: {}",
            git_failure_detail(&output)
        );
    }
    abort_merge(project_root)?;
    bail!(
        "pulling {upstream} into {branch} conflicted in {}, so Corgi aborted it and the \
         primary checkout is clean. Pull by hand in {} with git pull, resolve the \
         conflicts, then merge again.",
        files.join(", "),
        project_root.display()
    )
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::progress::Silent;

    use super::*;

    #[test]
    fn reviewed_worktree_branch_merges_and_pushes_to_the_primary_checkout_branch() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        let repo = std::env::temp_dir().join(format!("corgi-merge-test-{nonce}"));
        let worktree = std::env::temp_dir().join(format!("corgi-merge-worktree-{nonce}"));
        let remote = std::env::temp_dir().join(format!("corgi-merge-remote-{nonce}"));
        fs::create_dir_all(&repo).expect("create test repository");
        fs::create_dir_all(&remote).expect("create test remote");
        git_test(&remote, &["init", "--bare", "-b", "main"]);
        git_test(&repo, &["init", "-b", "main"]);
        git_test(&repo, &["config", "user.name", "Corgi test"]);
        git_test(&repo, &["config", "user.email", "corgi@example.test"]);
        fs::write(repo.join("status.txt"), "base\n").expect("write initial file");
        git_test(&repo, &["add", "status.txt"]);
        git_test(&repo, &["commit", "-m", "initial"]);
        git_test(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git_test(&repo, &["push", "--set-upstream", "origin", "main"]);
        git_test(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "worktree/reviewed-agent",
                worktree.to_str().unwrap(),
            ],
        );
        fs::write(worktree.join("status.txt"), "reviewed\n").expect("write agent change");
        git_test(&worktree, &["add", "status.txt"]);
        git_test(&worktree, &["commit", "-m", "agent result"]);

        let (source_branch, target_branch) =
            prepare_worktree_merge(&repo, &worktree).expect("prepare clean merge");
        assert_eq!(source_branch, "worktree/reviewed-agent");
        assert_eq!(target_branch, "main");
        merge_worktree_branch(
            &MergeWorktreeForm {
                agent: "w-reviewed-agent".into(),
                project_root: repo.clone(),
                worktree_checkout: worktree.clone(),
                upstream: Upstream::of(git_upstream(&repo, &target_branch)),
                source_branch,
                target_branch,
                task: String::new(),
            },
            &mut Silent,
        )
        .expect("merge and push reviewed worktree");
        assert_eq!(
            fs::read_to_string(repo.join("status.txt")).unwrap(),
            "reviewed\n"
        );
        assert_eq!(
            git_test_stdout(&repo, &["rev-parse", "main"]),
            git_test_stdout(&remote, &["rev-parse", "refs/heads/main"])
        );

        git_test(
            &repo,
            &["worktree", "remove", "--force", worktree.to_str().unwrap()],
        );
        fs::remove_dir_all(repo).expect("remove test repository");
        fs::remove_dir_all(remote).expect("remove test remote");
    }

    /// A merge as the lines for the supervisor's inbox see it: nothing here
    /// touches Git.
    fn reviewed_merge() -> MergeWorktreeForm {
        MergeWorktreeForm {
            agent: "w-reviewed-agent".into(),
            project_root: "/repos/corgi".into(),
            worktree_checkout: "/worktrees/corgi/reviewed-agent".into(),
            source_branch: "worktree/reviewed-agent".into(),
            target_branch: "main".into(),
            task: "Make the status\nfile say agent".into(),
            upstream: Upstream::None,
        }
    }

    /// `item` as it is typed into the supervisor's box when it goes out
    /// alone.
    fn line(item: &Item) -> String {
        crate::inbox::prompt_text(&[item])
    }

    #[test]
    fn the_lines_that_hand_a_merge_to_the_supervisor_read_exactly() {
        let form = reviewed_merge();
        let asked_to_fix = "[Corgi] Merging w-reviewed-agent (task \"Make the status file say agent\"), \
             branch worktree/reviewed-agent in worktree /worktrees/corgi/reviewed-agent, into main \
             in /repos/corgi failed: ";

        // A merge that failed or was refused, with the error as the user saw it.
        let error = "agent worktree has uncommitted or untracked changes: ?? notes.txt";
        let failed = line(&merge_failed_item(&form, error));
        assert!(
            failed.starts_with(&format!(
                "{asked_to_fix}{error}. The user asks you to fix it and report back. Find the \
                 cause; have w-reviewed-agent fix what lies in its worktree or branch"
            )),
            "{failed}"
        );
        let fetch = line(&merge_failed_item(
            &form,
            "could not fetch origin/main: no such remote",
        ));
        assert!(
            fetch.starts_with(&format!(
                "{asked_to_fix}could not fetch origin/main: no such remote"
            )),
            "{fetch}"
        );

        // A conflict, which Corgi aborted first.
        let conflict = line(&conflict_item(&form, &["status.txt".to_string()]));
        assert!(
            conflict.starts_with(
                "[Corgi] The user's merge of w-reviewed-agent (task \"Make the status file say agent\"), \
                 branch worktree/reviewed-agent in worktree /worktrees/corgi/reviewed-agent, into main \
                 conflicted in: status.txt. Corgi aborted it, so the primary checkout is clean. Have \
                 w-reviewed-agent merge main into its own branch, resolve the conflicts there, rerun \
                 its checks and report; the user then merges again. The user asked for this, "
            ),
            "{conflict}"
        );

        // A push that failed after the merge landed, which the user hands over.
        let push = line(&push_failed_item(&form, "abc1234", "main is protected"));
        assert!(
            push.starts_with(
                "[Corgi] The user asks for help with a failed push. The user merged \
                 w-reviewed-agent (task \"Make the status file say agent\"), branch \
                 worktree/reviewed-agent, into main at abc1234, and git push failed: main is protected"
            ),
            "{push}"
        );
        assert!(
            push.contains("w-reviewed-agent is merged. If w-reviewed-agent is in your ledger"),
            "{push}"
        );
        assert!(
            push.ends_with("never pull, push or merge yourself."),
            "{push}"
        );
        // It says the merge landed too, so it shares that line's key.
        assert_eq!(
            push_failed_item(&form, "abc1234", "x").key,
            "w-reviewed-agent merged"
        );
    }

    #[test]
    fn a_merge_tells_the_projects_supervisor_and_no_one_in_a_project_that_never_had_one() {
        let scratch = crate::test_support::ScratchDir::new("merge-told");
        let inboxes = Inboxes::in_dir(&scratch);
        let form = reviewed_merge();
        let root = "/repos/corgi";
        let agent = |name: &str, supervisor| Agent {
            info: crate::model::AgentInfo {
                name: Some(name.into()),
                ..Default::default()
            },
            project_root: root.into(),
            supervisor,
            ..Agent::default()
        };
        let worker = agent("w-reviewed-agent", false);

        // No supervisor runs, and none ever did: nothing is written.
        tell_supervisor_about_merge(
            &inboxes,
            std::slice::from_ref(&worker),
            &form,
            "abc1234",
            None,
        )
        .unwrap();
        assert!(inboxes.pending_for(root).is_empty());

        let agents = [agent("supervisor-corgi", true), worker];
        tell_supervisor_about_merge(&inboxes, &agents, &form, "abc1234", None).unwrap();
        let pending = inboxes.pending_for(root);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert!(
            pending[0].starts_with(
                "[Corgi] The user merged w-reviewed-agent (task \"Make the status file say \
                 agent\"), branch worktree/reviewed-agent, into main at abc1234. If \
                 w-reviewed-agent is in your ledger"
            ),
            "{pending:?}"
        );

        // A push that failed is said on the same line, which replaces it.
        tell_supervisor_about_merge(
            &inboxes,
            &agents,
            &form,
            "abc1234",
            Some("main is protected"),
        )
        .unwrap();
        let pending = inboxes.pending_for(root);
        assert!(
            pending.last().unwrap().starts_with(
                "[Corgi] The user merged w-reviewed-agent (task \"Make the status file say \
                 agent\"), branch worktree/reviewed-agent, into main at abc1234, but git push \
                 failed: main is protected"
            ),
            "{pending:?}"
        );
    }

    fn git_test(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .expect("run git for test");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_test_stdout(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .expect("run git for test");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("Git test output is UTF-8")
            .trim()
            .to_string()
    }
}
