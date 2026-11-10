//! Pause and Resume on herdr-core (`hc`), the Herdr app's "⏸ Agents" and
//! "▶ Agents": before the network goes (a Wi-Fi or VPN left), the agents
//! that are working at that moment are stopped, so none fails mid-turn
//! without it, and Resume brings exactly those back on their own
//! conversations.
//!
//! - Pause stops every agent of Corgi's on hc that is working (mid-turn)
//!   when it is pressed, supervisors and workers alike: Esc first, which
//!   ends its turn, then its harness's own exit (`/exit`, `/quit`), so its
//!   conversation is saved. Idle, done and blocked agents are left as they
//!   are, and with none working nothing changes at all.
//! - Only an agent Corgi can bring back is stopped: one whose terminal holds
//!   its resume line ([`RESUME_LABEL`], which Corgi writes for the Claude
//!   Code sessions it starts). Another is left running and named.
//! - Nothing is ever killed: an agent that keeps working after Esc, holds
//!   text in its input box, or does not exit, is left running, and the
//!   outcome says so.
//!
//! The pause is kept in a file of Corgi's state, `pause/<socket>.json`
//! ([`record_path`]), not in terminal labels: a worker's terminal finishes
//! when its agent exits, and the file outlasts the terminal, the core, the
//! app and the daemon alike, so a restart keeps the agents paused. Every
//! agent is marked before any key goes out. While any agent is paused (a
//! pause is "in force", [`Holds::in_force`]), `corgi daemon`:
//!
//! - resumes no paused agent's terminal after a core restart;
//! - adds no wake for a paused agent's stop;
//! - types nothing into any supervisor, paused or not, and starts no
//!   handover step, since either would start a turn while the person may be
//!   offline. Wakes still queue in the inboxes and go out after Resume.
//!
//! Resume types the resume line into a supervisor's kept shell (or a
//! worker's, when a core restart replaced its agent with one), or starts a
//! worker again in a new terminal with its old one's name and labels, then
//! removes the old, finished one. Each agent's mark then waits for the
//! daemon's next pass ([`settle`]), which follows it back without a wake. An
//! agent that does not come back is reported and its mark goes, so it holds
//! no one's wakes.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    HcClient, PID_LABEL, PROJECT_LABEL, RESUME_LABEL, ROLE_LABEL, RunSpec, SESSION_LABEL,
    SUPERVISOR_ROLE, Terminal,
    agents::{agent_state, corgi_terminals, shell_in_front},
};
use crate::{
    draft, harness::Harness, model::Agent, model::AgentState, paths::socket_state_file,
    progress::Progress,
};

/// How long a mark may wait for its pause to finish, or for the daemon to
/// see its agent back after Resume: longer than either takes, so a pause
/// that stopped halfway (its process gone) holds nothing for long.
pub const STALE_SECS: u64 = 120;

/// How often a pause or resume looks again while it waits, and how many
/// looks it waits for working agents to stop after Esc, for agents to exit,
/// and for a resumed agent to come back.
const POLL: Duration = Duration::from_millis(250);
const INTERRUPT_POLLS: usize = 40;
const EXIT_POLLS: usize = 80;
const BACK_POLLS: usize = 80;

/// Where the pause of the core at `socket` is kept, in Corgi's state
/// directory: `pause/<socket>.json`.
pub fn record_path(socket: &Path) -> Option<PathBuf> {
    socket_state_file("pause", socket, "json")
}

/// Where a paused agent is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Marked; its pause is stopping it.
    Stopping,
    /// Stopped: Resume brings it back.
    Paused,
    /// Started again; the daemon's next pass takes it up.
    Resuming,
}

/// One agent a pause stopped, as the pause file keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PausedAgent {
    /// The terminal it ran in; after Resume, the one it runs in again.
    pub address: String,
    /// The terminal's name, which it keeps.
    pub name: String,
    /// Its project's primary checkout.
    pub project: String,
    pub supervisor: bool,
    /// `claude`, `codex` or `opencode`.
    pub harness: String,
    #[serde(default)]
    pub session: Option<String>,
    /// The line that resumes it in a shell ([`RESUME_LABEL`]).
    pub resume: String,
    /// The folder its terminal started in.
    pub cwd: String,
    /// Its terminal's labels, which a worker's new terminal takes.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub phase: Phase,
    /// When it was marked, or, once resuming, started again (Unix seconds).
    pub at: u64,
}

