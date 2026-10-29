//! The dashboard's merge dialog: its keys, and the background job that runs
//! the merge of [`corgi_core::merge`] once the user confirms.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent};

pub(crate) use corgi_core::merge::*;

use crate::job::{Job, Update};

use super::{
    App,
    close::{CloseTarget, CloseWorkspaceForm, dashboard_supervisor_workspaces},
    overlay::Overlay,
    progress::JobReport,
    project_main::project_main_workspace,
};

impl App {
    /// Starts an explicitly confirmed merge from an agent's isolated checkout.
    /// Corgi performs the merge in the primary checkout so agents never need
    /// to touch another branch or worktree.
    pub fn begin_merge_worktree(&mut self) {
        let Some(agent) = self.selected_agent() else {
            self.status = "No agent selected".into();
            return;
        };
        if agent.worktree_checkout.is_none() {
            self.status = "Merge is available only for an agent in a linked worktree".into();
            return;
        }
        match merge_form(agent) {
            Ok(form) => self.overlay = Overlay::merge(form),
            Err(error) => self.status = format!("Merge unavailable: {error:#}"),
        }
    }

    /// Starts the merge, doing `upstream` of what the form's upstream is:
    /// checking it, pulling it, or skipping it.
    fn start_merge_worktree(&mut self, upstream: fn(&Upstream) -> Upstream) {
        if self.merge_job.is_running() {
            return;
        }
        let Some(form) = self.overlay.merge_worktree_form_mut() else {
            return;
        };
        form.upstream = upstream(&form.upstream);
        form.phase = MergePhase::Running(0);
        let form = form.clone();
        self.status = format!("Merging {}…", form.source_branch);
        self.merge_job = Job::spawn(move |mut progress| run_merge(&form, &mut progress));
    }

