//! Small synchronous client for Herdr's newline-delimited JSON socket API.
//!
//! A Corgi pane receives `HERDR_SOCKET_PATH` from Herdr.  Requests are kept
//! deliberately short-lived: each call opens a fresh Unix socket, writes one
//! JSON request, and reads one JSON response.  That makes the client suitable
//! for use from the dashboard's background worker without owning a runtime or
//! a long-lived connection.

use std::{
    collections::BTreeMap,
    env, fmt,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    model::{AgentInfo, WorkspaceInfo, WorkspaceWorktreeInfo},
    paths::socket_state_file,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const CONFIRMED_PROMPT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

/// The longest metadata token value Herdr keeps, in characters. Herdr cuts
/// longer values short without reporting an error, so a value that must be
/// read back exactly has to fit.
pub const METADATA_VALUE_MAX_CHARS: usize = 80;

/// A direct client for the Herdr server belonging to the current plugin pane.
#[derive(Clone, Debug)]
pub struct HerdrClient {
    socket_path: Arc<PathBuf>,
    next_request_id: Arc<AtomicU64>,
    /// Where Corgi records the metadata tokens it sets in this Herdr session,
    /// so it can put them back after Herdr loses them (see
    /// `app::markers`). `None` keeps no record.
    marker_record: Option<Arc<PathBuf>>,
}

impl HerdrClient {
    /// Builds a client using the socket injected by Herdr into plugin panes.
    ///
    /// We intentionally do not guess a fallback socket here. A plugin must
    /// operate on the session that opened it, rather than accidentally
    /// attaching to an unrelated local session.
    pub fn from_env() -> Result<Self> {
        let socket_path = env::var_os("HERDR_SOCKET_PATH")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .context("HERDR_SOCKET_PATH is not set; open Corgi from a Herdr plugin pane")?;
        Ok(Self::from_socket_path(socket_path))
    }

    /// Builds a client for a known Unix socket. Primarily useful for tests and
    /// for callers that receive a socket path through another trusted channel.
    ///
    /// Its marker record is the session's file under Corgi's state directory,
    /// except in tests, which keep none unless they ask for one with
    /// [`HerdrClient::with_marker_record`], so they never write to the
    /// developer's own state.
    pub fn from_socket_path(socket_path: impl Into<PathBuf>) -> Self {
        let socket_path = socket_path.into();
        let marker_record = if cfg!(test) {
            None
        } else {
            socket_state_file("markers", &socket_path, "json")
        };
        Self {
            socket_path: Arc::new(socket_path),
            next_request_id: Arc::new(AtomicU64::new(1)),
            marker_record: marker_record.map(Arc::new),
        }
    }

    /// The same client, recording its markers at `path` instead.
    pub fn with_marker_record(mut self, path: impl Into<PathBuf>) -> Self {
        self.marker_record = Some(Arc::new(path.into()));
        self
    }

    pub fn socket_path(&self) -> &Path {
        self.socket_path.as_ref()
    }

    /// Where Corgi records the tokens it sets in this Herdr session, if it
    /// keeps a record.
    pub fn marker_record(&self) -> Option<&Path> {
        self.marker_record.as_deref().map(PathBuf::as_path)
    }

    /// Returns the live session state, including all currently detected agents.
    pub fn snapshot(&self) -> Result<SessionSnapshot> {
        let mut result = self.request("session.snapshot", json!({}))?;
        expect_result_type(&result, "session_snapshot")?;
        serde_json::from_value(result["snapshot"].take())
            .context("Herdr returned an invalid session snapshot")
    }

    /// Every workspace in Herdr's own sidebar order. A session snapshot's
    /// `workspaces` happen to come back in that order too, but nothing
    /// documents that they must, so anything that needs the order itself
    /// (finding the workspace Herdr treats as a repository's root) asks here
    /// instead of relying on it.
    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceListEntry>> {
        let mut result = self.request("workspace.list", json!({}))?;
        expect_result_type(&result, "workspace_list")?;
        serde_json::from_value(result["workspaces"].take())
            .context("Herdr returned an invalid workspace list")
    }

    /// Reads one recognized agent's terminal buffer.
    pub fn read_agent(
        &self,
        target: &str,
        source: ReadSource,
        lines: Option<u32>,
    ) -> Result<PaneRead> {
        self.read_agent_as(target, source, lines, false)
    }

    /// Reads one recognized agent's whole visible screen with its styling,
    /// as ANSI escapes, for a caller that tells dim text from plain.
    pub fn read_agent_styled(&self, target: &str) -> Result<PaneRead> {
        self.read_agent_as(target, ReadSource::Visible, None, true)
    }

    fn read_agent_as(
        &self,
        target: &str,
        source: ReadSource,
        lines: Option<u32>,
        ansi: bool,
    ) -> Result<PaneRead> {
        let mut result = self.request(
            "agent.read",
            json!({
                "target": target,
                "source": source,
                "lines": lines,
                "format": if ansi { "ansi" } else { "text" },
                "strip_ansi": !ansi,
            }),
        )?;
        expect_result_type(&result, "pane_read")?;
        serde_json::from_value(result["read"].take())
            .with_context(|| format!("Herdr returned an invalid read for agent {target:?}"))
    }

    /// Sends a prompt to an existing agent without waiting for it to finish.
    pub fn prompt_agent(&self, target: &str, text: &str) -> Result<AgentInfo> {
        let result = self.request(
            "agent.prompt",
            json!({
                "target": target,
                "text": text,
                "wait": null,
            }),
        )?;
        parse_agent_result(result, "agent_prompted")
    }

    /// Waits for the first task to make the agent change state. A successful
    /// write alone can leave the launch looking complete when Enter was not
    /// processed by the newly started agent.
    pub fn prompt_agent_confirmed(&self, target: &str, text: &str) -> Result<AgentInfo> {
        let result = self.request_with_timeout(
            "agent.prompt",
            json!({
                "target": target,
                "text": text,
                "wait": {
                    "timeout_ms": 7000,
                    "until": ["working", "blocked", "done", "idle"],
                },
            }),
            CONFIRMED_PROMPT_TIMEOUT,
        )?;
        parse_agent_result(result, "agent_prompted")
    }

    /// Brings an agent's pane into focus and returns its current metadata.
    fn focus_agent(&self, target: &str) -> Result<AgentInfo> {
        let result = self.request("agent.focus", json!({ "target": target }))?;
        parse_agent_result(result, "agent_info")
    }

    /// Focuses any pane, including the Corgi plugin pane itself.
    fn focus_pane(&self, pane_id: &str) -> Result<()> {
        let result = self.request("pane.focus", json!({ "pane_id": pane_id }))?;
        expect_result_type(&result, "pane_info")
    }

    fn focus_workspace(&self, workspace_id: &str) -> Result<()> {
        let result = self.request("workspace.focus", json!({ "workspace_id": workspace_id }))?;
        expect_result_type(&result, "workspace_info")
    }

    fn focus_tab(&self, tab_id: &str) -> Result<()> {
        let result = self.request("tab.focus", json!({ "tab_id": tab_id }))?;
        expect_result_type(&result, "tab_info")
    }

    /// Brings an agent into view from anywhere in the session.
    ///
    /// `agent.focus` alone only updates Herdr's bookkeeping; the client changes
    /// what it shows on `workspace.focus` and `tab.focus`. Walk down through
    /// those first so focusing works across workspaces.
    pub fn go_to_agent(&self, agent: &AgentInfo) -> Result<AgentInfo> {
        self.walk_to(&agent.workspace_id, &agent.tab_id)?;
        self.focus_agent(&agent.pane_id)
    }

    /// Brings any pane into view; see [`HerdrClient::go_to_agent`].
    pub fn go_to_pane(&self, pane: &PaneInfo) -> Result<()> {
        self.walk_to(&pane.workspace_id, &pane.tab_id)?;
        self.focus_pane(&pane.pane_id)
    }

    fn walk_to(&self, workspace_id: &str, tab_id: &str) -> Result<()> {
        if !workspace_id.is_empty() {
            self.focus_workspace(workspace_id)?;
        }
        if !tab_id.is_empty() {
            self.focus_tab(tab_id)?;
        }
        Ok(())
    }

    /// Closes a workspace and all of its tabs and panes.
    pub fn close_workspace(&self, workspace_id: &str) -> Result<()> {
        let result = self.request("workspace.close", json!({ "workspace_id": workspace_id }))?;
        expect_result_type(&result, "workspace_closed")
    }

    /// Patches metadata tokens on a pane: `Some` sets a value, `None` sends
    /// JSON null to remove the key, and omitted keys remain unchanged.
    /// Herdr keeps them with the pane, across
    /// tab moves and the agents that come and go in it, and reports them in
    /// that pane's agent `tokens`, but not across its own restart. Corgi's
    /// own marks go through `app::markers`, which records them too.
    pub fn report_pane_metadata(
        &self,
        pane_id: &str,
        source: &str,
        tokens: &[(&str, Option<&str>)],
    ) -> Result<()> {
        let tokens = tokens
            .iter()
            .map(|(name, value)| ((*name).to_string(), json!(value)))
            .collect::<serde_json::Map<_, _>>();
        self.request(
            "pane.report_metadata",
            json!({ "pane_id": pane_id, "source": source, "tokens": tokens }),
        )
        .map(|_| ())
    }

    /// Presses logical keys, such as `down` or `enter`, in an agent's UI.
    pub fn send_agent_keys(&self, target: &str, keys: &[&str]) -> Result<()> {
        self.request("agent.send_keys", json!({ "target": target, "keys": keys }))
            .map(|_| ())
    }

    /// The directory an installed Herdr plugin runs from, if it is installed.
    pub fn plugin_root(&self, plugin_id: &str) -> Result<Option<PathBuf>> {
        let result = self.request("plugin.list", json!({}))?;
        Ok(result["plugins"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|plugin| plugin["plugin_id"] == plugin_id)
            .and_then(|plugin| plugin["plugin_root"].as_str())
            .map(PathBuf::from))
    }

    /// Changes the user-visible label of a workspace.
    pub fn rename_workspace(&self, workspace_id: &str, label: &str) -> Result<()> {
        self.request(
            "workspace.rename",
            json!({ "workspace_id": workspace_id, "label": label }),
        )
        .map(|_| ())
    }

    /// Removes a linked worktree checkout with `git worktree remove` and closes
    /// its workspace. Fails with `dirty_worktree_requires_force` for a checkout
    /// with modified or untracked files unless `force` is set; the branch is
    /// never deleted.
    pub fn remove_worktree(&self, workspace_id: &str, force: bool) -> Result<RemovedWorktree> {
        let result = self.request(
            "worktree.remove",
            json!({ "workspace_id": workspace_id, "force": force }),
        )?;
        expect_result_type(&result, "worktree_removed")?;
        serde_json::from_value(result).context("Herdr returned an invalid removed worktree")
    }

    /// Creates an unfocused workspace with its initial tab and shell pane.
    pub fn create_workspace(
        &self,
        cwd: Option<&Path>,
        label: Option<&str>,
    ) -> Result<CreatedWorkspace> {
        let cwd = cwd.map(|path| path.to_string_lossy().into_owned());
        let result = self.request(
            "workspace.create",
            json!({
                "cwd": cwd,
                "label": label,
                "focus": false,
            }),
        )?;
        expect_result_type(&result, "workspace_created")?;
        parse_created_workspace(&result)
    }

    /// Opens an unfocused tab and its initial shell pane in `workspace_id`.
    pub fn create_tab(
        &self,
        workspace_id: &str,
        cwd: Option<&Path>,
        label: Option<&str>,
    ) -> Result<CreatedTab> {
        let cwd = cwd.map(|path| path.to_string_lossy().into_owned());
        let result = self.request(
            "tab.create",
            json!({
                "workspace_id": workspace_id,
                "cwd": cwd,
                "label": label,
                "focus": false,
            }),
        )?;
        expect_result_type(&result, "tab_created")?;
        parse_created_tab(&result)
    }

    /// Labels a workspace with metadata Corgi can recognize after its own
    /// dashboard has been closed and reopened. Metadata ownership is safer
    /// than inferring it from a user-visible label. Like a pane's, the tokens
    /// do not survive a Herdr restart; Corgi's own marks go through
    /// `app::markers`, which records them too.
    pub fn report_workspace_metadata(
        &self,
        workspace_id: &str,
        source: &str,
        tokens: &[(&str, &str)],
    ) -> Result<()> {
        let tokens: Map<String, Value> = tokens
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect();
        self.request(
            "workspace.report_metadata",
            json!({
                "workspace_id": workspace_id,
                "source": source,
                "tokens": tokens,
            }),
        )?;
        Ok(())
    }

    /// Resolves the Git repository a directory belongs to.
    ///
    /// Herdr only creates worktrees from the repository's primary checkout, so
    /// callers use `repo_root` even when `cwd` is inside a linked worktree.
    /// Fails with `not_git_worktree` for directories outside any Git work tree.
    pub fn worktree_source(&self, cwd: &Path) -> Result<WorktreeSource> {
        let mut result = self.request("worktree.list", json!({ "cwd": cwd.to_string_lossy() }))?;
        expect_result_type(&result, "worktree_list")?;
        serde_json::from_value(result["source"].take())
            .context("Herdr returned an invalid worktree source")
    }

    /// Creates a new Git worktree checkout from an explicit project workspace
    /// and opens it as an unfocused child workspace.
    ///
    /// Herdr picks a fresh branch name and a checkout path under its
    /// configured worktrees directory, so two agents never share a checkout.
    pub fn create_worktree(&self, workspace_id: &str) -> Result<CreatedWorkspace> {
        let mut result = self.request(
            "worktree.create",
            json!({
                "workspace_id": workspace_id,
                "focus": false,
            }),
        )?;
        expect_result_type(&result, "worktree_created")?;
        let mut created = parse_created_workspace(&result)?;
        created.worktree = serde_json::from_value(result["worktree"].take())
            .context("Herdr returned an invalid created worktree")?;
        Ok(created)
    }

    /// Starts a supported coding agent in an available shell pane, passing
    /// `args` on to its command line.
    pub fn start_agent(
        &self,
        name: &str,
        kind: &str,
        args: &[String],
        pane_id: &str,
    ) -> Result<StartedAgent> {
        let mut result = self.request(
            "agent.start",
            json!({
                "name": name,
                "kind": kind,
                "pane_id": pane_id,
                "args": args,
            }),
        )?;
        expect_result_type(&result, "agent_started")?;
        let agent = serde_json::from_value(result["agent"].take())
            .context("Herdr returned an invalid started agent")?;
        let argv = serde_json::from_value(result["argv"].take())
            .context("Herdr returned invalid started-agent arguments")?;
        Ok(StartedAgent { agent, argv })
    }

    /// Closes one tab and its panes without affecting the other agent tabs in
    /// a Corgi-managed project workspace.
    pub fn close_tab(&self, tab_id: &str) -> Result<()> {
        let result = self.request("tab.close", json!({ "tab_id": tab_id }))?;
        expect_result_type(&result, "tab_closed")
    }

    fn request(&self, method: &str, params: Value) -> Result<Value> {
        self.request_with_timeout(method, params, REQUEST_TIMEOUT)
    }

    fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let mut stream = UnixStream::connect(self.socket_path()).with_context(|| {
            format!(
                "cannot connect to Herdr at {}",
                self.socket_path().display()
            )
        })?;
        stream
            .set_read_timeout(Some(timeout))
            .context("cannot set Herdr socket read timeout")?;
        stream
            .set_write_timeout(Some(REQUEST_TIMEOUT))
            .context("cannot set Herdr socket write timeout")?;

        let id = format!(
            "corgi:{}",
            self.next_request_id.fetch_add(1, Ordering::Relaxed)
        );
        let mut request = serde_json::to_vec(&json!({
            "id": id,
            "method": method,
            "params": params,
        }))
        .with_context(|| format!("cannot encode Herdr {method} request"))?;
        request.push(b'\n');
        stream
            .write_all(&request)
            .with_context(|| format!("cannot write Herdr {method} request"))?;
        stream
            .flush()
            .with_context(|| format!("cannot flush Herdr {method} request"))?;

        let mut response = Vec::new();
        let bytes_read = BufReader::new(stream)
            .take(MAX_RESPONSE_BYTES + 1)
            .read_until(b'\n', &mut response)
            .with_context(|| format!("cannot read Herdr {method} response"))?;
        ensure!(
            bytes_read != 0,
            "Herdr closed the connection without replying to {method}"
        );
        ensure!(
            response.len() as u64 <= MAX_RESPONSE_BYTES,
            "Herdr {method} response exceeded the {} MiB limit",
            MAX_RESPONSE_BYTES / (1024 * 1024)
        );
        ensure!(
            response.last() == Some(&b'\n'),
            "Herdr closed the connection before completing its {method} response"
        );

        let response: Value = serde_json::from_slice(&response)
            .with_context(|| format!("Herdr sent invalid JSON for {method}"))?;
        let response_id = response
            .get("id")
            .and_then(Value::as_str)
            .context("Herdr response did not include a string request ID")?;
        ensure!(
            response_id == id,
            "Herdr response ID {response_id:?} did not match request ID {id:?}"
        );

        if let Some(error) = response.get("error") {
            let code = error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("herdr_error");
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Herdr returned an error without a message");
            return Err(HerdrError {
                method: method.to_string(),
                code: code.to_string(),
                message: message.to_string(),
            }
            .into());
        }

        response
            .get("result")
            .cloned()
            .context("Herdr response did not include a result or error")
    }
}

