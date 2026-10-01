//! Merging an agent's clean worktree branch into the branch checked out in
//! its project's primary checkout, then pushing it, once the user confirms.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use crossterm::event::{KeyCode, KeyEvent};

use crate::{
    git::{ensure_git_clean, git_current_branch, git_failure_detail, git_output, worktree_commits},
    job::{Job, Update},
};

use super::{
    App,
    close::{CloseTarget, CloseWorkspaceForm},
    overlay::Overlay,
    progress::{JobReport, Progress},
    project_main::project_main_workspace,
};

/// A reviewed agent branch ready to be merged from its isolated worktree into
/// the branch currently checked out in the repository's primary checkout.
#[derive(Debug, Clone)]
pub(crate) struct MergeWorktreeForm {
    pub(crate) label: String,
    /// Workspace behind the agent, so a finished merge can offer to close the
    /// worktree without going back through the dashboard selection.
    pub(crate) workspace_id: String,
    pub(crate) project_root: PathBuf,
    pub(crate) worktree_checkout: PathBuf,
    pub(crate) source_branch: String,
    pub(crate) target_branch: String,
    /// What the agent was asked to do, as its session line says, so the
    /// dialog names the work and not only the branch.
    pub(crate) task: String,
    /// The commits the merge brings in, newest first, each as its short hash
    /// and subject. Empty when Git could not list them.
    pub(crate) commits: Vec<String>,
    pub(crate) phase: MergePhase,
}

#[derive(Debug, Clone)]
pub(crate) enum MergePhase {
    Confirm,
    /// Running the step of [`MergeWorktreeForm::steps`] at this index.
    Running(usize),
    Succeeded,
    Failed(String),
}

impl MergeWorktreeForm {
    /// What a merge does, in order, as its dialog lists them while it runs.
    pub(crate) fn steps(&self) -> [String; 3] {
        [
            "Check both worktrees".into(),
            format!("Merge {} into {}", self.source_branch, self.target_branch),
            format!("Push {}", self.target_branch),
        ]
    }
}

impl App {
    /// Starts an explicitly confirmed merge from an agent's isolated checkout.
    /// Corgi performs the merge in the primary checkout so agents never need
    /// to touch another branch or worktree.
    pub(super) fn begin_merge_worktree(&mut self) {
        let Some(agent) = self.selected_agent() else {
            self.status = "No agent selected".into();
            return;
        };
        let Some(worktree_checkout) = agent.worktree_checkout.as_deref() else {
            self.status = "Merge is available only for an agent in a linked worktree".into();
            return;
        };
        let project_root = PathBuf::from(&agent.project_root);
        let worktree_checkout = PathBuf::from(worktree_checkout);
        match prepare_worktree_merge(&project_root, &worktree_checkout) {
            Ok((source_branch, target_branch)) => {
                let commits = worktree_commits(&worktree_checkout, &target_branch, &source_branch);
                self.overlay = Overlay::merge(MergeWorktreeForm {
                    label: agent.project.clone(),
                    workspace_id: agent.info.workspace_id.clone(),
                    project_root,
                    worktree_checkout,
                    source_branch,
                    target_branch,
                    task: agent.task.clone(),
                    commits,
                    phase: MergePhase::Confirm,
                });
            }
            Err(error) => self.status = format!("Merge unavailable: {error:#}"),
        }
    }

    fn start_merge_worktree(&mut self) {
        if self.merge_job.is_running() {
            return;
        }
        let Some(form) = self.overlay.merge_worktree_form_mut() else {
            return;
        };
        form.phase = MergePhase::Running(0);
        let form = form.clone();
        self.status = format!("Merging {}…", form.source_branch);
        self.merge_job = Job::spawn(move |mut progress| {
            merge_worktree_branch(&form, &mut progress).map_err(|error| format!("{error:#}"))
        });
    }

