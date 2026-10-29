//! Closing an agent: its worktree and checkout, its tab in a project
//! workspace, or its whole workspace, then retiring the project workspace
//! when nothing is left in it.

use anyhow::Result;

use crate::{
    herdr::{HerdrClient, HerdrError},
    model::{DashboardAgent, WorkspaceInfo},
    project_main::{
        close_project_main_agent_tab, is_corgi_project_main, project_main_workspace,
        retire_duplicate_project_mains, retire_project_main_if_unused,
    },
};

#[derive(Debug, Clone)]
pub struct CloseWorkspaceForm {
    pub workspace_id: String,
    pub tab_id: String,
    pub label: String,
    /// The repository a Corgi-owned main workspace belongs to. `None` means
    /// this is an unrelated plain workspace and must not trigger cleanup.
    pub project_root: Option<String>,
    pub target: CloseTarget,
}

/// What closing an agent takes down with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseTarget {
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

/// What closing `agent` takes down, as the confirmation asks: its worktree,
/// its tab in a Corgi project workspace, or its whole workspace. `workspaces`
/// and `agents` are the dashboard's last snapshot, for the project workspace.
pub fn close_form(
    agent: &DashboardAgent,
    workspaces: &[WorkspaceInfo],
    agents: &[DashboardAgent],
) -> CloseWorkspaceForm {
    let supervisors = dashboard_supervisor_workspaces(agents);
    let project_main = project_main_workspace(workspaces, &supervisors, &agent.project_root);
    let target = if let Some(checkout) = &agent.worktree_checkout {
        CloseTarget::Worktree {
            checkout: checkout.clone(),
            force: false,
        }
    } else if workspaces
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
    CloseWorkspaceForm {
        workspace_id: agent.info.workspace_id.clone(),
        tab_id: agent.info.tab_id.clone(),
        label: agent.project.clone(),
        project_root: project_main.map(|main| main.project_root),
        target,
    }
}

/// The workspaces the dashboard's supervisors run in, for
/// [`project_main_workspace`].
pub fn dashboard_supervisor_workspaces(agents: &[DashboardAgent]) -> Vec<&str> {
    agents
        .iter()
        .filter(|agent| agent.supervisor)
        .map(|agent| agent.info.workspace_id.as_str())
        .collect()
}

/// How a confirmed close ended.
#[derive(Debug, Clone)]
pub enum Closed {
    /// Done, as the status says.
    Done(String),
    /// Herdr refused to remove a dirty checkout; confirming the returned form
    /// again discards its uncommitted changes.
    Dirty(CloseWorkspaceForm),
    /// Herdr refused, as the status says.
    Failed(String),
}

/// Closes what `form` confirms. For a worktree agent this removes the
/// checkout from disk too; a dirty checkout is refused once and removed on
/// the next confirmation. The branch always stays. Once the last worker of a
/// Corgi project workspace is gone, the workspace is retired too, unless
/// the dashboard in `dashboard_pane_id` or anything else still holds it.
pub fn close(
    client: &HerdrClient,
    dashboard_pane_id: Option<&str>,
    form: CloseWorkspaceForm,
) -> Closed {
    match &form.target {
        CloseTarget::Worktree { checkout, force } => {
            match client.remove_worktree(&form.workspace_id, *force) {
                Ok(_) => Closed::Done(closed_agent_status(
                    client,
                    dashboard_pane_id,
                    &form,
                    "Removed worktree and its checkout",
                )),
                Err(error) if !force && HerdrError::is_dirty_worktree(&error) => {
                    let target = CloseTarget::Worktree {
                        checkout: checkout.clone(),
                        force: true,
                    };
                    Closed::Dirty(CloseWorkspaceForm { target, ..form })
                }
                Err(error) => Closed::Failed(format!("Remove failed: {error:#}")),
            }
        }
        CloseTarget::Tab { root_main } => {
            let closed = if *root_main {
                close_project_main_agent_tab(client, &form)
            } else {
                client.close_tab(&form.tab_id)
            };
            match closed {
                Ok(()) => Closed::Done(closed_agent_status(
                    client,
                    dashboard_pane_id,
                    &form,
                    "Closed agent tab",
                )),
                Err(error) => Closed::Failed(format!("Close tab failed: {error:#}")),
            }
        }
        CloseTarget::Workspace => match client.close_workspace(&form.workspace_id) {
            Ok(()) => Closed::Done(format!("Closed workspace {}", form.label)),
            Err(error) => Closed::Failed(format!("Close failed: {error:#}")),
        },
    }
}

/// Retire the agentless project main only after Corgi has deliberately
/// removed the final worker. A stale, unowned workspace or a live agent
/// always wins over cleanup.
fn closed_agent_status(
    client: &HerdrClient,
    dashboard_pane_id: Option<&str>,
    form: &CloseWorkspaceForm,
    action: &str,
) -> String {
    let Some(project_root) = form.project_root.as_deref() else {
        return format!("{action} {}", form.label);
    };
    let duplicates: String =
        retire_duplicate_project_mains(client, dashboard_pane_id, project_root)
            .iter()
            .map(|label| format!("; closed duplicate project workspace {label}"))
            .collect();
    let retired: Result<Option<String>> =
        retire_project_main_if_unused(client, dashboard_pane_id, project_root);
    match retired {
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