/// An error reply from Herdr: the request reached Herdr, which refused it
/// with a machine-readable `code`. Callers that react to a particular refusal
/// ask for its code with [`HerdrError::code_of`] rather than searching the
/// error's text. Failures to reach Herdr or to understand its reply are plain
/// `anyhow` errors; no caller tells them apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrError {
    pub method: String,
    pub code: String,
    pub message: String,
}

impl HerdrError {
    /// The code of the Herdr error reply anywhere in `error`'s chain, if the
    /// error came from one.
    pub fn code_of(error: &anyhow::Error) -> Option<&str> {
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<Self>())
            .map(|error| error.code.as_str())
    }

    /// `agent.start` met a freshly created pane whose shell has not reported
    /// itself available yet.
    pub(crate) fn is_pane_not_ready(error: &anyhow::Error) -> bool {
        Self::code_of(error) == Some("agent_pane_busy")
    }

    /// A just-started agent cannot take a prompt yet.
    pub(crate) fn is_transient_startup(error: &anyhow::Error) -> bool {
        matches!(
            Self::code_of(error),
            Some("agent_not_ready" | "agent_launch_pending")
        )
    }

    /// The agent is waiting for an answer to a question in its pane.
    pub(crate) fn is_agent_blocked(error: &anyhow::Error) -> bool {
        Self::code_of(error) == Some("agent_blocked")
    }

    /// The directory is outside any Git work tree.
    pub(crate) fn is_not_git_worktree(error: &anyhow::Error) -> bool {
        Self::code_of(error) == Some("not_git_worktree")
    }

    /// `worktree.remove` refuses a checkout with changes unless forced.
    pub(crate) fn is_dirty_worktree(error: &anyhow::Error) -> bool {
        Self::code_of(error) == Some("dirty_worktree_requires_force")
    }
}