    pub(super) fn poll_merge(&mut self) {
        for update in self.merge_job.poll() {
            let result = match update {
                Update::Progress(JobReport::Step(step)) => {
                    if let Some(form) = self.overlay.merge_worktree_form_mut() {
                        form.phase = MergePhase::Running(step);
                    }
                    continue;
                }
                // The step list says what runs; the steps' own lines say
                // the same in words.
                Update::Progress(JobReport::Status(_) | JobReport::Project(_)) => continue,
                Update::Finished(result) => result,
                // Without an outcome the dialog would stay running and never
                // accept a key.
                Update::Panicked => Err("the merge stopped without reporting its outcome".into()),
            };
            match result {
                Ok(()) => {
                    if let Some(form) = self.overlay.merge_worktree_form_mut() {
                        form.phase = MergePhase::Succeeded;
                        self.status = format!(
                            "Merged and pushed {} into {}",
                            form.source_branch, form.target_branch
                        );
                    }
                    self.request_refresh();
                }
                Err(error) => {
                    if let Some(form) = self.overlay.merge_worktree_form_mut() {
                        form.phase = MergePhase::Failed(error.clone());
                    }
                    self.status = format!("Merge/push failed: {error}");
                    self.request_refresh();
                }
            }
        }
    }

    fn dismiss_merge_worktree(&mut self) {
        self.overlay = Overlay::None;
    }

    /// Answers the "close the worktree too?" question on a finished merge by
    /// running the same removal as `x` from the dashboard. A dirty checkout
    /// still falls back to the force confirmation.
    fn close_worktree_after_merge(&mut self) {
        let Some(form) = self.overlay.take_merge_worktree_form() else {
            return;
        };
        let project_root = form.project_root.to_string_lossy().into_owned();
        self.overlay = Overlay::Close(CloseWorkspaceForm {
            workspace_id: form.workspace_id,
            tab_id: String::new(),
            label: form.label,
            project_root: project_main_workspace(
                &self.workspaces,
                &self.handler_workspaces(),
                &project_root,
            )
            .map(|main| main.project_root),
            target: CloseTarget::Worktree {
                checkout: form.worktree_checkout.to_string_lossy().into_owned(),
                force: false,
            },
        });
        self.close_workspace();
    }

    pub(super) fn handle_merge_worktree_key(&mut self, key: KeyEvent) -> bool {
        let phase = self.overlay.merge_worktree_form().map(|form| &form.phase);
        match (phase, key.code) {
            (Some(MergePhase::Confirm), KeyCode::Esc) => self.cancel_editor(),
            (Some(MergePhase::Confirm), KeyCode::Enter) => self.start_merge_worktree(),
            (Some(MergePhase::Running(_)), KeyCode::Esc) => {
                self.status = "Merge and push are still running".into()
            }
            (Some(MergePhase::Succeeded), KeyCode::Char('x')) => self.close_worktree_after_merge(),
            (Some(MergePhase::Succeeded), KeyCode::Enter | KeyCode::Esc) => {
                self.dismiss_merge_worktree()
            }
            (Some(MergePhase::Failed(_)), KeyCode::Enter) => self.start_merge_worktree(),
            (Some(MergePhase::Failed(_)), KeyCode::Esc) => self.dismiss_merge_worktree(),
            _ => {}
        }
        false
    }
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

/// Revalidates the reviewed merge immediately before changing Git state. This
/// catches an agent resuming work, a user switching branches, or local edits
/// made while the confirmation dialog was open.
fn merge_worktree_branch(form: &MergeWorktreeForm, progress: &mut dyn Progress) -> Result<()> {
    progress.step(0);
    progress.report("Checking both worktrees…".into());
    let (source_branch, target_branch) =
        prepare_worktree_merge(&form.project_root, &form.worktree_checkout)?;
    if source_branch != form.source_branch || target_branch != form.target_branch {
        bail!(
            "branches changed while waiting for confirmation (now {source_branch} → {target_branch})"
        );
    }

    progress.step(1);
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
        let conflicts = git_output(
            &form.project_root,
            &["diff", "--name-only", "--diff-filter=U"],
        )
        .map(|paths| !paths.trim().is_empty())
        .unwrap_or(false);
        if conflicts {
            bail!(
                "conflicts in {}; resolve them there and run git merge --continue, or git merge --abort",
                form.project_root.display()
            );
        }
        bail!(
            "git merge {}: {}",
            form.source_branch,
            git_failure_detail(&output)
        );
    }

