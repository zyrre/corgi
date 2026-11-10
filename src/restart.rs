//! Bringing Corgi's agents back with the options they were launched with.
//!
//! Herdr 0.9.2 and later resume each agent after a restart with the command
//! a client reported for it (`resume_argv`), in place of their built-in
//! `claude --resume <id>`, which drops a supervisor's role and memory access
//! and a worker's model, effort and extra arguments. So Corgi records how it
//! launched each agent, by pane, in `markers/<socket>.launches.json` beside
//! the marker record, and the pass holding the wake lock ([`reconcile`])
//! reports each one's full resume command with `pane.report_agent_session`
//! from Corgi's own plugin source, never under Herdr's `herdr:` sources
//! (Herdr ignores the command there) and never with `pane.report_agent`
//! (which would take over the pane's state). The command follows the
//! agent's session, as after `/clear`, and is released once the pane holds
//! another agent. Herdr 0.9.0 and 0.9.1 take the report and ignore the
//! command.
//!
//! An agent that exits while Herdr keeps running drops to its shell, and
//! Herdr resumes nothing there. Such a pane, where Corgi saw an agent
//! session before, offers Resume ([`resumable`], [`resume`]), which starts
//! the agent again in the same pane with the same command. Corgi never
//! relaunches by itself.
//!
//! Each check of a new process compares its command line with the options
//! Corgi launched it with; what it lacks ([`Launched::missing`]) is shown as
//! a warning on the agent's row, as for an agent an older Herdr resumed. The
//! command line is the agent's own process's, never a child's, and an agent
//! flagged, or whose process could not be told, is checked again each pass.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    harness::{CODEX_DEVELOPER_INSTRUCTIONS, Harness, codex_supervisor_flags},
    herdr::{HerdrClient, PaneProcesses, SessionSnapshot},
    model::AgentInfo,
    recovery::{
        self,
        native::{self, SupervisorFiles},
    },
    supervisor,
};

/// The source Corgi reports resume commands under: its own plugin's.
pub const SOURCE: &str = "plugin:io.github.zyrre.corgi";

/// The agents Corgi launched in one Herdr session, by pane ID.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub panes: BTreeMap<String, Launched>,
}

/// How Corgi launched the agent in one pane, and what it last saw of it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launched {
    /// The harness, as Herdr names it: `claude` or `codex`.
    pub kind: String,
    /// The agent's Herdr name, as last seen.
    pub name: String,
    /// The directory it was started in, where it must be resumed.
    pub cwd: PathBuf,
    /// Its launch options after the CLI's name: model, effort and the extra
    /// arguments, which the warning checks.
    #[serde(default)]
    pub options: Vec<String>,
    /// The folder-trust answer Corgi adds for a project it created: passed
    /// again, not checked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trust: Vec<String>,
    /// A supervisor's role file and state directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supervisor: Option<SupervisorRole>,
    /// The native session Corgi last saw running in the pane. Without one
    /// the pane offers no Resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Herdr's terminal id at the last check; a Herdr restart replaces it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    /// The agent's foreground processes at the last check. Empty when it
    /// was never checked or was last seen gone, so the next agent in the
    /// pane is checked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processes: Vec<u32>,
    /// What the running process lacks of its launch options, at the last
    /// check: `role`, `memory`, `model`, `effort` or a flag. While it lacks
    /// any, every pass checks again, so a wrong reading clears itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// The last check found no process that is surely the agent's own (only
    /// its children, say), so it warned of nothing and the next pass checks
    /// again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unidentified: bool,
    /// The command last reported to Herdr, and that Herdr's version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported: Option<Reported>,
    /// Stopped by Pause all ([`crate::pause`]): its stops wake no
    /// supervisor and it is not handed over until it is back, and Resume
    /// all starts it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<Paused>,
}

/// An agent's mark from Pause all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paused {
    /// When it was set, Unix seconds.
    pub at: u64,
    /// The agent's project, whose supervisor keeps its handover while it is
    /// away.
    pub project_root: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub supervisor: bool,
    /// Resume started it again: the mark goes once a pass has seen it back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resuming: bool,
}