    pub fn poll_merge(&mut self) {
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
                Ok(commit) => {
                    self.clear_merged_tag();
                    self.tell_supervisor_about_merge(&commit, None);
                    if let Some(form) = self.overlay.merge_worktree_form_mut() {
                        form.phase = MergePhase::Succeeded;
                        self.status = format!(
                            "Merged and pushed {} into {}",
                            form.source_branch, form.target_branch
                        );
                    }
                    self.request_refresh();
                }
                Err(MergeError::PushFailed { commit, error }) => {
                    // Only the push failed: the branch is merged all the same.
                    self.clear_merged_tag();
                    self.tell_supervisor_about_merge(&commit, Some(&error));
                    let help = self.conflict_help();
                    if let Some(form) = self.overlay.merge_worktree_form_mut() {
                        self.status = format!(
                            "Merged {} into {}, but the push failed",
                            form.source_branch, form.target_branch
                        );
                        form.phase = MergePhase::PushFailed {
                            commit,
                            error,
                            help,
                        };
                    }
                    self.request_refresh();
                }
                Err(MergeError::Behind(commits)) => {
                    if let Some(form) = self.overlay.merge_worktree_form_mut() {
                        self.status = format!(
                            "{} has {} commit(s) {} lacks",
                            form.upstream.name().unwrap_or("The upstream"),
                            commits.len(),
                            form.target_branch
                        );
                        form.phase = MergePhase::Behind(commits);
                    }
                }
                Err(MergeError::FetchFailed(error)) => {
                    self.status = format!("Fetch failed: {error}");
                    if let Some(form) = self.overlay.merge_worktree_form_mut() {
                        form.phase = MergePhase::FetchFailed(error);
                    }
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

    /// Clears the merge tag of the agent whose branch the open merge dialog
    /// has just merged: there is nothing left for the user to merge.
    pub fn clear_merged_tag(&mut self) {
        if let Some(form) = self.overlay.merge_worktree_form() {
            clear_merged_tag(&self.client, &mut self.agents, form);
        }
    }

    /// Adds a line to the inbox of the project's supervisor saying that the
    /// open merge dialog's branch was merged into its base at `commit`, and
    /// that the push failed with `push_error` when it did, so the supervisor
    /// records it and goes on with what waited on it. It is kept for the
    /// supervisor's next turn when none runs now, and not written for a
    /// project that never had one, nor from a dashboard without an inbox.
    fn tell_supervisor_about_merge(&mut self, commit: &str, push_error: Option<&str>) {
        let (Some(form), Some(inboxes)) = (self.overlay.merge_worktree_form(), &self.inboxes)
        else {
            return;
        };
        if let Err(error) =
            tell_supervisor_about_merge(inboxes, &self.agents, form, commit, push_error)
        {
            self.set_status(
                format!("Merged, but the supervisor could not be told: {error:#}"),
                Some(Duration::from_secs(15)),
            );
        }
    }

    /// Who can take over the open merge dialog's conflict: its project's
    /// supervisor, when this dashboard can write to its inbox, as every
    /// interactive one can, whichever of them delivers.
    fn conflict_help(&self) -> ConflictHelp {
        let Some(form) = self.overlay.merge_worktree_form() else {
            return ConflictHelp::Unavailable;
        };
        conflict_help(&self.agents, form, self.inboxes.is_some())
    }

    /// Aborts the conflicted merge Corgi started in the primary checkout,
    /// and adds a line to the inbox of the project's supervisor to have the worker
    /// merge the base branch into its own branch and resolve it there. The
    /// worktree stays open; the user merges again once the worker reports.
    fn ask_supervisor_about_conflict(&mut self) {
        let Some(form) = self.overlay.merge_worktree_form() else {
            return;
        };
        let MergePhase::Conflicted { files, .. } = &form.phase else {
            return;
        };
        let Some(supervisor) = project_supervisor(&self.agents, &form.project_root) else {
            self.supervisor_gone();
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
        let item = conflict_item(form, files);
        let mut status = format!(
            "Merge aborted; asked {supervisor} to have {} merge {} into its branch",
            form.agent, form.target_branch
        );
        if let Some(inboxes) = &self.inboxes
            && let Err(error) = inboxes.notify(item)
        {
            status = format!("Merge aborted, but {supervisor} could not be told: {error:#}");
        }
        self.overlay = Overlay::None;
        self.motion.succeeded();
        self.set_status(status, Some(Duration::from_secs(15)));
        self.request_refresh();
    }

    /// Adds a line to the inbox of the project's supervisor handing it the
    /// push that failed after the open merge dialog's merge, so it tells the
    /// user what the error means and what to run. It shares the merged
    /// line's key: it says the merge landed too, and stands in for that line
    /// when it has not gone out yet.
    fn ask_supervisor_about_push(&mut self) {
        let Some(form) = self.overlay.merge_worktree_form() else {
            return;
        };
        let MergePhase::PushFailed { commit, error, .. } = &form.phase else {
            return;
        };
        let Some(supervisor) = project_supervisor(&self.agents, &form.project_root) else {
            self.supervisor_gone();
            return;
        };
        let item = push_failed_item(form, commit, error);
        let mut status = format!(
            "Asked {supervisor} about the failed push of {}",
            form.target_branch
        );
        if let Some(inboxes) = &self.inboxes
            && let Err(error) = inboxes.notify(item)
        {
            status = format!("{supervisor} could not be asked about the push: {error:#}");
        }
        self.overlay = Overlay::None;
        self.motion.succeeded();
        self.set_status(status, Some(Duration::from_secs(15)));
        self.request_refresh();
    }

    /// Takes the handoff key off the open merge dialog: the supervisor it
    /// would go to has stopped since the dialog offered it.
    fn supervisor_gone(&mut self) {
        self.status = "The supervisor is no longer running".into();
        if let Some(form) = self.overlay.merge_worktree_form_mut()
            && let MergePhase::Conflicted { help, .. } | MergePhase::PushFailed { help, .. } =
                &mut form.phase
        {
            *help = ConflictHelp::NoSupervisor;
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
                &dashboard_supervisor_workspaces(&self.agents),
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

    pub fn handle_merge_worktree_key(&mut self, key: KeyEvent) -> bool {
        let phase = self.overlay.merge_worktree_form().map(|form| &form.phase);
        match (phase, key.code) {
            (
                Some(MergePhase::Confirm | MergePhase::Behind(_) | MergePhase::FetchFailed(_)),
                KeyCode::Esc,
            ) => self.cancel_editor(),
            (Some(MergePhase::Confirm), KeyCode::Enter) => {
                self.start_merge_worktree(Upstream::checked)
            }
            (Some(MergePhase::Behind(_)), KeyCode::Enter) => {
                self.start_merge_worktree(Upstream::pulled)
            }
            (Some(MergePhase::FetchFailed(_)), KeyCode::Enter) => {
                self.start_merge_worktree(Upstream::skipped)
            }
            (Some(MergePhase::Running(_)), KeyCode::Esc) => {
                self.status = "Merge and push are still running".into()
            }
            (Some(MergePhase::Succeeded), KeyCode::Char('x')) => self.close_worktree_after_merge(),
            (Some(MergePhase::Succeeded), KeyCode::Enter | KeyCode::Esc) => {
                self.dismiss_merge_worktree()
            }
            (
                Some(
                    MergePhase::Failed(_)
                    | MergePhase::Conflicted { .. }
                    | MergePhase::PushFailed { .. },
                ),
                KeyCode::Enter,
            ) => self.start_merge_worktree(Upstream::checked),
            (
                Some(
                    MergePhase::Failed(_)
                    | MergePhase::Conflicted { .. }
                    | MergePhase::PushFailed { .. },
                ),
                KeyCode::Esc,
            ) => self.dismiss_merge_worktree(),
            (
                Some(MergePhase::Conflicted {
                    help: ConflictHelp::Supervisor(_),
                    ..
                }),
                KeyCode::Char('s'),
            ) => self.ask_supervisor_about_conflict(),
            (
                Some(MergePhase::PushFailed {
                    help: ConflictHelp::Supervisor(_),
                    ..
                }),
                KeyCode::Char('s'),
            ) => self.ask_supervisor_about_push(),
            _ => {}
        }
        false
    }
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
        app::{progress::Silent, test_helpers::poll_until, waker::SupervisorWaker},
        git::{ensure_git_clean, git_upstream, worktree_commits},
        herdr::HerdrClient,
        model::DashboardAgent,
        supervisor,
        test_support::{answer, fake_herdr, test_app},
    };

    use super::*;

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
            upstream: Upstream::None,
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
            upstream: Upstream::None,
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
        git_test(&repo, &["init", "-b", "main"]);
        git_test(&repo, &["config", "user.name", "Corgi test"]);
        git_test(&repo, &["config", "user.email", "corgi@example.test"]);
        fs::write(repo.join("status.txt"), "base\n").expect("write initial file");
        git_test(&repo, &["add", "status.txt"]);
        git_test(&repo, &["commit", "-m", "initial"]);
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
            upstream: Upstream::None,
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

    fn project_agent(name: &str, root: &Path, supervisor: bool) -> DashboardAgent {
        DashboardAgent {
            info: crate::model::AgentInfo {
                pane_id: format!("{name}:p1"),
                name: Some(name.into()),
                ..Default::default()
            },
            project_root: root.to_string_lossy().into_owned(),
            supervisor,
            ..DashboardAgent::default()
        }
    }

    /// The dashboard that wakes supervisors, with the conflicted merge of
    /// `form` reported by its merge job, and `agents` in view.
    fn conflicted_dashboard(form: &MergeWorktreeForm, agents: Vec<DashboardAgent>) -> App {
        let mut app = test_app();
        let mut waker =
            SupervisorWaker::in_dir(&form.project_root.join(".git").join("corgi-state"));
        assert!(waker.lead(&form.project_root.join(".git").join("corgi-wake.lock")));
        app.wake_with(waker);
        app.agents = agents;
        let mut running = form.clone();
        running.phase = MergePhase::Running(1);
        app.overlay = Overlay::merge(running);
        let form = form.clone();
        app.merge_job = Job::spawn(move |_| run_merge(&form, &mut Silent));
        poll_until(&mut app, App::poll_merge, |app| !app.merge_job.is_running());
        app
    }

    /// A repository with a bare remote and an agent worktree whose branch
    /// merges cleanly into `main`. Returns the repository, the worktree, the
    /// remote, and the form for that merge.
    fn mergeable_repo(label: &str) -> (PathBuf, PathBuf, PathBuf, MergeWorktreeForm) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        let repo = std::env::temp_dir().join(format!("corgi-{label}-{nonce}"));
        let worktree = std::env::temp_dir().join(format!("corgi-{label}-worktree-{nonce}"));
        let remote = std::env::temp_dir().join(format!("corgi-{label}-remote-{nonce}"));
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
        fs::write(worktree.join("status.txt"), "agent\n").expect("write agent change");
        git_test(&worktree, &["commit", "-am", "agent result"]);
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
            upstream: Upstream::of(git_upstream(&repo, "main")),
            phase: MergePhase::Confirm,
        };
        assert_eq!(form.upstream, Upstream::Check("origin/main".into()));
        (repo, worktree, remote, form)
    }

    fn remove_mergeable_repo(repo: PathBuf, worktree: PathBuf, remote: PathBuf) {
        remove_conflicting_merge(repo, worktree);
        fs::remove_dir_all(remote).expect("remove test remote");
    }

    /// The state directory the test dashboard keeps `repo`'s inbox in, with
    /// a supervisor's role written to it as a launch writes it.
    fn prepare_supervisor_state(repo: &Path) {
        let dir = repo
            .join(".git")
            .join("corgi-state")
            .join(repo.file_name().unwrap());
        fs::create_dir_all(&dir).expect("create state dir");
        fs::write(dir.join(supervisor::ROLE_FILE), "role").expect("write role");
    }

    #[test]
    fn a_merge_tells_the_running_project_supervisor_once() {
        let (repo, worktree, remote, form) = mergeable_repo("merge-landed");
        let root = repo.to_string_lossy().into_owned();
        let app = conflicted_dashboard(
            &form,
            vec![
                project_agent("supervisor-corgi", &repo, true),
                project_agent("w-reviewed-agent", &repo, false),
            ],
        );
        assert_eq!(
            app.overlay.merge_worktree_form().map(|form| &form.phase),
            Some(&MergePhase::Succeeded)
        );
        let commit = git_test_stdout(&repo, &["rev-parse", "--short", "main"]);
        let pending = app.inboxes.as_ref().unwrap().pending_for(&root);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert!(
            pending[0].starts_with(&format!(
                "[Corgi] The user merged w-reviewed-agent (task \"Make the status file say \
                 agent\"), branch worktree/reviewed-agent, into main at {commit}. If \
                 w-reviewed-agent is in your ledger"
            )),
            "{pending:?}"
        );

        remove_mergeable_repo(repo, worktree, remote);
    }

    #[test]
    fn a_merge_in_a_project_that_never_had_a_supervisor_tells_no_one() {
        let (repo, worktree, remote, form) = mergeable_repo("merge-landed-alone");
        let root = repo.to_string_lossy().into_owned();
        let app =
            conflicted_dashboard(&form, vec![project_agent("w-reviewed-agent", &repo, false)]);
        assert_eq!(
            app.overlay.merge_worktree_form().map(|form| &form.phase),
            Some(&MergePhase::Succeeded)
        );
        assert!(app.inboxes.as_ref().unwrap().pending_for(&root).is_empty());

        remove_mergeable_repo(repo, worktree, remote);
    }

    #[test]
    fn a_merge_from_the_popup_without_an_inbox_writes_nothing() {
        let (repo, worktree, remote, form) = mergeable_repo("merge-landed-popup");
        prepare_supervisor_state(&repo);
        let mut app = test_app();
        app.agents = vec![project_agent("supervisor-corgi", &repo, true)];
        let mut running = form.clone();
        running.phase = MergePhase::Running(1);
        app.overlay = Overlay::merge(running);
        let job_form = form.clone();
        app.merge_job = Job::spawn(move |_| run_merge(&job_form, &mut Silent));
        poll_until(&mut app, App::poll_merge, |app| !app.merge_job.is_running());
        assert_eq!(
            app.overlay.merge_worktree_form().map(|form| &form.phase),
            Some(&MergePhase::Succeeded)
        );
        let state = repo
            .join(".git")
            .join("corgi-state")
            .join(repo.file_name().unwrap());
        assert!(!state.join(crate::inbox::INBOX_FILE).exists());

        remove_mergeable_repo(repo, worktree, remote);
    }

    #[test]
    fn a_failed_fetch_or_cancelled_merge_tells_no_one() {
        let (repo, worktree, remote, form) = mergeable_repo("merge-landed-failed");
        let root = repo.to_string_lossy().into_owned();
        // Neither the fetch nor the push has anywhere to go.
        fs::remove_dir_all(&remote).expect("remove test remote");
        let mut app =
            conflicted_dashboard(&form, vec![project_agent("supervisor-corgi", &repo, true)]);
        assert!(matches!(
            app.overlay.merge_worktree_form().map(|form| &form.phase),
            Some(MergePhase::FetchFailed(_))
        ));
        assert!(app.inboxes.as_ref().unwrap().pending_for(&root).is_empty());

        // Esc on the confirmation runs nothing.
        app.overlay = Overlay::merge(form);
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Esc)));
        assert!(app.overlay.merge_worktree_form().is_none());
        assert!(app.inboxes.as_ref().unwrap().pending_for(&root).is_empty());

