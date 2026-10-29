//! `corgi serve`'s commands, each the dashboard's action on the same core
//! flow: what a key does there, a command does here, with the confirmation
//! and its outcome as data. A command that waits on Herdr, Git or a harness
//! runs on a thread of its own and replies from there; every failure is a
//! reply, never a crash.

use std::{
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::atomic::Ordering,
    thread,
};

use anyhow::{Context, Result, anyhow, bail};
use corgi_core::{
    catalog::{MODEL_CATALOG_TTL, default_harness, effort_choices, harness_choices, model_choices},
    choices,
    close::{CloseTarget, Closed, close, close_form},
    digest::memory::{DigestLookup, digest_lookup},
    harness::Harness,
    herdr::HerdrError,
    inbox::{Item, Kind},
    launch::{
        Checkout, FormRow, LaunchPlan, NewAgentValues, dashboard_supervisor_plan, is_new_project,
        launch_agent, launch_steps, new_agent_plan, starts_supervisor,
    },
    merge::{
        ConflictHelp, MergeError, MergePhase, abort_merge, clear_merged_tag, conflict_help,
        conflict_item, merge_form, project_supervisor, push_failed_item, run_merge,
        tell_supervisor_about_merge,
    },
    model::DashboardAgent,
    progress,
    projects::{ProjectMemory, known_projects, new_project_parent, project_choices},
    protocol::{
        Choice, CloseForm, Command, DigestQuery, MergeForm, MergeOutcome, NewAgentForm, Progress,
        Reply, ReplyResult, Request, ServerMessage, SpawnRequest, UpstreamChoice,
    },
    supervisor,
    tag::{TagOptions, tag_agent},
};
use serde_json::Value;

use super::{
    Server, Shared,
    state::{open_questions, question_number},
};

/// Why a merge cannot go on with the form it has.
const NO_MERGE: &str = "no merge is open for this agent; send merge_check first";