/// A supervisor's durable files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorRole {
    pub role_file: PathBuf,
    pub state_dir: PathBuf,
}

/// A resume command Herdr took.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reported {
    pub argv: Vec<String>,
    /// The Herdr version that took it: a newer one, after an upgrade by
    /// handoff, is told again, since an older one may have ignored it.
    pub herdr: String,
}

impl Launched {
    /// The whole command, CLI name first, that resumes `session` here.
    pub fn argv(&self, session: &str) -> Result<Vec<String>> {
        native::restart_argv(
            &self.kind,
            session,
            &self.cwd,
            &self.options,
            &self.trust,
            self.supervisor.as_ref().map(|role| SupervisorFiles {
                role_file: &role.role_file,
                state_dir: &role.state_dir,
            }),
        )
    }

    /// The model and effort it was launched on, when its options name them.
    pub fn model_and_effort(&self) -> (Option<String>, Option<String>) {
        let (mut model, mut effort) = (None, None);
        for (label, option) in options(&self.options) {
            let Expected::Pair(_, value) = option else {
                continue;
            };
            match label.as_str() {
                "model" => model = Some(value),
                "effort" => effort = Some(value.rsplit('=').next().unwrap_or(&value).to_string()),
                _ => {}
            }
        }
        (model, effort)
    }

    /// What a process started as `argv` lacks of this launch's options,
    /// role and memory access, by name.
    pub fn lacks(&self, argv: &[String]) -> Vec<String> {
        let mut missing: Vec<String> = Vec::new();
        for (label, expected) in self.expected() {
            if !expected.found_in(argv) && !missing.contains(&label) {
                missing.push(label);
            }
        }
        missing
    }

    /// The options a running process must show, each with the name the
    /// warning gives it.
    fn expected(&self) -> Vec<(String, Expected)> {
        let mut expected = options(&self.options);
        if let Some(role) = &self.supervisor {
            let state_dir = role.state_dir.to_string_lossy().into_owned();
            if self.kind == "codex" {
                expected.extend(options(&codex_supervisor_flags(&role.state_dir)));
                expected.push((
                    "role".into(),
                    Expected::Prefix("-c".into(), CODEX_DEVELOPER_INSTRUCTIONS.into()),
                ));
            } else {
                expected.push((
                    "role".into(),
                    Expected::Pair(
                        "--append-system-prompt-file".into(),
                        role.role_file.to_string_lossy().into_owned(),
                    ),
                ));
                expected.push((
                    "memory".into(),
                    Expected::Pair("--add-dir".into(), state_dir.clone()),
                ));
            }
            for (label, option) in &mut expected {
                if *option == Expected::Pair("--add-dir".into(), state_dir.clone()) {
                    *label = "memory".into();
                }
            }
        }
        expected
    }
}

/// One option a process must show: a flag and its value next to each other,
/// a flag alone, or a flag whose value starts a certain way.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expected {
    Pair(String, String),
    Flag(String),
    Prefix(String, String),
}

impl Expected {
    fn found_in(&self, argv: &[String]) -> bool {
        match self {
            Self::Flag(flag) => argv.contains(flag),
            Self::Pair(flag, value) => argv
                .windows(2)
                .any(|pair| pair[0] == *flag && pair[1] == *value),
            Self::Prefix(flag, prefix) => argv
                .windows(2)
                .any(|pair| pair[0] == *flag && pair[1].starts_with(prefix.as_str())),
        }
    }
}

/// `args` as options to look for: a word starting with `-` takes the next
/// as its value unless that starts with `-` too.
fn options(args: &[String]) -> Vec<(String, Expected)> {
    let mut options = Vec::new();
    let mut words = args.iter().peekable();
    while let Some(word) = words.next() {
        let value = words
            .next_if(|next| word.starts_with('-') && !next.starts_with('-'))
            .cloned();
        let label = option_label(word, value.as_deref());
        options.push((
            label,
            match value {
                Some(value) => Expected::Pair(word.clone(), value),
                None => Expected::Flag(word.clone()),
            },
        ));
    }
    options
}

