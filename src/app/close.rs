//! Closing an agent: its worktree and checkout, its tab in a project
//! workspace, or its whole workspace, then retiring the project workspace
//! when nothing is left in it.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent};

use crate::herdr::HerdrError;

use super::{
    App,
    overlay::Overlay,
    project_main::{close_project_main_agent_tab, is_corgi_project_main, project_main_workspace},
};

#[derive(Debug, Clone)]
pub(crate) struct CloseWorkspaceForm {
    pub(super) workspace_id: String,
    pub(super) tab_id: String,
    pub(crate) label: String,
    /// The repository a Corgi-owned main workspace belongs to. `None` means
    /// this is an unrelated plain workspace and must not trigger cleanup.
    pub(super) project_root: Option<String>,
    pub(crate) target: CloseTarget,
}

/// What closing an agent takes down with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloseTarget {
    /// The agent lives in a linked worktree; closing removes the checkout at
    /// `checkout` from disk as well as the workspace.
    Worktree {
        checkout: String,
        /// Set after Herdr refused to remove a dirty checkout, so the next
        /// Enter discards uncommitted changes.
        force: bool,
    },
    /// A shared-checkout agent runs in its own tab of a Corgi project main.
    /// Closing that tab leaves other agents alone.
    Tab {
        /// The first agent of a new project uses the main tab itself. Its
        /// root tab is replaced before it closes, so later agents keep a
        /// project parent.
        root_main: bool,
    },
    /// Any other workspace, closed with everything in it.
    Workspace,
}

impl App {
    pub(super) fn begin_close_workspace(&mut self) {
        let Some(agent) = self.selected_agent() else {
            self.status = "No agent selected".into();
            return;
        };
        let project_main = project_main_workspace(
            &self.workspaces,
            &self.corgi_workspaces(),
            &agent.project_root,
        );
        let target = if let Some(checkout) = &agent.worktree_checkout {
            CloseTarget::Worktree {
                checkout: checkout.clone(),
                force: false,
            }
        } else if self
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == agent.info.workspace_id)
            .is_some_and(|workspace| is_corgi_project_main(workspace, &agent.project_root))
        {
            CloseTarget::Tab {
                root_main: project_main.as_ref().is_some_and(|main| {
                    main.workspace_id == agent.info.workspace_id
                        && main.root_tab_id == agent.info.tab_id
                }),
            }
        } else {
            CloseTarget::Workspace
        };
        self.overlay = Overlay::Close(CloseWorkspaceForm {
            workspace_id: agent.info.workspace_id.clone(),
            tab_id: agent.info.tab_id.clone(),
            label: agent.project.clone(),
            project_root: project_main.map(|main| main.project_root),
            target,
        });
    }

    /// Closes the confirmed workspace. For a worktree agent this removes the
    /// checkout from disk too; a dirty checkout is refused once and removed on
    /// the next confirmation. The branch always stays.
    pub(super) fn close_workspace(&mut self) {
        let Some(form) = self.overlay.take_close_workspace_form() else {
            return;
        };
        let status = match &form.target {
            CloseTarget::Worktree { checkout, force } => {
                match self.client.remove_worktree(&form.workspace_id, *force) {
                    Ok(_) => {
                        self.motion.succeeded();
                        self.closed_agent_status(&form, "Removed worktree and its checkout")
                    }
                    Err(error) if !force && HerdrError::is_dirty_worktree(&error) => {
                        let target = CloseTarget::Worktree {
                            checkout: checkout.clone(),
                            force: true,
                        };
                        self.overlay = Overlay::Close(CloseWorkspaceForm { target, ..form });
                        "Checkout has uncommitted changes; Enter again discards them".into()
                    }
                    Err(error) => format!("Remove failed: {error:#}"),
                }
            }
            CloseTarget::Tab { root_main } => {
                let closed = if *root_main {
                    close_project_main_agent_tab(&self.client, &form)
                } else {
                    self.client.close_tab(&form.tab_id)
                };
                match closed {
                    Ok(()) => {
                        self.motion.succeeded();
                        self.closed_agent_status(&form, "Closed agent tab")
                    }
                    Err(error) => format!("Close tab failed: {error:#}"),
                }
            }
            CloseTarget::Workspace => match self.client.close_workspace(&form.workspace_id) {
                Ok(()) => {
                    self.motion.succeeded();
                    format!("Closed workspace {}", form.label)
                }
                Err(error) => format!("Close failed: {error:#}"),
            },
        };
        self.set_status(status, Some(Duration::from_secs(6)));
        self.request_refresh();
    }

    /// Retire the agentless project main only after Corgi has deliberately
    /// removed the final worker. A stale, unowned workspace or a live agent
    /// always wins over cleanup.
    fn closed_agent_status(&self, form: &CloseWorkspaceForm, action: &str) -> String {
        let Some(project_root) = form.project_root.as_deref() else {
            return format!("{action} {}", form.label);
        };
        let duplicates: String = self
            .retire_duplicate_project_mains(project_root)
            .iter()
            .map(|label| format!("; closed duplicate project workspace {label}"))
            .collect();
        match self.retire_project_main_if_unused(project_root) {
            Ok(Some(label)) => format!(
                "{action} {}{duplicates}; closed project workspace {label}",
                form.label
            ),
            Ok(None) => format!("{action} {}{duplicates}", form.label),
            Err(error) => format!(
                "{action} {}{duplicates}; project workspace kept: {error:#}",
                form.label
            ),
        }
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
