//! Merging an agent's clean worktree branch into the branch checked out in
//! its project's primary checkout, then pushing it, once the user confirms.

use std::{
    fmt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use crossterm::event::{KeyCode, KeyEvent};

use crate::{
    corgi::{self, MergeConflict},
    git::{ensure_git_clean, git_current_branch, git_failure_detail, git_output, worktree_commits},
    job::{Job, Update},
    model::DashboardAgent,
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
    /// The agent's name, by which its corgi knows it.
    pub(crate) agent: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MergePhase {
    Confirm,
    /// Running the step of [`MergeWorktreeForm::steps`] at this index.
    Running(usize),
    Succeeded,
    Failed(String),
    /// `git merge` stopped on conflicts in these files, and the primary
    /// checkout is mid-merge.
    Conflicted {
        files: Vec<String>,
        help: ConflictHelp,
    },
}

/// Whether a conflicted merge can be handed to the project's corgi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConflictHelp {
    /// The project's corgi, by name, which `c` asks.
    Corgi(String),
    /// The project has no running corgi.
    NoCorgi,
    /// This dashboard has no inbox to write to (it is the Omarchy popup),
    /// so it does not hand conflicts to a corgi.
    Unavailable,
}

/// How a merge job ends when it does not succeed.
#[derive(Debug, Clone)]
pub(crate) enum MergeError {
    Failed(String),
    /// The merge stopped on conflicts in these files.
    Conflicts(Vec<String>),
}

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
                    agent: agent
                        .info
                        .name
                        .clone()
                        .unwrap_or_else(|| agent.info.pane_id.clone()),
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
        self.merge_job = Job::spawn(move |mut progress| run_merge(&form, &mut progress));
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
                Update::Panicked => Err(MergeError::Failed(
                    "the merge stopped without reporting its outcome".into(),
                )),
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
                Err(MergeError::Failed(error)) => {
                    if let Some(form) = self.overlay.merge_worktree_form_mut() {
                        form.phase = MergePhase::Failed(error.clone());
                    }
                    self.status = format!("Merge/push failed: {error}");
                    self.request_refresh();
                }
                Err(MergeError::Conflicts(files)) => {
                    let help = self.conflict_help();
                    if let Some(form) = self.overlay.merge_worktree_form_mut() {
                        self.status = format!(
                            "Merge of {} stopped on conflicts in {} file(s)",
                            form.source_branch,
                            files.len()
                        );
                        form.phase = MergePhase::Conflicted { files, help };
                    }
                    self.request_refresh();
                }
            }
        }
    }

    /// Who can take over the open merge dialog's conflict: its project's
    /// corgi, when this dashboard can write to its inbox, as every
    /// interactive one can, whichever of them delivers.
    fn conflict_help(&self) -> ConflictHelp {
        let Some(form) = self.overlay.merge_worktree_form() else {
            return ConflictHelp::Unavailable;
        };
        match project_corgi(&self.agents, &form.project_root) {
            // The inbox keeps the message for whichever dashboard delivers.
            _ if self.corgi_waker.is_none() => ConflictHelp::Unavailable,
            Some(corgi) => ConflictHelp::Corgi(corgi),
            None => ConflictHelp::NoCorgi,
        }
    }

    /// Aborts the conflicted merge Corgi started in the primary checkout,
    /// and adds a line to the inbox of the project's corgi to have the worker
    /// merge the base branch into its own branch and resolve it there. The
    /// worktree stays open; the user merges again once the worker reports.
    fn ask_corgi_about_conflict(&mut self) {
        let Some(form) = self.overlay.merge_worktree_form() else {
            return;
        };
        let MergePhase::Conflicted { files, .. } = &form.phase else {
            return;
        };
        let Some(corgi) = project_corgi(&self.agents, &form.project_root) else {
            self.status = "The corgi is no longer running".into();
            if let Some(form) = self.overlay.merge_worktree_form_mut()
                && let MergePhase::Conflicted { help, .. } = &mut form.phase
            {
                *help = ConflictHelp::NoCorgi;
            }
            return;
        };
        if let Err(error) = abort_merge(&form.project_root) {
            let error = format!("{error:#}");
            self.status = format!("Merge abort failed: {error}");
            if let Some(form) = self.overlay.merge_worktree_form_mut() {
                form.phase = MergePhase::Failed(error);
            }
            return;
        }
        let message = corgi::merge_conflict_message(&MergeConflict {
            worker: &form.agent,
            task: &form.task,
            branch: &form.source_branch,
            worktree: &form.worktree_checkout,
            base: &form.target_branch,
            files,
        });
        let item = crate::inbox::Item {
            project: form.project_root.to_string_lossy().into_owned(),
            source: "merge".into(),
            kind: crate::inbox::Kind::Conflict,
            agent: Some(form.agent.clone()),
            key: format!("{} merge conflict", form.agent),
            text: message,
            ..Default::default()
        };
        let mut status = format!(
            "Merge aborted; asked {corgi} to have {} merge {} into its branch",
            form.agent, form.target_branch
        );
        if let Some(waker) = self.corgi_waker.as_mut()
            && let Err(error) = waker.notify(item)
        {
            status = format!("Merge aborted, but {corgi} could not be told: {error:#}");
        }
        self.overlay = Overlay::None;
        self.motion.succeeded();
        self.set_status(status, Some(Duration::from_secs(15)));
        self.request_refresh();
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
                &self.corgi_workspaces(),
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
            (Some(MergePhase::Failed(_) | MergePhase::Conflicted { .. }), KeyCode::Enter) => {
                self.start_merge_worktree()
            }
            (Some(MergePhase::Failed(_) | MergePhase::Conflicted { .. }), KeyCode::Esc) => {
                self.dismiss_merge_worktree()
            }
            (
                Some(MergePhase::Conflicted {
                    help: ConflictHelp::Corgi(_),
                    ..
                }),
                KeyCode::Char('c'),
            ) => self.ask_corgi_about_conflict(),
            _ => {}
        }
        false
    }
}