    progress.step(2);
    progress.report(format!("Pushing {}…", form.target_branch));
    let output = Command::new("git")
        .arg("-C")
        .arg(&form.project_root)
        .arg("push")
        .output()
        .with_context(|| format!("run git push in {}", form.project_root.display()))?;
    if !output.status.success() {
        bail!(
            "merge completed, but git push failed: {}",
            git_failure_detail(&output)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crossterm::event::{KeyCode, KeyEvent};
    use serde_json::json;

    use crate::{
        app::progress::Silent,
        herdr::HerdrClient,
        test_support::{answer, fake_herdr, test_app},
    };

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
        git_test(&remote, &["init", "--bare"]);
        git_test(&repo, &["init"]);
        git_test(&repo, &["config", "user.name", "Corgi test"]);
        git_test(&repo, &["config", "user.email", "corgi@example.test"]);
        fs::write(repo.join("status.txt"), "base\n").expect("write initial file");
        git_test(&repo, &["add", "status.txt"]);
        git_test(&repo, &["commit", "-m", "initial"]);
        git_test(&repo, &["branch", "-M", "main"]);
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
        let commits = worktree_commits(&worktree, &target_branch, &source_branch);
        assert_eq!(commits.len(), 1, "{commits:?}");
        assert!(commits[0].ends_with(" agent result"), "{commits:?}");
        assert!(
            worktree_commits(&worktree, "main", "no-such-branch").is_empty(),
            "an unlistable range shows nothing rather than failing"
        );
        merge_worktree_branch(
            &MergeWorktreeForm {
                label: "reviewed-agent".into(),
                workspace_id: "w1".into(),
                project_root: repo.clone(),
                worktree_checkout: worktree.clone(),
                source_branch,
                target_branch,
                task: String::new(),
                commits: Vec::new(),
                phase: MergePhase::Confirm,
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

    #[test]
    fn finished_merge_offers_to_close_the_worktree_like_the_dashboard_key() {
        let (socket_path, server) = fake_herdr("merge-close", move |listener| {
            answer(&listener, |request| {
                assert_eq!(request["method"], "worktree.remove");
                assert_eq!(request["params"]["workspace_id"], "w7");
                assert_eq!(request["params"]["force"], false);
                json!({
                    "result": {
                        "type": "worktree_removed",
                        "workspace_id": "w7",
                        "path": "/tmp/corgi-worktrees/corgi/worktree-reviewed-agent",
                        "branch": "worktree/reviewed-agent",
                        "forced": false
                    }
                })
            });
        });

        let mut app = test_app();
        app.client = HerdrClient::from_socket_path(&socket_path);
        app.overlay = Overlay::merge(MergeWorktreeForm {
            label: "corgi/reviewed-agent".into(),
            workspace_id: "w7".into(),
            project_root: PathBuf::from("/repos/corgi"),
            worktree_checkout: PathBuf::from("/tmp/corgi-worktrees/corgi/worktree-reviewed-agent"),
            source_branch: "worktree/reviewed-agent".into(),
            target_branch: "main".into(),
            task: String::new(),
            commits: Vec::new(),
            phase: MergePhase::Succeeded,
        });

        assert!(!app.handle_key(KeyEvent::from(KeyCode::Char('x'))));
        assert!(app.overlay.merge_worktree_form().is_none());
        assert!(app.overlay.close_workspace_form().is_none());
        assert!(matches!(app.overlay, Overlay::None));
        assert!(
            app.status.contains("Removed worktree"),
            "unexpected status: {}",
            app.status
        );

        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");
    }

    #[test]
    fn finished_merge_keeps_the_worktree_when_the_question_is_declined() {
        let mut app = test_app();
        app.overlay = Overlay::merge(MergeWorktreeForm {
            label: "corgi/reviewed-agent".into(),
            workspace_id: "w7".into(),
            project_root: PathBuf::from("/repos/corgi"),
            worktree_checkout: PathBuf::from("/tmp/corgi-worktrees/corgi/worktree-reviewed-agent"),
            source_branch: "worktree/reviewed-agent".into(),
            target_branch: "main".into(),
            task: String::new(),
            commits: Vec::new(),
            phase: MergePhase::Succeeded,
        });

        assert!(!app.handle_key(KeyEvent::from(KeyCode::Esc)));
        assert!(app.overlay.merge_worktree_form().is_none());
        assert!(app.overlay.close_workspace_form().is_none());
        assert!(matches!(app.overlay, Overlay::None));
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
