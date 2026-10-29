//! Confirmed native conversation recovery. No function in this module sends a prompt.
pub mod native;
pub mod setup;
pub mod store;

#[cfg(any(test, feature = "test-support"))]
pub mod fixture;
#[cfg(test)]
mod tests;

use crate::{
    harness::Harness,
    herdr::{HerdrClient, SessionSnapshot},
    model::{AgentInfo, AgentState, DashboardAgent},
    paths::is_home,
    supervisor,
    time::{unix_now, utc_stamp},
};
use anyhow::{Context, Result, bail, ensure};
use native::{Profile, Settings};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};
pub use store::Store;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Supervisor {
    pub state_dir: PathBuf,
    pub role_file: PathBuf,
    pub role_fingerprint: String,
    pub command: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub kind: String,
    pub native_id: Option<String>,
    pub native_store: Option<PathBuf>,
    pub native_file: Option<PathBuf>,
    pub cwd: PathBuf,
    pub shell_cwd: PathBuf,
    pub checkout: PathBuf,
    pub project: String,
    pub role: String,
    pub supervisor: Option<Supervisor>,
    pub workspace: String,
    pub workspace_label: String,
    pub tab: String,
    pub tab_label: String,
    pub pane: String,
    pub linked_checkout: bool,
    pub git_common: Option<PathBuf>,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub previous_merge: bool,
    pub previous_handover: bool,
    pub pending_inbox_ids: Vec<String>,
    pub configs: Vec<native::ConfigReference>,
    pub request_id: Option<String>,
    pub workspace_tokens: BTreeMap<String, String>,
    pub state: AgentState,
    pub settings: Settings,
    pub warnings: Vec<String>,
}