/// The name the warning gives an option.
fn option_label(flag: &str, value: Option<&str>) -> String {
    match (flag, value) {
        ("--model", _) => "model".into(),
        ("--effort", _) => "effort".into(),
        ("-c", Some(value)) => match value.split_once('=').map(|(key, _)| key) {
            Some("model_reasoning_effort") => "effort".into(),
            Some("sandbox_workspace_write.network_access") => "network access".into(),
            Some(key) => key.into(),
            None => value.into(),
        },
        _ => flag.into(),
    }
}

/// The warning for an agent running without `missing`, its launch options.
pub fn warning_text(missing: &[String]) -> String {
    let lacks = |what: &str| missing.iter().any(|item| item == what);
    let what = if lacks("role") && lacks("memory") {
        "its supervisor role and memory access".to_string()
    } else if lacks("role") {
        "its supervisor role".to_string()
    } else if lacks("memory") && missing.len() == 1 {
        "write access to its memory".to_string()
    } else {
        format!("its launch options ({})", missing.join(", "))
    };
    format!("Running without {what}: exit it, then Resume")
}

/// Where the record of `client`'s session is kept: beside its marker
/// record, so a test's scratch record keeps both out of the real state.
pub fn record_path(client: &HerdrClient) -> Option<PathBuf> {
    let markers = client.marker_record()?;
    let stem = markers.file_stem()?.to_string_lossy().into_owned();
    Some(markers.with_file_name(format!("{stem}.launches.json")))
}

/// `client`'s record; an empty one when there is none.
pub fn read(client: &HerdrClient) -> Record {
    record_path(client)
        .map(|path| read_at(&path))
        .unwrap_or_default()
}

pub(crate) fn read_at(path: &Path) -> Record {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Reads the record at `path`, lets `change` edit it and writes it back if
/// it changed, under a lock beside it, as the marker record is kept.
pub(crate) fn update<T>(path: &Path, change: impl FnOnce(&mut Record) -> T) -> Result<T> {
    let dir = path
        .parent()
        .context("the launch record has no directory")?;
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let lock = fs::File::create(path.with_extension("lock"))?;
    lock.lock()?;
    let mut record = read_at(path);
    let before = record.clone();
    let result = change(&mut record);
    let written = if record == before {
        Ok(())
    } else {
        write(path, &record)
    };
    let _ = lock.unlock();
    written.map(|()| result)
}

fn write(path: &Path, record: &Record) -> Result<()> {
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let written = fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(&serde_json::to_vec_pretty(record)?)?;
        file.sync_all()
    });
    if let Err(error) = written.and_then(|()| fs::rename(&temporary, path)) {
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("write {}", path.display()));
    }
    Ok(())
}

/// Records how Corgi launched `agent` on `harness` (just started by
/// `agent.start`, whose reply names no harness): its launch `options` after
/// the CLI's name (model, effort, extra arguments), the folder-trust answer
/// `trust`, and a supervisor's `role`. The next pass holding the wake lock
/// reports its resume command.
pub fn record_launch(
    client: &HerdrClient,
    harness: &Harness,
    agent: &AgentInfo,
    options: Vec<String>,
    trust: Vec<String>,
    role: Option<SupervisorRole>,
) -> Result<()> {
    if !matches!(harness, Harness::Claude | Harness::Codex) {
        return Ok(());
    }
    let Some(path) = record_path(client) else {
        return Ok(());
    };
    let launched = Launched {
        kind: harness.kind().into(),
        name: agent.name.clone().unwrap_or_else(|| harness.kind().into()),
        cwd: PathBuf::from(agent.cwd()),
        options,
        trust,
        supervisor: role,
        session: supervisor::native_session(agent)
            .filter(|id| native::valid_id(id))
            .map(str::to_string),
        ..Launched::default()
    };
    update(&path, |record| {
        record.panes.insert(agent.pane_id.clone(), launched);
    })
}

/// Forgets the agent Corgi launched in `pane_id`, whose pane is about to
/// hold another, as a supervisor's successor: no Resume is offered for the
/// one leaving.
pub fn forget(client: &HerdrClient, pane_id: &str) {
    if let Some(path) = record_path(client) {
        let _ = update(&path, |record| record.panes.remove(pane_id));
    }
}

