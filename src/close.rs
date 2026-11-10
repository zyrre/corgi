//! Closing an agent: its worktree and checkout, its tab in a project
//! workspace, its whole workspace, or, in a workspace Corgi did not create,
//! its pane alone; then retiring the project workspace when nothing is left
//! in it.

use std::path::Path;

use anyhow::Result;

use crate::{
    git::{git_output, git_status_clean},
    herdr::{HerdrClient, HerdrError},
    model::{DashboardAgent, WorkspaceInfo},
    project_main::{
        close_project_main_agent_tab, is_corgi_project_main, project_main_workspace,
        retire_duplicate_project_mains, retire_project_main_if_unused,
    },
    rows::is_corgi_workspace,
};

#[derive(Debug, Clone)]
pub struct CloseWorkspaceForm {
    pub workspace_id: String,
    pub pane_id: String,
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
        /// Set after Herdr or Git refused to remove a dirty checkout, so the
        /// next Enter discards uncommitted changes.
        force: bool,
        /// `None` for Herdr's own worktree workspace, which Herdr's
        /// `worktree.remove` takes down with the checkout. A plain
        /// `git worktree add` checkout is Git's to remove; Herdr's side
        /// closes first, as for any other agent there.
        plain: Option<PlainWorktree>,
    },
    /// A shared-checkout agent runs in its own tab of a Corgi project main.
    /// Closing that tab leaves other agents alone.
    Tab {
        /// The first agent of a new project uses the main tab itself. Its
        /// root tab is replaced before it closes, so later agents keep a
        /// project parent.
        root_main: bool,
    },
    /// A workspace Corgi created for this agent (a scratch or plain agent
    /// workspace), closed with everything in it.
    Workspace,
    /// A workspace Corgi did not create, the user's own: only the agent's
    /// pane closes, and the user's other tabs and shells stay.
    Pane,
}

/// A plain linked worktree (`git worktree add`, not Herdr's) and what of
/// Herdr closes before Git removes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainWorktree {
    /// The repository's primary checkout, where `git worktree remove` runs.
    pub repo_root: String,
    /// The pane, tab or workspace, as for an agent outside a worktree.
    pub then: Box<CloseTarget>,
}

impl CloseWorkspaceForm {
    /// The same close, now discarding a dirty worktree's changes.
    pub fn forced(mut self, force: bool) -> Self {
        if let CloseTarget::Worktree { force: forced, .. } = &mut self.target {
            *forced = force;
        }
        self
    }
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
    let workspace = workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == agent.info.workspace_id);
    let container = if workspace
        .is_some_and(|workspace| is_corgi_project_main(workspace, &agent.project_root))
    {
        CloseTarget::Tab {
            root_main: project_main.as_ref().is_some_and(|main| {
                main.workspace_id == agent.info.workspace_id
                    && main.root_tab_id == agent.info.tab_id
            }),
        }
    } else if workspace.is_some_and(is_corgi_workspace) {
        CloseTarget::Workspace
    } else {
        CloseTarget::Pane
    };
    let target = match &agent.worktree_checkout {
        Some(checkout) => CloseTarget::Worktree {
            checkout: checkout.clone(),
            force: false,
            plain: (!is_herdr_worktree(workspace, checkout)).then(|| PlainWorktree {
                repo_root: agent.project_root.clone(),
                then: Box::new(container),
            }),
        },
        None => container,
    };
    CloseWorkspaceForm {
        workspace_id: agent.info.workspace_id.clone(),
        pane_id: agent.info.pane_id.clone(),
        tab_id: agent.info.tab_id.clone(),
        label: agent.project.clone(),
        project_root: project_main.map(|main| main.project_root),
        target,
    }
}

/// Whether `workspace` is Herdr's linked worktree workspace on `checkout`,
/// which Herdr's `worktree.remove` takes down.
fn is_herdr_worktree(workspace: Option<&WorkspaceInfo>, checkout: &str) -> bool {
    workspace
        .and_then(|workspace| workspace.worktree.as_ref())
        .is_some_and(|worktree| {
            worktree.is_linked_worktree
                && worktree.checkout_path.trim_end_matches('/') == checkout.trim_end_matches('/')
        })
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
        CloseTarget::Worktree {
            checkout,
            force,
            plain: Some(plain),
        } => close_plain_worktree(client, dashboard_pane_id, &form, checkout, *force, plain),
        CloseTarget::Worktree {
            force, plain: None, ..
        } => match client.remove_worktree(&form.workspace_id, *force) {
            Ok(_) => Closed::Done(closed_agent_status(
                client,
                dashboard_pane_id,
                &form,
                "Removed worktree and its checkout",
            )),
            Err(error) if !force && HerdrError::is_dirty_worktree(&error) => {
                Closed::Dirty(form.forced(true))
            }
            Err(error) => Closed::Failed(format!("Remove failed: {error:#}")),
        },
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
        CloseTarget::Pane => match client.close_pane(&form.pane_id) {
            Ok(()) => Closed::Done(format!("Closed the pane of {}", form.label)),
            Err(error) => Closed::Failed(format!("Close pane failed: {error:#}")),
        },
    }
}

