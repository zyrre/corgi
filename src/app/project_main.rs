//! The Corgi-owned main workspace of each project: the metadata that marks
//! it, finding or creating it, its label, and retiring it once unused.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use anyhow::{Context, Result, bail};

use crate::{
    herdr::{HerdrClient, METADATA_VALUE_MAX_CHARS, PaneInfo, SessionSnapshot, TabInfo},
    model::{AgentInfo, WorkspaceInfo},
    paths::dir_name,
    supervisor,
};

use super::{
    App, DASHBOARD_PANE_LABEL, close::CloseWorkspaceForm, launch::resolve_repo, markers,
    progress::Progress, rows::has_real_agent_session,
};

/// The metadata source and tokens that identify workspaces Corgi created for
/// its own project lifecycle. Labels are user-facing and therefore not a safe
/// ownership boundary on their own.
pub(super) const CORGI_METADATA_SOURCE: &str = "corgi";
pub(super) const CORGI_WORKSPACE_ROLE_TOKEN: &str = "corgi_workspace_role";
pub(super) const CORGI_PROJECT_MAIN_ROLE: &str = "project-main";
pub(super) const CORGI_AGENT_WORKSPACE_ROLE: &str = "agent-workspace";
pub(super) const CORGI_PROJECT_MAIN_TAB_TOKEN: &str = "corgi_project_main_tab";
/// The directory a project workspace belongs to. Herdr records a workspace's
/// Git details only when it creates it, so a workspace opened in a plain
/// directory that later became a repository is found by this token.
///
/// Herdr cuts long token values short, so this token is written only for a
/// root that fits, and [`CORGI_PROJECT_ROOT_HASH_TOKEN`] is what matches.
pub(super) const CORGI_PROJECT_ROOT_TOKEN: &str = "corgi_project_root";
/// A digest of the project directory, [`project_root_digest`], which fits in
/// a token for a root of any length. Workspaces marked before it existed
/// carry only [`CORGI_PROJECT_ROOT_TOKEN`].
pub(super) const CORGI_PROJECT_ROOT_HASH_TOKEN: &str = "corgi_project_root_hash";

/// Corgi's agentless root workspace for one Git repository. Every isolated
/// worker created for that repository is explicitly attached beneath this
/// workspace, never beneath the dashboard pane that happened to launch it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProjectMain {
    pub(super) workspace_id: String,
    pub(super) label: String,
    pub(super) project_root: String,
    pub(super) root_tab_id: String,
}

/// Keep a project main tab after its first agent is closed. Metadata is moved
/// before the old tab closes, so a failed close leaves both tabs recoverable.
pub(super) fn close_project_main_agent_tab(
    client: &HerdrClient,
    form: &CloseWorkspaceForm,
) -> Result<()> {
    let project_root = form
        .project_root
        .as_deref()
        .context("project main tab has no repository root")?;
    let replacement = client
        .create_tab(
            &form.workspace_id,
            Some(Path::new(project_root)),
            Some(&form.label),
        )
        .context("create replacement project tab")?;
    markers::mark_workspace(
        client,
        &form.workspace_id,
        None,
        &project_main_tokens(
            &replacement.tab_id,
            project_root,
            &project_root_digest(project_root),
        ),
    )
    .context("mark replacement project tab")?;
    client
        .close_tab(&form.tab_id)
        .context("close first agent tab")
}

/// Finds the Corgi-owned main workspace for one repository, creating and
/// marking it if this is the project's first Corgi agent. The marker lets a
/// later dashboard process find the same workspace without taking ownership of
/// a user workspace that merely happens to have the same label.
pub(super) fn ensure_project_main_workspace(
    client: &HerdrClient,
    root: &str,
    progress: &mut dyn Progress,
) -> Result<ProjectMain> {
    let mut snapshot = client
        .snapshot()
        .context("inspect existing project workspaces")?;
    relabel_project_mains(client, &mut snapshot.workspaces);
    let supervisors = supervisor_workspaces(&snapshot.agents);
    if let Some(main) = project_main_workspace(&snapshot.workspaces, &supervisors, root) {
        progress.report(format!("Using project workspace {}…", main.label));
        return Ok(main);
    }
    create_project_main_workspace(client, root, progress)
}

/// Creates and marks a fresh Corgi project workspace for `root`, when no
/// existing workspace can serve as it.
fn create_project_main_workspace(
    client: &HerdrClient,
    root: &str,
    progress: &mut dyn Progress,
) -> Result<ProjectMain> {
    let label = project_workspace_label(root);
    progress.report(format!("Creating project workspace {label}…"));
    let created = client
        .create_workspace(Some(Path::new(root)), Some(&label))
        .with_context(|| format!("create project workspace {label}"))?;
    markers::mark_workspace(
        client,
        &created.workspace_id,
        markers::checkout_of(&created.workspace),
        &project_main_tokens(&created.tab_id, root, &project_root_digest(root)),
    )
    .with_context(|| format!("mark Corgi project workspace {label}"))?;
    Ok(ProjectMain {
        workspace_id: created.workspace_id,
        label,
        project_root: root.to_string(),
        root_tab_id: created.tab_id,
    })
}

/// The Corgi project-main workspace to start a supervisor of `root` in, when the
/// project already has agent sessions elsewhere (a supervisor launch beside
/// running work, as opposed to a new project's or an idle one's, which are
/// unaffected). Herdr's own root workspace for the repository is adopted —
/// marked, relabelled, and used as Corgi's project-main — when it is not
/// already Corgi's and holds no agent, so the supervisor lands where Herdr's own
/// tree shows the project's root instead of nested under it. Otherwise this
/// behaves exactly like [`ensure_project_main_workspace`].
///
/// Once adopted, both it and any older Corgi project-main of the same
/// repository can satisfy [`project_main_workspace`]'s lookup; that lookup
/// prefers the one the supervisor runs in, which is the adopted root once the
/// supervisor starts there, and otherwise takes the first match in the session
/// snapshot's own workspace order, which Herdr returns in sidebar order, so
/// the adopted root — Herdr's own, earlier in that order than a
/// later-created duplicate — is still what later lookups find. An older,
/// now-idle duplicate is closed so at most one remains; one still running an
/// agent is left alone.
pub(super) fn corgi_project_main(
    client: &HerdrClient,
    root: &str,
    progress: &mut dyn Progress,
) -> Result<ProjectMain> {
    let mut snapshot = client
        .snapshot()
        .context("inspect existing project workspaces")?;
    relabel_project_mains(client, &mut snapshot.workspaces);

    if project_has_other_agent_sessions(&snapshot, root)
        && let Some(herdr_root_id) = herdr_root_workspace_id(client, root)?
        && let Some(herdr_root) = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == herdr_root_id)
        && !is_corgi_project_main(herdr_root, root)
        && !workspace_has_agent(&snapshot, &herdr_root.workspace_id)
        && let Some(root_tab) = workspace_root_tab(&snapshot, &herdr_root.workspace_id)
    {
        let root_tab_id = root_tab.tab_id.clone();
        let adopted = adopt_project_main(client, herdr_root, &root_tab_id, root, progress)?;
        retire_other_project_mains(client, &snapshot, root, &herdr_root_id);
        return Ok(adopted);
    }

    let supervisors = supervisor_workspaces(&snapshot.agents);
    if let Some(main) = project_main_workspace(&snapshot.workspaces, &supervisors, root) {
        progress.report(format!("Using project workspace {}…", main.label));
        return Ok(main);
    }
    create_project_main_workspace(client, root, progress)
}

