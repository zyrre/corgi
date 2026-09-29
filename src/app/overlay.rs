//! What the dashboard shows over its agent list: nothing, or exactly one form
//! or dialog. Keys go to it and it is drawn on top, so one value says both.

use crate::motion::Outcome;

use super::{
    close::CloseWorkspaceForm,
    form::{NewAgentForm, NewField},
    launch::{LaunchState, NewAgentLaunch},
    merge::{MergePhase, MergeWorktreeForm},
    prompt::PromptForm,
};

/// The one form or dialog open over the agent list, holding everything it
/// shows. The background job a launch or merge runs stays on [`App`](super::App):
/// dropping a job does not stop its worker, so a launch or a push that lost
/// its dialog would carry on with no one to report its outcome. Esc never
/// closes either while its job runs, so the two always end together.
#[derive(Debug, Default)]
pub(crate) enum Overlay {
    /// The agent list takes the keys.
    #[default]
    None,
    /// A prompt being typed for one agent.
    Prompt(PromptForm),
    /// The new-agent form, until it is submitted.
    NewAgent(NewAgentForm),
    /// A submitted agent starting, or the reason it did not. The form is
    /// gone by then; a failure keeps this panel open until Esc.
    Launch(NewAgentLaunch),
    /// The confirmation before an agent's workspace, tab or worktree closes.
    Close(CloseWorkspaceForm),
    /// A worktree merge from its confirmation to its outcome.
    Merge(MergeWorktreeForm),
}

impl Overlay {
    /// A merge dialog for `form`.
    pub(crate) fn merge(form: MergeWorktreeForm) -> Self {
        Self::Merge(form)
    }

    /// The outcome the overlay shows, which plays its effect when it
    /// arrives: a launch or merge that finished or failed.
    pub(crate) fn outcome(&self) -> Option<Outcome> {
        match self {
            Self::Launch(launch) => match launch.state {
                LaunchState::Running { .. } => None,
                LaunchState::Started { .. } => Some(Outcome::Success),
                LaunchState::Failed(_) => Some(Outcome::Failure),
            },
            Self::Merge(form) => match form.phase {
                MergePhase::Confirm | MergePhase::Running(_) => None,
                MergePhase::Succeeded => Some(Outcome::Success),
                MergePhase::Failed(_) => Some(Outcome::Failure),
            },
            _ => None,
        }
    }

    /// Whether the overlay shows a spinner, which turns on its own clock.
    pub(crate) fn spins(&self) -> bool {
        match self {
            Self::Launch(launch) => matches!(launch.state, LaunchState::Running { .. }),
            Self::Merge(form) => matches!(form.phase, MergePhase::Running(_)),
            _ => false,
        }
    }

    /// Whether the overlay has a focused text field, whose caret blinks.
    pub(crate) fn blinks(&self) -> bool {
        match self {
            Self::Prompt(_) => true,
            Self::NewAgent(form) => form.field == NewField::Task || form.list.is_some(),
            _ => false,
        }
    }

    pub(crate) fn prompt_form_mut(&mut self) -> Option<&mut PromptForm> {
        match self {
            Self::Prompt(form) => Some(form),
            _ => None,
        }
    }

    pub(super) fn new_agent_form(&self) -> Option<&NewAgentForm> {
        match self {
            Self::NewAgent(form) => Some(form),
            _ => None,
        }
    }

    pub(crate) fn new_agent_form_mut(&mut self) -> Option<&mut NewAgentForm> {
        match self {
            Self::NewAgent(form) => Some(form),
            _ => None,
        }
    }

    pub(super) fn new_agent_launch(&self) -> Option<&NewAgentLaunch> {
        match self {
            Self::Launch(launch) => Some(launch),
            _ => None,
        }
    }

    pub(super) fn new_agent_launch_mut(&mut self) -> Option<&mut NewAgentLaunch> {
        match self {
            Self::Launch(launch) => Some(launch),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(super) fn close_workspace_form(&self) -> Option<&CloseWorkspaceForm> {
        match self {
            Self::Close(form) => Some(form),
            _ => None,
        }
    }

    pub(super) fn merge_worktree_form(&self) -> Option<&MergeWorktreeForm> {
        match self {
            Self::Merge(form) => Some(form),
            _ => None,
        }
    }

    pub(crate) fn merge_worktree_form_mut(&mut self) -> Option<&mut MergeWorktreeForm> {
        match self {
            Self::Merge(form) => Some(form),
            _ => None,
        }
    }

    /// Closes an open prompt, handing back what was typed. Any other overlay
    /// stays open.
    pub(super) fn take_prompt_form(&mut self) -> Option<PromptForm> {
        match std::mem::take(self) {
            Self::Prompt(form) => Some(form),
            other => {
                *self = other;
                None
            }
        }
    }

    /// Closes an open close confirmation, handing back its form. Any other
    /// overlay stays open.
    pub(super) fn take_close_workspace_form(&mut self) -> Option<CloseWorkspaceForm> {
        match std::mem::take(self) {
            Self::Close(form) => Some(form),
            other => {
                *self = other;
                None
            }
        }
    }

    /// Closes an open merge dialog, handing back its form. Any other overlay
    /// stays open.
    pub(super) fn take_merge_worktree_form(&mut self) -> Option<MergeWorktreeForm> {
        match std::mem::take(self) {
            Self::Merge(form) => Some(form),
            other => {
                *self = other;
                None
            }
        }
    }
}