/// What [`reconcile`] asks of Herdr, so its tests need no socket.
trait Herdr {
    fn process_info(&self, pane_id: &str) -> Result<PaneProcesses>;
    fn report(&self, pane_id: &str, agent: &str, session: &str, argv: &[String]) -> Result<()>;
    fn release(&self, pane_id: &str, agent: &str) -> Result<()>;
}

impl Herdr for HerdrClient {
    fn process_info(&self, pane_id: &str) -> Result<PaneProcesses> {
        HerdrClient::process_info(self, pane_id)
    }

    fn report(&self, pane_id: &str, agent: &str, session: &str, argv: &[String]) -> Result<()> {
        self.report_agent_session(pane_id, SOURCE, agent, session, argv)
    }

    fn release(&self, pane_id: &str, agent: &str) -> Result<()> {
        self.release_agent(pane_id, SOURCE, agent)
    }
}

/// Keeps the resume command Herdr holds for each of Corgi's agents in
/// `snapshot` current, and the record in step with it: reports a pane's
/// command when its session changed (as after `/clear`), when a newer Herdr
/// took over, or when it was never reported; releases it once the pane
/// holds another agent; forgets panes that are gone. A supervisor or a
/// worker Corgi launched before it kept this record is taken in from its
/// saved launch or recovery profile. Only the pass holding the wake lock
/// calls this. Returns a status line when it reported or released any.
pub fn reconcile(client: &HerdrClient, snapshot: &SessionSnapshot) -> Option<String> {
    let path = record_path(client)?;
    let profiles = recovery::Store::for_client(client).map(|store| store.0.join("profiles"));
    update(&path, |record| {
        adopt(record, snapshot, profiles.as_deref());
        reconcile_record(client, record, snapshot)
    })
    .ok()
    .flatten()
}

/// Takes in the supervisors and the workers with a recovery profile that
/// Corgi launched before it recorded launches here.
fn adopt(record: &mut Record, snapshot: &SessionSnapshot, profiles: Option<&Path>) {
    for agent in &snapshot.agents {
        if record.panes.contains_key(&agent.pane_id)
            || !matches!(agent.kind(), "claude" | "codex")
            || !supervisor::native_session(agent).is_some_and(native::valid_id)
        {
            continue;
        }
        let workspace = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == agent.workspace_id);
        if let Some(launched) = adopted(agent, workspace, profiles) {
            record.panes.insert(agent.pane_id.clone(), launched);
        }
    }
}

fn adopted(
    agent: &AgentInfo,
    workspace: Option<&crate::model::WorkspaceInfo>,
    profiles: Option<&Path>,
) -> Option<Launched> {
    let harness = agent.harness();
    let base = Launched {
        kind: agent.kind().into(),
        name: agent.name.clone().unwrap_or_else(|| agent.kind().into()),
        cwd: PathBuf::from(agent.cwd()),
        ..Launched::default()
    };
    if supervisor::is_supervisor(agent) {
        let project = workspace
            .and_then(|workspace| workspace.repo_root())
            .unwrap_or(agent.cwd());
        let state_dir = supervisor::state_dir(project).ok()?;
        let role_file = state_dir.join(supervisor::ROLE_FILE);
        if !role_file.is_file() {
            return None;
        }
        let launch = supervisor::launch_of(&state_dir, &harness);
        let mut options = harness.launch_args(&launch.model, &launch.effort);
        options.extend(launch.extra_args);
        return Some(Launched {
            options,
            trust: crate::launch::trusted_project_args(&harness, Path::new(project)),
            supervisor: Some(SupervisorRole {
                role_file,
                state_dir,
            }),
            ..base
        });
    }
    let id = supervisor::native_session(agent)?;
    let path = profiles?.join(format!("{}-{id}.json", agent.kind()));
    let profile: native::Profile = recovery::store::read(&path).ok()?;
    if profile.id != id || profile.kind != agent.kind() || profile.cwd != Path::new(agent.cwd()) {
        return None;
    }
    Some(Launched {
        options: native::settings_args(agent.kind(), &profile.settings).ok()?,
        ..base
    })
}