impl PausedAgent {
    fn of(terminal: &Terminal, resume: &str, now: u64) -> Self {
        Self {
            address: terminal.address.clone(),
            name: terminal.display_name().to_string(),
            project: terminal
                .label(PROJECT_LABEL)
                .unwrap_or_default()
                .to_string(),
            supervisor: terminal.label(ROLE_LABEL) == Some(SUPERVISOR_ROLE),
            harness: terminal
                .label(super::HARNESS_LABEL)
                .unwrap_or("agent")
                .to_string(),
            session: terminal.label(SESSION_LABEL).map(str::to_string),
            resume: resume.to_string(),
            cwd: terminal.cwd.clone(),
            labels: terminal.labels.clone(),
            phase: Phase::Stopping,
            at: now,
        }
    }

    /// Whether it holds: stopping or stopped, not on its way back.
    pub fn is_paused(&self) -> bool {
        self.phase != Phase::Resuming
    }

    /// Whether `terminal` is this agent's: the same name in the same
    /// project. A worker's comes back in a new terminal, and a core restart
    /// may give an address to another.
    fn owns(&self, terminal: &Terminal) -> bool {
        terminal.display_name() == self.name
            && terminal.label(PROJECT_LABEL) == Some(self.project.as_str())
    }

    /// Whether `agent`, a row, is this agent's.
    fn is_row(&self, agent: &Agent) -> bool {
        agent.project_root == self.project
            && (agent.info.address == self.address
                || agent.info.name.as_deref() == Some(self.name.as_str()))
    }
}

/// The pause file: the agents a pause stopped and has not yet seen back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub agents: Vec<PausedAgent>,
}

impl Record {
    /// Whether a pause is in force: any agent stopping or stopped.
    pub fn in_force(&self) -> bool {
        self.agents.iter().any(PausedAgent::is_paused)
    }
}

/// The pause file at `path`; an empty one when there is none.
pub fn read(path: &Path) -> Record {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Reads the pause file at `path`, lets `change` edit it and writes it back
/// if it changed, under a lock beside it. An empty record removes the file.
pub fn update<T>(path: &Path, change: impl FnOnce(&mut Record) -> T) -> Result<T> {
    let dir = path.parent().context("the pause file has no directory")?;
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let lock = fs::File::create(path.with_extension("lock"))?;
    lock.lock()?;
    let mut record = read(path);
    let before = record.clone();
    let result = change(&mut record);
    let written = if record == before {
        Ok(())
    } else if record.agents.is_empty() {
        match fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(error).with_context(|| format!("remove {}", path.display()))
            }
            _ => Ok(()),
        }
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

/// What the daemon holds back while agents are paused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Holds {
    /// Whether a pause is in force: no delivery to any supervisor and no
    /// handover step then.
    pub in_force: bool,
    /// Every marked agent, paused or on its way back: its stops wake no one
    /// and the daemon resumes nothing of it.
    agents: Vec<PausedAgent>,
}

impl Holds {
    pub fn of(record: &Record) -> Self {
        Self {
            in_force: record.in_force(),
            agents: record.agents.clone(),
        }
    }