/// The workspace ID Herdr's own sidebar treats as `root`'s repository root:
/// the lowest-numbered workspace whose checkout is that repository's primary
/// one. `None` outside Git, or when Herdr has no such workspace open.
fn herdr_root_workspace_id(client: &HerdrClient, root: &str) -> Result<Option<String>> {
    let entries = client
        .list_workspaces()
        .context("inspect Herdr's own workspace order")?;
    Ok(entries
        .into_iter()
        .filter(|entry| {
            entry
                .worktree
                .as_ref()
                .is_some_and(|worktree| !worktree.is_linked_worktree && worktree.repo_root == root)
        })
        .min_by_key(|entry| entry.number)
        .map(|entry| entry.workspace_id))
}

/// Whether `project_root` already has a real agent session in some workspace
/// of its own, the condition under which a new supervisor launch may adopt
/// Herdr's root workspace instead of always using Corgi's own.
fn project_has_other_agent_sessions(snapshot: &SessionSnapshot, project_root: &str) -> bool {
    let workspaces: HashMap<&str, &WorkspaceInfo> = snapshot
        .workspaces
        .iter()
        .map(|workspace| (workspace.workspace_id.as_str(), workspace))
        .collect();
    snapshot.agents.iter().any(|agent| {
        has_real_agent_session(agent)
            && workspaces
                .get(agent.workspace_id.as_str())
                .is_some_and(|workspace| workspace.repo_root() == Some(project_root))
    })
}

/// Whether any pane of `workspace_id` holds a real agent session.
fn workspace_has_agent(snapshot: &SessionSnapshot, workspace_id: &str) -> bool {
    snapshot
        .agents
        .iter()
        .any(|agent| has_real_agent_session(agent) && agent.workspace_id == workspace_id)
}

/// The root tab of `workspace_id`: its lowest-numbered tab, the one Herdr
/// shows first.
fn workspace_root_tab<'a>(
    snapshot: &'a SessionSnapshot,
    workspace_id: &str,
) -> Option<&'a TabInfo> {
    snapshot
        .tabs
        .iter()
        .filter(|tab| tab.workspace_id == workspace_id)
        .min_by_key(|tab| tab.number)
}

/// Marks `workspace` as Corgi's project-main for `project_root`, using
/// `root_tab_id` as its root tab, and relabels it the way Corgi labels its
/// own. A failed rename is left for [`relabel_project_mains`] to retry, as
/// elsewhere; the mark itself must succeed for the adoption to count.
fn adopt_project_main(
    client: &HerdrClient,
    workspace: &WorkspaceInfo,
    root_tab_id: &str,
    project_root: &str,
    progress: &mut dyn Progress,
) -> Result<ProjectMain> {
    let label = project_workspace_label(project_root);
    progress.report(format!(
        "Taking over {} as the project workspace…",
        workspace.label
    ));
    markers::mark_workspace(
        client,
        &workspace.workspace_id,
        markers::checkout_of(workspace),
        &project_main_tokens(
            root_tab_id,
            project_root,
            &project_root_digest(project_root),
        ),
    )
    .with_context(|| format!("mark {} as the project workspace", workspace.label))?;
    let _ = client.rename_workspace(&workspace.workspace_id, &label);
    Ok(ProjectMain {
        workspace_id: workspace.workspace_id.clone(),
        label,
        project_root: project_root.to_string(),
        root_tab_id: root_tab_id.to_string(),
    })
}

/// Closes any other Corgi project-main workspace of `project_root` besides
/// `keep`, once adoption gave the project a new one, so at most one remains.
/// A duplicate that still holds an agent is left for that agent to finish
/// with; failing to close an idle one is not fatal to the launch that
/// triggered the cleanup.
fn retire_other_project_mains(
    client: &HerdrClient,
    snapshot: &SessionSnapshot,
    project_root: &str,
    keep: &str,
) {
    for workspace in &snapshot.workspaces {
        if workspace.workspace_id == keep || !is_corgi_project_main(workspace, project_root) {
            continue;
        }
        if !workspace_has_agent(snapshot, &workspace.workspace_id) {
            let _ = client.close_workspace(&workspace.workspace_id);
        }
    }
}

/// The metadata that makes a workspace the Corgi project workspace of `root`,
/// given the root's [`project_root_digest`]: the digest always, and the root
/// itself when Herdr keeps it whole.
fn project_main_tokens<'a>(
    root_tab_id: &'a str,
    root: &'a str,
    root_digest: &'a str,
) -> Vec<(&'static str, &'a str)> {
    let mut tokens = vec![
        (CORGI_WORKSPACE_ROLE_TOKEN, CORGI_PROJECT_MAIN_ROLE),
        (CORGI_PROJECT_MAIN_TAB_TOKEN, root_tab_id),
        (CORGI_PROJECT_ROOT_HASH_TOKEN, root_digest),
    ];
    if root.chars().count() <= METADATA_VALUE_MAX_CHARS {
        tokens.push((CORGI_PROJECT_ROOT_TOKEN, root));
    }
    tokens
}

/// A fixed-length digest of a project directory: 64-bit FNV-1a over its
/// UTF-8 bytes, in hex behind the algorithm's name. It must read the same in
/// every Corgi build, which rules out std's `DefaultHasher`. It only tells
/// Corgi's own marked workspaces apart, so it need not resist forgery.
pub(super) fn project_root_digest(root: &str) -> String {
    let hash = root.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("fnv1a64:{hash:016x}")
}

/// The label of a project's Corgi workspace: `<project> supervisor`, since that
/// workspace's root tab is where the project's supervisor lives. It sets the
/// workspace apart from worker workspaces and from plain workspaces that
/// share the project's name.
fn project_workspace_label(project_root: &str) -> String {
    format!(
        "{} supervisor",
        dir_name(project_root).unwrap_or(supervisor::UNNAMED_PROJECT)
    )
}

/// The label a project workspace should change to, if it still carries a
/// label Corgi used to give it: the bare project name, `<project> corgi` from
/// while the supervisor was called the corgi, `<project> handler` from while
/// it was the Project handler, or `<project> steward` from before that, when
/// it was the Steward. Any other label was chosen by the user and is kept.
fn project_main_relabel(workspace: &WorkspaceInfo) -> Option<String> {
    let root = project_main_root(workspace)?;
    let project = dir_name(root).unwrap_or(supervisor::UNNAMED_PROJECT);
    let label = workspace.label.trim();
    (label == project
        || ["corgi", "handler", "steward"]
            .iter()
            .any(|old| label == format!("{project} {old}")))
    .then(|| project_workspace_label(root))
}

/// Gives every project workspace that still has its old label the current
/// one, updating the snapshot to match. A failed rename is retried on the next
/// snapshot and never blocks what the snapshot was taken for.
pub(super) fn relabel_project_mains(client: &HerdrClient, workspaces: &mut [WorkspaceInfo]) {
    for workspace in workspaces {
        if let Some(label) = project_main_relabel(workspace)
            && client
                .rename_workspace(&workspace.workspace_id, &label)
                .is_ok()
        {
            workspace.label = label;
        }
    }
}