impl fmt::Display for HerdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Herdr {} failed ({}): {}",
            self.method, self.code, self.message
        )
    }
}

impl std::error::Error for HerdrError {}

/// Which terminal representation to read from an agent pane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadSource {
    #[serde(rename = "visible")]
    Visible,
    #[serde(rename = "recent")]
    Recent,
    #[serde(rename = "recent_unwrapped")]
    RecentUnwrapped,
    #[serde(rename = "detection")]
    #[default]
    Detection,
}

/// The subset of a Herdr session snapshot Corgi needs to render its dashboard.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SessionSnapshot {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub protocol: u32,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceInfo>,
    #[serde(default)]
    pub agents: Vec<AgentInfo>,
    /// Every tab, including empty shell tabs in a project main workspace.
    #[serde(default)]
    pub tabs: Vec<TabInfo>,
    /// Every pane, including plugin panes such as Corgi itself.
    #[serde(default)]
    pub panes: Vec<PaneInfo>,
    #[serde(default)]
    pub focused_workspace_id: Option<String>,
    #[serde(default)]
    pub focused_tab_id: Option<String>,
    #[serde(default)]
    pub focused_pane_id: Option<String>,
}

/// Location and identity of any pane in the session snapshot.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PaneInfo {
    #[serde(default)]
    pub pane_id: String,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub tab_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// The metadata tokens every source set on the pane, the same ones an
    /// agent in it reports, and kept here when no agent is in it.
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
}