fn reconcile_record(
    herdr: &dyn Herdr,
    record: &mut Record,
    snapshot: &SessionSnapshot,
) -> Option<String> {
    let panes: HashSet<&str> = snapshot
        .panes
        .iter()
        .map(|pane| pane.pane_id.as_str())
        .collect();
    let agents: HashMap<&str, &AgentInfo> = snapshot
        .agents
        .iter()
        .filter(|agent| is_agent(agent))
        .map(|agent| (agent.pane_id.as_str(), agent))
        .collect();
    let (mut reported, mut released) = (0, 0);
    let withdraw = |entry: &Launched, pane: &str| {
        entry.reported.is_some() && herdr.release(pane, &entry.kind).is_ok()
    };
    let keys: Vec<String> = record.panes.keys().cloned().collect();
    for pane in keys {
        if !panes.contains(pane.as_str()) {
            // Gone with its pane, and Herdr with it.
            record.panes.remove(&pane);
            continue;
        }
        let entry = record.panes.get_mut(&pane).expect("listed above");
        let Some(agent) = agents.get(pane.as_str()) else {
            // Back at its shell, or not detected again yet: whatever comes
            // next in the pane is checked.
            entry.processes.clear();
            continue;
        };
        if agent.kind() != entry.kind {
            released += usize::from(withdraw(entry, &pane));
            record.panes.remove(&pane);
            continue;
        }
        let Some(live) = supervisor::native_session(agent).filter(|id| native::valid_id(id)) else {
            continue;
        };
        let same_session = entry.session.as_deref() == Some(live);
        // A warning, or a check that could not tell the agent's process,
        // is checked again each pass: a child picked or read too early must
        // not leave a false warning until the agent restarts.
        let unsettled = !entry.missing.is_empty() || entry.unidentified;
        if !same_session
            || entry.terminal != agent.terminal_id
            || entry.processes.is_empty()
            || unsettled
        {
            let Ok(info) = herdr.process_info(&pane) else {
                continue;
            };
            let pids = info.agent_pids();
            // A new session in the same process, as after `/clear`, is
            // still Corgi's agent; a new process with another is not.
            let continuing = entry.session.is_none()
                || same_session
                || pids.iter().any(|pid| entry.processes.contains(pid));
            if !continuing {
                released += usize::from(withdraw(entry, &pane));
                record.panes.remove(&pane);
                continue;
            }
            if pids.is_empty() {
                continue;
            }
            // Herdr forgets a reported command with the agent it was for, so
            // a new process (after an exit and Resume, or a restart) is told
            // again.
            if !pids.iter().any(|pid| entry.processes.contains(pid)) {
                entry.reported = None;
            }
            // No guess: without the agent's own command line, no warning.
            let argv = agent_argv(&info, &entry.kind);
            entry.missing = argv
                .as_deref()
                .map(|argv| entry.lacks(argv))
                .unwrap_or_default();
            entry.unidentified = argv.is_none();
            entry.session = Some(live.to_string());
            entry.terminal = agent.terminal_id.clone();
            entry.processes = pids;
        }
        if let Some(name) = agent.name.as_ref().filter(|name| !name.is_empty()) {
            entry.name = name.clone();
        }
        match entry.argv(live) {
            Ok(argv) => {
                let wanted = Reported {
                    argv,
                    herdr: snapshot.version.clone(),
                };
                if entry.reported.as_ref() != Some(&wanted)
                    && herdr.report(&pane, &entry.kind, live, &wanted.argv).is_ok()
                {
                    entry.reported = Some(wanted);
                    reported += 1;
                }
            }
            // A command Herdr would refuse: rather its own resume of the
            // live session than Corgi's of an old one.
            Err(_) => {
                if withdraw(entry, &pane) {
                    entry.reported = None;
                    released += 1;
                }
            }
        }
    }
    match (reported, released) {
        (0, 0) => None,
        (reported, 0) => Some(format!(
            "Gave Herdr the resume command of {reported} agent{}",
            plural(reported)
        )),
        (reported, released) => Some(format!(
            "Gave Herdr the resume command of {reported} agent{}, withdrew {released}",
            plural(reported)
        )),
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// Whether Herdr has an agent in the pane. An agent that just exited stays
/// in the snapshot for a moment without a harness, which is no other agent.
pub(crate) fn is_agent(agent: &AgentInfo) -> bool {
    agent.agent.as_deref().is_some_and(|kind| !kind.is_empty())
}

/// The command line of the agent's own process in a pane, never one of its
/// children (MCP servers, hooks, `caffeinate`), which Herdr lists with it,
/// newest first: the foreground group's leader, which the shell started,
/// else the oldest process running the harness's CLI by name. `None` when
/// neither is there with a readable command line.
fn agent_argv(info: &PaneProcesses, kind: &str) -> Option<Vec<String>> {
    let readable = || {
        info.foreground_processes
            .iter()
            .filter(|process| Some(process.pid) != info.shell_pid && process.argv.is_some())
    };
    let leader = info
        .foreground_process_group_id
        .filter(|pid| Some(*pid) != info.shell_pid);
    readable()
        .find(|process| Some(process.pid) == leader)
        .or_else(|| readable().rfind(|process| process.runs(kind)))
        .and_then(|process| process.argv.clone())
}

/// The panes of `snapshot` where Corgi saw one of its agents' sessions
/// running and that have dropped to their shell: each offers Resume. A pane
/// Corgi saw no session in, or one with an agent, never does.
pub fn resumable<'a>(
    record: &'a Record,
    snapshot: &SessionSnapshot,
) -> Vec<(&'a str, &'a Launched)> {
    record
        .panes
        .iter()
        .filter(|(pane, entry)| {
            entry.session.is_some()
                && snapshot.panes.iter().any(|p| p.pane_id == **pane)
                && !snapshot
                    .agents
                    .iter()
                    .any(|a| a.pane_id == **pane && is_agent(a))
        })
        .map(|(pane, entry)| (pane.as_str(), entry))
        .collect()
}

/// What the running `agent` lacks of the options Corgi launched it with,
/// when it is the process the record last checked.
pub fn missing<'a>(record: &'a Record, agent: &AgentInfo) -> Option<&'a [String]> {
    let entry = record.panes.get(&agent.pane_id)?;
    (entry.kind == agent.kind()
        && entry.session.as_deref() == supervisor::native_session(agent)
        && !entry.missing.is_empty())
    .then_some(entry.missing.as_slice())
}