    /// The holds of the pause file at `path`.
    pub fn read(path: &Path) -> Self {
        Self::of(&read(path))
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Whether `terminal` runs a marked agent.
    pub fn holds_terminal(&self, terminal: &Terminal) -> bool {
        self.agents
            .iter()
            .any(|paused| paused.address == terminal.address || paused.owns(terminal))
    }

    /// The terminal addresses of `agents` that are marked agents'.
    pub fn panes(&self, agents: &[Agent]) -> HashSet<String> {
        agents
            .iter()
            .filter(|agent| self.agents.iter().any(|paused| paused.is_row(agent)))
            .map(|agent| agent.info.address.clone())
            .collect()
    }

    /// The projects whose supervisor is marked, which keep their handover
    /// record while it is away.
    pub fn supervisor_roots(&self) -> HashSet<String> {
        self.agents
            .iter()
            .filter(|paused| paused.supervisor)
            .map(|paused| paused.project.clone())
            .collect()
    }
}

/// Lets go of the marks the daemon's pass over `agents` (the core's agents,
/// with the marked ones' stops held) shows are done with: an agent seen back
/// after Resume, with its agent in front, and a mark older than
/// [`STALE_SECS`] that is not paused (a pause or a resume that stopped
/// halfway). Called after the pass recorded what it saw, so the next one
/// takes the agent up from there and wakes no one for its return. Returns
/// the names it let go of.
pub fn settle(path: &Path, agents: &[Agent], now: u64) -> Vec<String> {
    let settled = |record: &Record| -> Vec<String> {
        record
            .agents
            .iter()
            .filter(|paused| {
                let back = paused.phase == Phase::Resuming
                    && agents
                        .iter()
                        .any(|agent| !agent.exited && paused.is_row(agent));
                let stale =
                    paused.phase != Phase::Paused && now.saturating_sub(paused.at) > STALE_SECS;
                back || stale
            })
            .map(|paused| paused.address.clone())
            .collect()
    };
    let record = read(path);
    if settled(&record).is_empty() {
        return Vec::new();
    }
    update(path, |record| {
        let gone = settled(record);
        let names = record
            .agents
            .iter()
            .filter(|paused| gone.contains(&paused.address))
            .map(|paused| paused.name.clone())
            .collect();
        record
            .agents
            .retain(|paused| !gone.contains(&paused.address));
        names
    })
    .unwrap_or_default()
}

/// What a pause or a resume asks of hc, so their tests need no core.
pub trait Core {
    /// The live terminals Corgi labelled.
    fn terminals(&self) -> Result<Vec<Terminal>>;
    /// The terminal `address`, live or finished, if hc has it.
    fn get(&self, address: &str) -> Result<Option<Terminal>>;
    fn keys(&self, address: &str, keys: &[&str]) -> Result<()>;
    fn send(&self, address: &str, text: &str) -> Result<()>;
    /// Whether `terminal`'s input box holds text; `None` when it cannot
    /// be told.
    fn holds_draft(&self, terminal: &Terminal) -> Option<bool>;
    fn run(&self, spec: &RunSpec) -> Result<Terminal>;
    fn remove(&self, address: &str) -> Result<()>;
    fn label(&self, address: &str, labels: &[(&str, &str)]) -> Result<()>;
    fn wait(&self) {
        thread::sleep(POLL);
    }
}

impl Core for HcClient {
    fn terminals(&self) -> Result<Vec<Terminal>> {
        corgi_terminals(self)
    }

    fn get(&self, address: &str) -> Result<Option<Terminal>> {
        self.find(address)
    }

    fn keys(&self, address: &str, keys: &[&str]) -> Result<()> {
        HcClient::keys(self, address, keys)
    }

    fn send(&self, address: &str, text: &str) -> Result<()> {
        HcClient::send(self, address, text).map(drop)
    }

    fn holds_draft(&self, terminal: &Terminal) -> Option<bool> {
        let harness = Harness::from_kind(terminal.label(super::HARNESS_LABEL)?);
        let paint = self
            .observe_paint(&terminal.address, terminal.cols, terminal.rows)
            .ok()?;
        draft::holds_draft(&harness, &paint)
    }

    fn run(&self, spec: &RunSpec) -> Result<Terminal> {
        HcClient::run(self, spec)
    }

    fn remove(&self, address: &str) -> Result<()> {
        HcClient::remove(self, address)
    }

    fn label(&self, address: &str, labels: &[(&str, &str)]) -> Result<()> {
        HcClient::label(self, address, labels).map(drop)
    }
}

/// What a pause or a resume did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The agents stopped, or brought back.
    pub done: Vec<String>,
    /// The agents it tried and could not, each with why: left running by a
    /// pause, not back after a resume.
    pub failed: Vec<(String, String)>,
    /// The working agents a pause left alone, each with why.
    pub left: Vec<(String, String)>,
}