/// Closes an agent in a plain `git worktree add` checkout: refuses a dirty
/// checkout once, as Herdr does for its own; then closes the agent's pane,
/// tab or workspace as `plain.then` says, and removes the checkout with
/// `git worktree remove` in the primary checkout. The branch stays.
fn close_plain_worktree(
    client: &HerdrClient,
    dashboard_pane_id: Option<&str>,
    form: &CloseWorkspaceForm,
    checkout: &str,
    force: bool,
    plain: &PlainWorktree,
) -> Closed {
    if !force {
        match git_status_clean(Path::new(checkout)) {
            Ok(true) => {}
            Ok(false) => return Closed::Dirty(form.clone().forced(true)),
            Err(error) => return Closed::Failed(format!("Remove failed: {error:#}")),
        }
    }
    let first = CloseWorkspaceForm {
        target: (*plain.then).clone(),
        ..form.clone()
    };
    let closed = match close(client, dashboard_pane_id, first) {
        Closed::Done(status) => status,
        other => return other,
    };
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(checkout);
    match git_output(Path::new(&plain.repo_root), &args) {
        Ok(_) => Closed::Done(format!("{closed}; removed its checkout {checkout}")),
        Err(error) => Closed::Failed(format!("{closed}, but the checkout stays: {error:#}")),
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        model::{AgentInfo, WorkspaceWorktreeInfo},
        project_main::{CORGI_AGENT_WORKSPACE_ROLE, CORGI_WORKSPACE_ROLE_TOKEN},
        test_support::{FakeHerdr, ScratchDir, git, git_repo, git_worktree},
    };

    fn row(workspace: &str, cwd: &str, root: &str, checkout: Option<&str>) -> DashboardAgent {
        DashboardAgent {
            info: AgentInfo {
                pane_id: format!("{workspace}:p2"),
                workspace_id: workspace.into(),
                tab_id: format!("{workspace}:t1"),
                cwd: Some(cwd.into()),
                ..AgentInfo::default()
            },
            project: "webshop/pay".into(),
            project_root: root.into(),
            worktree_checkout: checkout.map(str::to_string),
            ..DashboardAgent::default()
        }
    }

    fn plain(id: &str) -> WorkspaceInfo {
        WorkspaceInfo {
            workspace_id: id.into(),
            label: "mine".into(),
            ..WorkspaceInfo::default()
        }
    }

    #[test]
    fn an_agent_in_the_users_own_workspace_closes_its_pane_alone() {
        let agent = row("w1", "/repos/notes", "/repos/notes", None);
        let form = close_form(&agent, &[plain("w1")], &[]);
        assert_eq!(form.target, CloseTarget::Pane);
        assert_eq!(form.pane_id, "w1:p2");

        // A workspace Corgi created keeps today's close of the whole of it.
        let mut corgis = plain("w1");
        corgis.tokens.insert(
            CORGI_WORKSPACE_ROLE_TOKEN.into(),
            CORGI_AGENT_WORKSPACE_ROLE.into(),
        );
        assert_eq!(
            close_form(&agent, &[corgis], &[]).target,
            CloseTarget::Workspace
        );

        let herdr = FakeHerdr::start("close-pane", json!({}));
        let client = HerdrClient::from_socket_path(&herdr.socket);
        assert!(matches!(close(&client, None, form), Closed::Done(_)));
        let closed = herdr.requests("pane.close");
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0]["params"]["pane_id"], "w1:p2");
        assert!(herdr.requests("workspace.close").is_empty());
    }

    #[test]
    fn a_herdr_worktree_is_removed_by_herdr_and_a_plain_one_by_git() {
        let herdr_worktree = WorkspaceInfo {
            worktree: Some(WorkspaceWorktreeInfo {
                repo_name: "webshop".into(),
                repo_root: "/repos/webshop".into(),
                checkout_path: "/wt/pay".into(),
                is_linked_worktree: true,
            }),
            ..plain("w1")
        };
        let agent = row("w1", "/wt/pay", "/repos/webshop", Some("/wt/pay"));
        assert_eq!(
            close_form(&agent, &[herdr_worktree], &[]).target,
            CloseTarget::Worktree {
                checkout: "/wt/pay".into(),
                force: false,
                plain: None,
            }
        );
        // In the primary's own workspace, as Claude Code's `--worktree`
        // runs: the pane closes, then Git removes the checkout.
        assert_eq!(
            close_form(&agent, &[plain("w1")], &[]).target,
            CloseTarget::Worktree {
                checkout: "/wt/pay".into(),
                force: false,
                plain: Some(PlainWorktree {
                    repo_root: "/repos/webshop".into(),
                    then: Box::new(CloseTarget::Pane),
                }),
            }
        );
    }

    #[test]
    fn a_plain_worktree_is_refused_once_when_dirty_then_removed_keeping_its_branch() {
        let scratch = ScratchDir::new("close-plain-worktree");
        let root = git_repo(&scratch.join("webshop"));
        let linked = git_worktree(&root, &root.join(".claude/worktrees/pay"), "pay");
        let (root_text, linked_text) = (
            root.to_string_lossy().into_owned(),
            linked.to_string_lossy().into_owned(),
        );
        std::fs::write(linked.join("draft.txt"), "unsaved").unwrap();
        let herdr = FakeHerdr::start("close-plain-worktree", json!({}));
        let client = HerdrClient::from_socket_path(&herdr.socket);
        let agent = row("w1", &linked_text, &root_text, Some(&linked_text));
        let form = close_form(&agent, &[plain("w1")], &[]);

        let Closed::Dirty(forced) = close(&client, None, form) else {
            panic!("a dirty checkout is refused first");
        };
        assert!(
            herdr.requests("pane.close").is_empty(),
            "nothing closed yet"
        );
        assert!(linked.exists());

        let Closed::Done(status) = close(&client, None, forced) else {
            panic!("the forced close goes through");
        };
        assert!(status.contains("removed its checkout"), "{status}");
        assert_eq!(herdr.requests("pane.close").len(), 1);
        assert!(!linked.exists());
        // The branch stays.
        git(&root, &["rev-parse", "--verify", "-q", "pay"]);
    }
}