/// Location and identity of any tab in the session snapshot.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TabInfo {
    #[serde(default)]
    pub tab_id: String,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
    /// This tab's position among its workspace's tabs, in Herdr's own order.
    /// The lowest is the workspace's root tab.
    #[serde(default)]
    pub number: u32,
}

/// One workspace as `workspace.list` reports it: with the sidebar position a
/// session snapshot does not promise to preserve.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorkspaceListEntry {
    #[serde(default)]
    pub workspace_id: String,
    /// This workspace's position in Herdr's sidebar. The lowest-numbered
    /// workspace whose checkout is a repository's primary one is the
    /// workspace Herdr treats as that repository's root.
    #[serde(default)]
    pub number: u32,
    #[serde(default)]
    pub worktree: Option<WorkspaceWorktreeInfo>,
}

/// Text and revision information returned by `agent.read`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PaneRead {
    #[serde(default)]
    pub pane_id: String,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub tab_id: String,
    #[serde(default)]
    pub source: ReadSource,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub truncated: bool,
}

/// The workspace, tab, and shell pane created together by `workspace.create`
/// or `worktree.create`.
#[derive(Debug, Clone)]
pub struct CreatedWorkspace {
    pub workspace_id: String,
    pub workspace: WorkspaceInfo,
    pub tab_id: String,
    pub root_pane_id: String,
    /// The Git checkout behind the workspace when it came from `worktree.create`.
    pub worktree: Option<WorktreeInfo>,
}