impl Outcome {
    /// `paused`, `resumed`, `nothing` (none to pause or resume), `partial`
    /// or `failed`.
    pub fn result(&self, done: &'static str) -> &'static str {
        match (self.done.is_empty(), self.failed.is_empty()) {
            (true, true) => "nothing",
            (false, true) => done,
            (false, false) => "partial",
            (true, false) => "failed",
        }
    }

    /// One line on it: what was done, then what was not, then what was
    /// left alone.
    pub fn message(&self, verb: &str, nothing: &str) -> String {
        let mut message = if self.done.is_empty() && self.failed.is_empty() {
            nothing.to_string()
        } else {
            format!("{verb} {}", count(self.done.len(), "agent"))
        };
        if !self.failed.is_empty() {
            message.push_str(&format!(
                ". Not {}: {}",
                verb.to_lowercase(),
                listed(&self.failed).join("; ")
            ));
        }
        if !self.left.is_empty() {
            message.push_str(&format!(
                ". Left running: {}",
                listed(&self.left).join("; ")
            ));
        }
        message
    }

    /// What `corgi pause --json` and `corgi resume --json` print.
    pub fn json(&self, done: &'static str, verb: &str, nothing: &str) -> Value {
        let pairs = |list: &[(String, String)]| -> Vec<Value> {
            list.iter()
                .map(|(name, why)| json!({"name": name, "why": why}))
                .collect()
        };
        let mut value = json!({
            "result": self.result(done),
            "message": self.message(verb, nothing),
            "failed": pairs(&self.failed),
            "left": pairs(&self.left),
        });
        value[done] = json!(self.done);
        value
    }
}

fn listed(list: &[(String, String)]) -> Vec<String> {
    list.iter()
        .map(|(name, why)| format!("{name} {why}"))
        .collect()
}

fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// The message of a pause with no agent working.
pub const NOTHING_WORKING: &str = "No agent is working";
/// The message of a resume with no agent paused.
pub const NOTHING_PAUSED: &str = "No agent is paused";

/// The terminals of `terminals` a pause stops: each live one whose agent
/// is in front and working. Those it cannot bring back again go to `left`,
/// with why, as does `own`, the terminal the pause runs in.
fn plan<'a>(
    terminals: &'a [Terminal],
    own: Option<&str>,
    left: &mut Vec<(String, String)>,
) -> Vec<(&'a Terminal, String)> {
    let mut targets = Vec::new();
    for terminal in terminals {
        if !terminal.is_live() || agent_state(terminal) != AgentState::Working {
            continue;
        }
        let name = terminal.display_name().to_string();
        if own == Some(terminal.address.as_str()) {
            left.push((name, "runs this pause".into()));
            continue;
        }
        match terminal.label(RESUME_LABEL) {
            Some(resume) => targets.push((terminal, resume.to_string())),
            None => left.push((
                name,
                "has no resume line, so Corgi could not bring it back".into(),
            )),
        }
    }
    targets
}

/// Pause on hc's client `client`, keeping the pause in the file at `path`.
/// `own` is the terminal this runs in, never paused.
pub fn pause(
    client: &HcClient,
    path: &Path,
    own: Option<&str>,
    now: u64,
    progress: &mut dyn Progress,
) -> Result<Outcome> {
    pause_with(client, path, own, now, progress)
}

