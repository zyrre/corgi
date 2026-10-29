//! The dashboard's close dialog: its keys, and the close of
//! [`corgi_core::close`] once the user confirms.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent};

pub(crate) use corgi_core::close::*;

use super::{App, overlay::Overlay};

impl App {
    pub(super) fn begin_close_workspace(&mut self) {
        let Some(agent) = self.selected_agent() else {
            self.status = "No agent selected".into();
            return;
        };
        self.overlay = Overlay::Close(close_form(agent, &self.workspaces, &self.agents));
    }

    /// Closes the confirmed workspace. For a worktree agent this removes the
    /// checkout from disk too; a dirty checkout is refused once and removed on
    /// the next confirmation. The branch always stays.
    pub(super) fn close_workspace(&mut self) {
        let Some(form) = self.overlay.take_close_workspace_form() else {
            return;
        };
        let status = match close(&self.client, self.dashboard_pane_id.as_deref(), form) {
            Closed::Done(status) => {
                self.motion.succeeded();
                status
            }
            Closed::Dirty(form) => {
                self.overlay = Overlay::Close(form);
                "Checkout has uncommitted changes; Enter again discards them".into()
            }
            Closed::Failed(status) => status,
        };
        self.set_status(status, Some(Duration::from_secs(6)));
        self.request_refresh();
    }

    pub(super) fn handle_close_workspace_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => self.cancel_editor(),
            KeyCode::Enter => self.close_workspace(),
            _ => {}
        }
        false
    }
}