/// The Corgi project workspace of `project_root`. A project can have two,
/// when Herdr lost Corgi's marks in a restart and a launch created another
/// before they were put back; the one a supervisor runs in, among the
/// workspaces in `supervisors`, is the project's, and otherwise the first in
/// snapshot order.
pub(super) fn project_main_workspace(
    workspaces: &[WorkspaceInfo],
    supervisors: &[&str],
    project_root: &str,
) -> Option<ProjectMain> {
    let mut mains = workspaces
        .iter()
        .filter(|workspace| is_corgi_project_main(workspace, project_root));
    let workspace = mains
        .clone()
        .find(|workspace| supervisors.contains(&workspace.workspace_id.as_str()))
        .or_else(|| mains.next())?;
    Some(ProjectMain {
        workspace_id: workspace.workspace_id.clone(),
        label: workspace.label.clone(),
        project_root: project_root.to_string(),
        root_tab_id: workspace
            .tokens
            .get(CORGI_PROJECT_MAIN_TAB_TOKEN)
            .cloned()
            .unwrap_or_default(),
    })
}

/// The workspaces the supervisors among `agents` run in, for
/// [`project_main_workspace`].
pub(super) fn supervisor_workspaces(agents: &[AgentInfo]) -> Vec<&str> {
    agents
        .iter()
        .filter(|agent| supervisor::is_supervisor(agent))
        .map(|agent| agent.workspace_id.as_str())
        .collect()
}

/// Whether `workspace` is the Corgi project workspace of `project_root`: by its
/// root's digest, or, for a workspace marked before digests, by its root.
pub(super) fn is_corgi_project_main(workspace: &WorkspaceInfo, project_root: &str) -> bool {
    if !has_project_main_role(workspace) {
        return false;
    }
    match workspace.tokens.get(CORGI_PROJECT_ROOT_HASH_TOKEN) {
        Some(digest) => *digest == project_root_digest(project_root),
        None => project_main_root(workspace) == Some(project_root),
    }
}

fn has_project_main_role(workspace: &WorkspaceInfo) -> bool {
    workspace
        .tokens
        .get(CORGI_WORKSPACE_ROLE_TOKEN)
        .is_some_and(|role| role == CORGI_PROJECT_MAIN_ROLE)
}

/// The project directory of a Corgi project workspace: the one it recorded
/// when Corgi created it, or else the primary checkout Herdr reports for it.
/// A root that Herdr may have cut short is never returned, so this is `None`
/// for a long root outside Git, as for every workspace Corgi does not own.
pub(super) fn project_main_root(workspace: &WorkspaceInfo) -> Option<&str> {
    if !has_project_main_role(workspace) {
        return None;
    }
    let recorded = workspace
        .tokens
        .get(CORGI_PROJECT_ROOT_TOKEN)
        .map(String::as_str)
        .filter(|root| !root.is_empty());
    let checkout = workspace
        .worktree
        .as_ref()
        .filter(|worktree| !worktree.is_linked_worktree)
        .and_then(|_| workspace.repo_root());
    match workspace.tokens.get(CORGI_PROJECT_ROOT_HASH_TOKEN) {
        Some(digest) => recorded
            .into_iter()
            .chain(checkout)
            .find(|root| project_root_digest(root) == *digest),
        // Without a digest, a root as long as Herdr's limit may be the
        // start of a longer one, and would claim another project's workspace.
        None => recorded
            .filter(|root| root.chars().count() < METADATA_VALUE_MAX_CHARS)
            .or(checkout),
    }
}

fn is_corgi_agent_workspace(workspace: &WorkspaceInfo, project_root: &str) -> bool {
    workspace.repo_root() == Some(project_root)
        && workspace
            .worktree
            .as_ref()
            .is_some_and(|worktree| worktree.is_linked_worktree)
        && workspace
            .tokens
            .get(CORGI_WORKSPACE_ROLE_TOKEN)
            .is_some_and(|role| role == CORGI_AGENT_WORKSPACE_ROLE)
}

/// Whether a Corgi project workspace holds nothing that closing it would
/// take down: no agent in any pane, no dashboard, and only its recorded root
/// tab, with at most one pane.
fn holds_nothing(
    snapshot: &SessionSnapshot,
    workspace: &WorkspaceInfo,
    dashboard_pane_ids: &HashSet<&str>,
) -> bool {
    let id = workspace.workspace_id.as_str();
    let root_tab = workspace.tokens.get(CORGI_PROJECT_MAIN_TAB_TOKEN);
    let tabs: Vec<_> = snapshot
        .tabs
        .iter()
        .filter(|tab| tab.workspace_id == id)
        .collect();
    let panes: Vec<_> = snapshot
        .panes
        .iter()
        .filter(|pane| pane.workspace_id == id)
        .collect();
    matches!(tabs.as_slice(), [tab] if Some(&tab.tab_id) == root_tab)
        && panes.len() <= 1
        && panes
            .iter()
            .all(|pane| !dashboard_pane_ids.contains(pane.pane_id.as_str()))
        && !snapshot.agents.iter().any(|agent| agent.workspace_id == id)
}

/// The directory a project is known by: its repository's primary checkout,
/// or the directory itself outside Git.
pub(super) fn project_root_of(client: &HerdrClient, project: &Path) -> Result<String> {
    Ok(resolve_repo(client, project)?.map_or_else(
        || project.to_string_lossy().into_owned(),
        |source| source.repo_root,
    ))
}

/// The panes among `panes` that are a Corgi dashboard: the one Herdr started
/// in `dashboard_pane_id`, and any labelled like one, since the launcher may
/// have moved that dashboard and changed its pane ID.
pub(super) fn dashboard_pane_ids<'a>(
    dashboard_pane_id: Option<&str>,
    panes: &'a [PaneInfo],
) -> HashSet<&'a str> {
    panes
        .iter()
        .filter(|pane| {
            dashboard_pane_id == Some(pane.pane_id.as_str())
                || pane.label.as_deref() == Some(DASHBOARD_PANE_LABEL)
        })
        .map(|pane| pane.pane_id.as_str())
        .collect()
}