/// [`pause`] through `core`.
pub fn pause_with(
    core: &dyn Core,
    path: &Path,
    own: Option<&str>,
    now: u64,
    progress: &mut dyn Progress,
) -> Result<Outcome> {
    let terminals = core.terminals()?;
    let mut outcome = Outcome::default();
    let targets = plan(&terminals, own, &mut outcome.left);
    if targets.is_empty() {
        return Ok(outcome);
    }
    progress.report(format!("Pausing {}…", count(targets.len(), "agent")));
    // Marked before any key goes out, so the daemon's passes while they stop
    // already hold their wakes and every delivery.
    update(path, |record| {
        for (terminal, resume) in &targets {
            record.agents.retain(|paused| !paused.owns(terminal));
            record.agents.push(PausedAgent::of(terminal, resume, now));
        }
    })?;
    let fail = |outcome: &mut Outcome, terminal: &Terminal, why: String| {
        outcome
            .failed
            .push((terminal.display_name().to_string(), why));
    };
    // Esc ends each one's turn; the exit is typed at its prompt.
    let mut interrupted: Vec<&Terminal> = Vec::new();
    for (terminal, _) in &targets {
        match core.keys(&terminal.address, &["esc"]) {
            Ok(()) => interrupted.push(terminal),
            Err(error) => fail(
                &mut outcome,
                terminal,
                format!("could not be interrupted: {error:#}"),
            ),
        }
    }
    let mut working: Vec<&Terminal> = interrupted.clone();
    let mut now_of: BTreeMap<String, Terminal> = BTreeMap::new();
    for _ in 0..INTERRUPT_POLLS {
        working.retain(|terminal| match core.get(&terminal.address) {
            Ok(Some(seen)) => {
                let busy = seen.is_live() && agent_state(&seen) == AgentState::Working;
                now_of.insert(seen.address.clone(), seen);
                busy
            }
            _ => false,
        });
        if working.is_empty() {
            break;
        }
        core.wait();
    }
    let mut exiting: Vec<&Terminal> = Vec::new();
    for terminal in interrupted {
        if working.contains(&terminal) {
            fail(
                &mut outcome,
                terminal,
                "kept working after Esc; left running".into(),
            );
            continue;
        }
        let seen = now_of.get(&terminal.address).unwrap_or(terminal);
        if gone(seen) {
            fail(&mut outcome, terminal, "ended before it was paused".into());
            continue;
        }
        if core.holds_draft(seen) == Some(true) {
            fail(
                &mut outcome,
                terminal,
                "has unsent text in its input box; left running".into(),
            );
            continue;
        }
        let harness = Harness::from_kind(terminal.label(super::HARNESS_LABEL).unwrap_or(""));
        match core.send(&terminal.address, harness.exit_command()) {
            Ok(()) => exiting.push(terminal),
            Err(error) => fail(
                &mut outcome,
                terminal,
                format!("could not be asked to exit ({error:#}); left running"),
            ),
        }
    }
    let mut running = exiting.clone();
    for _ in 0..EXIT_POLLS {
        running.retain(|terminal| match core.get(&terminal.address) {
            Ok(Some(seen)) => !gone(&seen),
            Ok(None) => false,
            Err(_) => true,
        });
        if running.is_empty() {
            break;
        }
        core.wait();
    }
    let mut stopped: HashSet<String> = HashSet::new();
    for terminal in exiting {
        if running.contains(&terminal) {
            fail(&mut outcome, terminal, "did not exit; left running".into());
        } else {
            outcome.done.push(terminal.display_name().to_string());
            stopped.insert(terminal.address.clone());
        }
    }
    let marked: HashSet<&str> = targets.iter().map(|(t, _)| t.address.as_str()).collect();
    update(path, |record| {
        record.agents.retain_mut(|paused| {
            if !marked.contains(paused.address.as_str()) || paused.phase != Phase::Stopping {
                return true;
            }
            // Still running, or gone some other way: nothing for Resume to
            // bring back, and its wakes are no longer held.
            if !stopped.contains(&paused.address) {
                return false;
            }
            paused.phase = Phase::Paused;
            true
        });
    })?;
    Ok(outcome)
}

/// Whether the agent of `terminal` has stopped: its terminal finished (a
/// worker, whose agent is its program), or a shell is in front of it (a
/// supervisor's kept shell).
fn gone(terminal: &Terminal) -> bool {
    !terminal.is_live() || shell_in_front(terminal)
}

/// Resume on hc's client `client`, from the pause kept at `path`.
pub fn resume(
    client: &HcClient,
    path: &Path,
    now: u64,
    progress: &mut dyn Progress,
) -> Result<Outcome> {
    resume_with(client, path, now, progress)
}