impl Server {
    /// Answers one line from the frontend.
    pub(super) fn command(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                return self
                    .shared
                    .output
                    .reply(refused(None, format!("not a JSON object: {error}")));
            }
        };
        let id = value.get("id").and_then(Value::as_u64);
        // A prompt's text must never come back, even in a parser's message.
        let text = value
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string);
        let request: Request = match serde_json::from_value(value) {
            Ok(request) => request,
            Err(error) => {
                let mut error = error.to_string();
                if text.is_some_and(|text| !text.is_empty() && error.contains(&text)) {
                    error = "invalid fields".into();
                }
                return self
                    .shared
                    .output
                    .reply(refused(id, format!("unreadable request: {error}")));
            }
        };
        self.dispatch(request.id, request.command);
    }

    fn dispatch(&mut self, id: u64, command: Command) {
        match command {
            Command::Resync => {
                self.resync();
                self.done(id, "Sent the whole state");
            }
            Command::Refresh => {
                self.refresh_requested = true;
                self.done(id, "Refreshing");
            }
            Command::Select { pane_id } => {
                self.selection = pane_id;
                self.share_selection();
                self.done(id, "Selected");
            }
            Command::Watch { pane_id } => {
                if self
                    .transcript
                    .as_ref()
                    .map(|transcript| &transcript.pane_id)
                    != pane_id.as_ref()
                {
                    self.transcript = None;
                }
                self.watch = pane_id;
                self.refresh_requested = true;
                self.done(id, "Watching");
            }
            Command::Prompt { pane_id, text } => self.prompt(id, pane_id, text),
            Command::Answer {
                project_root,
                question,
                text,
            } => self.answer(id, &project_root, &question, text),
            Command::CloseQuestion {
                project_root,
                question,
            } => self.close_question(id, &project_root, &question),
            Command::NewAgentForm {
                pane_id,
                project,
                harness,
                model,
            } => self.new_agent_form(id, pane_id, project, harness, model),
            Command::Spawn(request) => self.spawn(id, request),
            Command::StartSupervisor { project_root } => self.start_supervisor(id, project_root),
            Command::MergeCheck { pane_id } => self.merge_check(id, pane_id),
            Command::Merge { pane_id, upstream } => self.merge(id, pane_id, upstream),
            Command::MergeHandoff { pane_id } => self.merge_handoff(id, pane_id),
            Command::MergeDismiss { pane_id } => self.merge_dismiss(id, &pane_id),
            Command::CloseCheck { pane_id } => self.close_check(id, &pane_id),
            Command::Close { pane_id, force } => self.close(id, &pane_id, force),
            Command::Tag { pane_id, merge } => self.tag(id, &pane_id, merge),
            Command::Focus { pane_id } => self.focus(id, &pane_id),
            Command::Edit { pane_id } => self.edit(id, &pane_id),
            Command::Digest {
                project_root,
                lookup,
            } => self.digest(id, project_root, lookup),
        }
    }

    fn done(&self, id: u64, message: &str) {
        self.shared.output.reply(answered(
            id,
            Ok(ReplyResult::Done {
                message: message.into(),
            }),
        ));
    }

    fn refuse(&self, id: u64, error: impl fmt::Display) {
        self.shared
            .output
            .reply(refused(Some(id), format!("{error:#}")));
    }

    /// Runs `job` on a thread of its own and replies with what it returns,
    /// or with why it stopped, even if it panicked. A job that changes
    /// Herdr or Git asks for a refresh when it ends.
    fn work(
        &self,
        id: u64,
        changes: bool,
        job: impl FnOnce(&Shared) -> Result<ReplyResult> + Send + 'static,
    ) {
        let shared = self.shared.clone();
        shared.running.fetch_add(1, Ordering::SeqCst);
        thread::spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| job(&shared)))
                .unwrap_or_else(|_| Err(anyhow!("the command stopped unexpectedly")));
            shared.output.reply(answered(id, result));
            if changes {
                let _ = shared.events.send(super::Event::Changed);
            }
            shared.running.fetch_sub(1, Ordering::SeqCst);
        });
    }

    /// `pane_id`'s agent, or a refusal already sent.
    fn agent_or_refuse(&self, id: u64, pane_id: &str) -> Option<DashboardAgent> {
        match self.agent(pane_id) {
            Ok(agent) => Some(agent.clone()),
            Err(error) => {
                self.refuse(id, error);
                None
            }
        }
    }

    fn prompt(&mut self, id: u64, pane_id: String, text: String) {
        if text.trim().is_empty() {
            return self.refuse(id, "Prompt cannot be empty");
        }
        let Some(agent) = self.agent_or_refuse(id, &pane_id) else {
            return;
        };
        let label = agent.info.display_name().to_string();
        self.work(id, true, move |shared| {
            deliver(shared, &pane_id, text.trim())?;
            Ok(ReplyResult::Done {
                message: format!("Prompt sent to {label}"),
            })
        });
    }

    /// Answers the supervisor's question `question` (`Q3`) by prompting it
    /// with `Q3: <text>`, the form its role asks the user to answer in.
    fn answer(&mut self, id: u64, root: &str, question: &str, text: String) {
        let Some(number) = question_number(question) else {
            return self.refuse(
                id,
                format!("{question:?} is not a question number such as Q3"),
            );
        };
        if text.trim().is_empty() {
            return self.refuse(id, "The answer cannot be empty");
        }
        let root = root.trim_end_matches('/');
        let Some(supervisor) = self
            .agents
            .iter()
            .find(|agent| agent.supervisor && agent.project_root.trim_end_matches('/') == root)
        else {
            return self.refuse(
                id,
                format!("No supervisor runs for {root}; start it to answer its questions"),
            );
        };
        let pane_id = supervisor.info.pane_id.clone();
        self.work(id, true, move |shared| {
            deliver(shared, &pane_id, &format!("Q{number}: {}", text.trim()))?;
            Ok(ReplyResult::Done {
                message: format!("Answered Q{number}"),
            })
        });
    }

    /// Closes the supervisor's open question `question` (`Q3`) without an
    /// answer: an inbox line asks the supervisor to drop it from its
    /// ledger, typed in once it is between turns, or when it next runs.
    /// The question leaves the list once the ledger no longer has it.
    fn close_question(&mut self, id: u64, root: &str, question: &str) {
        let Some(number) = question_number(question) else {
            return self.refuse(
                id,
                format!("{question:?} is not a question number such as Q3"),
            );
        };
        let root = root.trim_end_matches('/').to_string();
        let handle = format!("Q{number}");
        if !open_questions(&self.shared.inboxes, &root)
            .iter()
            .any(|open| open.id == handle)
        {
            return self.refuse(id, format!("{handle} is not open for {root}"));
        }
        let supervisor = self
            .agents
            .iter()
            .find(|agent| agent.supervisor && agent.project_root.trim_end_matches('/') == root)
            .map(|agent| agent.info.display_name().to_string());
        self.work(id, true, move |shared| {
            // One key per question: closing it twice before the line goes
            // out still sends it once.
            let item = Item {
                project: root,
                source: "serve".into(),
                kind: Kind::Note,
                key: format!("{handle} closed"),
                text: supervisor::question_closed_message(number),
                ..Default::default()
            };
            shared
                .inboxes
                .notify(item)
                .context("The supervisor could not be told")?;
            Ok(ReplyResult::Done {
                message: match supervisor {
                    Some(name) => format!("Asked {name} to drop {handle}"),
                    None => format!("{handle} is dropped once the supervisor starts"),
                },
            })
        });
    }

    /// The new-agent form preset as the dashboard's `n` presets it: the
    /// selected agent's repository, else `project`, else the project used
    /// last, on the preset harness.
    fn new_agent_form(
        &mut self,
        id: u64,
        pane_id: Option<String>,
        project: Option<String>,
        harness: Option<String>,
        model: Option<String>,
    ) {
        let agents = self.agents.clone();
        let workspaces = self.workspaces.clone();
        let memory = self.options.projects.clone();
        let installed = self.options.installed.clone();
        let discover = self.options.discover_models;
        let from_agent = match pane_id {
            Some(pane_id) => match self.agent(&pane_id) {
                // A scratch agent's home directory is no project to add to.
                Ok(agent) => Some(agent.project_root.clone())
                    .filter(|root| !agent.scratch && !root.is_empty()),
                Err(error) => return self.refuse(id, error),
            },
            None => None,
        };
        self.work(id, false, move |shared| {
            let memory = memory.unwrap_or_else(ProjectMemory::load);
            let known = known_projects(&agents, &workspaces, &memory);
            let project = from_agent
                .or(project.filter(|project| !project.trim().is_empty()))
                .or_else(|| {
                    memory
                        .roots()
                        .iter()
                        .find(|root| Path::new(root).is_dir())
                        .cloned()
                })
                .or_else(|| known.first().map(|project| project.root.clone()))
                .unwrap_or_default();
            let kind = harness
                .filter(|kind| !kind.trim().is_empty())
                .unwrap_or_else(|| default_harness().kind().to_string());
            let harness = Harness::from_typed(&kind);
            let model = model.unwrap_or_default();
            let models = model_options(shared, &harness, discover);
            let defaults = shared
                .catalogs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .defaults(&harness, &model);
            Ok(ReplyResult::NewAgentForm(Box::new(NewAgentForm {
                starts_supervisor: starts_supervisor(false, &project, &agents),
                projects: wire(project_choices(&known)),
                new_project_parent: new_project_parent(&known),
                project,
                harnesses: wire(harness_choices(&installed, &kind)),
                harness: kind,
                models: wire(model_choices(&models, &model, defaults.model.as_deref())),
                model,
                effort: String::new(),
                efforts: wire(effort_choices(defaults.effort.as_deref())),
                supports_effort: harness.supports_effort(),
                checkout: Checkout::default().value().to_string(),
                checkouts: [Checkout::Worktree, Checkout::Directory]
                    .into_iter()
                    .map(|checkout| Choice {
                        value: checkout.value().into(),
                        label: checkout.label().into(),
                        detail: checkout.detail().into(),
                        badge: String::new(),
                    })
                    .collect(),
                default_model: defaults.model,
                default_effort: defaults.effort,
            })))
        });
    }

    /// Starts the agent the filled-in form describes, with the form's own
    /// checks: a worker, or the project's supervisor when it has no agent.
    fn spawn(&mut self, id: u64, request: SpawnRequest) {
        let checkout = if request.checkout.trim().is_empty() {
            Checkout::default()
        } else {
            match Checkout::from_value(request.checkout.trim()) {
                Some(checkout) => checkout,
                None => {
                    return self.refuse(
                        id,
                        format!(
                            "checkout must be worktree or directory, not {:?}",
                            request.checkout
                        ),
                    );
                }
            }
        };
        let values = NewAgentValues {
            project: &request.project,
            new_project: is_new_project(&request.project, request.new_project),
            kind: &request.harness,
            model: &request.model,
            effort: &request.effort,
            prompt: &request.task,
            checkout,
        };
        let plan = match new_agent_plan(&values, &self.agents) {
            Ok(plan) => plan,
            Err((row, message)) => {
                let field = row.map(|row| match row {
                    FormRow::Task => "task",
                    FormRow::Harness => "harness",
                    FormRow::Project => "project",
                });
                return self.shared.output.reply(Reply {
                    field: field.map(str::to_string),
                    ..refused(Some(id), message)
                });
            }
        };
        let remember = self.options.projects.is_none();
        self.work(id, true, move |shared| {
            let launched = launch(shared, id, &plan, remember)?;
            Ok(ReplyResult::Done {
                message: format!("{} started {}", plan.name, launched.1),
            })
        });
    }

    /// The dashboard's supervisor key: names the running supervisor of
    /// `root`, or starts it on the harness it last ran on.
    fn start_supervisor(&mut self, id: u64, root: String) {
        let root = root.trim_end_matches('/').to_string();
        if let Some(running) = self
            .agents
            .iter()
            .find(|agent| agent.supervisor && agent.project_root.trim_end_matches('/') == root)
        {
            return self.shared.output.reply(answered(
                id,
                Ok(ReplyResult::Supervisor {
                    pane_id: Some(running.info.pane_id.clone()),
                    started: false,
                    message: format!("The supervisor runs as {}", running.info.display_name()),
                }),
            ));
        }
        let saved = self
            .shared
            .inboxes
            .dir(&root)
            .and_then(|dir| supervisor::saved_launch(&dir));
        let plan = match dashboard_supervisor_plan(&self.agents, &root, saved, default_harness) {
            Ok(plan) => plan,
            Err(error) => return self.refuse(id, error),
        };
        self.work(id, true, move |shared| {
            let (pane_id, location) = launch(shared, id, &plan, false)?;
            Ok(ReplyResult::Supervisor {
                pane_id: Some(pane_id),
                started: true,
                message: format!("{} started {location}", plan.name),
            })
        });
    }

    /// The merge dialog for `pane_id`'s worktree, kept for `merge` to run.
    fn merge_check(&mut self, id: u64, pane_id: String) {
        let Some(agent) = self.agent_or_refuse(id, &pane_id) else {
            return;
        };
        if merge_running(&self.shared, &pane_id) {
            return self.refuse(id, "this agent's merge is running");
        }
        self.work(id, false, move |shared| {
            let form = merge_form(&agent).context("Merge unavailable")?;
            let reply = MergeForm {
                pane_id: pane_id.clone(),
                agent: form.agent.clone(),
                task: form.task.clone(),
                project_root: form.project_root.display().to_string(),
                worktree: form.worktree_checkout.display().to_string(),
                source_branch: form.source_branch.clone(),
                target_branch: form.target_branch.clone(),
                commits: form.commits.clone(),
                upstream: form.upstream.name().map(str::to_string),
                steps: form.steps(),
            };
            lock(&shared.merges).insert(pane_id, form);
            Ok(ReplyResult::MergeForm(Box::new(reply)))
        });
    }

    /// Runs the merge `merge_check` prepared, doing `upstream` about the
    /// target's upstream, and does what the dashboard does after it: clears
    /// the merged agent's tag and tells its supervisor.
    fn merge(&mut self, id: u64, pane_id: String, upstream: UpstreamChoice) {
        let form = {
            let mut merges = lock(&self.shared.merges);
            let Some(form) = merges.get_mut(&pane_id) else {
                drop(merges);
                return self.refuse(id, NO_MERGE);
            };
            if matches!(form.phase, MergePhase::Running(_)) {
                drop(merges);
                return self.refuse(id, "this agent's merge is running");
            }
            form.upstream = match upstream {
                UpstreamChoice::Check => form.upstream.checked(),
                UpstreamChoice::Pull => form.upstream.pulled(),
                UpstreamChoice::Skip => form.upstream.skipped(),
            };
            form.phase = MergePhase::Running(0);
            form.clone()
        };
        let mut agents = self.agents.clone();
        self.work(id, true, move |shared| {
            started(shared, id, form.steps());
            let result = run_merge(&form, &mut Sink::new(shared, id, false));
            // Who can take a conflict or a failed push over.
            let helper = match conflict_help(&agents, &form, true) {
                ConflictHelp::Supervisor(name) => Some(name),
                ConflictHelp::NoSupervisor | ConflictHelp::Unavailable => None,
            };
            let (phase, outcome) = match result {
                Ok(commit) => {
                    clear_merged_tag(&shared.client, &mut agents, &form);
                    let mut message = format!(
                        "Merged and pushed {} into {}",
                        form.source_branch, form.target_branch
                    );
                    if let Err(error) =
                        tell_supervisor_about_merge(&shared.inboxes, &agents, &form, &commit, None)
                    {
                        message =
                            format!("{message}, but the supervisor could not be told: {error:#}");
                    }
                    (None, MergeOutcome::Succeeded { commit, message })
                }
                Err(MergeError::PushFailed { commit, error }) => {
                    // Only the push failed: the branch is merged all the same.
                    clear_merged_tag(&shared.client, &mut agents, &form);
                    let _ = tell_supervisor_about_merge(
                        &shared.inboxes,
                        &agents,
                        &form,
                        &commit,
                        Some(&error),
                    );
                    let phase = MergePhase::PushFailed {
                        commit: commit.clone(),
                        error: error.clone(),
                        help: ConflictHelp::NoSupervisor,
                    };
                    (
                        Some(phase),
                        MergeOutcome::PushFailed {
                            commit,
                            error,
                            supervisor: helper,
                        },
                    )
                }
                Err(MergeError::Conflicts(files)) => {
                    let phase = MergePhase::Conflicted {
                        files: files.clone(),
                        help: ConflictHelp::NoSupervisor,
                    };
                    (
                        Some(phase),
                        MergeOutcome::Conflicted {
                            files,
                            supervisor: helper,
                        },
                    )
                }
                Err(MergeError::Behind(commits)) => (
                    Some(MergePhase::Behind(commits.clone())),
                    MergeOutcome::Behind { commits },
                ),
                Err(MergeError::FetchFailed(error)) => (
                    Some(MergePhase::FetchFailed(error.clone())),
                    MergeOutcome::FetchFailed { error },
                ),
                Err(MergeError::Failed(error)) => (None, MergeOutcome::Failed { error }),
            };
            // A merge that is over is forgotten; one that waits on the user
            // (a pull, a skip, a handoff) is kept.
            let mut merges = lock(&shared.merges);
            match phase {
                Some(phase) => {
                    if let Some(kept) = merges.get_mut(&pane_id) {
                        kept.phase = phase;
                    }
                }
                None => {
                    merges.remove(&pane_id);
                }
            }
            Ok(ReplyResult::MergeOutcome(outcome))
        });
    }

    /// Hands the conflicted merge, or the push that failed after it, to the
    /// project's supervisor, as the dashboard's `s` on the merge dialog does.
    fn merge_handoff(&mut self, id: u64, pane_id: String) {
        let Some(form) = lock(&self.shared.merges).get(&pane_id).cloned() else {
            return self.refuse(id, NO_MERGE);
        };
        let Some(supervisor) = project_supervisor(&self.agents, &form.project_root) else {
            return self.refuse(id, "The supervisor is no longer running");
        };
        self.work(id, true, move |shared| {
            let (item, message) = match &form.phase {
                MergePhase::Conflicted { files, .. } => {
                    abort_merge(&form.project_root).context("Merge abort failed")?;
                    (
                        conflict_item(&form, files),
                        format!(
                            "Merge aborted; asked {supervisor} to have {} merge {} into its branch",
                            form.agent, form.target_branch
                        ),
                    )
                }
                MergePhase::PushFailed { commit, error, .. } => (
                    push_failed_item(&form, commit, error),
                    format!(
                        "Asked {supervisor} about the failed push of {}",
                        form.target_branch
                    ),
                ),
                _ => bail!("only a conflicted merge or a failed push can be handed over"),
            };
            lock(&shared.merges).remove(&pane_id);
            shared
                .inboxes
                .notify(item)
                .with_context(|| format!("{supervisor} could not be told"))?;
            Ok(ReplyResult::Done { message })
        });
    }

    fn merge_dismiss(&mut self, id: u64, pane_id: &str) {
        if merge_running(&self.shared, pane_id) {
            return self.refuse(id, "this agent's merge is running");
        }
        lock(&self.shared.merges).remove(pane_id);
        self.done(id, "Merge dismissed");
    }

    /// What closing `pane_id` takes down, for the confirmation.
    fn close_check(&mut self, id: u64, pane_id: &str) {
        let Some(agent) = self.agent_or_refuse(id, pane_id) else {
            return;
        };
        let form = close_form(&agent, &self.workspaces, &self.agents);
        let (target, checkout, confirm) = match &form.target {
            CloseTarget::Worktree { checkout, .. } => {
                ("worktree", Some(checkout.clone()), "Remove worktree")
            }
            CloseTarget::Tab { .. } => ("tab", None, "Close tab"),
            CloseTarget::Workspace => ("workspace", None, "Close workspace"),
        };
        self.shared.output.reply(answered(
            id,
            Ok(ReplyResult::CloseForm(CloseForm {
                pane_id: pane_id.to_string(),
                label: form.label,
                target: target.into(),
                checkout,
                confirm: confirm.into(),
            })),
        ));
    }

    /// Closes `pane_id` as `close_check` described; `force` discards a
    /// dirty worktree's changes, which the first attempt refuses.
    fn close(&mut self, id: u64, pane_id: &str, force: bool) {
        let Some(agent) = self.agent_or_refuse(id, pane_id) else {
            return;
        };
        let mut form = close_form(&agent, &self.workspaces, &self.agents);
        if let CloseTarget::Worktree { force: forced, .. } = &mut form.target {
            *forced = force;
        }
        self.work(id, true, move |shared| {
            match close(&shared.client, None, form) {
                Closed::Done(message) => Ok(ReplyResult::Done { message }),
                Closed::Dirty(_) => Ok(ReplyResult::CloseDirty {
                    message:
                        "The worktree has uncommitted changes; close with force to discard them"
                            .into(),
                }),
                Closed::Failed(error) => Err(anyhow!(error)),
            }
        });
    }

    fn tag(&mut self, id: u64, pane_id: &str, merge: bool) {
        let Some(agent) = self.agent_or_refuse(id, pane_id) else {
            return;
        };
        let agents = self.agents.clone();
        let options = TagOptions {
            name: pane_id.to_string(),
            merge,
            project: None,
        };
        self.work(id, true, move |shared| {
            let message = tag_agent(&shared.client, &agents, &agent.project_root, &options)?;
            Ok(ReplyResult::Done { message })
        });
    }

    /// Brings the agent's pane into view in Herdr, which moves every
    /// attached client: never from a GUI.
    fn focus(&mut self, id: u64, pane_id: &str) {
        let Some(agent) = self.agent_or_refuse(id, pane_id) else {
            return;
        };
        self.work(id, false, move |shared| {
            shared
                .client
                .go_to_agent(&agent.info)
                .context("Focus failed")?;
            Ok(ReplyResult::Done {
                message: format!("Focused {}", agent.info.display_name()),
            })
        });
    }

    /// The dashboard's `e`: the agent's checkout in the editor, in a new
    /// tab Herdr then shows, so it touches focus.
    fn edit(&mut self, id: u64, pane_id: &str) {
        let Some(agent) = self.agent_or_refuse(id, pane_id) else {
            return;
        };
        self.work(id, true, move |shared| {
            let info = &agent.info;
            let label = crate::app::open_checkout(&shared.client, &info.workspace_id, info.cwd())
                .context("Edit failed")?;
            Ok(ReplyResult::Done {
                message: format!("Opened {label} in the editor"),
            })
        });
    }

    /// `corgi digest` on the memory of `root`'s supervisor.
    fn digest(&mut self, id: u64, root: String, query: DigestQuery) {
        let root = root.trim_end_matches('/').to_string();
        let agents = self.agents.clone();
        self.work(id, false, move |shared| {
            let state_dir = shared
                .inboxes
                .dir(&root)
                .context("Corgi has no state directory")?;
            let lookup = match query {
                DigestQuery::Summary => {
                    return Ok(ReplyResult::Digest {
                        text: crate::app::digest_summary(&root, &state_dir, &agents),
                    });
                }
                DigestQuery::Search { words } => DigestLookup::Search(words),
                DigestQuery::Decision { words } => DigestLookup::Decision(words),
                DigestQuery::Show { handle } => DigestLookup::Show(handle),
                DigestQuery::Ledger { id } => DigestLookup::Ledger(id),
                DigestQuery::Brief { id } => DigestLookup::Brief(id),
            };
            Ok(ReplyResult::Digest {
                text: digest_lookup(&state_dir, &root, lookup)?,
            })
        });
    }
}