impl Entry {
    fn key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.kind,
            self.native_store
                .as_deref()
                .unwrap_or(Path::new(""))
                .display(),
            self.native_id.as_deref().unwrap_or("")
        )
    }
    pub fn label(&self) -> String {
        format!(
            "{} / {} ({})",
            if self.role == "scratch" {
                "Scratch"
            } else {
                &self.project
            },
            self.name,
            self.role
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub id: String,
    pub created: u64,
    pub sequence: u64,
    pub scope: String,
    pub uid: u32,
    pub host: String,
    pub boot: String,
    pub manual_prepared: bool,
    pub entries: Vec<Entry>,
}

impl Snapshot {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            "unsupported recovery snapshot version (inspect with its producer)"
        );
        ensure!(
            !self.id.is_empty()
                && self.id.len() < 128
                && self
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-'),
            "invalid snapshot ID"
        );
        ensure!(self.entries.len() <= 500, "too many saved sessions");
        let mut identities = BTreeSet::new();
        for entry in &self.entries {
            ensure!(
                entry.cwd.is_absolute() || entry.cwd.as_os_str().is_empty(),
                "saved directory is not absolute"
            );
            if let Some(id) = &entry.native_id {
                ensure!(native::valid_id(id), "invalid saved native ID");
                ensure!(
                    identities.insert(entry.key()),
                    "duplicate native conversation in snapshot"
                );
            }
            ensure!(
                entry.name.len() <= 256 && entry.project.len() <= 4096,
                "snapshot field too large"
            );
            ensure!(
                entry.settings.model.len() <= 256 && entry.settings.effort.len() <= 32,
                "invalid saved model/effort"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Attempt {
    pub boot: String,
    pub pane: String,
    pub workspace: String,
    pub tab: String,
    pub phase: String,
    pub at: u64,
    pub snapshot: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Journal {
    pub version: u32,
    pub attempts: BTreeMap<String, Attempt>,
    pub restored: BTreeMap<String, u64>,
    pub held: BTreeMap<String, BTreeMap<String, String>>,
    pub released: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    pub deliveries: BTreeSet<String>,
}
impl Default for Journal {
    fn default() -> Self {
        Self {
            version: 1,
            attempts: BTreeMap::new(),
            restored: BTreeMap::new(),
            held: BTreeMap::new(),
            released: BTreeMap::new(),
            deliveries: BTreeSet::new(),
        }
    }
}

pub fn capture(client: &HerdrClient, displayed: &[DashboardAgent]) -> Result<Snapshot> {
    let live = client.snapshot()?;
    let mut agents = live.agents.clone();
    for agent in displayed {
        if agent.scratch
            && !agents.iter().any(|a| a.pane_id == agent.info.pane_id)
            && live.panes.iter().any(|p| p.pane_id == agent.info.pane_id)
            && client
                .has_foreground_codex(&agent.info.pane_id, agent.info.cwd())
                .unwrap_or(false)
        {
            let mut scratch = agent.info.clone();
            scratch.agent_session = None;
            agents.push(scratch);
        }
    }
    let store = Store::for_client(client).context("recovery storage unavailable")?;
    let mut entries = Vec::new();
    for agent in &agents {
        let workspace = live
            .workspaces
            .iter()
            .find(|w| w.workspace_id == agent.workspace_id);
        let tab = live.tabs.iter().find(|t| t.tab_id == agent.tab_id);
        let sup = supervisor::is_supervisor(agent);

        let mut project = workspace
            .and_then(|w| w.repo_root())
            .unwrap_or(agent.cwd())
            .to_string();
        let mut warnings = Vec::new();
        let native_id = agent
            .agent_session
            .as_ref()
            .filter(|r| r.kind == "id" && r.agent == agent.kind())
            .map(|r| r.value.clone())
            .filter(|id| native::valid_id(id));
        let mut root = None;
        let mut file = None;
        let mut settings = Settings {
            unknown: true,
            ..Settings::default()
        };
        if let Some(id) = &native_id {
            let found = (|| {
                let base = native::store_root(agent.kind())?;
                let path = native::locate(&base, agent.kind(), id)?;
                let observed = native::metadata(&path, agent.kind(), id)?;
                Ok::<_, anyhow::Error>((base, path, observed))
            })();
            match found {
                Ok((base, path, observed)) => {
                    root = Some(base);
                    file = Some(path);
                    let profile_path = store
                        .0
                        .join("profiles")
                        .join(format!("{}-{id}.json", agent.kind()));
                    if profile_path.is_file() {
                        let profile: Profile = store::read(&profile_path)?;
                        if profile.id == *id
                            && profile.kind == agent.kind()
                            && profile.cwd == Path::new(agent.cwd())
                        {
                            settings = profile.settings;
                        }
                    }
                    settings.apply_native(observed);
                }
                Err(_) => {
                    warnings.push("Exact native conversation file could not be verified".into())
                }
            }
        } else {
            warnings.push("Native conversation ID unavailable".into());
        }
        if settings.unknown {
            warnings.push(
                "Some launch/permission details are unverified. Listed settings are reapplied; other options use native defaults".into(),
            );
        }
        if let Some(error) = &settings.permission_error {
            warnings.push(format!("Permissions cannot be restored: {error}"));
        }
        if sup && agent.kind() == "codex" && settings.permission_error.is_none() {
            settings
                .sandbox
                .get_or_insert_with(|| "workspace-write".into());
            settings.approval.get_or_insert_with(|| "on-request".into());
            if settings.sandbox.as_deref() == Some("workspace-write") {
                settings.network.get_or_insert(true);
            }
        }
        if settings.model.is_empty() || settings.effort.is_empty() {
            warnings
                .push("Model/effort not fully reported; unknown values use native defaults".into());
        }
        let shell_cwd = PathBuf::from(agent.cwd());
        let cwd = settings
            .native_cwd
            .clone()
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| shell_cwd.clone());
        let checkout = workspace
            .and_then(|w| w.worktree.as_ref())
            .map(|w| PathBuf::from(&w.checkout_path))
            .or_else(|| {
                crate::git::git_output(&cwd, &["rev-parse", "--show-toplevel"])
                    .ok()
                    .map(|s| PathBuf::from(s.trim()))
            })
            .unwrap_or_else(|| cwd.clone());
        if cwd != shell_cwd {
            warnings.push(format!(
                "Native working root {} differs from the shell directory {}",
                cwd.display(),
                shell_cwd.display()
            ));
        }
        for path in [&cwd, &checkout].into_iter().chain(root.iter()) {
            if store::volatile(path) {
                warnings.push(format!(
                    "Volatile dependency {} may not survive reboot",
                    path.display()
                ));
            }
        }
        let scratch = is_home(&cwd);
        if !sup && !scratch && workspace.and_then(|w| w.repo_root()).is_none() {
            project = crate::git::trust_root(&cwd).to_string_lossy().into_owned();
        }
        let git_common = crate::git::git_output(
            &cwd,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .ok()
        .map(|s| PathBuf::from(s.trim()));
        let branch = crate::git::git_current_branch(&cwd).ok();
        let head = crate::git::git_output(&cwd, &["rev-parse", "HEAD"])
            .ok()
            .map(|s| s.trim().to_string());
        let configs = if let Some(root) = &root {
            match native::configuration(root, agent.kind(), &cwd) {
                Ok((refs, hooks)) => {
                    if hooks {
                        warnings.push(
                            "Native startup hooks are configured and may run on resume".into(),
                        );
                    }
                    refs
                }
                Err(_) => {
                    warnings
                        .push("Native configuration references could not be fully verified".into());
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        if settings.native_pending {
            warnings.push("Native pending turn/input detected; automatic resume is blocked until it is resolved".into());
        }
        let previous_merge = supervisor::merge_tag(agent) == supervisor::MergeTag::Applies;
        let previous_handover = agent.tokens.contains_key(supervisor::HANDOVER_TOKEN);
        if previous_merge {
            warnings.push("Previously marked ready for merge; recheck after restore".into());
        }
        if previous_handover {
            warnings.push(
                "Handover state is historical; no replacement or request will be replayed".into(),
            );
        }
        let supervisor = if sup {
            let dir = supervisor::state_dir(&project)?;
            let role_file = dir.join(supervisor::ROLE_FILE);
            for path in [&dir, &role_file] {
                if store::volatile(path) {
                    warnings.push(format!(
                        "Volatile supervisor dependency {} may not survive reboot",
                        path.display()
                    ));
                }
            }
            let command = client
                .plugin_root(supervisor::PLUGIN_ID)
                .ok()
                .flatten()
                .and_then(|installed| supervisor::corgi_bin(Some(&installed)).ok());
            let command = command.filter(|bin| {
                std::fs::read_to_string(&role_file)
                    .ok()
                    .is_some_and(|role| role.contains(bin.to_string_lossy().as_ref()))
            });
            if command.is_none() {
                warnings.push("Corgi command referenced by the existing role could not be verified; instructions are retained as-is".into());
            }
            match native::fingerprint(&role_file) {
                Ok(role_fingerprint) => Some(Supervisor {
                    state_dir: dir,
                    role_file,
                    role_fingerprint,
                    command,
                }),
                Err(_) => {
                    warnings.push("Supervisor role file unavailable".into());
                    None
                }
            }
        } else {
            None
        };
        if matches!(
            agent.state,
            AgentState::Working | AgentState::Blocked | AgentState::Unknown
        ) {
            warnings.push(format!(
                "Saved while {}; tools, pending work and unsent input are not checkpointed",
                agent.state.label()
            ));
        }
        let workspace_tokens = workspace
            .map(|w| {
                w.tokens
                    .iter()
                    .filter(|(k, _)| {
                        [
                            "corgi_workspace_role",
                            "corgi_project_root",
                            "corgi_project_root_hash",
                            "corgi_project_main_tab",
                        ]
                        .contains(&k.as_str())
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let pending_inbox_ids = supervisor
            .as_ref()
            .map(|s| {
                crate::inbox::Inbox::read(&s.state_dir)
                    .undelivered_for(&project)
                    .into_iter()
                    .map(|i| i.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        entries.push(Entry {
            name: agent.name.clone().unwrap_or_else(|| agent.kind().into()),
            kind: agent.kind().into(),
            native_id,
            native_store: root,
            native_file: file,
            cwd,
            shell_cwd,
            checkout,
            project,
            role: if sup {
                "supervisor"
            } else if scratch {
                "scratch"
            } else {
                "worker"
            }
            .into(),
            supervisor,
            workspace: agent.workspace_id.clone(),
            workspace_label: workspace.map(|w| w.label.clone()).unwrap_or_default(),
            tab: agent.tab_id.clone(),
            tab_label: tab.map(|t| t.label.clone()).unwrap_or_default(),
            pane: agent.pane_id.clone(),
            linked_checkout: workspace
                .and_then(|w| w.worktree.as_ref())
                .is_some_and(|w| w.is_linked_worktree),
            request_id: confirmed_request(client, agent),
            workspace_tokens,
            state: agent.state,
            settings,
            warnings,
            git_common,
            branch,
            head,
            previous_merge,
            previous_handover,
            pending_inbox_ids,
            configs,
        });
    }
    entries.sort_by(|a, b| (&a.project, &a.role, &a.name).cmp(&(&b.project, &b.role, &b.name)));
    Ok(Snapshot {
        version: 1,
        id: format!("{}-{}", utc_stamp(unix_now()), store::nonce()?),
        created: unix_now(),
        sequence: 0,
        scope: client.socket_path().to_string_lossy().into_owned(),
        uid: unsafe { libc::geteuid() },
        host: store::host()?,
        boot: store::boot()?,
        manual_prepared: setup::prepared(client),
        entries,
    })
}

pub fn record_launch(
    client: &HerdrClient,
    agent: &AgentInfo,
    model: &str,
    effort: &str,
    extra: &[String],
    supervisor: bool,
) -> Result<()> {
    let Some(store) = Store::for_client(client) else {
        return Ok(());
    };
    let Some(id) = supervisor::native_session(agent).filter(|id| native::valid_id(id)) else {
        return Ok(());
    };
    if !matches!(agent.kind(), "codex" | "claude") {
        return Ok(());
    }
    let _lock = store.lock()?;
    let dir = store.0.join("profiles");
    store::private_dir(&dir)?;
    let mut settings = Settings::from_launch(model, effort, extra);
    if supervisor && agent.kind() == "codex" {
        settings.sandbox = Some("workspace-write".into());
        settings.approval = Some("on-request".into());
        settings.network = Some(true);
    }
    let profile = Profile {
        kind: agent.kind().into(),
        id: id.into(),
        cwd: PathBuf::from(agent.cwd()),
        settings,
    };
    store::atomic(
        &dir.join(format!("{}-{id}.json", agent.kind())),
        &serde_json::to_vec(&profile)?,
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Metadata {
        workspace: String,
        tab: String,
        pane: String,
    },
    Original {
        workspace: String,
        tab: String,
        pane: String,
    },
    NewTab {
        workspace: String,
    },
    OpenWorkspace,
}
impl Target {
    pub fn description(&self) -> String {
        match self {
            Self::Metadata { pane, .. } => {
                format!("already running in {pane}; finish metadata ONLY")
            }
            Self::Original {
                workspace,
                tab,
                pane,
            } => format!("existing {workspace}/{tab}/{pane}"),
            Self::NewTab { workspace } => {
                format!("NEW TAB in {workspace} (original occupied/missing)")
            }
            Self::OpenWorkspace => "OPEN workspace at existing directory (original missing)".into(),
        }
    }
}
#[derive(Clone, Debug)]
pub struct Row {
    pub target: Option<Target>,
    pub alternative: Option<Target>,
    pub reason: String,
    pub differences: Vec<String>,
}

pub fn plan(client: &HerdrClient, saved: &Snapshot, manual: bool) -> Result<Vec<Row>> {
    plan_in(client, saved, &Runtime::read(manual)?)
}

pub struct Runtime {
    pub boot: String,
    pub manual: bool,
    pub stores: BTreeMap<String, PathBuf>,
    pub clis: BTreeSet<String>,
}
impl Runtime {
    fn read(manual: bool) -> Result<Self> {
        Ok(Self {
            boot: store::boot()?,
            manual,
            stores: ["codex", "claude"]
                .into_iter()
                .filter_map(|k| native::store_root(k).ok().map(|p| (k.into(), p)))
                .collect(),
            clis: ["codex", "claude"]
                .into_iter()
                .filter(|k| Harness::from_kind(k).executable().is_some())
                .map(str::to_string)
                .collect(),
        })
    }
}

fn plan_in(client: &HerdrClient, saved: &Snapshot, runtime: &Runtime) -> Result<Vec<Row>> {
    saved.validate()?;
    ensure!(
        saved.scope == client.socket_path().to_string_lossy()
            && saved.uid == unsafe { libc::geteuid() }
            && saved.host == store::host()?,
        "snapshot belongs to a different local machine/Herdr session/account"
    );
    let live = client.snapshot()?;
    let store = Store::for_client(client).context("recovery storage unavailable")?;
    let journal = store.journal()?;
    Ok(saved
        .entries
        .iter()
        .map(|entry| {
            if let Some(sup) = &entry.supervisor
                && saved.entries.iter().any(|other| {
                    other.project != entry.project
                        && other
                            .supervisor
                            .as_ref()
                            .is_some_and(|s| s.state_dir == sup.state_dir)
                })
            {
                return Row {
                    target: None,
                    alternative: None,
                    reason: "Skipped: supervisor memory is shared by different project roots"
                        .into(),
                    differences: Vec::new(),
                };
            }
            match plan_entry(client, entry, &live, &journal, runtime) {
                Ok(row) => row,
                Err(error) => Row {
                    target: None,
                    alternative: None,
                    reason: format!("Skipped: {error}"),
                    differences: Vec::new(),
                },
            }
        })
        .collect())
}

fn plan_entry(
    client: &HerdrClient,
    entry: &Entry,
    live: &SessionSnapshot,
    journal: &Journal,
    runtime: &Runtime,
) -> Result<Row> {
    let id = entry.native_id.as_deref().context("missing native ID")?;
    ensure!(
        matches!(entry.kind.as_str(), "codex" | "claude"),
        "unsupported harness"
    );
    let matches: Vec<_> = live
        .agents
        .iter()
        .filter(|a| {
            a.kind() == entry.kind && supervisor::native_session(a) == Some(id) && !a.launch_pending
        })
        .collect();
    ensure!(matches.len() < 2, "duplicate live conversation identities");
    if let Some(agent) = matches.first() {
        if journal.attempts.get(&entry.key()).is_some_and(|a| {
            a.boot == runtime.boot
                && a.pane == agent.pane_id
                && matches!(a.phase.as_str(), "start_pending" | "metadata_pending")
        }) && agent.interactive_ready
            && matches!(agent.state, AgentState::Idle | AgentState::Done)
        {
            return Ok(Row {
                target: Some(Target::Metadata {
                    workspace: agent.workspace_id.clone(),
                    tab: agent.tab_id.clone(),
                    pane: agent.pane_id.clone(),
                }),
                alternative: None,
                reason: "Conversation already running; metadata reconciliation only".into(),
                differences: Vec::new(),
            });
        }
        return Ok(Row {
            target: None,
            alternative: None,
            reason: format!(
                "Already running in {} (reuse; setup not changed)",
                agent.pane_id
            ),
            differences: Vec::new(),
        });
    }
    ensure!(
        runtime.manual,
        "Herdr automatic resume enabled/unknown; confirm manual-recovery setup first"
    );
    if let Some(attempt) = journal.attempts.get(&entry.key()) {
        ensure!(
            attempt.boot != runtime.boot || attempt.phase == "placement_ready",
            "previous launch outcome unresolved in {}; inspect it, do not retry",
            attempt.pane
        );
    }
    ensure!(
        !live.agents.iter().any(|a| a.launch_pending
            && (a.pane_id == entry.pane
                || (a.kind() == entry.kind && supervisor::native_session(a) == Some(id)))),
        "competing native launch pending"
    );
    ensure!(runtime.clis.contains(&entry.kind), "harness CLI missing");
    ensure!(entry.cwd.is_dir(), "saved directory/worktree missing");
    let mut differences = Vec::new();
    if let Some(common) = &entry.git_common {
        let current = crate::git::git_output(
            &entry.cwd,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .context("saved repository unavailable")?;
        ensure!(
            Path::new(current.trim()).canonicalize()? == common.canonicalize()?,
            "repository identity at saved directory changed"
        );
        let branch = crate::git::git_current_branch(&entry.cwd).ok();
        let head = crate::git::git_output(&entry.cwd, &["rev-parse", "HEAD"])
            .ok()
            .map(|s| s.trim().to_string());
        if branch != entry.branch || head != entry.head {
            differences.push("Existing branch/HEAD changed since Save; it will be used as-is, without switching/resetting".into());
        }
    }
    if entry.linked_checkout {
        ensure!(
            entry.checkout.join(".git").is_file(),
            "saved linked worktree is no longer linked"
        );
    }
    let root = runtime
        .stores
        .get(&entry.kind)
        .context("native conversation store unavailable")?;
    ensure!(
        entry.native_store.as_ref() == Some(root),
        "native account/store differs or was not verified at Save"
    );
    let file = native::exact_file(root, &entry.kind, id, entry.native_file.as_deref())?;
    let latest = native::metadata(&file, &entry.kind, id)?;
    if let Some(error) = &latest.permission_error {
        bail!("Current native permissions cannot be restored: {error}");
    }
    ensure!(
        !latest.native_pending,
        "native pending turn/input detected; inspect it manually, no replay will be attempted"
    );
    for reference in &entry.configs {
        if native::fingerprint(&reference.path).ok().as_ref() != Some(&reference.fingerprint) {
            differences.push(format!(
                "Native configuration {} changed/missing; current native configuration will apply",
                reference.path.display()
            ));
        }
    }
    if entry.role == "supervisor" {
        ensure!(entry.supervisor.is_some(), "supervisor role unavailable");
        ensure!(
            !live
                .agents
                .iter()
                .any(|a| supervisor::is_supervisor(a) && a.cwd() == entry.project),
            "another supervisor already owns this project"
        );
    }
    native::resume_args(entry)?;
    let retry = journal
        .attempts
        .get(&entry.key())
        .filter(|a| a.boot == runtime.boot && a.phase == "placement_ready");
    let (expected_ws, expected_tab, expected_pane) = retry
        .map(|a| (&a.workspace, &a.tab, &a.pane))
        .unwrap_or((&entry.workspace, &entry.tab, &entry.pane));
    let same_pane = live.panes.iter().find(|p| {
        p.pane_id == *expected_pane && p.tab_id == *expected_tab && p.workspace_id == *expected_ws
    });
    let workspace = live
        .workspaces
        .iter()
        .find(|w| w.workspace_id == *expected_ws);
    if let Some(workspace) = workspace
        && let Some(worktree) = &workspace.worktree
    {
        ensure!(
            Path::new(&worktree.checkout_path).canonicalize()? == entry.checkout.canonicalize()?,
            "original workspace checkout changed"
        );
    }
    if let Some(pane) = same_pane {
        let occupied = live.agents.iter().any(|a| a.pane_id == pane.pane_id);
        let shell_cwd = if entry.kind == "codex" && pane.pane_id == entry.pane {
            &entry.shell_cwd
        } else {
            &entry.cwd
        };
        if !occupied && available_shell(client, &pane.pane_id, shell_cwd)? {
            return Ok(Row {
                target: Some(Target::Original {
                    workspace: expected_ws.clone(),
                    tab: expected_tab.clone(),
                    pane: expected_pane.clone(),
                }),
                alternative: None,
                reason: "Ready in original terminal".into(),
                differences,
            });
        }
        return Ok(Row {
            target: None,
            alternative: Some(Target::NewTab {
                workspace: expected_ws.clone(),
            }),
            reason: "Original terminal occupied or shell not available; F offers a new tab".into(),
            differences,
        });
    }
    Ok(Row {
        target: None,
        alternative: Some(if workspace.is_some() {
            Target::NewTab {
                workspace: expected_ws.clone(),
            }
        } else {
            Target::OpenWorkspace
        }),
        reason: "Original destination missing; F offers a new placement".into(),
        differences,
    })
}

/// Herdr owns launch readiness. A restored shell is usable when its shell PID
/// owns the foreground group, all foreground processes are that shell, and cwd
/// matches. No terminal text, command history or draft is copied or injected.
fn available_shell(client: &HerdrClient, pane: &str, cwd: &Path) -> Result<bool> {
    let reply = client.request("pane.process_info", json!({"pane_id":pane}))?;
    let info = &reply["process_info"];
    let Some(shell) = info["shell_pid"].as_u64() else {
        return Ok(false);
    };
    if info["foreground_process_group_id"].as_u64() != Some(shell) {
        return Ok(false);
    }
    let Some(processes) = info["foreground_processes"].as_array() else {
        return Ok(false);
    };
    Ok(processes.len() == 1 && processes.iter().all(|process| {
        process["pid"].as_u64() == Some(shell)
            && ["sh","bash","zsh","fish","dash","nu"].contains(&process["name"].as_str().unwrap_or(""))
            && process["cwd"].as_str().is_some_and(|p| matches!((Path::new(p).canonicalize(),cwd.canonicalize()),(Ok(a),Ok(b)) if a==b))
    }))
}

/// Holds are session-bound. Every immutable manifest retains its next-boot
/// intent until Continue acknowledges it; a newer Save cannot erase a hold.
/// This also works before the first waker tick and across dashboard restarts.
pub fn held_projects(client: &HerdrClient, agents: &[DashboardAgent]) -> Result<BTreeSet<String>> {
    let Some(store) = Store::for_client(client) else {
        return Ok(BTreeSet::new());
    };
    let journal = store.journal()?;
    let boot = store::boot()?;
    let scope = client.socket_path().to_string_lossy();
    let host = store::host()?;
    let snapshots = store.snapshots()?;
    let mut held = BTreeSet::new();
    for agent in agents.iter().filter(|a| a.supervisor) {
        let Some(id) = supervisor::native_session(&agent.info) else {
            continue;
        };
        let key = format!("{}:{id}", agent.info.kind());
        let explicit = journal
            .held
            .get(scope.as_ref())
            .is_some_and(|v| v.contains_key(&key));
        let armed = snapshots.iter().any(|s| {
            s.scope == scope
                && s.host == host
                && s.uid == unsafe { libc::geteuid() }
                && s.boot != boot
                && s.entries.iter().any(|e| {
                    e.role == "supervisor"
                        && e.kind == agent.info.kind()
                        && e.native_id.as_deref() == Some(id)
                })
                && !journal
                    .released
                    .get(&s.id)
                    .is_some_and(|v| v.contains(&key))
        });
        if explicit || armed {
            held.insert(agent.project_root.clone());
        }
    }
    Ok(held)
}

pub fn continue_coordination(client: &HerdrClient, projects: &BTreeSet<String>) -> Result<()> {
    let store = Store::for_client(client).context("recovery storage unavailable")?;
    let _lock = store.lock()?;
    let mut journal = store.journal()?;
    let scope = client.socket_path().to_string_lossy();
    if let Some(held) = journal.held.get_mut(scope.as_ref()) {
        held.retain(|_, project| !projects.contains(project));
    }
    let host = store::host()?;
    let boot = store::boot()?;
    for saved in store.snapshots()?.into_iter().filter(|s| {
        s.scope == scope && s.host == host && s.uid == unsafe { libc::geteuid() } && s.boot != boot
    }) {
        journal.released.entry(saved.id).or_default().extend(
            saved
                .entries
                .iter()
                .filter(|e| e.role == "supervisor" && projects.contains(&e.project))
                .filter_map(|e| e.native_id.as_ref().map(|id| format!("{}:{id}", e.kind))),
        );
    }
    store.write_journal(&journal)
}

pub fn restore(
    client: &HerdrClient,
    saved: &Snapshot,
    chosen: &[(usize, Target, Vec<String>)],
    manual: bool,
    stop: &AtomicBool,
    mut report: impl FnMut(String),
) -> Result<()> {
    restore_in(
        client,
        saved,
        chosen,
        &Runtime::read(manual)?,
        stop,
        &mut report,
    )
}

fn restore_in(
    client: &HerdrClient,
    saved: &Snapshot,
    chosen: &[(usize, Target, Vec<String>)],
    runtime: &Runtime,
    stop: &AtomicBool,
    mut report: impl FnMut(String),
) -> Result<()> {
    let store = Store::for_client(client).context("recovery storage unavailable")?;
    let _lock = store.lock()?;
    let rows = plan_in(client, saved, runtime)?;
    for (index, target, differences) in chosen {
        let row = rows.get(*index).context("invalid selected row")?;
        ensure!(
            row.target.as_ref() == Some(target) || row.alternative.as_ref() == Some(target),
            "restore plan changed; review again"
        );
        ensure!(
            row.differences == *differences,
            "setup differences changed; review again"
        );
    }
    let mut journal = store.journal()?;
    let boot = runtime.boot.clone();
    for (index, target, differences) in chosen {
        if stop.load(Ordering::Relaxed) {
            report("Stopped scheduling; started agents remain open".into());
            break;
        }
        let entry = &saved.entries[*index];
        let live = client.snapshot()?;
        let row = match plan_entry(client, entry, &live, &journal, runtime) {
            Ok(row) => row,
            Err(error) => {
                report(format!(
                    "{}: skipped; validation changed ({error})",
                    entry.name
                ));
                continue;
            }
        };
        if (row.target.as_ref() != Some(target) && row.alternative.as_ref() != Some(target))
            || row.differences != *differences
        {
            report(format!(
                "{}: skipped; plan changed ({})",
                entry.name, row.reason
            ));
            continue;
        }
        if entry.role == "supervisor" {
            journal.held.entry(saved.scope.clone()).or_default().insert(
                format!("{}:{}", entry.kind, entry.native_id.as_deref().unwrap()),
                entry.project.clone(),
            );
            store.write_journal(&journal)?;
        }
        let key = entry.key();
        if let Target::Metadata {
            workspace,
            tab,
            pane,
        } = target
        {
            let live = client.snapshot()?;
            let agent = live
                .agents
                .iter()
                .find(|a| {
                    a.pane_id == *pane
                        && a.kind() == entry.kind
                        && supervisor::native_session(a) == entry.native_id.as_deref()
                })
                .context("native metadata target changed; review again")?;
            bind_metadata(client, entry, agent, workspace, tab)?;
            journal
                .attempts
                .get_mut(&key)
                .context("recovery intent missing")?
                .phase = "complete".into();
            journal.restored.insert(saved.id.clone(), unix_now());
            store.write_journal(&journal)?;
            report(format!(
                "{}: metadata reconciled in {pane}; no launch",
                entry.name
            ));
            continue;
        }
        journal.attempts.insert(
            key.clone(),
            Attempt {
                boot: boot.clone(),
                pane: entry.pane.clone(),
                workspace: entry.workspace.clone(),
                tab: entry.tab.clone(),
                phase: "placement_pending".into(),
                at: unix_now(),
                snapshot: saved.id.clone(),
            },
        );
        if let Target::Original {
            workspace,
            tab,
            pane,
        } = target
        {
            let attempt = journal.attempts.get_mut(&key).unwrap();
            attempt.workspace = workspace.clone();
            attempt.tab = tab.clone();
            attempt.pane = pane.clone();
        }
        store.write_journal(&journal)?;
        let location = match target {
            Target::Metadata { .. } => unreachable!(),
            Target::Original {
                workspace,
                tab,
                pane,
            } => Ok((workspace.clone(), tab.clone(), pane.clone())),
            Target::NewTab { workspace } => client
                .create_tab(workspace, Some(&entry.cwd), Some(&entry.tab_label))
                .map(|tab| (workspace.clone(), tab.tab_id, tab.root_pane_id)),
            Target::OpenWorkspace if entry.linked_checkout => client
                .request(
                    "worktree.open",
                    json!({"cwd":entry.project,"path":entry.checkout,"focus":false}),
                )
                .and_then(|r| {
                    Ok((
                        r["workspace"]["workspace_id"]
                            .as_str()
                            .context("missing workspace ID")?
                            .into(),
                        r["tab"]["tab_id"]
                            .as_str()
                            .context("missing tab ID")?
                            .into(),
                        r["root_pane"]["pane_id"]
                            .as_str()
                            .context("missing pane ID")?
                            .into(),
                    ))
                }),
            Target::OpenWorkspace => client
                .create_workspace(Some(&entry.cwd), Some(&entry.workspace_label))
                .map(|ws| (ws.workspace_id, ws.tab_id, ws.root_pane_id)),
        };
        let (workspace, tab, pane) = match location {
            Ok(location) => location,
            Err(_) => {
                report(format!(
                    "{}: placement outcome unknown; inspect before retry",
                    entry.name
                ));
                continue;
            }
        };
        let attempt = journal.attempts.get_mut(&key).unwrap();
        attempt.pane = pane.clone();
        attempt.tab = tab.clone();
        attempt.workspace = workspace.clone();
        attempt.phase = "placement_ready".into();
        store.write_journal(&journal)?;
        report(format!("{}: resuming in {pane}; no prompt", entry.name));
        if !matches!(target, Target::Original { .. }) {
            let mut ready = false;
            for _ in 0..20 {
                if available_shell(client, &pane, &entry.cwd).unwrap_or(false) {
                    ready = true;
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
            if !ready {
                report(format!(
                    "{}: destination not ready; no start sent",
                    entry.name
                ));
                continue;
            }
        }
        let args = native::resume_args(entry)?;
        if stop.load(Ordering::Relaxed) {
            report("Stopped scheduling; destination retained for review".into());
            break;
        }
        let shell_cwd = if entry.kind == "codex" && pane == entry.pane {
            &entry.shell_cwd
        } else {
            &entry.cwd
        };
        if !available_shell(client, &pane, shell_cwd)? {
            report(format!("{}: shell changed; no command sent", entry.name));
            continue;
        }
        journal.attempts.get_mut(&key).unwrap().phase = "start_pending".into();
        store.write_journal(&journal)?;
        let name = resume_name(entry, &client.snapshot()?.agents);
        // Herdr waits up to 30 seconds for native startup. The client's usual
        // five-second read deadline is too short for that acknowledged wait.
        let started = client.request_with_timeout(
            "agent.start",
            json!({"name":name,"kind":entry.kind,"args":args,"pane_id":pane,"timeout_ms":30000}),
            Duration::from_secs(35),
        );
        if let Err(error) = started {
            let code = crate::herdr::HerdrError::code_of(&error);
            if matches!(
                code,
                Some("agent_pane_busy" | "agent_kind_not_found" | "agent_already_running")
            ) {
                journal.attempts.get_mut(&key).unwrap().phase = "placement_ready".into();
                store.write_journal(&journal)?;
                report(format!(
                    "{}: launch rejected ({})",
                    entry.name,
                    code.unwrap()
                ));
                continue;
            }
            report(format!(
                "{}: start outcome unknown; no automatic retry",
                entry.name
            ));
            continue;
        }
        let mut verified = None;
        for _ in 0..40 {
            if let Ok(live) = client.snapshot() {
                verified = live.agents.into_iter().find(|a| {
                    a.pane_id == pane
                        && a.kind() == entry.kind
                        && supervisor::native_session(a) == entry.native_id.as_deref()
                        && a.interactive_ready
                        && !a.launch_pending
                        && matches!(a.state, AgentState::Idle | AgentState::Done)
                });
                if verified.is_some() {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(250));
        }
        let Some(agent) = verified else {
            report(format!(
                "{}: native identity/readiness not confirmed; inspect {pane}, do not retry",
                entry.name
            ));
            continue;
        };
        journal.attempts.get_mut(&key).unwrap().phase = "metadata_pending".into();
        store.write_journal(&journal)?;
        let marked = bind_metadata(client, entry, &agent, &workspace, &tab);
        if marked.is_err() {
            report(format!(
                "{}: conversation resumed; Corgi metadata pending (do not relaunch)",
                entry.name
            ));
            continue;
        }
        journal.attempts.get_mut(&key).unwrap().phase = "complete".into();
        journal.restored.insert(saved.id.clone(), unix_now());
        store.write_journal(&journal)?;
        report(format!(
            "{}: same conversation ready in {pane}{}",
            entry.name,
            if entry.role == "supervisor" {
                "; coordination held"
            } else {
                ""
            }
        ));
    }
    Ok(())
}

fn bind_metadata(
    client: &HerdrClient,
    entry: &Entry,
    agent: &AgentInfo,
    workspace: &str,
    tab: &str,
) -> Result<()> {
    let mut tokens = Vec::new();
    if entry.role == "supervisor" {
        tokens.push((supervisor::SUPERVISOR_TOKEN, entry.name.as_str()));
    }
    if let Some(id) = &entry.request_id {
        tokens.push(("corgi_request", id.as_str()));
    }
    restore_marks(client, agent, &tokens, workspace, tab, entry)
}

/// Persist only item identities, never prompt/report text. A failed or unknown
/// send remains quarantined across dashboard restarts, including after Continue.
pub fn begin_delivery(store: &Store, scope: &str, project: &str, ids: &[String]) -> Result<()> {
    let _lock = store.lock()?;
    let mut journal = store.journal()?;
    let boot = store::boot()?;
    let host = store::host()?;
    let snapshots: Vec<_> = store
        .snapshots()?
        .into_iter()
        .filter(|s| {
            s.scope == scope
                && s.host == host
                && s.uid == unsafe { libc::geteuid() }
                && s.boot != boot
        })
        .collect();
    ensure!(
        !journal
            .held
            .get(scope)
            .is_some_and(|held| held.values().any(|root| root == project)),
        "recovered supervisor coordination held"
    );
    for saved in &snapshots {
        ensure!(
            !saved.entries.iter().any(|e| e.role == "supervisor"
                && e.project == project
                && !journal
                    .released
                    .get(&saved.id)
                    .is_some_and(|released| released.contains(&format!(
                        "{}:{}",
                        e.kind,
                        e.native_id.as_deref().unwrap_or("")
                    )))),
            "saved supervisor coordination held until explicitly continued"
        );
    }
    let key = |id: &str| format!("{scope}\n{project}\n{id}");
    ensure!(
        !ids.iter().any(|id| journal.deliveries.contains(&key(id))),
        "inbox delivery outcome unknown; quarantined, inspect with corgi inbox"
    );
    for saved in &snapshots {
        ensure!(
            !saved
                .entries
                .iter()
                .any(|e| e.project == project
                    && e.pending_inbox_ids.iter().any(|id| ids.contains(id))),
            "pre-reboot inbox delivery unconfirmed; retained for review with corgi inbox"
        );
    }
    for id in ids {
        journal.deliveries.insert(key(id));
    }
    store.write_journal(&journal)
}

pub fn finish_delivery(store: &Store, scope: &str, project: &str, ids: &[String]) -> Result<()> {
    let _lock = store.lock()?;
    let mut journal = store.journal()?;
    for id in ids {
        journal
            .deliveries
            .remove(&format!("{scope}\n{project}\n{id}"));
    }
    store.write_journal(&journal)
}

/// Takes back the `ids` [`begin_delivery`] recorded, for a delivery Herdr
/// refused before typing anything: they were not sent, so they are no longer
/// quarantined and go out at a later pass.
pub fn abort_delivery(store: &Store, scope: &str, project: &str, ids: &[String]) -> Result<()> {
    finish_delivery(store, scope, project, ids)
}

/// The ids of `project`'s inbox items on the Herdr session `scope` whose
/// delivery began and was never confirmed: quarantined, so they are never
/// typed in again, and with them every later item of the project, until
/// someone looks.
pub fn quarantined(store: &Store, scope: &str, project: &str) -> Result<Vec<String>> {
    let journal = store.journal()?;
    let prefix = format!("{scope}\n{project}\n");
    Ok(journal
        .deliveries
        .iter()
        .filter_map(|key| key.strip_prefix(&prefix).map(str::to_string))
        .collect())
}

/// Puts back the pane tokens and, for a recovered workspace, the workspace
/// tokens Corgi had set for the agent `entry` recorded.
pub fn restore_marks(
    client: &HerdrClient,
    agent: &AgentInfo,
    tokens: &[(&str, &str)],
    workspace: &str,
    tab: &str,
    entry: &Entry,
) -> Result<()> {
    crate::markers::mark_pane(
        client,
        &agent.pane_id,
        agent.name.as_deref().unwrap_or("recovered"),
        crate::markers::session_of(agent),
        tokens,
    )?;
    if !entry.workspace_tokens.is_empty() {
        let tokens = entry
            .workspace_tokens
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str(),
                    if k == "corgi_project_main_tab" && v == &entry.tab {
                        tab
                    } else {
                        v.as_str()
                    },
                )
            })
            .collect::<Vec<_>>();
        crate::markers::mark_workspace(
            client,
            workspace,
            if entry.linked_checkout {
                entry.cwd.to_str()
            } else {
                None
            },
            &tokens,
        )?;
    }
    Ok(())
}

/// The `corgi spawn --request-id` the agent was tagged with, once confirmed.
pub fn confirmed_request(client: &HerdrClient, agent: &AgentInfo) -> Option<String> {
    crate::markers::confirmed_request(client, agent)
}

/// The name the agent resumes as: its own, unless a live agent took it.
pub fn resume_name(entry: &Entry, live: &[AgentInfo]) -> String {
    let original = crate::launch::sanitize_agent_name(&entry.name);
    if live.iter().any(|a| a.name.as_deref() == Some(&original)) {
        format!(
            "r-{}",
            &entry.native_id.as_deref().unwrap().replace('-', "")[..30]
        )
    } else {
        original
    }
}