/// The tab and root pane created together by `tab.create`.
#[derive(Debug, Clone)]
pub struct CreatedTab {
    pub tab_id: String,
    pub root_pane_id: String,
}

/// The checkout deleted by `worktree.remove`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RemovedWorktree {
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub forced: bool,
}

/// The repository a `worktree.list` query resolved its `cwd` to.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorktreeSource {
    #[serde(default)]
    pub repo_name: String,
    #[serde(default)]
    pub repo_root: String,
    #[serde(default)]
    pub source_checkout_path: String,
    #[serde(default)]
    pub source_workspace_id: Option<String>,
}

/// One Git checkout as reported by Herdr's worktree methods.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorktreeInfo {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub is_linked_worktree: bool,
}

fn parse_created_workspace(result: &Value) -> Result<CreatedWorkspace> {
    let workspace: WorkspaceInfo = serde_json::from_value(result["workspace"].clone())
        .context("Herdr returned an invalid created workspace")?;
    let tab_id = required_string(result, &["tab", "tab_id"], "created workspace tab ID")?;
    let root_pane_id = required_string(
        result,
        &["root_pane", "pane_id"],
        "created workspace root pane ID",
    )?;

    Ok(CreatedWorkspace {
        workspace_id: workspace.workspace_id.clone(),
        workspace,
        tab_id,
        root_pane_id,
        worktree: None,
    })
}

fn parse_created_tab(result: &Value) -> Result<CreatedTab> {
    Ok(CreatedTab {
        tab_id: required_string(result, &["tab", "tab_id"], "created tab ID")?,
        root_pane_id: required_string(result, &["root_pane", "pane_id"], "created tab pane ID")?,
    })
}