/// Types `text` into `pane_id`'s agent once. A failure never echoes the
/// text, which Herdr's reply can carry, and is never retried: it can follow
/// delivery.
fn deliver(shared: &Shared, pane_id: &str, text: &str) -> Result<()> {
    shared
        .client
        .prompt_agent(pane_id, text)
        .map(|_| ())
        .map_err(|error| {
            if HerdrError::is_agent_blocked(&error) {
                return anyhow!("Blocked agents must be handled in their own pane");
            }
            // Herdr's code says why without the text its message can carry.
            let code = HerdrError::code_of(&error)
                .map(|code| format!(" ({code})"))
                .unwrap_or_default();
            anyhow!("Could not confirm prompt delivery{code}. Check the agent before retrying.")
        })
}

/// Launches `plan` with its steps and progress sent for request `id`, and
/// returns the new agent's pane and where it runs.
fn launch(shared: &Shared, id: u64, plan: &LaunchPlan, remember: bool) -> Result<(String, String)> {
    started(shared, id, launch_steps(plan));
    let launched = launch_agent(&shared.client, plan, &mut Sink::new(shared, id, remember))?;
    Ok((launched.agent.pane_id, launched.location))
}

/// The models `harness` offers: its catalog, discovered again when it is
/// older than the dashboard trusts one and `discover` allows, else the
/// curated fallback.
fn model_options(shared: &Shared, harness: &Harness, discover: bool) -> Vec<String> {
    let stale = {
        let catalogs = lock(&shared.catalogs);
        catalogs
            .fetched_at
            .get(harness)
            .is_none_or(|at| at.elapsed() >= MODEL_CATALOG_TTL)
    };
    if discover && stale && harness.supports_model_discovery() {
        let models = harness
            .discover_models()
            .map_err(|error| format!("{error:#}"));
        // A failed discovery leaves the fallback in place.
        let _ = lock(&shared.catalogs).install(harness, models);
    }
    lock(&shared.catalogs).options(harness)
}