impl App {
    /// The panes among `panes` that are a Corgi dashboard: this one, and any
    /// labelled like one, since the launcher may have moved this dashboard
    /// and changed its pane ID.
    pub(super) fn dashboard_pane_ids<'a>(&self, panes: &'a [PaneInfo]) -> HashSet<&'a str> {
        dashboard_pane_ids(self.dashboard_pane_id.as_deref(), panes)
    }

    /// The workspaces the dashboard's supervisors run in, for
    /// [`project_main_workspace`].
    pub(super) fn supervisor_workspaces(&self) -> Vec<&str> {
        self.agents
            .iter()
            .filter(|agent| agent.supervisor)
            .map(|agent| agent.info.workspace_id.as_str())
            .collect()
    }

    /// Closes every Corgi project workspace of `project_root` other than the
    /// one [`project_main_workspace`] settles on, as long as it holds
    /// nothing: no agent, no dashboard, and one tab, its recorded root tab,
    /// with at most one pane. Such a duplicate is what a launch leaves behind
    /// when it ran while Herdr had lost Corgi's marks after a restart. Returns
    /// the labels of the workspaces it closed; one that fails to close is
    /// left for a later cleanup.
    pub(super) fn retire_duplicate_project_mains(&self, project_root: &str) -> Vec<String> {
        let Ok(snapshot) = self.client.snapshot() else {
            return Vec::new();
        };
        let supervisors = supervisor_workspaces(&snapshot.agents);
        let Some(main) = project_main_workspace(&snapshot.workspaces, &supervisors, project_root)
        else {
            return Vec::new();
        };
        let dashboard_pane_ids = self.dashboard_pane_ids(&snapshot.panes);
        snapshot
            .workspaces
            .iter()
            .filter(|workspace| {
                workspace.workspace_id != main.workspace_id
                    && is_corgi_project_main(workspace, project_root)
                    && holds_nothing(&snapshot, workspace, &dashboard_pane_ids)
            })
            .filter(|workspace| self.client.close_workspace(&workspace.workspace_id).is_ok())
            .map(|workspace| workspace.label.clone())
            .collect()
    }

    pub(super) fn retire_project_main_if_unused(
        &self,
        project_root: &str,
    ) -> Result<Option<String>> {
        let snapshot = self
            .client
            .snapshot()
            .context("refresh project workspace before cleanup")?;
        let supervisors = supervisor_workspaces(&snapshot.agents);
        let Some(main) = project_main_workspace(&snapshot.workspaces, &supervisors, project_root)
        else {
            return Ok(None);
        };
        let dashboard_pane_ids = self.dashboard_pane_ids(&snapshot.panes);
        let workspaces: HashMap<&str, &WorkspaceInfo> = snapshot
            .workspaces
            .iter()
            .map(|workspace| (workspace.workspace_id.as_str(), workspace))
            .collect();
        // An agent in the project workspace itself counts even when Herdr
        // knows no repository for it, as in a directory outside Git.
        if snapshot.agents.iter().any(|agent| {
            !dashboard_pane_ids.contains(agent.pane_id.as_str())
                && has_real_agent_session(agent)
                && (agent.workspace_id == main.workspace_id
                    || workspaces
                        .get(agent.workspace_id.as_str())
                        .is_some_and(|workspace| workspace.repo_root() == Some(project_root)))
        }) {
            bail!("agent sessions are still running in this project");
        }
        let linked: Vec<_> = snapshot
            .workspaces
            .iter()
            .filter(|workspace| {
                workspace.repo_root() == Some(project_root)
                    && workspace
                        .worktree
                        .as_ref()
                        .is_some_and(|worktree| worktree.is_linked_worktree)
            })
            .collect();
        if !linked.is_empty() {
            let unowned = linked
                .iter()
                .filter(|workspace| !is_corgi_agent_workspace(workspace, project_root))
                .count();
            if unowned > 0 {
                bail!("{unowned} unowned worktree workspace(s) remain");
            }
            bail!("{} Corgi worktree workspace(s) remain", linked.len());
        }
        let project_tabs: Vec<_> = snapshot
            .tabs
            .iter()
            .filter(|tab| tab.workspace_id == main.workspace_id)
            .collect();
        if project_tabs.len() != 1
            || project_tabs.first().map(|tab| tab.tab_id.as_str())
                != Some(main.root_tab_id.as_str())
        {
            bail!(
                "cannot verify the project root tab is the only tab (found {})",
                project_tabs.len()
            );
        }
        self.client
            .close_workspace(&main.workspace_id)
            .with_context(|| format!("close unused project workspace {}", main.label))?;
        Ok(Some(main.label))
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, path::PathBuf, thread};

    use serde_json::{Value, json};

    use crate::{
        app::{CloseTarget, progress::Silent},
        model::WorkspaceWorktreeInfo,
        test_support::{answer, fake_herdr, test_app},
    };

    use super::*;

    /// A root longer than Herdr keeps, like a deep scratchpad directory.
    const LONG_ROOT: &str = "/private/tmp/claude-000/-Users-someone--herdr-worktrees-corgi-worktree-quiet-owl/00000000-0000-0000-0000-000000000000/scratchpad/long-project";

    fn marked(tokens: &[(&str, &str)]) -> WorkspaceInfo {
        WorkspaceInfo {
            workspace_id: "w9".into(),
            label: "long-project supervisor".into(),
            tokens: tokens
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
            worktree: None,
        }
    }

    /// A fake Herdr that keeps the workspaces Corgi creates and marks, and
    /// cuts token values short as Herdr does. It serves `requests`
    /// connections and returns the workspaces it ended with.
    fn truncating_herdr(label: &str, requests: usize) -> (PathBuf, thread::JoinHandle<Vec<Value>>) {
        let (socket_path, server) = fake_herdr(label, move |listener| {
            let mut workspaces: Vec<Value> = Vec::new();
            for _ in 0..requests {
                answer(&listener, |request| {
                    let params = &request["params"];
                    let result = match request["method"].as_str() {
                        Some("session.snapshot") => json!({
                            "type": "session_snapshot",
                            "snapshot": { "workspaces": workspaces }
                        }),
                        Some("workspace.create") => {
                            let id = format!("w{}", workspaces.len() + 1);
                            workspaces.push(json!({
                                "workspace_id": id, "label": params["label"], "tokens": {}
                            }));
                            json!({
                                "type": "workspace_created",
                                "workspace": { "workspace_id": id, "label": params["label"] },
                                "tab": { "tab_id": format!("{id}:t1") },
                                "root_pane": { "pane_id": format!("{id}:p1") }
                            })
                        }
                        Some("workspace.report_metadata") => {
                            let workspace = workspaces
                                .iter_mut()
                                .find(|workspace| {
                                    workspace["workspace_id"] == params["workspace_id"]
                                })
                                .expect("metadata for a known workspace");
                            for (key, value) in params["tokens"].as_object().expect("tokens") {
                                let kept: String = value
                                    .as_str()
                                    .expect("string token")
                                    .chars()
                                    .take(METADATA_VALUE_MAX_CHARS)
                                    .collect();
                                workspace["tokens"][key] = kept.into();
                            }
                            json!({ "type": "workspace_metadata_updated" })
                        }
                        method => panic!("unexpected Herdr request {method:?}"),
                    };
                    json!({ "result": result })
                });
            }
            workspaces
        });
        (socket_path, server)
    }

    #[test]
    fn the_root_digest_is_fnv1a_and_fits_in_a_token() {
        assert_eq!(project_root_digest(""), "fnv1a64:cbf29ce484222325");
        assert_eq!(project_root_digest("a"), "fnv1a64:af63dc4c8601ec8c");
        assert_eq!(project_root_digest("foobar"), "fnv1a64:85944171f73967e8");
        assert!(project_root_digest(LONG_ROOT).chars().count() <= METADATA_VALUE_MAX_CHARS);
        assert!(
            CORGI_PROJECT_ROOT_HASH_TOKEN.len() <= 25,
            "Herdr rejects longer keys"
        );
    }

    #[test]
    fn a_long_root_is_marked_by_its_digest_alone() {
        assert!(LONG_ROOT.chars().count() > METADATA_VALUE_MAX_CHARS);
        let digest = project_root_digest(LONG_ROOT);
        let tokens = project_main_tokens("w9:t1", LONG_ROOT, &digest);
        assert!(tokens.contains(&(CORGI_PROJECT_ROOT_HASH_TOKEN, digest.as_str())));
        assert!(
            tokens
                .iter()
                .all(|(key, _)| *key != CORGI_PROJECT_ROOT_TOKEN)
        );

        let digest = project_root_digest("/repos/corgi");
        let tokens = project_main_tokens("w9:t1", "/repos/corgi", &digest);
        assert!(tokens.contains(&(CORGI_PROJECT_ROOT_TOKEN, "/repos/corgi")));
    }

    #[test]
    fn a_second_launch_into_a_long_root_reuses_its_project_workspace() {
        // First launch: snapshot, create, mark. Second launch: snapshot.
        let (socket_path, server) = truncating_herdr("long-root", 4);
        let client = HerdrClient::from_socket_path(&socket_path);
        let first = ensure_project_main_workspace(&client, LONG_ROOT, &mut Silent)
            .expect("create the project workspace");
        let second = ensure_project_main_workspace(&client, LONG_ROOT, &mut Silent)
            .expect("find the project workspace");
        let workspaces = server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");

        assert_eq!(workspaces.len(), 1, "one project workspace: {workspaces:?}");
        assert_eq!(second, first);
        assert_eq!(second.project_root, LONG_ROOT);
        assert_eq!(second.root_tab_id, "w1:t1");
    }

    #[test]
    fn a_digest_marked_workspace_belongs_to_that_root_only() {
        let digest = project_root_digest(LONG_ROOT);
        let workspace = marked(&[
            (CORGI_WORKSPACE_ROLE_TOKEN, CORGI_PROJECT_MAIN_ROLE),
            (CORGI_PROJECT_ROOT_HASH_TOKEN, &digest),
        ]);
        assert!(is_corgi_project_main(&workspace, LONG_ROOT));
        assert!(!is_corgi_project_main(&workspace, "/repos/corgi"));
        assert_eq!(project_main_root(&workspace), None);

        // The digest alone never makes a workspace Corgi's.
        let unowned = marked(&[(CORGI_PROJECT_ROOT_HASH_TOKEN, &digest)]);
        assert!(!is_corgi_project_main(&unowned, LONG_ROOT));
    }

    #[test]
    fn a_workspace_marked_before_digests_is_still_found() {
        let workspace = marked(&[
            (CORGI_WORKSPACE_ROLE_TOKEN, CORGI_PROJECT_MAIN_ROLE),
            (CORGI_PROJECT_MAIN_TAB_TOKEN, "w9:t1"),
            (CORGI_PROJECT_ROOT_TOKEN, "/repos/corgi"),
        ]);
        let main = project_main_workspace(std::slice::from_ref(&workspace), &[], "/repos/corgi")
            .expect("found by its recorded root");
        assert_eq!(main.workspace_id, "w9");
        assert_eq!(main.root_tab_id, "w9:t1");
        assert_eq!(project_main_root(&workspace), Some("/repos/corgi"));
        assert!(!is_corgi_project_main(&workspace, "/repos/other"));
    }

    #[test]
    fn a_cut_root_from_before_digests_claims_no_other_project() {
        // Herdr kept only the start of a long root, which may itself be a
        // directory, here a project nested in the long root's parent path.
        let cut: String = LONG_ROOT.chars().take(METADATA_VALUE_MAX_CHARS).collect();
        let workspace = marked(&[
            (CORGI_WORKSPACE_ROLE_TOKEN, CORGI_PROJECT_MAIN_ROLE),
            (CORGI_PROJECT_ROOT_TOKEN, &cut),
        ]);
        assert!(!is_corgi_project_main(&workspace, &cut));
        assert_eq!(project_main_root(&workspace), None);
    }

    #[test]
    fn project_mains_are_reused_only_when_corgi_marked_them() {
        let project_root = "/repos/corgi";
        let dashboard = WorkspaceInfo {
            workspace_id: "w-dashboard".into(),
            label: "Corgi dashboard".into(),
            worktree: Some(WorkspaceWorktreeInfo {
                repo_root: project_root.into(),
                checkout_path: project_root.into(),
                is_linked_worktree: false,
                ..WorkspaceWorktreeInfo::default()
            }),
            ..WorkspaceInfo::default()
        };
        let main = WorkspaceInfo {
            workspace_id: "w-main".into(),
            label: "corgi".into(),
            tokens: BTreeMap::from([
                (
                    CORGI_WORKSPACE_ROLE_TOKEN.into(),
                    CORGI_PROJECT_MAIN_ROLE.into(),
                ),
                (CORGI_PROJECT_MAIN_TAB_TOKEN.into(), "w-main:t1".into()),
            ]),
            worktree: Some(WorkspaceWorktreeInfo {
                repo_root: project_root.into(),
                checkout_path: project_root.into(),
                is_linked_worktree: false,
                ..WorkspaceWorktreeInfo::default()
            }),
        };

        let discovered = project_main_workspace(&[dashboard, main], &[], project_root)
            .expect("Corgi main workspace");
        assert_eq!(discovered.workspace_id, "w-main");
        assert_eq!(discovered.root_tab_id, "w-main:t1");
        assert_eq!(project_workspace_label(project_root), "corgi supervisor");
    }
    #[test]
    fn a_project_main_with_an_old_label_is_relabelled_and_no_other() {
        let main = |label: &str, role: &str, linked: bool| WorkspaceInfo {
            workspace_id: "w-main".into(),
            label: label.into(),
            tokens: BTreeMap::from([(CORGI_WORKSPACE_ROLE_TOKEN.into(), role.into())]),
            worktree: Some(WorkspaceWorktreeInfo {
                repo_root: "/repos/weather".into(),
                checkout_path: "/repos/weather".into(),
                is_linked_worktree: linked,
                ..WorkspaceWorktreeInfo::default()
            }),
        };

        // The bare project name, and the labels an older Corgi gave it while
        // the supervisor was the corgi, the Project handler and, before that,
        // the Steward.
        for old in [
            "weather",
            "weather corgi",
            "weather handler",
            "weather steward",
        ] {
            assert_eq!(
                project_main_relabel(&main(old, CORGI_PROJECT_MAIN_ROLE, false)).as_deref(),
                Some("weather supervisor"),
                "{old}"
            );
        }
        // Already current, renamed by the user, or not Corgi's project main.
        for workspace in [
            main("weather supervisor", CORGI_PROJECT_MAIN_ROLE, false),
            main("my weather", CORGI_PROJECT_MAIN_ROLE, false),
            main("weather", CORGI_AGENT_WORKSPACE_ROLE, true),
            main("weather", "", false),
        ] {
            assert_eq!(
                project_main_relabel(&workspace),
                None,
                "{}",
                workspace.label
            );
        }
    }
    #[test]
    fn project_mains_do_not_claim_another_repositories_workspace() {
        let workspace = WorkspaceInfo {
            workspace_id: "w-main".into(),
            label: "corgi".into(),
            tokens: BTreeMap::from([(
                CORGI_WORKSPACE_ROLE_TOKEN.into(),
                CORGI_PROJECT_MAIN_ROLE.into(),
            )]),
            worktree: Some(WorkspaceWorktreeInfo {
                repo_root: "/repos/corgi".into(),
                checkout_path: "/repos/corgi".into(),
                is_linked_worktree: false,
                ..WorkspaceWorktreeInfo::default()
            }),
        };
        let worker = WorkspaceInfo {
            workspace_id: "w-worker".into(),
            tokens: BTreeMap::from([(
                CORGI_WORKSPACE_ROLE_TOKEN.into(),
                CORGI_AGENT_WORKSPACE_ROLE.into(),
            )]),
            worktree: Some(WorkspaceWorktreeInfo {
                repo_root: "/repos/weather".into(),
                checkout_path: "/worktrees/weather/task".into(),
                is_linked_worktree: true,
                ..WorkspaceWorktreeInfo::default()
            }),
            ..WorkspaceInfo::default()
        };

        assert!(project_main_workspace(&[workspace, worker], &[], "/repos/weather").is_none());
    }
    #[test]
    fn an_idle_corgi_project_main_is_retired_after_the_last_agent_closes() {
        let (socket_path, server) = fake_herdr("project-retire", move |listener| {
            for expected_method in ["session.snapshot", "workspace.close"] {
                answer(&listener, |request| {
                    assert_eq!(request["method"], expected_method);

                    let result = match expected_method {
                        "session.snapshot" => json!({
                            "type": "session_snapshot",
                            "snapshot": {
                                "workspaces": [{
                                    "workspace_id": "w-main",
                                    "label": "corgi",
                                    "tokens": {
                                        CORGI_WORKSPACE_ROLE_TOKEN: CORGI_PROJECT_MAIN_ROLE,
                                        CORGI_PROJECT_MAIN_TAB_TOKEN: "w-main:t1"
                                    },
                                    "worktree": {
                                        "repo_root": "/repos/corgi",
                                        "checkout_path": "/repos/corgi",
                                        "is_linked_worktree": false
                                    }
                                }],
                                "tabs": [{
                                    "tab_id": "w-main:t1",
                                    "workspace_id": "w-main",
                                    "label": "corgi"
                                }],
                                "agents": [],
                                "panes": []
                            }
                        }),
                        "workspace.close" => {
                            assert_eq!(request["params"]["workspace_id"], "w-main");
                            json!({
                                "type": "workspace_closed",
                                "workspace": { "workspace_id": "w-main", "label": "corgi" }
                            })
                        }
                        _ => unreachable!(),
                    };
                    json!({ "result": result })
                });
            }
        });

        let mut app = test_app();
        app.client = HerdrClient::from_socket_path(&socket_path);
        assert_eq!(
            app.retire_project_main_if_unused("/repos/corgi")
                .expect("retire idle project main"),
            Some("corgi".into())
        );

        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");
    }
    #[test]
    fn project_main_is_kept_when_any_agent_session_remains() {
        let (socket_path, server) = fake_herdr("project-keep", move |listener| {
            answer(&listener, |request| {
                assert_eq!(request["method"], "session.snapshot");
                json!({
                    "result": {
                        "type": "session_snapshot",
                        "snapshot": {
                            "workspaces": [
                                {
                                    "workspace_id": "w-main",
                                    "label": "corgi",
                                    "tokens": {
                                        CORGI_WORKSPACE_ROLE_TOKEN: CORGI_PROJECT_MAIN_ROLE,
                                        CORGI_PROJECT_MAIN_TAB_TOKEN: "w-main:t1"
                                    },
                                    "worktree": {
                                        "repo_root": "/repos/corgi",
                                        "checkout_path": "/repos/corgi",
                                        "is_linked_worktree": false
                                    }
                                },
                                {
                                    "workspace_id": "w-worker",
                                    "label": "worktree-quiet-owl",
                                    "worktree": {
                                        "repo_root": "/repos/corgi",
                                        "checkout_path": "/worktrees/corgi/quiet-owl",
                                        "is_linked_worktree": true
                                    }
                                }
                            ],
                            "tabs": [{
                                "tab_id": "w-main:t1",
                                "workspace_id": "w-main",
                                "label": "corgi"
                            }],
                            "agents": [{
                                "agent": "codex",
                                "agent_status": "working",
                                "agent_session": {
                                    "agent": "codex",
                                    "kind": "id",
                                    "source": "herdr:codex",
                                    "value": "worker-session"
                                },
                                "pane_id": "w-worker:p1",
                                "workspace_id": "w-worker",
                                "tab_id": "w-worker:t1"
                            }],
                            "panes": []
                        }
                    }
                })
            });
        });

        let mut app = test_app();
        app.client = HerdrClient::from_socket_path(&socket_path);
        let error = app
            .retire_project_main_if_unused("/repos/corgi")
            .expect_err("a live agent must keep the project main");
        assert!(
            error
                .to_string()
                .contains("agent sessions are still running")
        );

        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");
    }
    #[test]
    fn closing_first_agent_replaces_the_project_root_tab() {
        let (socket_path, server) = fake_herdr("replace-root", move |listener| {
            for method in ["tab.create", "workspace.report_metadata", "tab.close"] {
                answer(&listener, |request| {
                    assert_eq!(request["method"], method);
                    let result = match method {
                        "tab.create" => {
                            assert_eq!(request["params"]["workspace_id"], "w9");
                            assert_eq!(request["params"]["cwd"], "/tmp/corgi-new-project");
                            json!({
                                "type": "tab_created",
                                "tab": { "tab_id": "w9:t2" },
                                "root_pane": { "pane_id": "w9:p2" }
                            })
                        }
                        "workspace.report_metadata" => {
                            assert_eq!(
                                request["params"]["tokens"]["corgi_workspace_role"],
                                "project-main"
                            );
                            assert_eq!(
                                request["params"]["tokens"]["corgi_project_main_tab"],
                                "w9:t2"
                            );
                            json!({ "type": "workspace_metadata_updated" })
                        }
                        "tab.close" => {
                            assert_eq!(request["params"]["tab_id"], "w9:t1");
                            json!({ "type": "tab_closed" })
                        }
                        _ => unreachable!(),
                    };
                    json!({ "result": result })
                });
            }
        });

        let client = HerdrClient::from_socket_path(&socket_path);
        let form = CloseWorkspaceForm {
            workspace_id: "w9".into(),
            tab_id: "w9:t1".into(),
            label: "corgi-new-project".into(),
            project_root: Some("/tmp/corgi-new-project".into()),
            target: CloseTarget::Tab { root_main: true },
        };
        close_project_main_agent_tab(&client, &form).expect("close first agent tab");
        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");
    }

    /// A snapshot with one agent session elsewhere in `root` (so a supervisor
    /// launch may adopt), a root workspace `wR` at sidebar position 1 (Herdr's
    /// root), and the `workspace.list` order backing it.
    fn adoption_snapshot(root: &str, root_tokens: Value, root_tabs: Value) -> Value {
        json!({
            "workspaces": [
                {
                    "workspace_id": "wR",
                    "label": "webshop-backend",
                    "tokens": root_tokens,
                    "worktree": {
                        "repo_root": root,
                        "checkout_path": root,
                        "is_linked_worktree": false
                    }
                },
                {
                    "workspace_id": "wW",
                    "label": "worktree-quiet-owl",
                    "worktree": {
                        "repo_root": root,
                        "checkout_path": "/worktrees/webshop-backend/quiet-owl",
                        "is_linked_worktree": true
                    }
                }
            ],
            "tabs": root_tabs,
            "agents": [{
                "agent": "codex",
                "agent_status": "working",
                "agent_session": {
                    "agent": "codex", "kind": "id", "source": "herdr:codex", "value": "worker-session"
                },
                "pane_id": "wW:p1", "workspace_id": "wW", "tab_id": "wW:t1"
            }],
            "panes": []
        })
    }

    fn workspace_list_json(root: &str) -> Value {
        json!({
            "workspaces": [
                {
                    "workspace_id": "wR", "number": 1,
                    "worktree": { "repo_root": root, "checkout_path": root, "is_linked_worktree": false }
                },
                {
                    "workspace_id": "wW", "number": 2,
                    "worktree": {
                        "repo_root": root,
                        "checkout_path": "/worktrees/webshop-backend/quiet-owl",
                        "is_linked_worktree": true
                    }
                }
            ]
        })
    }

    #[test]
    fn a_supervisor_launch_adopts_a_free_herdr_root_workspace() {
        let root = "/repos/webshop-backend";
        let (socket_path, server) = fake_herdr("adopt-free-root", move |listener| {
            for method in [
                "session.snapshot",
                "workspace.list",
                "workspace.report_metadata",
                "workspace.rename",
            ] {
                answer(&listener, |request| {
                    assert_eq!(request["method"], method);
                    let result = match method {
                        "session.snapshot" => json!({
                            "type": "session_snapshot",
                            "snapshot": adoption_snapshot(
                                root,
                                json!({}),
                                json!([{ "tab_id": "wR:t1", "workspace_id": "wR", "label": "1", "number": 1 }])
                            )
                        }),
                        "workspace.list" => json!({
                            "type": "workspace_list",
                            "workspaces": workspace_list_json(root)["workspaces"]
                        }),
                        "workspace.report_metadata" => {
                            assert_eq!(request["params"]["workspace_id"], "wR");
                            let tokens = &request["params"]["tokens"];
                            assert_eq!(tokens["corgi_workspace_role"], "project-main");
                            assert_eq!(tokens["corgi_project_main_tab"], "wR:t1");
                            assert_eq!(tokens["corgi_project_root"], root);
                            json!({ "type": "workspace_metadata_updated" })
                        }
                        "workspace.rename" => {
                            assert_eq!(request["params"]["workspace_id"], "wR");
                            assert_eq!(request["params"]["label"], "webshop-backend supervisor");
                            json!({ "type": "workspace_renamed" })
                        }
                        _ => unreachable!(),
                    };
                    json!({ "result": result })
                });
            }
        });

        let client = HerdrClient::from_socket_path(&socket_path);
        let main = corgi_project_main(&client, root, &mut Silent).expect("adopt the free root");
        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");

        assert_eq!(main.workspace_id, "wR");
        assert_eq!(main.root_tab_id, "wR:t1");
        assert_eq!(main.label, "webshop-backend supervisor");
    }

    #[test]
    fn a_supervisor_launch_falls_back_when_herdr_s_root_runs_an_agent() {
        let root = "/repos/webshop-backend";
        let (socket_path, server) = fake_herdr("adopt-busy-root", move |listener| {
            for method in [
                "session.snapshot",
                "workspace.list",
                "workspace.create",
                "workspace.report_metadata",
            ] {
                answer(&listener, |request| {
                    assert_eq!(request["method"], method);
                    let result = match method {
                        "session.snapshot" => {
                            let mut snapshot = adoption_snapshot(
                                root,
                                json!({}),
                                json!([{ "tab_id": "wR:t1", "workspace_id": "wR", "label": "1", "number": 1 }]),
                            );
                            // The root workspace also runs an agent: never
                            // adopted, however free its sidebar position.
                            snapshot["agents"].as_array_mut().unwrap().push(json!({
                                "agent": "claude",
                                "agent_status": "working",
                                "agent_session": {
                                    "agent": "claude", "kind": "id", "source": "herdr:claude", "value": "root-session"
                                },
                                "pane_id": "wR:p1", "workspace_id": "wR", "tab_id": "wR:t1"
                            }));
                            json!({ "type": "session_snapshot", "snapshot": snapshot })
                        }
                        "workspace.list" => json!({
                            "type": "workspace_list",
                            "workspaces": workspace_list_json(root)["workspaces"]
                        }),
                        // No existing Corgi project-main for this project yet,
                        // so today's behaviour creates one, never touching wR.
                        "workspace.create" => {
                            assert_eq!(request["params"]["cwd"], root);
                            json!({
                                "type": "workspace_created",
                                "workspace": { "workspace_id": "wNew", "label": "webshop-backend supervisor" },
                                "tab": { "tab_id": "wNew:t1" },
                                "root_pane": { "pane_id": "wNew:p1" }
                            })
                        }
                        "workspace.report_metadata" => {
                            assert_eq!(request["params"]["workspace_id"], "wNew");
                            json!({ "type": "workspace_metadata_updated" })
                        }
                        _ => unreachable!(),
                    };
                    json!({ "result": result })
                });
            }
        });

        let client = HerdrClient::from_socket_path(&socket_path);
        let main = corgi_project_main(&client, root, &mut Silent)
            .expect("fall back to Corgi's own project workspace");
        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");

        assert_eq!(
            main.workspace_id, "wNew",
            "a busy Herdr root is never adopted"
        );
    }

    #[test]
    fn a_supervisor_launch_is_unaffected_when_herdr_s_root_is_already_corgi_s() {
        let root = "/repos/webshop-backend";
        let (socket_path, server) = fake_herdr("adopt-supervisors", move |listener| {
            for method in ["session.snapshot", "workspace.rename", "workspace.list"] {
                answer(&listener, |request| {
                    assert_eq!(request["method"], method);
                    let result = match method {
                        "session.snapshot" => {
                            // Marked, but still under its old bare label, as
                            // `relabel_project_mains` finds and fixes today.
                            let snapshot = adoption_snapshot(
                                root,
                                json!({
                                    CORGI_WORKSPACE_ROLE_TOKEN: CORGI_PROJECT_MAIN_ROLE,
                                    CORGI_PROJECT_MAIN_TAB_TOKEN: "wR:t1"
                                }),
                                json!([{ "tab_id": "wR:t1", "workspace_id": "wR", "label": "1", "number": 1 }]),
                            );
                            json!({ "type": "session_snapshot", "snapshot": snapshot })
                        }
                        "workspace.rename" => {
                            assert_eq!(request["params"]["workspace_id"], "wR");
                            assert_eq!(request["params"]["label"], "webshop-backend supervisor");
                            json!({ "type": "workspace_renamed" })
                        }
                        "workspace.list" => json!({
                            "type": "workspace_list",
                            "workspaces": workspace_list_json(root)["workspaces"]
                        }),
                        _ => unreachable!(),
                    };
                    json!({ "result": result })
                });
            }
        });

        let client = HerdrClient::from_socket_path(&socket_path);
        let main = corgi_project_main(&client, root, &mut Silent)
            .expect("an already-marked root is used as it is today");
        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");

        // No new mark, and no adoption: the fake answers nothing else.
        assert_eq!(main.workspace_id, "wR");
        assert_eq!(main.label, "webshop-backend supervisor");
    }

    #[test]
    fn adopting_a_free_root_retires_an_idle_duplicate_project_main() {
        let root = "/repos/webshop-backend";
        let (socket_path, server) = fake_herdr("adopt-retire", move |listener| {
            for method in [
                "session.snapshot",
                "workspace.list",
                "workspace.report_metadata",
                "workspace.rename",
                "workspace.close",
            ] {
                answer(&listener, |request| {
                    assert_eq!(request["method"], method);
                    let result = match method {
                        "session.snapshot" => {
                            let mut snapshot = adoption_snapshot(
                                root,
                                json!({}),
                                json!([{ "tab_id": "wR:t1", "workspace_id": "wR", "label": "1", "number": 1 }]),
                            );
                            // An older, unused Corgi project-main for the same
                            // repository, the kind adoption should retire.
                            snapshot["workspaces"].as_array_mut().unwrap().push(json!({
                                "workspace_id": "wOld",
                                "label": "webshop-backend supervisor",
                                "tokens": {
                                    CORGI_WORKSPACE_ROLE_TOKEN: CORGI_PROJECT_MAIN_ROLE,
                                    CORGI_PROJECT_MAIN_TAB_TOKEN: "wOld:t1"
                                },
                                "worktree": {
                                    "repo_root": root, "checkout_path": root, "is_linked_worktree": false
                                }
                            }));
                            json!({ "type": "session_snapshot", "snapshot": snapshot })
                        }
                        "workspace.list" => json!({
                            "type": "workspace_list",
                            "workspaces": workspace_list_json(root)["workspaces"]
                        }),
                        "workspace.report_metadata" => {
                            json!({ "type": "workspace_metadata_updated" })
                        }
                        "workspace.rename" => json!({ "type": "workspace_renamed" }),
                        "workspace.close" => {
                            assert_eq!(request["params"]["workspace_id"], "wOld");
                            json!({
                                "type": "workspace_closed",
                                "workspace": { "workspace_id": "wOld", "label": "webshop-backend supervisor" }
                            })
                        }
                        _ => unreachable!(),
                    };
                    json!({ "result": result })
                });
            }
        });

        let client = HerdrClient::from_socket_path(&socket_path);
        let main = corgi_project_main(&client, root, &mut Silent).expect("adopt the free root");
        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");

        assert_eq!(main.workspace_id, "wR");
    }
}

