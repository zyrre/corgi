//! The window's dialogs and what each reply from `corgi serve` does to them:
//! the prompt and answer boxes, the new-agent form, the merge confirmation
//! with its outcome, and the close confirmation. Pure, so every flow is
//! tested without a window.

use std::time::Instant;

use corgi_core::protocol::{
    CloseForm, MergeForm, MergeOutcome, Progress, Question, Reply, ReplyResult, UpstreamChoice,
};

use crate::{
    form::{self, NewAgent},
    store::Purpose,
};

/// A long command's steps, as its progress tells them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Steps {
    pub steps: Vec<String>,
    pub step: Option<usize>,
    pub status: Option<String>,
}

impl Steps {
    pub fn update(&mut self, progress: &Progress) {
        if let Some(steps) = &progress.steps {
            self.steps = steps.clone();
        }
        if progress.step.is_some() {
            self.step = progress.step;
        }
        if progress.status.is_some() {
            self.status = progress.status.clone();
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Dialog {
    /// Types a prompt into an agent.
    Prompt {
        pane_id: String,
        agent: String,
        sending: bool,
        error: Option<String>,
    },
    /// Answers one of a supervisor's numbered questions.
    Answer {
        project_root: String,
        question: Question,
        sending: bool,
        error: Option<String>,
    },
    NewAgent(Box<NewAgent>),
    /// The new-agent form, asked for and not here yet: the first one waits
    /// on the harnesses' model lists, which can take seconds.
    NewAgentLoading,
    Merge(Box<Merge>),
    Close(Close),
}

impl Dialog {
    /// The keys a prompt or answer box offers. A refused one stays open
    /// with its text and keeps the keyboard, so Esc, the way back to the
    /// overview's keys, comes first.
    pub fn text_hints(&self) -> &'static [(&'static str, &'static str)] {
        match self {
            Dialog::Prompt { error: Some(_), .. } | Dialog::Answer { error: Some(_), .. } => {
                &[("Esc", "close"), ("↵", "retry")]
            }
            _ => &[("↵", "send"), ("Esc", "cancel")],
        }
    }

    /// Whether a prompt or answer box shows a refusal.
    pub fn refused(&self) -> bool {
        matches!(
            self,
            Dialog::Prompt { error: Some(_), .. } | Dialog::Answer { error: Some(_), .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Merge {
    pub form: MergeForm,
    pub stage: MergeStage,
    pub progress: Steps,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MergeStage {
    Confirm,
    Running,
    Outcome(MergeOutcome),
    /// The command itself was refused, or the connection was lost.
    Failed(String),
}

/// What a button in the merge dialog does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeAction {
    Run(UpstreamChoice),
    Handoff,
    Dismiss,
    /// Offer to close the merged agent.
    CloseAgent,
    Done,
}

impl Merge {
    /// The buttons the dialog offers now, the default first (Enter).
    pub fn actions(&self) -> Vec<(&'static str, MergeAction)> {
        match &self.stage {
            MergeStage::Confirm => vec![
                ("Merge", MergeAction::Run(UpstreamChoice::Check)),
                ("Cancel", MergeAction::Done),
            ],
            MergeStage::Running => Vec::new(),
            MergeStage::Outcome(outcome) => match outcome {
                MergeOutcome::Succeeded { .. } => vec![
                    ("Close agent…", MergeAction::CloseAgent),
                    ("Done", MergeAction::Done),
                ],
                MergeOutcome::Behind { .. } => vec![
                    ("Pull, then merge", MergeAction::Run(UpstreamChoice::Pull)),
                    ("Cancel", MergeAction::Done),
                ],
                MergeOutcome::FetchFailed { .. } => vec![
                    ("Merge without it", MergeAction::Run(UpstreamChoice::Skip)),
                    ("Cancel", MergeAction::Done),
                ],
                MergeOutcome::Conflicted { supervisor, .. }
                | MergeOutcome::PushFailed { supervisor, .. } => {
                    let mut actions = Vec::new();
                    if supervisor.is_some() {
                        actions.push(("Hand to supervisor", MergeAction::Handoff));
                    }
                    actions.push(("Leave it to me", MergeAction::Dismiss));
                    actions
                }
                MergeOutcome::Failed { .. } => vec![("Done", MergeAction::Done)],
            },
            MergeStage::Failed(_) => vec![("Done", MergeAction::Done)],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Close {
    pub form: CloseForm,
    /// A worktree with uncommitted changes: confirm again to discard them.
    pub dirty: Option<String>,
    pub running: bool,
    pub error: Option<String>,
}

/// A line in the status bar saying what a command did.
#[derive(Debug, Clone, PartialEq)]
pub struct Flash {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

impl Flash {
    pub fn new(text: impl Into<String>, error: bool) -> Self {
        Self {
            text: text.into(),
            error,
            at: Instant::now(),
        }
    }
}

/// What the window does after a reply, beyond what the dialog shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Followup {
    /// Select this agent and show its project's supervisor panel.
    ShowSupervisor(String),
    /// The new-agent form changed rows: move the keyboard.
    Form(form::Effect),
}

/// The dialogs and the status line's flash.
#[derive(Debug, Default)]
pub struct Dialogs {
    pub open: Option<Dialog>,
    pub flash: Option<Flash>,
}

impl Dialogs {
    pub fn say(&mut self, text: impl Into<String>, error: bool) {
        self.flash = Some(Flash::new(text, error));
    }

    /// Shows a long command's progress in its dialog.
    pub fn progress(&mut self, purpose: &Purpose, progress: &Progress) {
        match (&mut self.open, purpose) {
            (Some(Dialog::Merge(merge)), Purpose::Merge) => merge.progress.update(progress),
            (Some(Dialog::NewAgent(new)), Purpose::Spawn) => new
                .running
                .get_or_insert_with(Steps::default)
                .update(progress),
            (_, Purpose::StartSupervisor) => {
                if let Some(status) = &progress.status {
                    self.say(status.clone(), false);
                }
            }
            _ => {}
        }
    }

    /// What `reply` to a request sent for `purpose` does.
    pub fn reply(&mut self, purpose: Purpose, reply: Reply) -> Option<Followup> {
        let error = (!reply.ok).then(|| reply.error.clone().unwrap_or_default());
        let message = match &reply.result {
            Some(ReplyResult::Done { message }) => Some(message.clone()),
            _ => None,
        };
        match purpose {
            Purpose::Quiet => {
                if let Some(error) = error {
                    self.say(error, true);
                }
                None
            }
            // No dialog: the card shows it closing until the ledger drops it.
            Purpose::CloseQuestion { .. } => {
                match error {
                    Some(error) => self.say(error, true),
                    None => self.say(message.unwrap_or_else(|| "Closing".into()), false),
                }
                None
            }
            Purpose::Prompt | Purpose::Answer => {
                match (error, &mut self.open) {
                    (
                        Some(error),
                        Some(Dialog::Prompt {
                            sending,
                            error: shown,
                            ..
                        }),
                    )
                    | (
                        Some(error),
                        Some(Dialog::Answer {
                            sending,
                            error: shown,
                            ..
                        }),
                    ) => {
                        // The box stays open with its text, to retry by hand.
                        *sending = false;
                        *shown = Some(error);
                    }
                    (Some(error), _) => self.say(error, true),
                    (None, _) => {
                        if matches!(
                            self.open,
                            Some(Dialog::Prompt { .. } | Dialog::Answer { .. })
                        ) {
                            self.open = None;
                        }
                        self.say(message.unwrap_or_else(|| "Sent".into()), false);
                    }
                }
                None
            }
            Purpose::NewAgentForm => {
                match (error, reply.result) {
                    (Some(error), _) => {
                        if matches!(self.open, Some(Dialog::NewAgentLoading)) {
                            self.open = None;
                        }
                        self.say(error, true);
                    }
                    (None, Some(ReplyResult::NewAgentForm(form))) => match &mut self.open {
                        Some(Dialog::NewAgent(new)) => new.reloaded(*form),
                        Some(Dialog::NewAgentLoading) => {
                            self.open = Some(Dialog::NewAgent(Box::new(NewAgent::new(*form))));
                        }
                        // Closed while it loaded: the user has moved on.
                        _ => {}
                    },
                    _ => {}
                }
                None
            }
            Purpose::Spawn => match (error, &mut self.open) {
                (Some(error), Some(Dialog::NewAgent(new))) => {
                    Some(Followup::Form(new.refused(error, reply.field.as_deref())))
                }
                (Some(error), _) => {
                    self.say(error, true);
                    None
                }
                (None, _) => {
                    if matches!(self.open, Some(Dialog::NewAgent(_))) {
                        self.open = None;
                    }
                    self.say(message.unwrap_or_else(|| "Agent started".into()), false);
                    None
                }
            },
            Purpose::StartSupervisor => match (error, reply.result) {
                (Some(error), _) => {
                    self.say(error, true);
                    None
                }
                (
                    None,
                    Some(ReplyResult::Supervisor {
                        pane_id, message, ..
                    }),
                ) => {
                    self.say(message, false);
                    pane_id.map(Followup::ShowSupervisor)
                }
                _ => None,
            },
            Purpose::MergeCheck => {
                match (error, reply.result) {
                    (Some(error), _) => self.say(error, true),
                    (None, Some(ReplyResult::MergeForm(form))) => {
                        self.open = Some(Dialog::Merge(Box::new(Merge {
                            progress: Steps {
                                steps: form.steps.clone(),
                                ..Steps::default()
                            },
                            form: *form,
                            stage: MergeStage::Confirm,
                        })));
                    }
                    _ => {}
                }
                None
            }
            Purpose::Merge => {
                let Some(Dialog::Merge(merge)) = &mut self.open else {
                    // The dialog was closed while it ran: say how it ended.
                    match (error, reply.result) {
                        (Some(error), _) => self.say(error, true),
                        (None, Some(ReplyResult::MergeOutcome(outcome))) => {
                            let failed = !matches!(outcome, MergeOutcome::Succeeded { .. });
                            self.say(outcome_line(&outcome), failed);
                        }
                        _ => {}
                    }
                    return None;
                };
                merge.stage = match (error, reply.result) {
                    (Some(error), _) => MergeStage::Failed(error),
                    (None, Some(ReplyResult::MergeOutcome(outcome))) => {
                        MergeStage::Outcome(outcome)
                    }
                    _ => MergeStage::Failed("corgi serve sent no outcome".into()),
                };
                None
            }
            Purpose::MergeEnd | Purpose::Tag => {
                if matches!(self.open, Some(Dialog::Merge(_))) && purpose == Purpose::MergeEnd {
                    self.open = None;
                }
                match error {
                    Some(error) => self.say(error, true),
                    None => self.say(message.unwrap_or_else(|| "Done".into()), false),
                }
                None
            }
            Purpose::CloseCheck => {
                match (error, reply.result) {
                    (Some(error), _) => self.say(error, true),
                    (None, Some(ReplyResult::CloseForm(form))) => {
                        self.open = Some(Dialog::Close(Close {
                            form,
                            dirty: None,
                            running: false,
                            error: None,
                        }));
                    }
                    _ => {}
                }
                None
            }
            Purpose::Close => {
                match (error, reply.result, &mut self.open) {
                    (Some(error), _, Some(Dialog::Close(close))) => {
                        close.running = false;
                        close.error = Some(error);
                    }
                    (Some(error), _, _) => self.say(error, true),
                    (
                        None,
                        Some(ReplyResult::CloseDirty { message }),
                        Some(Dialog::Close(close)),
                    ) => {
                        close.running = false;
                        close.dirty = Some(message);
                    }
                    (None, _, _) => {
                        if matches!(self.open, Some(Dialog::Close(_))) {
                            self.open = None;
                        }
                        self.say(message.unwrap_or_else(|| "Closed".into()), false);
                    }
                }
                None
            }
        }
    }

    /// The connection ended with these requests unanswered.
    pub fn lost(&mut self, purposes: &[Purpose]) {
        let why = "The connection to corgi serve was lost before it replied. Check the result before trying again.";
        for purpose in purposes {
            match (purpose, &mut self.open) {
                (Purpose::Merge, Some(Dialog::Merge(merge))) => {
                    merge.stage = MergeStage::Failed(why.into());
                }
                (Purpose::Spawn, Some(Dialog::NewAgent(new))) => {
                    new.refused(why.into(), None);
                }
                (
                    Purpose::Prompt | Purpose::Answer,
                    Some(Dialog::Prompt { sending, error, .. }),
                )
                | (
                    Purpose::Prompt | Purpose::Answer,
                    Some(Dialog::Answer { sending, error, .. }),
                ) => {
                    *sending = false;
                    *error = Some(why.into());
                }
                (Purpose::Close, Some(Dialog::Close(close))) => {
                    close.running = false;
                    close.error = Some(why.into());
                }
                (Purpose::NewAgentForm, Some(Dialog::NewAgentLoading)) => {
                    self.open = None;
                    self.say(why, true);
                }
                (Purpose::Quiet, _) => {}
                _ => self.say(why, true),
            }
        }
    }
}

/// A merge's outcome in one line.
pub fn outcome_line(outcome: &MergeOutcome) -> String {
    match outcome {
        MergeOutcome::Succeeded { message, .. } => message.clone(),
        MergeOutcome::PushFailed { error, .. } => {
            format!("Merged locally; the push failed: {error}")
        }
        MergeOutcome::Conflicted { files, .. } => {
            format!("Merge stopped on conflicts in {}", files.join(", "))
        }
        MergeOutcome::Behind { commits } => format!(
            "The upstream has {} commit{} the target lacks; nothing merged",
            commits.len(),
            if commits.len() == 1 { "" } else { "s" }
        ),
        MergeOutcome::FetchFailed { error } => format!("Fetching the upstream failed: {error}"),
        MergeOutcome::Failed { error } => format!("The merge failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use corgi_core::protocol::NewAgentForm;

    use super::*;

    fn ok(result: ReplyResult) -> Reply {
        Reply {
            id: Some(1),
            ok: true,
            error: None,
            field: None,
            result: Some(result),
        }
    }

    fn refused(error: &str, field: Option<&str>) -> Reply {
        Reply {
            id: Some(1),
            ok: false,
            error: Some(error.into()),
            field: field.map(String::from),
            result: None,
        }
    }

    fn done(message: &str) -> Reply {
        ok(ReplyResult::Done {
            message: message.into(),
        })
    }

    fn prompt_dialog() -> Dialog {
        Dialog::Prompt {
            pane_id: "w2:p1".into(),
            agent: "w-fix".into(),
            sending: true,
            error: None,
        }
    }

    #[test]
    fn a_prompt_closes_when_sent_and_stays_with_its_error_when_not() {
        let mut dialogs = Dialogs {
            open: Some(prompt_dialog()),
            ..Dialogs::default()
        };
        let refusal =
            "Could not confirm prompt delivery (agent_not_ready). Check the agent before retrying.";
        assert_eq!(dialogs.open.as_ref().unwrap().text_hints()[0].0, "↵");
        dialogs.reply(Purpose::Prompt, refused(refusal, None));
        // Still open, and saying that Esc is the way out of it.
        let open = dialogs.open.as_ref().unwrap();
        assert!(open.refused());
        assert_eq!(open.text_hints()[0], ("Esc", "close"));
        assert_eq!(
            dialogs.open,
            Some(Dialog::Prompt {
                pane_id: "w2:p1".into(),
                agent: "w-fix".into(),
                sending: false,
                error: Some(refusal.into()),
            })
        );
        dialogs.reply(Purpose::Prompt, done("Prompt sent to w-fix"));
        assert_eq!(dialogs.open, None);
        assert_eq!(dialogs.flash.as_ref().unwrap().text, "Prompt sent to w-fix");
    }

    #[test]
    fn the_form_shows_loading_at_once_and_a_cancelled_one_stays_closed() {
        let form = || ok(ReplyResult::NewAgentForm(Box::default()));
        let mut dialogs = Dialogs {
            open: Some(Dialog::NewAgentLoading),
            ..Dialogs::default()
        };
        dialogs.reply(Purpose::NewAgentForm, form());
        assert!(matches!(dialogs.open, Some(Dialog::NewAgent(_))));
        // Esc while it loaded: the late form does not pop up.
        dialogs.open = None;
        dialogs.reply(Purpose::NewAgentForm, form());
        assert_eq!(dialogs.open, None);
        // A refusal or a lost connection closes the loading box and is said.
        dialogs.open = Some(Dialog::NewAgentLoading);
        dialogs.reply(Purpose::NewAgentForm, refused("No harness installed", None));
        assert_eq!(dialogs.open, None);
        assert_eq!(dialogs.flash.as_ref().unwrap().text, "No harness installed");
        dialogs.open = Some(Dialog::NewAgentLoading);
        dialogs.lost(&[Purpose::NewAgentForm]);
        assert_eq!(dialogs.open, None);
        assert!(dialogs.flash.as_ref().unwrap().error);
    }

    #[test]
    fn the_form_opens_reloads_and_a_refused_spawn_points_at_its_row() {
        let mut dialogs = Dialogs {
            open: Some(Dialog::NewAgentLoading),
            ..Dialogs::default()
        };
        let form = NewAgentForm {
            project: "/r".into(),
            harness: "claude".into(),
            ..NewAgentForm::default()
        };
        dialogs.reply(
            Purpose::NewAgentForm,
            ok(ReplyResult::NewAgentForm(Box::new(form.clone()))),
        );
        let Some(Dialog::NewAgent(new)) = &mut dialogs.open else {
            panic!("no form")
        };
        new.project = "/other".into();
        let codex = NewAgentForm {
            harness: "codex".into(),
            ..form
        };
        dialogs.reply(
            Purpose::NewAgentForm,
            ok(ReplyResult::NewAgentForm(Box::new(codex))),
        );
        let Some(Dialog::NewAgent(new)) = &dialogs.open else {
            panic!("no form")
        };
        assert_eq!(
            (new.harness.as_str(), new.project.as_str()),
            ("codex", "/other")
        );
        let followup = dialogs.reply(
            Purpose::Spawn,
            refused(
                "Describe the first task before creating the agent",
                Some("task"),
            ),
        );
        assert_eq!(followup, Some(Followup::Form(form::Effect::None)));
        assert!(matches!(&dialogs.open, Some(Dialog::NewAgent(new)) if new.error.is_some()));
        dialogs.reply(Purpose::Spawn, done("weather started"));
        assert_eq!(dialogs.open, None);
    }

    #[test]
    fn a_merge_runs_with_progress_and_offers_what_its_outcome_allows() {
        let mut dialogs = Dialogs::default();
        let form = MergeForm {
            pane_id: "w2:p1".into(),
            steps: vec!["Check".into(), "Merge".into()],
            ..MergeForm::default()
        };
        dialogs.reply(
            Purpose::MergeCheck,
            ok(ReplyResult::MergeForm(Box::new(form))),
        );
        let merge = |dialogs: &Dialogs| match &dialogs.open {
            Some(Dialog::Merge(merge)) => (**merge).clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            merge(&dialogs).actions()[0].1,
            MergeAction::Run(UpstreamChoice::Check)
        );
        if let Some(Dialog::Merge(merge)) = &mut dialogs.open {
            merge.stage = MergeStage::Running;
        }
        dialogs.progress(
            &Purpose::Merge,
            &Progress {
                id: 1,
                step: Some(1),
                ..Progress::default()
            },
        );
        assert_eq!(merge(&dialogs).progress.step, Some(1));
        assert!(merge(&dialogs).actions().is_empty());
        dialogs.reply(
            Purpose::Merge,
            ok(ReplyResult::MergeOutcome(MergeOutcome::Behind {
                commits: vec!["abc Upstream".into()],
            })),
        );
        assert_eq!(
            merge(&dialogs).actions()[0].1,
            MergeAction::Run(UpstreamChoice::Pull)
        );
        dialogs.reply(
            Purpose::Merge,
            ok(ReplyResult::MergeOutcome(MergeOutcome::Conflicted {
                files: vec!["a.rs".into()],
                supervisor: None,
            })),
        );
        let actions: Vec<_> = merge(&dialogs).actions().into_iter().map(|a| a.1).collect();
        assert_eq!(
            actions,
            [MergeAction::Dismiss],
            "no supervisor to hand it to"
        );
        dialogs.reply(Purpose::MergeEnd, done("Left the merge to resolve by hand"));
        assert_eq!(dialogs.open, None);
    }

    #[test]
    fn closing_a_dirty_worktree_asks_again_and_a_lost_connection_is_said() {
        let mut dialogs = Dialogs::default();
        let form = CloseForm {
            pane_id: "w2:p1".into(),
            target: "worktree".into(),
            confirm: "Remove worktree".into(),
            ..CloseForm::default()
        };
        dialogs.reply(Purpose::CloseCheck, ok(ReplyResult::CloseForm(form)));
        dialogs.reply(
            Purpose::Close,
            ok(ReplyResult::CloseDirty {
                message: "weather-fix has uncommitted changes".into(),
            }),
        );
        assert!(matches!(&dialogs.open, Some(Dialog::Close(close)) if close.dirty.is_some()));
        dialogs.lost(&[Purpose::Close, Purpose::Quiet]);
        assert!(matches!(&dialogs.open, Some(Dialog::Close(close)) if close.error.is_some()));
    }

    #[test]
    fn a_running_supervisor_is_shown_and_a_failure_is_said() {
        let mut dialogs = Dialogs::default();
        let followup = dialogs.reply(
            Purpose::StartSupervisor,
            ok(ReplyResult::Supervisor {
                pane_id: Some("w1:p1".into()),
                started: false,
                message: "weather's supervisor is running".into(),
            }),
        );
        assert_eq!(followup, Some(Followup::ShowSupervisor("w1:p1".into())));
        dialogs.reply(Purpose::Tag, refused("w-fix is working", None));
        assert!(dialogs.flash.as_ref().unwrap().error);
    }
}
