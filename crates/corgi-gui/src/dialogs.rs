//! The window's dialogs and what each reply from `corgi serve` does to them:
//! the prompt, answer and backlog boxes, the new-agent form, the merge confirmation
//! with its outcome, and the close confirmation. Pure, so every flow is
//! tested without a window.

use std::time::Instant;

use corgi_core::protocol::{
    BacklogEntry, CloseForm, MergeForm, MergeOutcome, Progress, Question, Reply, ReplyResult,
    UpstreamChoice,
};

use crate::{
    form::{self, NewAgent},
    keymap::{ENTER, ESCAPE},
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
    /// Shows one of a supervisor's backlog entries whole, and writes the
    /// supervisor about it.
    Backlog {
        project_root: String,
        entry: BacklogEntry,
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
    /// The keys a prompt, answer or backlog box offers. A refused one stays
    /// open with its text and keeps the keyboard, so Esc, the way back to
    /// the overview's keys, comes first.
    pub fn text_hints(&self) -> &'static [(&'static str, &'static str)] {
        if self.refused() {
            &[(ESCAPE, "close"), (ENTER, "retry")]
        } else {
            &[(ENTER, "send"), (ESCAPE, "cancel")]
        }
    }

    /// Whether a prompt, answer or backlog box shows a refusal.
    pub fn refused(&self) -> bool {
        matches!(
            self,
            Dialog::Prompt { error: Some(_), .. }
                | Dialog::Answer { error: Some(_), .. }
                | Dialog::Backlog { error: Some(_), .. }
        )
    }

    /// Whether it is a box with a text field that sends what is typed.
    pub fn is_text(&self) -> bool {
        matches!(
            self,
            Dialog::Prompt { .. } | Dialog::Answer { .. } | Dialog::Backlog { .. }
        )
    }

    /// A text box's sending flag and the refusal it shows.
    pub fn text_state(&mut self) -> Option<(&mut bool, &mut Option<String>)> {
        match self {
            Dialog::Prompt { sending, error, .. }
            | Dialog::Answer { sending, error, .. }
            | Dialog::Backlog { sending, error, .. } => Some((sending, error)),
            _ => None,
        }
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
    /// Git refuses the merge before it starts, as `merge_check` found: a
    /// checkout with uncommitted changes, both on one branch. Nothing ran.
    Refused(String),
    /// The command itself was refused, or the connection was lost.
    Failed(String),
}

/// What a button in the merge dialog does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeAction {
    Run(UpstreamChoice),
    /// Ask the project's supervisor to fix the failure (`merge_handoff`),
    /// with the key [`crate::keymap::Action::AskSupervisor`] too.
    Handoff,
    Dismiss,
    /// Offer to close the merged agent.
    CloseAgent,
    Done,
}

/// The words of the button that asks the supervisor to fix a failure.
pub const ASK_SUPERVISOR: &str = "Ask supervisor to fix";

impl Merge {
    /// The buttons the dialog offers now, each with whether it can be
    /// pressed; the first that can is the default (Enter). Every failure
    /// offers to ask the supervisor to fix it, which cannot be pressed
    /// while none runs ([`Merge::no_supervisor`] says why).
    pub fn actions(&self) -> Vec<(&'static str, MergeAction, bool)> {
        let ask = (
            ASK_SUPERVISOR,
            MergeAction::Handoff,
            self.helper().is_some(),
        );
        let on = |label, action| (label, action, true);
        match &self.stage {
            MergeStage::Confirm => vec![
                on("Merge", MergeAction::Run(UpstreamChoice::Check)),
                on("Cancel", MergeAction::Done),
            ],
            MergeStage::Running => Vec::new(),
            MergeStage::Outcome(outcome) => match outcome {
                MergeOutcome::Succeeded { .. } => vec![
                    on("Close agent…", MergeAction::CloseAgent),
                    on("Done", MergeAction::Done),
                ],
                MergeOutcome::Behind { .. } => vec![
                    on("Pull, then merge", MergeAction::Run(UpstreamChoice::Pull)),
                    on("Cancel", MergeAction::Done),
                ],
                MergeOutcome::FetchFailed { .. } => vec![
                    on("Merge without it", MergeAction::Run(UpstreamChoice::Skip)),
                    ask,
                    on("Cancel", MergeAction::Done),
                ],
                MergeOutcome::Conflicted { .. }
                | MergeOutcome::PushFailed { .. }
                | MergeOutcome::Failed { .. } => {
                    vec![ask, on("Leave it to me", MergeAction::Dismiss)]
                }
            },
            MergeStage::Refused(_) => vec![ask, on("Leave it to me", MergeAction::Dismiss)],
            MergeStage::Failed(_) => vec![ask, on("Done", MergeAction::Done)],
        }
    }