/// Starts the agent Corgi launched in `pane_id` again, now that it has
/// dropped to its shell: the same session, in the same pane, with the
/// command Corgi reports to Herdr for a restart. Refuses a pane without a
/// session Corgi saw, one with an agent, and a shell that is busy or has
/// left the agent's directory.
pub fn resume(client: &HerdrClient, pane_id: &str) -> Result<AgentInfo> {
    let path = record_path(client).context("Corgi keeps no record of its launches here")?;
    let entry = read_at(&path)
        .panes
        .get(pane_id)
        .cloned()
        .context("Corgi did not launch the agent in this pane")?;
    let session = entry
        .session
        .clone()
        .context("Corgi never saw a session of this pane's agent")?;
    let snapshot = client.snapshot()?;
    ensure!(
        snapshot.panes.iter().any(|pane| pane.pane_id == pane_id),
        "the pane is gone"
    );
    ensure!(
        !snapshot
            .agents
            .iter()
            .any(|agent| agent.pane_id == pane_id && is_agent(agent)),
        "an agent already runs in this pane"
    );
    ensure!(
        recovery::available_shell(client, pane_id, &entry.cwd)?,
        "the pane's shell is busy, or no longer in {}",
        entry.cwd.display()
    );
    let argv = entry.argv(&session)?;
    // Back from Pause all: marked before it starts, so the pass that sees
    // it come back already knows to drop the mark rather than wake anyone.
    if entry.paused.is_some() {
        update(&path, |record| {
            if let Some(paused) = record
                .panes
                .get_mut(pane_id)
                .and_then(|entry| entry.paused.as_mut())
            {
                paused.resuming = true;
            }
        })?;
    }
    let started = client
        .start_agent(
            &entry.name,
            Harness::from_kind(&entry.kind).kind(),
            &argv[1..],
            pane_id,
        )
        .with_context(|| format!("start {} again in {pane_id}", entry.name))?;
    Ok(started.agent)
}

#[cfg(test)]
mod tests;