fn merge_running(shared: &Shared, pane_id: &str) -> bool {
    lock(&shared.merges)
        .get(pane_id)
        .is_some_and(|form| matches!(form.phase, MergePhase::Running(_)))
}

fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Sends the steps a long command takes, before its first.
fn started(shared: &Shared, id: u64, steps: Vec<String>) {
    let _ = shared.output.send(&ServerMessage::Progress(Progress {
        id,
        steps: Some(steps),
        ..Progress::default()
    }));
}

/// A launch's or merge's progress, sent for request `id`.
struct Sink<'a> {
    shared: &'a Shared,
    id: u64,
    /// Remember the project a launch resolves to, as the dashboard does.
    remember: bool,
}

impl<'a> Sink<'a> {
    fn new(shared: &'a Shared, id: u64, remember: bool) -> Self {
        Self {
            shared,
            id,
            remember,
        }
    }

    fn send(&self, progress: Progress) {
        let _ = self.shared.output.send(&ServerMessage::Progress(progress));
    }
}

impl progress::Progress for Sink<'_> {
    fn report(&mut self, message: String) {
        self.send(Progress {
            id: self.id,
            status: Some(message),
            ..Progress::default()
        });
    }

    fn step(&mut self, index: usize) {
        self.send(Progress {
            id: self.id,
            step: Some(index),
            ..Progress::default()
        });
    }

    fn project(&mut self, root: &str) {
        if self.remember {
            ProjectMemory::load().remember(root);
        }
    }
}

/// The selector entries as the protocol carries them.
fn wire(list: Vec<choices::Choice>) -> Vec<Choice> {
    list.into_iter()
        .map(|choice| Choice {
            value: choice.value,
            label: choice.label,
            detail: choice.detail,
            badge: choice.badge,
        })
        .collect()
}

/// The reply to request `id` from what it gave.
fn answered(id: u64, result: Result<ReplyResult>) -> Reply {
    match result {
        Ok(result) => Reply {
            id: Some(id),
            ok: true,
            error: None,
            field: None,
            result: Some(result),
        },
        Err(error) => refused(Some(id), format!("{error:#}")),
    }
}

fn refused(id: Option<u64>, error: String) -> Reply {
    Reply {
        id,
        ok: false,
        error: Some(error),
        field: None,
        result: None,
    }
}