    /// The button Enter presses: the first that can be pressed.
    pub fn default_action(&self) -> Option<MergeAction> {
        self.actions()
            .into_iter()
            .find(|(_, _, enabled)| *enabled)
            .map(|(_, action, _)| action)
    }

    /// Whether the dialog offers `action` now.
    pub fn offers(&self, action: MergeAction) -> bool {
        self.actions()
            .iter()
            .any(|(_, offered, enabled)| *offered == action && *enabled)
    }

    /// The running supervisor that can be asked to fix the failure shown.
    pub fn helper(&self) -> Option<&str> {
        match &self.stage {
            MergeStage::Outcome(
                MergeOutcome::Conflicted { supervisor, .. }
                | MergeOutcome::PushFailed { supervisor, .. }
                | MergeOutcome::FetchFailed { supervisor, .. }
                | MergeOutcome::Failed { supervisor, .. },
            ) => supervisor.as_deref(),
            MergeStage::Refused(_) | MergeStage::Failed(_) => self.form.supervisor.as_deref(),
            MergeStage::Confirm | MergeStage::Running | MergeStage::Outcome(_) => None,
        }
    }

    /// Why the supervisor cannot be asked to fix the failure shown, for a
    /// failure with no supervisor to ask.
    pub fn no_supervisor(&self) -> Option<&'static str> {
        let offered = self
            .actions()
            .iter()
            .any(|(_, action, _)| *action == MergeAction::Handoff);
        (offered && self.helper().is_none())
            .then_some("No supervisor is running for this project, so none can be asked to fix it.")
    }

    /// The error `merge_handoff` sends with the request, for a failure
    /// `corgi serve` may not have kept: its own refusal, or a lost
    /// connection. Serve sends its own words for the rest.
    pub fn handoff_error(&self) -> Option<String> {
        match &self.stage {
            MergeStage::Refused(error) | MergeStage::Failed(error) => Some(error.clone()),
            _ => None,
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
    /// Select this agent, so the side panel shows its project.
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

    /// What Esc does to the open dialog, and a click outside it too: a
    /// list open in the form closes first, a form creating its agent stays,
    /// and a running merge goes on with its outcome said when it ends. A
    /// stopped merge is left to the user through `corgi serve`, and its
    /// dialog closes on the reply: that action is returned, to send.
    pub fn escape(&mut self) -> Option<MergeAction> {
        match &mut self.open {
            None => {}
            Some(Dialog::NewAgent(new)) => {
                if new.key(form::Key::Escape) == form::Effect::Cancel {
                    self.open = None;
                }
            }
            Some(Dialog::Merge(merge)) if merge.offers(MergeAction::Dismiss) => {
                return Some(MergeAction::Dismiss);
            }
            Some(_) => self.open = None,
        }
        None
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
            Purpose::Prompt | Purpose::Answer | Purpose::BacklogMessage => {
                match (error, self.open.as_mut().and_then(Dialog::text_state)) {
                    (Some(error), Some((sending, shown))) => {
                        // The box stays open with its text, to retry by hand.
                        *sending = false;
                        *shown = Some(error);
                    }
                    (Some(error), None) => self.say(error, true),
                    (None, _) => {
                        if self.open.as_ref().is_some_and(Dialog::is_text) {
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
                        // Refused before it starts: the dialog says why, and
                        // offers to ask the supervisor to fix it.
                        let stage = match &form.refused {
                            Some(error) => MergeStage::Refused(error.clone()),
                            None => MergeStage::Confirm,
                        };
                        self.open = Some(Dialog::Merge(Box::new(Merge {
                            progress: Steps {
                                steps: form.steps.clone(),
                                ..Steps::default()
                            },
                            form: *form,
                            stage,
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
            Purpose::MergeEnd | Purpose::Tag | Purpose::Resume => {
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
                (Purpose::Prompt | Purpose::Answer | Purpose::BacklogMessage, Some(dialog))
                    if dialog.is_text() =>
                {
                    if let Some((sending, error)) = dialog.text_state() {
                        *sending = false;
                        *error = Some(why.into());
                    }
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
        MergeOutcome::FetchFailed { error, .. } => {
            format!("Fetching the upstream failed: {error}")
        }
        MergeOutcome::Failed { error, .. } => format!("The merge failed: {error}"),
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
    fn a_backlog_message_closes_when_sent_and_keeps_its_box_when_refused_or_lost() {
        let open = || Dialog::Backlog {
            project_root: "/repos/weather".into(),
            entry: BacklogEntry {
                heading: "Stop all agents".into(),
                text: "For when the user leaves.".into(),
            },
            sending: true,
            error: None,
        };
        let mut dialogs = Dialogs {
            open: Some(open()),
            ..Dialogs::default()
        };
        assert_eq!(
            dialogs.open.as_ref().unwrap().text_hints()[0],
            ("↵", "send")
        );
        dialogs.reply(
            Purpose::BacklogMessage,
            refused("The message cannot be empty", None),
        );
        let shown = dialogs.open.as_ref().unwrap();
        assert!(shown.refused());
        assert_eq!(shown.text_hints()[0], ("Esc", "close"));
        assert!(matches!(shown, Dialog::Backlog { sending: false, .. }));

        dialogs.open = Some(open());
        dialogs.lost(&[Purpose::BacklogMessage]);
        assert!(matches!(
            &dialogs.open,
            Some(Dialog::Backlog { sending: false, error: Some(error), .. }) if error.contains("lost")
        ));

        dialogs.open = Some(open());
        dialogs.reply(Purpose::BacklogMessage, done("Sent to supervisor-weather"));
        assert_eq!(dialogs.open, None);
        assert_eq!(
            dialogs.flash.as_ref().map(|flash| flash.text.as_str()),
            Some("Sent to supervisor-weather")
        );
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
        let actions: Vec<_> = merge(&dialogs).actions();
        assert_eq!(
            actions,
            [
                (ASK_SUPERVISOR, MergeAction::Handoff, false),
                ("Leave it to me", MergeAction::Dismiss, true)
            ],
            "no supervisor to hand it to"
        );
        assert_eq!(merge(&dialogs).default_action(), Some(MergeAction::Dismiss));
        assert!(merge(&dialogs).no_supervisor().is_some());
        dialogs.reply(Purpose::MergeEnd, done("Left the merge to resolve by hand"));
        assert_eq!(dialogs.open, None);
    }

    #[test]
    fn every_merge_failure_offers_to_ask_the_supervisor_to_fix_it() {
        let supervisor = || Some("supervisor-weather".to_string());
        let with = |stage: MergeStage, supervisor: Option<String>| Merge {
            form: MergeForm {
                pane_id: "w2:p1".into(),
                supervisor,
                ..MergeForm::default()
            },
            stage,
            progress: Steps::default(),
        };
        let outcome = |outcome| MergeStage::Outcome(outcome);
        let failures = [
            outcome(MergeOutcome::Conflicted {
                files: vec!["a.rs".into()],
                supervisor: supervisor(),
            }),
            outcome(MergeOutcome::PushFailed {
                commit: "abc".into(),
                error: "rejected".into(),
                supervisor: supervisor(),
            }),
            outcome(MergeOutcome::FetchFailed {
                error: "Could not resolve host".into(),
                supervisor: supervisor(),
            }),
            outcome(MergeOutcome::Failed {
                error: "git merge: fatal".into(),
                supervisor: supervisor(),
            }),
            MergeStage::Refused("primary checkout has uncommitted changes: M AGENTS.md".into()),
            MergeStage::Failed("The connection to corgi serve was lost".into()),
        ];
        for stage in failures {
            let merge = with(stage.clone(), supervisor());
            assert!(merge.offers(MergeAction::Handoff), "{stage:?}");
            assert_eq!(merge.helper(), Some("supervisor-weather"), "{stage:?}");
            assert_eq!(merge.no_supervisor(), None, "{stage:?}");
            // Without a supervisor, it is shown greyed with the reason.
            let mut alone = with(stage.clone(), None);
            if let MergeStage::Outcome(
                MergeOutcome::Conflicted { supervisor, .. }
                | MergeOutcome::PushFailed { supervisor, .. }
                | MergeOutcome::FetchFailed { supervisor, .. }
                | MergeOutcome::Failed { supervisor, .. },
            ) = &mut alone.stage
            {
                *supervisor = None;
            }
            assert!(!alone.offers(MergeAction::Handoff), "{stage:?}");
            assert!(
                alone
                    .actions()
                    .contains(&(ASK_SUPERVISOR, MergeAction::Handoff, false)),
                "{stage:?}"
            );
            assert!(alone.no_supervisor().is_some(), "{stage:?}");
        }
        // Nothing to fix while it confirms, runs, succeeded, or waits on a
        // pull.
        for stage in [
            MergeStage::Confirm,
            MergeStage::Running,
            outcome(MergeOutcome::Behind {
                commits: vec!["abc".into()],
            }),
            outcome(MergeOutcome::Succeeded {
                commit: "abc".into(),
                message: "Merged".into(),
            }),
        ] {
            let merge = with(stage.clone(), supervisor());
            assert!(!merge.offers(MergeAction::Handoff), "{stage:?}");
            assert_eq!(merge.no_supervisor(), None, "{stage:?}");
        }
        // The window's own errors go with the request; serve keeps the rest.
        let failed = with(MergeStage::Failed("lost".into()), supervisor());
        assert_eq!(failed.handoff_error().as_deref(), Some("lost"));
        let fetch = with(
            outcome(MergeOutcome::FetchFailed {
                error: "x".into(),
                supervisor: supervisor(),
            }),
            supervisor(),
        );
        assert_eq!(fetch.handoff_error(), None);
        assert_eq!(
            fetch.default_action(),
            Some(MergeAction::Run(UpstreamChoice::Skip))
        );
    }

    #[test]
    fn a_merge_git_refuses_opens_saying_why_and_esc_leaves_it() {
        let mut dialogs = Dialogs::default();
        let error = "primary checkout has uncommitted or untracked changes: M AGENTS.md";
        let form = MergeForm {
            pane_id: "w2:p1".into(),
            refused: Some(error.into()),
            supervisor: Some("supervisor-weather".into()),
            ..MergeForm::default()
        };
        dialogs.reply(
            Purpose::MergeCheck,
            ok(ReplyResult::MergeForm(Box::new(form))),
        );
        let Some(Dialog::Merge(merge)) = &dialogs.open else {
            panic!("{:?}", dialogs.open)
        };
        assert_eq!(merge.stage, MergeStage::Refused(error.into()));
        assert_eq!(merge.default_action(), Some(MergeAction::Handoff));
        assert_eq!(merge.handoff_error().as_deref(), Some(error));
        assert_eq!(dialogs.flash, None, "the dialog says it");
        // Esc leaves it to the user, through serve, which forgets it.
        assert_eq!(dialogs.escape(), Some(MergeAction::Dismiss));
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
    fn esc_closes_one_level_and_leaves_what_cannot_be_cancelled() {
        let mut dialogs = Dialogs {
            open: Some(prompt_dialog()),
            ..Dialogs::default()
        };
        assert_eq!(dialogs.escape(), None);
        assert_eq!(dialogs.open, None);
        // Nothing open: nothing happens.
        assert_eq!(dialogs.escape(), None);

        // The form's list closes first, then the form.
        let mut new = NewAgent::new(NewAgentForm::default());
        new.key(form::Key::Down);
        new.key(form::Key::Space);
        assert!(new.picker.is_some());
        dialogs.open = Some(Dialog::NewAgent(Box::new(new)));
        dialogs.escape();
        assert!(matches!(&dialogs.open, Some(Dialog::NewAgent(new)) if new.picker.is_none()));
        dialogs.escape();
        assert_eq!(dialogs.open, None);

        // A form creating its agent cannot be cancelled.
        let mut new = NewAgent::new(NewAgentForm::default());
        new.running = Some(Steps::default());
        dialogs.open = Some(Dialog::NewAgent(Box::new(new)));
        dialogs.escape();
        assert!(matches!(dialogs.open, Some(Dialog::NewAgent(_))));

        dialogs.open = Some(Dialog::NewAgentLoading);
        dialogs.escape();
        assert_eq!(dialogs.open, None);

        // A merge closes, running or not, unless it stopped for the user:
        // then serve is told, and the dialog waits for its reply.
        let merge = |stage| {
            Some(Dialog::Merge(Box::new(Merge {
                form: MergeForm::default(),
                stage,
                progress: Steps::default(),
            })))
        };
        for stage in [MergeStage::Confirm, MergeStage::Running] {
            dialogs.open = merge(stage);
            assert_eq!(dialogs.escape(), None);
            assert_eq!(dialogs.open, None);
        }
        dialogs.open = merge(MergeStage::Outcome(MergeOutcome::Conflicted {
            files: vec!["a.rs".into()],
            supervisor: None,
        }));
        assert_eq!(dialogs.escape(), Some(MergeAction::Dismiss));
        assert!(matches!(dialogs.open, Some(Dialog::Merge(_))));

        dialogs.open = Some(Dialog::Close(Close {
            form: CloseForm::default(),
            dirty: None,
            running: true,
            error: None,
        }));
        dialogs.escape();
        assert_eq!(dialogs.open, None);
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