#[cfg(test)]
mod duplicate_tests {
    use std::fs;

    use serde_json::{Value, json};

    use crate::test_support::{answer, fake_herdr, test_app};

    use super::*;

    const ROOT: &str = "/repos/corgi";

    fn main_json(id: &str) -> Value {
        json!({
            "workspace_id": id,
            "label": "corgi supervisor",
            "tokens": {
                CORGI_WORKSPACE_ROLE_TOKEN: CORGI_PROJECT_MAIN_ROLE,
                CORGI_PROJECT_MAIN_TAB_TOKEN: format!("{id}:t1"),
                CORGI_PROJECT_ROOT_HASH_TOKEN: project_root_digest(ROOT)
            },
            "worktree": { "repo_root": ROOT, "checkout_path": ROOT, "is_linked_worktree": false }
        })
    }

    fn main_workspace(id: &str) -> WorkspaceInfo {
        serde_json::from_value(main_json(id)).expect("workspace")
    }

    #[test]
    fn of_two_project_workspaces_the_one_the_supervisor_runs_in_is_the_projects() {
        // The empty duplicate a spawn created while the marks were gone comes
        // first in snapshot order; the supervisor's workspace wins all the same.
        let workspaces = [main_workspace("w8Y"), main_workspace("w70")];
        let supervisor = AgentInfo {
            pane_id: "w70:p2".into(),
            workspace_id: "w70".into(),
            name: Some("supervisor-corgi".into()),
            tokens: [(
                supervisor::SUPERVISOR_TOKEN.into(),
                "supervisor-corgi".into(),
            )]
            .into(),
            ..AgentInfo::default()
        };
        let worker = AgentInfo {
            workspace_id: "w8Y".into(),
            name: Some("w-worker".into()),
            ..AgentInfo::default()
        };
        let agents = [worker, supervisor];
        let supervisors = supervisor_workspaces(&agents);
        assert_eq!(supervisors, ["w70"]);
        let main = project_main_workspace(&workspaces, &supervisors, ROOT).expect("main");
        assert_eq!(
            (main.workspace_id.as_str(), main.root_tab_id.as_str()),
            ("w70", "w70:t1")
        );
        // Without a supervisor, snapshot order decides, as before.
        let main = project_main_workspace(&workspaces, &[], ROOT).expect("main");
        assert_eq!(main.workspace_id, "w8Y");
    }