/// [`resume`] through `core`.
pub fn resume_with(
    core: &dyn Core,
    path: &Path,
    now: u64,
    progress: &mut dyn Progress,
) -> Result<Outcome> {
    let paused: Vec<PausedAgent> = read(path)
        .agents
        .into_iter()
        .filter(|paused| paused.phase == Phase::Paused)
        .collect();
    let mut outcome = Outcome::default();
    for (index, agent) in paused.iter().enumerate() {
        progress.report(format!(
            "Resuming {} ({}/{})…",
            agent.name,
            index + 1,
            paused.len()
        ));
        let back = bring_back(core, agent);
        update(path, |record| {
            let Some(position) = record
                .agents
                .iter()
                .position(|marked| marked.address == agent.address && marked.name == agent.name)
            else {
                return;
            };
            match &back {
                Ok(address) => {
                    let marked = &mut record.agents[position];
                    marked.address = address.clone();
                    marked.phase = Phase::Resuming;
                    marked.at = now;
                }
                // Not back: its mark goes, so it holds no one's wakes.
                Err(_) => {
                    record.agents.remove(position);
                }
            }
        })?;
        match back {
            Ok(_) => outcome.done.push(agent.name.clone()),
            Err(error) => outcome
                .failed
                .push((agent.name.clone(), format!("did not come back: {error:#}"))),
        }
    }
    Ok(outcome)
}

/// Starts `agent` again on its conversation and returns the terminal it runs
/// in once its agent is in front.
fn bring_back(core: &dyn Core, agent: &PausedAgent) -> Result<String> {
    let current = core
        .get(&agent.address)?
        .filter(|terminal| agent.owns(terminal));
    let address = match current {
        // Running already: brought back by hand.
        Some(terminal) if terminal.is_live() && !shell_in_front(&terminal) => {
            return Ok(terminal.address);
        }
        // A supervisor's kept shell, or a worker's after a core restart.
        Some(terminal) if terminal.is_live() => {
            core.send(&terminal.address, &agent.resume)
                .context("type its resume line")?;
            // The shell is the program Corgi started there now, so the
            // daemon does not type the line again.
            if let Some(pid) = terminal.pid {
                let _ = core.label(&terminal.address, &[(PID_LABEL, &pid.to_string())]);
            }
            terminal.address
        }
        _ if agent.supervisor => anyhow::bail!(
            "its terminal {} is gone; start it again with corgi supervisor",
            agent.address
        ),
        // A worker's terminal finished with its agent: a new one, with its
        // name and labels, and the old one removed.
        finished => {
            let labels: Vec<(String, String)> = agent
                .labels
                .iter()
                .filter(|(key, _)| key.as_str() != PID_LABEL)
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            let started = core
                .run(&RunSpec {
                    name: agent.name.clone(),
                    cwd: PathBuf::from(&agent.cwd),
                    labels,
                    keep: true,
                    argv: vec![
                        "/bin/sh".into(),
                        "-c".into(),
                        format!("exec {}", agent.resume),
                    ],
                })
                .context("start its terminal again")?;
            if let Some(old) = finished.filter(|old| !old.is_live()) {
                let _ = core.remove(&old.address);
            }
            started.address
        }
    };
    for _ in 0..BACK_POLLS {
        match core.get(&address)? {
            Some(terminal) if !terminal.is_live() => {
                anyhow::bail!("its agent exited at once (terminal {address})")
            }
            Some(terminal) if !shell_in_front(&terminal) && terminal.pid.is_some() => {
                if !agent.supervisor {
                    mark_pid_of(core, &terminal);
                }
                return Ok(address);
            }
            None => anyhow::bail!("its terminal {address} is gone"),
            _ => core.wait(),
        }
    }
    anyhow::bail!("its agent did not start in {address}")
}

/// Labels a worker's new terminal with its program's pid, as a launch does
/// ([`super::launch::mark_pid`]), so the daemon tells a later lost program apart.
fn mark_pid_of(core: &dyn Core, terminal: &Terminal) {
    if let Some(pid) = terminal.pid {
        let _ = core.label(&terminal.address, &[(PID_LABEL, &pid.to_string())]);
    }
}

/// For `corgi status --json`: whether a pause is in force, and its agents.
pub fn status_value(record: &Record) -> Value {
    let agents: Vec<Value> = record
        .agents
        .iter()
        .filter(|paused| paused.is_paused())
        .map(|paused| {
            json!({
                "name": paused.name,
                "project": paused.project,
                "role": if paused.supervisor { "supervisor" } else { "worker" },
                "harness": paused.harness,
                "terminal": paused.address,
                "phase": paused.phase,
                "at": paused.at,
            })
        })
        .collect();
    json!({"any": record.in_force(), "agents": agents})
}

#[cfg(test)]
mod tests;