/// The name of the running corgi of the project at `project_root`.
fn project_corgi(agents: &[DashboardAgent], project_root: &Path) -> Option<String> {
    agents
        .iter()
        .filter(|agent| agent.corgi && Path::new(&agent.project_root) == project_root)
        .find_map(|agent| agent.info.name.clone())
}

/// Aborts the merge in progress in `project_root`, and checks that the
/// checkout is back to clean.
fn abort_merge(project_root: &Path) -> Result<()> {
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

/// Runs the merge job for `form`, telling conflicts from other failures.
fn run_merge(form: &MergeWorktreeForm, progress: &mut dyn Progress) -> Result<(), MergeError> {
    merge_worktree_branch(form, progress).map_err(|error| match error.downcast::<Conflicts>() {
        Ok(conflicts) => MergeError::Conflicts(conflicts.files),
        Err(error) => MergeError::Failed(format!("{error:#}")),
    })
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
        app::{progress::Silent, test_helpers::poll_until, waker::CorgiWaker},
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
                agent: "w-reviewed-agent".into(),
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
            agent: "w-reviewed-agent".into(),
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
            agent: "w-reviewed-agent".into(),
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

    /// A repository whose `main` and an agent worktree's branch both
    /// changed `status.txt`, so merging the branch conflicts. Returns the
    /// repository, the worktree, and the form for that merge.
    fn conflicting_merge(label: &str) -> (PathBuf, PathBuf, MergeWorktreeForm) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        let repo = std::env::temp_dir().join(format!("corgi-{label}-{nonce}"));
        let worktree = std::env::temp_dir().join(format!("corgi-{label}-worktree-{nonce}"));
        fs::create_dir_all(&repo).expect("create test repository");
        git_test(&repo, &["init"]);
        git_test(&repo, &["config", "user.name", "Corgi test"]);
        git_test(&repo, &["config", "user.email", "corgi@example.test"]);
        fs::write(repo.join("status.txt"), "base\n").expect("write initial file");
        git_test(&repo, &["add", "status.txt"]);
        git_test(&repo, &["commit", "-m", "initial"]);
        git_test(&repo, &["branch", "-M", "main"]);
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
        fs::write(worktree.join("status.txt"), "agent\n").expect("write agent change");
        git_test(&worktree, &["commit", "-am", "agent result"]);
        fs::write(repo.join("status.txt"), "main\n").expect("write main change");
        git_test(&repo, &["commit", "-am", "main moved on"]);
        let form = MergeWorktreeForm {
            label: "corgi/reviewed-agent".into(),
            workspace_id: "w7".into(),
            agent: "w-reviewed-agent".into(),
            project_root: repo.clone(),
            worktree_checkout: worktree.clone(),
            source_branch: "worktree/reviewed-agent".into(),
            target_branch: "main".into(),
            task: "Make the status\nfile say agent".into(),
            commits: Vec::new(),
            phase: MergePhase::Confirm,
        };
        (repo, worktree, form)
    }

    fn remove_conflicting_merge(repo: PathBuf, worktree: PathBuf) {
        git_test(
            &repo,
            &["worktree", "remove", "--force", worktree.to_str().unwrap()],
        );
        fs::remove_dir_all(repo).expect("remove test repository");
    }

    fn merge_in_progress(repo: &Path) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "-q", "--verify", "MERGE_HEAD"])
            .output()
            .expect("run git rev-parse")
            .status
            .success()
    }

    fn project_agent(name: &str, root: &Path, corgi: bool) -> DashboardAgent {
        DashboardAgent {
            info: crate::model::AgentInfo {
                pane_id: format!("{name}:p1"),
                name: Some(name.into()),
                ..Default::default()
            },
            project_root: root.to_string_lossy().into_owned(),
            corgi,
            ..DashboardAgent::default()
        }
    }

    /// The dashboard that wakes corgis, with the conflicted merge of
    /// `form` reported by its merge job, and `agents` in view.
    fn conflicted_dashboard(form: &MergeWorktreeForm, agents: Vec<DashboardAgent>) -> App {
        let mut app = test_app();
        let mut waker = CorgiWaker::in_dir(&form.project_root.join(".git").join("corgi-state"));
        assert!(waker.lead(&form.project_root.join(".git").join("corgi-wake.lock")));
        app.corgi_waker = Some(waker);
        app.agents = agents;
        let mut running = form.clone();
        running.phase = MergePhase::Running(1);
        app.overlay = Overlay::merge(running);
        let form = form.clone();
        app.merge_job = Job::spawn(move |_| run_merge(&form, &mut Silent));
        poll_until(&mut app, App::poll_merge, |app| !app.merge_job.is_running());
        app
    }

    #[test]
    fn a_conflicted_merge_offers_to_hand_it_to_the_project_corgi() {
        let (repo, worktree, form) = conflicting_merge("merge-conflict-corgi");
        let root = repo.to_string_lossy().into_owned();
        let mut app = conflicted_dashboard(
            &form,
            vec![
                project_agent("corgi-corgi", &repo, true),
                project_agent("w-reviewed-agent", &repo, false),
            ],
        );
        assert_eq!(
            app.overlay.merge_worktree_form().map(|form| &form.phase),
            Some(&MergePhase::Conflicted {
                files: vec!["status.txt".into()],
                help: ConflictHelp::Corgi("corgi-corgi".into()),
            })
        );
        assert!(merge_in_progress(&repo));

        assert!(!app.handle_key(KeyEvent::from(KeyCode::Char('c'))));
        assert!(matches!(app.overlay, Overlay::None), "{:?}", app.overlay);
        assert!(!merge_in_progress(&repo));
        ensure_git_clean(&repo, "primary checkout").expect("abort leaves the checkout clean");
        assert!(worktree.exists(), "the worktree stays open");
        assert!(app.status.contains("asked corgi-corgi"), "{}", app.status);
        let pending = app.corgi_waker.as_ref().unwrap().pending_for(&root);
        assert_eq!(
            pending,
            [format!(
                "[corgi] The user's merge of w-reviewed-agent (task \"Make the status file say agent\"), \
                 branch worktree/reviewed-agent in worktree {}, into main conflicted in: status.txt. \
                 Corgi aborted it, so the primary checkout is clean. Have w-reviewed-agent merge main \
                 into its own branch, resolve the conflicts there, rerun its checks and report; the \
                 user then merges again with m.",
                worktree.display()
            )]
        );

        remove_conflicting_merge(repo, worktree);
    }

    #[test]
    fn a_conflicted_merge_without_a_corgi_offers_no_handoff() {
        let (repo, worktree, form) = conflicting_merge("merge-conflict-alone");
        let root = repo.to_string_lossy().into_owned();
        let mut app =
            conflicted_dashboard(&form, vec![project_agent("w-reviewed-agent", &repo, false)]);
        assert!(matches!(
            app.overlay.merge_worktree_form().map(|form| &form.phase),
            Some(MergePhase::Conflicted {
                help: ConflictHelp::NoCorgi,
                ..
            })
        ));
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Char('c'))));
        assert!(app.overlay.merge_worktree_form().is_some());
        assert!(merge_in_progress(&repo), "c does nothing without a corgi");
        assert!(
            app.corgi_waker
                .as_ref()
                .unwrap()
                .pending_for(&root)
                .is_empty()
        );

        remove_conflicting_merge(repo, worktree);
    }

    #[test]
    fn esc_on_a_conflicted_merge_leaves_it_to_resolve_by_hand() {
        let (repo, worktree, form) = conflicting_merge("merge-conflict-esc");
        let root = repo.to_string_lossy().into_owned();
        let mut app = conflicted_dashboard(&form, vec![project_agent("corgi-corgi", &repo, true)]);
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Esc)));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(
            merge_in_progress(&repo),
            "the primary checkout stays mid-merge"
        );
        assert!(
            app.corgi_waker
                .as_ref()
                .unwrap()
                .pending_for(&root)
                .is_empty()
        );

        remove_conflicting_merge(repo, worktree);
    }

    #[test]
    fn a_failed_abort_is_reported_in_the_popup_and_asks_no_one() {
        let (repo, worktree, form) = conflicting_merge("merge-conflict-abort");
        let root = repo.to_string_lossy().into_owned();
        let mut app = conflicted_dashboard(&form, vec![project_agent("corgi-corgi", &repo, true)]);
        // The user finished the merge by hand behind the popup's back.
        git_test(&repo, &["checkout", "--theirs", "status.txt"]);
        git_test(&repo, &["commit", "-am", "resolved by hand"]);
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Char('c'))));
        assert!(
            matches!(
                app.overlay.merge_worktree_form().map(|form| &form.phase),
                Some(MergePhase::Failed(error)) if error.contains("git merge --abort")
            ),
            "{:?}",
            app.overlay
        );
        assert!(
            app.corgi_waker
                .as_ref()
                .unwrap()
                .pending_for(&root)
                .is_empty()
        );

        remove_conflicting_merge(repo, worktree);
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