    #[test]
    fn an_empty_duplicate_project_workspace_is_retired_and_a_busy_one_kept() {
        let (socket_path, server) = fake_herdr("proj-dup", move |listener| {
            answer(&listener, |request| {
                assert_eq!(request["method"], "session.snapshot");
                json!({ "result": { "type": "session_snapshot", "snapshot": {
                    "workspaces": [main_json("w8Y"), main_json("w70"), main_json("w90")],
                    "tabs": [
                        { "tab_id": "w8Y:t1", "workspace_id": "w8Y", "label": "1", "number": 1 },
                        { "tab_id": "w70:t1", "workspace_id": "w70", "label": "1", "number": 1 },
                        { "tab_id": "w90:t1", "workspace_id": "w90", "label": "1", "number": 1 }
                    ],
                    "panes": [
                        { "pane_id": "w8Y:p1", "workspace_id": "w8Y", "tab_id": "w8Y:t1" },
                        { "pane_id": "w70:p1", "workspace_id": "w70", "tab_id": "w70:t1" },
                        { "pane_id": "w90:p1", "workspace_id": "w90", "tab_id": "w90:t1" }
                    ],
                    "agents": [
                        {
                            "pane_id": "w70:p1", "workspace_id": "w70", "tab_id": "w70:t1",
                            "agent": "claude", "name": "supervisor-corgi",
                            "agent_session": { "value": "s1" },
                            "tokens": { "corgi_handler": "supervisor-corgi" }
                        },
                        {
                            "pane_id": "w90:p1", "workspace_id": "w90", "tab_id": "w90:t1",
                            "agent": "claude", "name": "w-shared",
                            "agent_session": { "value": "s2" }
                        }
                    ]
                }}})
            });
            answer(&listener, |request| {
                assert_eq!(request["method"], "workspace.close");
                assert_eq!(request["params"]["workspace_id"], "w8Y");
                json!({ "result": { "type": "workspace_closed" } })
            });
        });

        let mut app = test_app();
        app.client = HerdrClient::from_socket_path(&socket_path);
        assert_eq!(
            app.retire_duplicate_project_mains(ROOT),
            ["corgi supervisor"]
        );

        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");
    }
}