        fs::create_dir_all(&remote).expect("recreate remote path");
        remove_mergeable_repo(repo, worktree, remote);
    }

    /// Pushes a commit writing `contents` to `file` into `remote` from a
    /// clone of its own, as another machine would, and removes the clone.
    fn push_from_elsewhere(remote: &Path, file: &str, contents: &str, subject: &str) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        let clone = std::env::temp_dir().join(format!("corgi-merge-elsewhere-{nonce}"));
        git_test(
            &std::env::temp_dir(),
            &["clone", remote.to_str().unwrap(), clone.to_str().unwrap()],
        );
        git_test(&clone, &["config", "user.name", "Elsewhere"]);
        git_test(&clone, &["config", "user.email", "elsewhere@example.test"]);
        fs::write(clone.join(file), contents).expect("write elsewhere change");
        git_test(&clone, &["add", file]);
        git_test(&clone, &["commit", "-m", subject]);
        git_test(&clone, &["push", "origin", "main"]);
        fs::remove_dir_all(clone).expect("remove elsewhere clone");
    }

    /// Runs the merge the open dialog starts on `key`, to its end.
    fn press_and_finish(app: &mut App, key: KeyCode) {
        assert!(!app.handle_key(KeyEvent::from(key)));
        poll_until(app, App::poll_merge, |app| !app.merge_job.is_running());
    }

    fn phase(app: &App) -> Option<&MergePhase> {
        app.overlay.merge_worktree_form().map(|form| &form.phase)
    }

    #[test]
    fn an_upstream_that_moved_on_is_listed_and_pulled_before_the_merge() {
        let (repo, worktree, remote, form) = mergeable_repo("merge-upstream-ahead");
        let root = repo.to_string_lossy().into_owned();
        push_from_elsewhere(&remote, "notes.txt", "one\n", "Note one");
        push_from_elsewhere(&remote, "notes.txt", "two\n", "Note two");
        let before = git_test_stdout(&repo, &["rev-parse", "main"]);
        assert_eq!(form.steps().len(), 4, "{:?}", form.steps());
        assert_eq!(form.steps()[1], "Fetch origin/main");

        let mut app =
            conflicted_dashboard(&form, vec![project_agent("supervisor-corgi", &repo, true)]);
        let Some(MergePhase::Behind(commits)) = phase(&app) else {
            panic!("{:?}", app.overlay);
        };
        assert_eq!(commits.len(), 2, "{commits:?}");
        assert!(commits[0].ends_with(" Note two"), "{commits:?}");
        assert!(commits[1].ends_with(" Note one"), "{commits:?}");
        assert_eq!(
            git_test_stdout(&repo, &["rev-parse", "main"]),
            before,
            "nothing is merged before the user answers"
        );
        assert!(app.inboxes.as_ref().unwrap().pending_for(&root).is_empty());

        press_and_finish(&mut app, KeyCode::Enter);
        assert_eq!(phase(&app), Some(&MergePhase::Succeeded), "{}", app.status);
        assert_eq!(fs::read_to_string(repo.join("notes.txt")).unwrap(), "two\n");
        assert_eq!(
            fs::read_to_string(repo.join("status.txt")).unwrap(),
            "agent\n"
        );
        assert_eq!(
            git_test_stdout(&repo, &["rev-parse", "main"]),
            git_test_stdout(&remote, &["rev-parse", "refs/heads/main"])
        );
        // The pull was a fast-forward: the merge's first parent is the
        // upstream's head.
        assert_eq!(
            git_test_stdout(&repo, &["log", "-1", "--format=%s", "main^1"]),
            "Note two"
        );
        let pending = app.inboxes.as_ref().unwrap().pending_for(&root);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert!(pending[0].starts_with("[Corgi] The user merged w-reviewed-agent"));
        assert!(!pending[0].contains("push failed"), "{pending:?}");

        remove_mergeable_repo(repo, worktree, remote);
    }

    #[test]
    fn a_diverged_base_merges_its_upstream_before_the_merge() {
        let (repo, worktree, remote, form) = mergeable_repo("merge-upstream-diverged");
        push_from_elsewhere(&remote, "notes.txt", "elsewhere\n", "Note elsewhere");
        fs::write(repo.join("local.txt"), "local\n").expect("write local change");
        git_test(&repo, &["add", "local.txt"]);
        git_test(&repo, &["commit", "-m", "Local only"]);

        let mut app = conflicted_dashboard(&form, Vec::new());
        assert!(matches!(phase(&app), Some(MergePhase::Behind(commits)) if commits.len() == 1));
        press_and_finish(&mut app, KeyCode::Enter);
        assert_eq!(phase(&app), Some(&MergePhase::Succeeded), "{}", app.status);
        for (file, contents) in [
            ("notes.txt", "elsewhere\n"),
            ("local.txt", "local\n"),
            ("status.txt", "agent\n"),
        ] {
            assert_eq!(fs::read_to_string(repo.join(file)).unwrap(), contents);
        }
        assert_eq!(
            git_test_stdout(&repo, &["rev-parse", "main"]),
            git_test_stdout(&remote, &["rev-parse", "refs/heads/main"])
        );

        remove_mergeable_repo(repo, worktree, remote);
    }

    #[test]
    fn a_pull_that_conflicts_is_aborted_and_left_to_the_user() {
        let (repo, worktree, remote, form) = mergeable_repo("merge-upstream-conflict");
        let root = repo.to_string_lossy().into_owned();
        push_from_elsewhere(&remote, "notes.txt", "elsewhere\n", "Note elsewhere");
        fs::write(repo.join("notes.txt"), "local\n").expect("write local change");
        git_test(&repo, &["add", "notes.txt"]);
        git_test(&repo, &["commit", "-m", "Note locally"]);
        let before = git_test_stdout(&repo, &["rev-parse", "main"]);

        let mut app =
            conflicted_dashboard(&form, vec![project_agent("supervisor-corgi", &repo, true)]);
        assert!(matches!(phase(&app), Some(MergePhase::Behind(_))));
        press_and_finish(&mut app, KeyCode::Enter);
        let Some(MergePhase::Failed(error)) = phase(&app) else {
            panic!("{:?}", app.overlay);
        };
        assert!(
            error.contains("pulling origin/main into main conflicted in notes.txt"),
            "{error}"
        );
        assert!(error.contains("Pull by hand"), "{error}");
        assert!(!merge_in_progress(&repo));
        ensure_git_clean(&repo, "primary checkout").expect("the abort leaves it clean");
        assert_eq!(git_test_stdout(&repo, &["rev-parse", "main"]), before);
        assert!(app.inboxes.as_ref().unwrap().pending_for(&root).is_empty());

        remove_mergeable_repo(repo, worktree, remote);
    }

    #[test]
    fn a_failed_fetch_warns_and_lets_the_user_merge_anyway() {
        let (repo, worktree, remote, form) = mergeable_repo("merge-fetch-failed");
        // Fetching fails, but pushing still reaches the remote.
        git_test(
            &repo,
            &[
                "remote",
                "set-url",
                "--push",
                "origin",
                remote.to_str().unwrap(),
            ],
        );
        git_test(
            &repo,
            &["remote", "set-url", "origin", "/nonexistent/corgi-remote"],
        );

        let mut app = conflicted_dashboard(&form, Vec::new());
        let Some(MergePhase::FetchFailed(error)) = phase(&app) else {
            panic!("{:?}", app.overlay);
        };
        assert!(error.contains("git fetch origin"), "{error}");
        assert!(!worktree_commits(&repo, "main", "worktree/reviewed-agent").is_empty());

        press_and_finish(&mut app, KeyCode::Enter);
        assert_eq!(phase(&app), Some(&MergePhase::Succeeded), "{}", app.status);
        let form = app.overlay.merge_worktree_form().unwrap();
        assert_eq!(form.upstream, Upstream::Skip("origin/main".into()));
        assert_eq!(form.steps().len(), 3, "a skipped fetch is not a step");
        assert_eq!(
            git_test_stdout(&repo, &["rev-parse", "main"]),
            git_test_stdout(&remote, &["rev-parse", "refs/heads/main"])
        );

        // Esc on the warning stops without merging.
        let (repo2, worktree2, remote2, form2) = mergeable_repo("merge-fetch-failed-esc");
        git_test(
            &repo2,
            &["remote", "set-url", "origin", "/nonexistent/corgi-remote"],
        );
        let before = git_test_stdout(&repo2, &["rev-parse", "main"]);
        let mut app = conflicted_dashboard(&form2, Vec::new());
        assert!(matches!(phase(&app), Some(MergePhase::FetchFailed(_))));
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Esc)));
        assert!(matches!(app.overlay, Overlay::None));
        assert_eq!(git_test_stdout(&repo2, &["rev-parse", "main"]), before);

        remove_mergeable_repo(repo, worktree, remote);
        remove_mergeable_repo(repo2, worktree2, remote2);
    }

    /// What the supervisor of `repo` would be sent now: its waiting lines,
    /// a newer one standing in for an older one of the same key.
    fn going_out(repo: &Path) -> Vec<String> {
        let dir = repo
            .join(".git")
            .join("corgi-state")
            .join(repo.file_name().unwrap());
        let inbox = crate::inbox::Inbox::read(&dir);
        let items = inbox.undelivered_for(&repo.to_string_lossy());
        crate::inbox::coalesce(&items)
            .into_iter()
            .map(|item| item.text.clone())
            .collect()
    }

    /// Makes `remote` refuse every push, as a protected branch would.
    fn refuse_pushes(remote: &Path) {
        let hook = remote.join("hooks").join("pre-receive");
        fs::write(&hook, "#!/bin/sh\necho 'main is protected' >&2\nexit 1\n").expect("write hook");
        let mut permissions = fs::metadata(&hook).expect("hook metadata").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        fs::set_permissions(&hook, permissions).expect("make hook executable");
    }

    #[test]
    fn a_failed_push_after_the_merge_tells_the_supervisor_and_offers_the_handoff() {
        let (repo, worktree, remote, form) = mergeable_repo("merge-push-failed");
        let root = repo.to_string_lossy().into_owned();
        refuse_pushes(&remote);
        let mut app = conflicted_dashboard(
            &form,
            vec![
                project_agent("supervisor-corgi", &repo, true),
                project_agent("w-reviewed-agent", &repo, false),
            ],
        );
        let commit = git_test_stdout(&repo, &["rev-parse", "--short", "main"]);
        let Some(MergePhase::PushFailed {
            commit: merged,
            error,
            help,
        }) = phase(&app)
        else {
            panic!("{:?}", app.overlay);
        };
        assert_eq!(merged, &commit);
        assert!(error.contains("main is protected"), "{error}");
        assert_eq!(help, &ConflictHelp::Supervisor("supervisor-corgi".into()));
        assert_eq!(
            fs::read_to_string(repo.join("status.txt")).unwrap(),
            "agent\n"
        );
        let pending = app.inboxes.as_ref().unwrap().pending_for(&root);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert!(
            pending[0].starts_with(&format!(
                "[Corgi] The user merged w-reviewed-agent (task \"Make the status file say \
                 agent\"), branch worktree/reviewed-agent, into main at {commit}, but git push \
                 failed: "
            )),
            "{pending:?}"
        );
        assert!(pending[0].contains("main is protected"), "{pending:?}");

        assert!(!app.handle_key(KeyEvent::from(KeyCode::Char('s'))));
        assert!(matches!(app.overlay, Overlay::None), "{:?}", app.overlay);
        assert!(
            app.status.contains("Asked supervisor-corgi"),
            "{}",
            app.status
        );
        // The handoff says the merge landed too, so it stands in for the
        // merged line that has not gone out yet.
        assert_eq!(app.inboxes.as_ref().unwrap().pending_for(&root).len(), 2);
        let pending = going_out(&repo);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert!(
            pending[0].starts_with(&format!(
                "[Corgi] The user asks for help with a failed push. The user merged \
                 w-reviewed-agent (task \"Make the status file say agent\"), branch \
                 worktree/reviewed-agent, into main at {commit}, and git push failed: "
            )),
            "{pending:?}"
        );
        assert!(
            pending[0]
                .contains("w-reviewed-agent is merged. If w-reviewed-agent is in your ledger"),
            "{pending:?}"
        );
        assert!(
            pending[0].ends_with("never pull, push or merge yourself."),
            "{pending:?}"
        );

        remove_mergeable_repo(repo, worktree, remote);
    }

    #[test]
    fn a_failed_push_without_a_running_supervisor_still_reports_the_merge() {
        let (repo, worktree, remote, form) = mergeable_repo("merge-push-failed-queued");
        let root = repo.to_string_lossy().into_owned();
        prepare_supervisor_state(&repo);
        refuse_pushes(&remote);
        let mut app =
            conflicted_dashboard(&form, vec![project_agent("w-reviewed-agent", &repo, false)]);
        assert!(matches!(
            phase(&app),
            Some(MergePhase::PushFailed {
                help: ConflictHelp::NoSupervisor,
                ..
            })
        ));
        let pending = app.inboxes.as_ref().unwrap().pending_for(&root);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert!(pending[0].contains("but git push failed"), "{pending:?}");
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Char('s'))));
        assert!(
            app.overlay.merge_worktree_form().is_some(),
            "s is not offered"
        );

        // Once the remote takes pushes again, Enter fetches and pushes.
        fs::remove_file(remote.join("hooks").join("pre-receive")).expect("remove hook");
        press_and_finish(&mut app, KeyCode::Enter);
        assert_eq!(phase(&app), Some(&MergePhase::Succeeded), "{}", app.status);
        assert_eq!(
            git_test_stdout(&repo, &["rev-parse", "main"]),
            git_test_stdout(&remote, &["rev-parse", "refs/heads/main"])
        );
        let pending = going_out(&repo);
        assert_eq!(pending.len(), 1, "the pushed line replaces the unsent one");
        assert!(!pending[0].contains("push failed"), "{pending:?}");

        remove_mergeable_repo(repo, worktree, remote);
    }

    #[test]
    fn a_conflicted_merge_offers_to_hand_it_to_the_project_supervisor() {
        let (repo, worktree, form) = conflicting_merge("merge-conflict-corgi");
        let root = repo.to_string_lossy().into_owned();
        let mut app = conflicted_dashboard(
            &form,
            vec![
                project_agent("supervisor-corgi", &repo, true),
                project_agent("w-reviewed-agent", &repo, false),
            ],
        );
        assert_eq!(
            app.overlay.merge_worktree_form().map(|form| &form.phase),
            Some(&MergePhase::Conflicted {
                files: vec!["status.txt".into()],
                help: ConflictHelp::Supervisor("supervisor-corgi".into()),
            })
        );
        assert!(merge_in_progress(&repo));
        assert!(
            app.inboxes.as_ref().unwrap().pending_for(&root).is_empty(),
            "a conflict is not a merge"
        );

        assert!(!app.handle_key(KeyEvent::from(KeyCode::Char('s'))));
        assert!(matches!(app.overlay, Overlay::None), "{:?}", app.overlay);
        assert!(!merge_in_progress(&repo));
        ensure_git_clean(&repo, "primary checkout").expect("abort leaves the checkout clean");
        assert!(worktree.exists(), "the worktree stays open");
        assert!(
            app.status.contains("asked supervisor-corgi"),
            "{}",
            app.status
        );
        let pending = app.inboxes.as_ref().unwrap().pending_for(&root);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert!(
            pending[0].starts_with(&format!(
                "[Corgi] The user's merge of w-reviewed-agent (task \"Make the status file say agent\"), \
                 branch worktree/reviewed-agent in worktree {}, into main conflicted in: status.txt. \
                 Corgi aborted it, so the primary checkout is clean. Have w-reviewed-agent merge main \
                 into its own branch, resolve the conflicts there, rerun its checks and report; the \
                 user then merges again with m. ",
                worktree.display()
            )),
            "{pending:?}"
        );

        remove_conflicting_merge(repo, worktree);
    }

    #[test]
    fn a_conflicted_merge_without_a_supervisor_offers_no_handoff() {
        let (repo, worktree, form) = conflicting_merge("merge-conflict-alone");
        let root = repo.to_string_lossy().into_owned();
        let mut app =
            conflicted_dashboard(&form, vec![project_agent("w-reviewed-agent", &repo, false)]);
        assert!(matches!(
            app.overlay.merge_worktree_form().map(|form| &form.phase),
            Some(MergePhase::Conflicted {
                help: ConflictHelp::NoSupervisor,
                ..
            })
        ));
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Char('s'))));
        assert!(app.overlay.merge_worktree_form().is_some());
        assert!(
            merge_in_progress(&repo),
            "c does nothing without a supervisor"
        );
        assert!(app.inboxes.as_ref().unwrap().pending_for(&root).is_empty());

        remove_conflicting_merge(repo, worktree);
    }

    #[test]
    fn esc_on_a_conflicted_merge_leaves_it_to_resolve_by_hand() {
        let (repo, worktree, form) = conflicting_merge("merge-conflict-esc");
        let root = repo.to_string_lossy().into_owned();
        let mut app =
            conflicted_dashboard(&form, vec![project_agent("supervisor-corgi", &repo, true)]);
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Esc)));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(
            merge_in_progress(&repo),
            "the primary checkout stays mid-merge"
        );
        assert!(app.inboxes.as_ref().unwrap().pending_for(&root).is_empty());

        remove_conflicting_merge(repo, worktree);
    }

    #[test]
    fn a_failed_abort_is_reported_in_the_popup_and_asks_no_one() {
        let (repo, worktree, form) = conflicting_merge("merge-conflict-abort");
        let root = repo.to_string_lossy().into_owned();
        let mut app =
            conflicted_dashboard(&form, vec![project_agent("supervisor-corgi", &repo, true)]);
        // The user finished the merge by hand behind the popup's back.
        git_test(&repo, &["checkout", "--theirs", "status.txt"]);
        git_test(&repo, &["commit", "-am", "resolved by hand"]);
        assert!(!app.handle_key(KeyEvent::from(KeyCode::Char('s'))));
        assert!(
            matches!(
                app.overlay.merge_worktree_form().map(|form| &form.phase),
                Some(MergePhase::Failed(error)) if error.contains("git merge --abort")
            ),
            "{:?}",
            app.overlay
        );
        assert!(app.inboxes.as_ref().unwrap().pending_for(&root).is_empty());

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

    #[test]
    fn a_merge_is_kept_for_a_supervisor_that_is_not_running() {
        let (repo, worktree, remote, form) = mergeable_repo("merge-landed-queued");
        let root = repo.to_string_lossy().into_owned();
        prepare_supervisor_state(&repo);
        let app =
            conflicted_dashboard(&form, vec![project_agent("w-reviewed-agent", &repo, false)]);
        let pending = app.inboxes.as_ref().unwrap().pending_for(&root);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert!(pending[0].starts_with("[Corgi] The user merged w-reviewed-agent"));

        remove_mergeable_repo(repo, worktree, remote);
    }

    #[test]
    fn a_base_without_an_upstream_merges_without_fetching() {
        let (repo, worktree, mut form) = conflicting_merge("merge-no-upstream");
        assert_eq!(git_upstream(&repo, "main"), None);
        form.upstream = Upstream::of(git_upstream(&repo, "main"));
        assert_eq!(form.upstream, Upstream::None);
        assert_eq!(
            form.steps(),
            [
                "Check both worktrees",
                "Merge worktree/reviewed-agent into main",
                "Push main"
            ]
        );
        // No fetch stands in the way: the merge runs, and stops on its
        // conflicts as before.
        let app = conflicted_dashboard(&form, Vec::new());
        assert!(matches!(phase(&app), Some(MergePhase::Conflicted { .. })));

        remove_conflicting_merge(repo, worktree);
    }
}