/// The recognized agent and command line reported by `agent.start`.
#[derive(Debug, Clone)]
pub struct StartedAgent {
    pub agent: AgentInfo,
    pub argv: Vec<String>,
}

fn parse_agent_result(mut result: Value, expected_type: &str) -> Result<AgentInfo> {
    expect_result_type(&result, expected_type)?;
    serde_json::from_value(result["agent"].take())
        .with_context(|| format!("Herdr returned an invalid {expected_type} agent"))
}

fn expect_result_type(result: &Value, expected: &str) -> Result<()> {
    let actual = result
        .get("type")
        .and_then(Value::as_str)
        .context("Herdr result did not include a type")?;
    ensure!(
        actual == expected,
        "Herdr returned result type {actual:?}; expected {expected:?}"
    );
    Ok(())
}

fn required_string(result: &Value, path: &[&str], description: &str) -> Result<String> {
    let mut value = result;
    for key in path {
        value = value
            .get(*key)
            .with_context(|| format!("Herdr result did not include {description}"))?;
    }
    value
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("Herdr returned a non-string {description}"))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use crate::test_support::{answer, fake_herdr};

    use serde_json::json;

    use super::{HerdrClient, HerdrError};

    #[test]
    fn snapshot_uses_newline_json_and_deserializes_agents() {
        let (socket_path, server) = fake_herdr("herdr-snapshot", move |listener| {
            answer(&listener, |request| {
                assert_eq!(request["method"].as_str(), Some("session.snapshot"));

                json!({
                    "result": {
                        "type": "session_snapshot",
                        "snapshot": {
                            "version": "test-20",
                            "protocol": 20,
                            "workspaces": [{ "workspace_id": "w1", "label": "Corgi" }],
                            "agents": [{
                                "agent": "codex",
                                "agent_status": "working",
                                "pane_id": "w1:p1",
                                "workspace_id": "w1",
                                "tab_id": "w1:t1"
                            }]
                        }
                    }
                })
            });
        });

        let snapshot = HerdrClient::from_socket_path(&socket_path)
            .snapshot()
            .expect("decode snapshot");
        server.join().expect("fake server panicked");
        fs::remove_file(&socket_path).expect("remove fake socket");

        assert_eq!(snapshot.version, "test-20");
        assert_eq!(snapshot.workspaces[0].label, "Corgi");
        assert_eq!(snapshot.agents[0].pane_id, "w1:p1");
    }

    #[test]
    fn server_errors_include_the_herdr_code_and_message() {
        let (socket_path, server) = fake_herdr("herdr-error", move |listener| {
            answer(&listener, |_| {
                json!({
                    "error": { "code": "agent_blocked", "message": "approval required" }
                })
            });
        });

        let error = HerdrClient::from_socket_path(&socket_path)
            .snapshot()
            .expect_err("error response should fail");
        server.join().expect("fake server panicked");
        fs::remove_file(&socket_path).expect("remove fake socket");

        assert_eq!(
            error.to_string(),
            "Herdr session.snapshot failed (agent_blocked): approval required"
        );
        assert_eq!(HerdrError::code_of(&error), Some("agent_blocked"));
    }

    #[test]
    fn herdr_error_codes_are_found_through_added_context() {
        let error = anyhow::Error::from(HerdrError {
            method: "agent.start".into(),
            code: "agent_pane_busy".into(),
            message: "not an available shell".into(),
        })
        .context("start codex in w1:p1");

        assert_eq!(HerdrError::code_of(&error), Some("agent_pane_busy"));
        assert_eq!(
            format!("{error:#}"),
            "start codex in w1:p1: Herdr agent.start failed (agent_pane_busy): not an available shell"
        );
        // Text that merely mentions a code is not a Herdr error reply.
        assert_eq!(
            HerdrError::code_of(&anyhow::anyhow!(
                "Herdr agent.start failed (agent_pane_busy): x"
            )),
            None
        );
    }

    fn reply(method: &str, code: &str) -> anyhow::Error {
        HerdrError {
            method: method.into(),
            code: code.into(),
            message: "refused".into(),
        }
        .into()
    }

    #[test]
    fn only_a_shell_that_is_still_starting_delays_agent_start() {
        assert!(HerdrError::is_pane_not_ready(&reply(
            "agent.start",
            "agent_pane_busy"
        )));
        assert!(!HerdrError::is_pane_not_ready(&reply(
            "agent.start",
            "unknown_agent_kind"
        )));
    }

    #[test]
    fn only_startup_races_are_retried() {
        assert!(HerdrError::is_transient_startup(&reply(
            "agent.prompt",
            "agent_not_ready"
        )));
        assert!(HerdrError::is_transient_startup(&reply(
            "agent.prompt",
            "agent_launch_pending"
        )));
        assert!(!HerdrError::is_transient_startup(&reply(
            "agent.prompt",
            "agent_blocked"
        )));
        assert!(HerdrError::is_agent_blocked(&reply(
            "agent.prompt",
            "agent_blocked"
        )));
    }

    #[test]
    fn only_non_git_directories_fall_back_to_plain_workspaces() {
        assert!(HerdrError::is_not_git_worktree(&reply(
            "worktree.list",
            "not_git_worktree"
        )));
        assert!(!HerdrError::is_not_git_worktree(&reply(
            "worktree.list",
            "linked_worktree_source"
        )));
    }

    #[test]
    fn only_dirty_checkouts_ask_before_forcing_removal() {
        assert!(HerdrError::is_dirty_worktree(&reply(
            "worktree.remove",
            "dirty_worktree_requires_force"
        )));
        assert!(!HerdrError::is_dirty_worktree(&reply(
            "worktree.remove",
            "not_linked_worktree"
        )));
    }

    #[test]
    fn create_start_and_prompt_use_the_expected_herdr_contract() {
        let (socket_path, server) = fake_herdr("herdr-launch", move |listener| {
            let cases = [
                (
                    "workspace.create",
                    json!({
                        "type": "workspace_created",
                        "workspace": { "workspace_id": "w9", "label": "corgi" },
                        "tab": { "tab_id": "w9:t1" },
                        "root_pane": { "pane_id": "w9:p1" }
                    }),
                ),
                (
                    "workspace.report_metadata",
                    json!({ "type": "workspace_metadata_updated" }),
                ),
                (
                    "tab.create",
                    json!({
                        "type": "tab_created",
                        "tab": { "tab_id": "w9:t2" },
                        "root_pane": { "pane_id": "w9:p2" }
                    }),
                ),
                (
                    "worktree.list",
                    json!({
                        "type": "worktree_list",
                        "source": {
                            "repo_key": "/tmp/corgi-project/.git",
                            "repo_name": "corgi-project",
                            "repo_root": "/tmp/corgi-project",
                            "source_checkout_path": "/tmp/corgi-worktrees/corgi-project/worktree-a1",
                            "source_workspace_id": "w8"
                        },
                        "worktrees": []
                    }),
                ),
                (
                    "worktree.create",
                    json!({
                        "type": "worktree_created",
                        "workspace": {
                            "workspace_id": "w10",
                            "label": "worktree-calm-otter-1f2e",
                            "worktree": {
                                "repo_key": "/tmp/corgi-project/.git",
                                "repo_name": "corgi-project",
                                "repo_root": "/tmp/corgi-project",
                                "checkout_path": "/tmp/corgi-worktrees/corgi-project/worktree-calm-otter-1f2e",
                                "is_linked_worktree": true
                            }
                        },
                        "tab": { "tab_id": "w10:t1", "workspace_id": "w10" },
                        "root_pane": { "pane_id": "w10:p1", "workspace_id": "w10", "tab_id": "w10:t1" },
                        "worktree": {
                            "path": "/tmp/corgi-worktrees/corgi-project/worktree-calm-otter-1f2e",
                            "branch": "worktree/calm-otter-1f2e",
                            "label": "corgi-project",
                            "is_linked_worktree": true
                        }
                    }),
                ),
                (
                    "agent.start",
                    json!({
                        "type": "agent_started",
                        "agent": {
                            "agent": "codex",
                            "agent_status": "idle",
                            "name": "corgi-worker",
                            "pane_id": "w9:p1",
                            "workspace_id": "w9",
                            "tab_id": "w9:t1"
                        },
                        "argv": ["codex", "--model", "gpt-5-codex"]
                    }),
                ),
                (
                    "agent.prompt",
                    json!({
                        "type": "agent_prompted",
                        "agent": {
                            "agent": "codex",
                            "agent_status": "working",
                            "name": "corgi-worker",
                            "pane_id": "w9:p1",
                            "workspace_id": "w9",
                            "tab_id": "w9:t1"
                        }
                    }),
                ),
                (
                    "pane.focus",
                    json!({
                        "type": "pane_info",
                        "pane": {
                            "pane_id": "w1:p2",
                            "workspace_id": "w1",
                            "tab_id": "w1:t2"
                        }
                    }),
                ),
                (
                    "tab.close",
                    json!({
                        "type": "tab_closed",
                        "tab_id": "w9:t2",
                        "workspace_id": "w9"
                    }),
                ),
                (
                    "workspace.close",
                    json!({
                        "type": "workspace_closed",
                        "workspace": { "workspace_id": "w9", "label": "corgi" }
                    }),
                ),
                (
                    "worktree.remove",
                    json!({
                        "type": "worktree_removed",
                        "workspace_id": "w10",
                        "path": "/tmp/corgi-worktrees/corgi-project/worktree-calm-otter-1f2e",
                        "forced": true
                    }),
                ),
            ];

            for (expected_method, result) in cases {
                answer(&listener, |request| {
                    assert_eq!(request["method"].as_str(), Some(expected_method));

                    match expected_method {
                        "workspace.create" => {
                            assert_eq!(request["params"]["cwd"], "/tmp/corgi-project");
                            assert_eq!(request["params"]["label"], "corgi");
                            assert_eq!(request["params"]["focus"], false);
                        }
                        "worktree.list" => {
                            assert_eq!(
                                request["params"]["cwd"],
                                "/tmp/corgi-worktrees/corgi-project/worktree-a1"
                            );
                        }
                        "worktree.create" => {
                            assert_eq!(request["params"]["workspace_id"], "w9");
                            assert_eq!(request["params"]["focus"], false);
                            assert!(request["params"]["branch"].is_null());
                            assert!(request["params"]["path"].is_null());
                        }
                        "workspace.report_metadata" => {
                            assert_eq!(request["params"]["workspace_id"], "w9");
                            assert_eq!(request["params"]["source"], "corgi");
                            assert_eq!(request["params"]["tokens"]["role"], "project-main");
                        }
                        "tab.create" => {
                            assert_eq!(request["params"]["workspace_id"], "w9");
                            assert_eq!(request["params"]["cwd"], "/tmp/corgi-project");
                            assert_eq!(request["params"]["label"], "shared-worker");
                            assert_eq!(request["params"]["focus"], false);
                        }
                        "agent.start" => {
                            assert_eq!(request["params"]["name"], "corgi-worker");
                            assert_eq!(request["params"]["kind"], "codex");
                            assert_eq!(request["params"]["pane_id"], "w9:p1");
                            assert_eq!(
                                request["params"]["args"],
                                json!(["--model", "gpt-5-codex"])
                            );
                        }
                        "agent.prompt" => {
                            assert_eq!(request["params"]["target"], "w9:p1");
                            assert_eq!(request["params"]["text"], "Implement the parser");
                            assert!(request["params"]["wait"].is_null());
                        }
                        "pane.focus" => {
                            assert_eq!(request["params"]["pane_id"], "w1:p2");
                        }
                        "tab.close" => {
                            assert_eq!(request["params"]["tab_id"], "w9:t2");
                        }
                        "workspace.close" => {
                            assert_eq!(request["params"]["workspace_id"], "w9");
                        }
                        "worktree.remove" => {
                            assert_eq!(request["params"]["workspace_id"], "w10");
                            assert_eq!(request["params"]["force"], true);
                        }
                        _ => unreachable!(),
                    }

                    json!({ "result": result })
                });
            }
        });

        let client = HerdrClient::from_socket_path(&socket_path);
        let workspace = client
            .create_workspace(
                Some(PathBuf::from("/tmp/corgi-project").as_path()),
                Some("corgi"),
            )
            .expect("create workspace");
        assert_eq!(workspace.root_pane_id, "w9:p1");
        assert!(workspace.worktree.is_none());
        client
            .report_workspace_metadata("w9", "corgi", &[("role", "project-main")])
            .expect("mark workspace");
        let tab = client
            .create_tab(
                "w9",
                Some(PathBuf::from("/tmp/corgi-project").as_path()),
                Some("shared-worker"),
            )
            .expect("create tab");
        assert_eq!(tab.root_pane_id, "w9:p2");

        let source = client
            .worktree_source(Path::new("/tmp/corgi-worktrees/corgi-project/worktree-a1"))
            .expect("resolve worktree source");
        assert_eq!(source.repo_root, "/tmp/corgi-project");
        assert_eq!(source.repo_name, "corgi-project");

        let worktree = client.create_worktree("w9").expect("create worktree");
        assert_eq!(worktree.root_pane_id, "w10:p1");
        assert_eq!(worktree.workspace_id, "w10");
        assert_eq!(
            worktree
                .worktree
                .as_ref()
                .and_then(|info| info.branch.as_deref()),
            Some("worktree/calm-otter-1f2e")
        );
        assert_eq!(worktree.workspace.repo_root(), Some("/tmp/corgi-project"));

        let started = client
            .start_agent(
                "corgi-worker",
                "codex",
                &["--model".to_string(), "gpt-5-codex".to_string()],
                &workspace.root_pane_id,
            )
            .expect("start agent");
        assert_eq!(started.argv, ["codex", "--model", "gpt-5-codex"]);

        let prompted = client
            .prompt_agent(&started.agent.pane_id, "Implement the parser")
            .expect("prompt agent");
        assert_eq!(prompted.state, crate::model::AgentState::Working);

        client.focus_pane("w1:p2").expect("focus Corgi pane");
        client.close_tab("w9:t2").expect("close worker tab");

        client.close_workspace("w9").expect("close workspace");

        let removed = client
            .remove_worktree("w10", true)
            .expect("remove worktree");
        assert!(removed.forced);
        assert_eq!(
            removed.path,
            "/tmp/corgi-worktrees/corgi-project/worktree-calm-otter-1f2e"
        );

        server.join().expect("fake server panicked");
        fs::remove_file(&socket_path).expect("remove fake socket");
    }
}
