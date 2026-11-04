//! Pause all: every Claude and Codex agent Corgi shows, in every project, is
//! stopped to its shell through its harness's own exit (`/exit`, `/quit`),
//! so its conversation is saved and Resume brings it back with its session
//! and launch options; and Resume all brings back every agent paused so.
//!
//! The pause lives in Corgi's launch record (`restart::Paused` on each
//! pane's entry in `markers/<socket>.launches.json`), not in a window, so it
//! outlasts the frontend that set it and a Herdr restart. Every agent is
//! marked before any key goes out, and the pass holding the wake lock
//! honours the marks ([`Holds`]): a marked pane's stops are followed but put
//! in no supervisor's inbox, and a marked supervisor gets no delivery and no
//! handover step while its project keeps its handover record. A mark goes
//! once a pass has seen the agent back after Resume ([`settle`]), so its
//! return wakes no one either, or when its agent kept running after all.
//!
//! An agent that exits leaves its shell, where Herdr resumes nothing after a
//! restart; Corgi also withdraws the resume command it reported for the
//! pane, so a paused agent comes back only through Resume. Nothing is ever
//! killed: an agent that does not stop is left running and said so.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, ensure};

use crate::{
    draft,
    harness::Harness,
    herdr::HerdrClient,
    model::{AgentInfo, AgentState, DashboardAgent},
    progress::Progress,
    protocol::{PauseForm, PauseProject},
    recovery::native,
    restart::{self, Launched, Paused, Record},
    rows::SCRATCH_GROUP,
    supervisor,
    time::unix_now,
};

/// How long a mark may stay on a pane whose agent still runs and was not
/// resumed: longer than a pause takes, so one that never stopped its agent
/// (a frontend that quit halfway) holds that agent's wakes no longer.
pub const STALE_SECS: u64 = 120;

/// How often a pause looks again while it waits, and how long it waits for
/// working agents to stop after Esc and for every agent to exit.
const POLL: Duration = Duration::from_millis(250);
const INTERRUPT_POLLS: usize = 40;
const EXIT_POLLS: usize = 80;

/// One agent Pause all stops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub pane_id: String,
    /// The name the dashboard shows.
    pub name: String,
    /// The name Herdr knows it by, recorded for its resume.
    pub herdr_name: String,
    /// `claude` or `codex`.
    pub kind: String,
    pub cwd: String,
    pub session: String,
    pub supervisor: bool,
    /// Working or waiting on a question: interrupted first, its turn lost.
    pub busy: bool,
    /// Its project's heading, and the project itself.
    pub project: String,
    pub project_root: String,
}

/// What Pause all would stop now, and what it leaves running and why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub targets: Vec<Target>,
    pub left: Vec<(String, &'static str)>,
}

/// The agents of `agents`, a refresh's rows, that Pause all stops: every
/// running Claude and Codex agent with a session, in all projects and the
/// scratch sessions, supervisors and workers alike. An exited row has
/// nothing to stop; one whose state Herdr does not know, or with no
/// session Corgi could resume, is left running and named.
pub fn plan(agents: &[DashboardAgent]) -> Plan {
    let mut plan = Plan::default();
    for agent in agents.iter().filter(|agent| !agent.exited) {
        let info = &agent.info;
        let name = info.display_name().to_string();
        let kind = info.kind();
        if !matches!(kind, "claude" | "codex") {
            plan.left.push((name, "Corgi cannot resume its harness"));
            continue;
        }
        let Some(session) = supervisor::native_session(info).filter(|id| native::valid_id(id))
        else {
            plan.left.push((name, "Corgi has seen no session of it"));
            continue;
        };
        if info.state == AgentState::Unknown {
            plan.left.push((name, "Herdr does not know its state"));
            continue;
        }
        plan.targets.push(Target {
            pane_id: info.pane_id.clone(),
            herdr_name: info.name.clone().unwrap_or_else(|| kind.to_string()),
            name,
            kind: kind.to_string(),
            cwd: info.cwd().to_string(),
            session: session.to_string(),
            supervisor: agent.supervisor,
            busy: is_busy(info.state),
            project: agent.project_group.clone(),
            project_root: agent.project_root.clone(),
        });
    }
    plan
}

fn is_busy(state: AgentState) -> bool {
    matches!(state, AgentState::Working | AgentState::Blocked)
}

impl Plan {
    /// The confirmation's contents, by project in the dashboard's order.
    pub fn form(&self) -> PauseForm {
        let mut projects: Vec<PauseProject> = Vec::new();
        for target in &self.targets {
            let index = match projects.iter().position(|p| p.project == target.project) {
                Some(index) => index,
                None => {
                    projects.push(PauseProject {
                        project: target.project.clone(),
                        ..PauseProject::default()
                    });
                    projects.len() - 1
                }
            };
            let project = &mut projects[index];
            if target.supervisor {
                project.supervisors += 1;
            } else {
                project.workers += 1;
            }
            if target.busy {
                project.working.push(target.name.clone());
            }
        }
        PauseForm {
            agents: self.targets.len(),
            working: self.targets.iter().filter(|t| t.busy).count(),
            projects,
            left: self
                .left
                .iter()
                .map(|(name, why)| format!("{name}: {why}"))
                .collect(),
        }
    }
}

/// The confirmation's lines, as both dashboards show them.
pub fn confirmation(form: &PauseForm) -> Vec<String> {
    let one = form.agents == 1;
    let mut lines = vec![format!(
        "Stops {} to {} shell: {} conversation is saved, and Resume all brings {} back with {} session and options.",
        count(form.agents, "agent"),
        if one { "its" } else { "their" },
        if one { "its" } else { "each one's" },
        if one { "it" } else { "them" },
        if one { "its" } else { "their" },
    )];
    for project in &form.projects {
        let mut parts = Vec::new();
        if project.supervisors > 0 {
            parts.push(if project.supervisors == 1 {
                "its supervisor".to_string()
            } else {
                count(project.supervisors, "supervisor")
            });
        }
        if project.workers > 0 {
            let noun = if project.project == SCRATCH_GROUP {
                "session"
            } else {
                "worker"
            };
            parts.push(count(project.workers, noun));
        }
        let mut line = format!("{}: {}", project.project, parts.join(" and "));
        if !project.working.is_empty() {
            line.push_str(&format!(" · working: {}", project.working.join(", ")));
        }
        lines.push(line);
    }
    if form.working > 0 {
        lines.push(format!(
            "{} working now {} interrupted with Esc first: {} current turn is lost.",
            if form.working == 1 {
                "The agent".to_string()
            } else {
                format!("The {}", count(form.working, "agent"))
            },
            if form.working == 1 { "is" } else { "are" },
            if form.working == 1 {
                "its"
            } else {
                "each one's"
            },
        ));
    }
    if !form.left.is_empty() {
        lines.push(format!("Left running: {}.", form.left.join("; ")));
    }
    lines
}

fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// What Pause all or Resume all did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The agents stopped, or started again.
    pub done: Vec<String>,
    /// The agents it could not, each with why.
    pub failed: Vec<(String, String)>,
}

impl Outcome {
    /// The status line: what was done, then what was not.
    pub fn message(&self, verb: &str) -> String {
        let mut message = format!("{verb} {}", count(self.done.len(), "agent"));
        if !self.failed.is_empty() {
            let failed: Vec<String> = self
                .failed
                .iter()
                .map(|(name, why)| format!("{name} {why}"))
                .collect();
            message.push_str(&format!(
                "; not {}: {}",
                count(self.failed.len(), "agent"),
                failed.join("; ")
            ));
        }
        message
    }
}

/// What a pause asks of Herdr, so its tests need no socket.
trait Panes {
    fn agents(&self) -> Result<Vec<AgentInfo>>;
    fn send_keys(&self, pane_id: &str, keys: &[&str]) -> Result<()>;
    fn prompt(&self, pane_id: &str, text: &str) -> Result<()>;
    /// The pane's visible screen with its styling, if it can be read.
    fn screen(&self, pane_id: &str) -> Option<String>;
    fn release(&self, pane_id: &str, kind: &str);
    fn wait(&self) {
        thread::sleep(POLL);
    }
}

impl Panes for HerdrClient {
    fn agents(&self) -> Result<Vec<AgentInfo>> {
        Ok(self.snapshot()?.agents)
    }

    fn send_keys(&self, pane_id: &str, keys: &[&str]) -> Result<()> {
        self.send_agent_keys(pane_id, keys)
    }

    fn prompt(&self, pane_id: &str, text: &str) -> Result<()> {
        self.prompt_agent(pane_id, text).map(|_| ())
    }

    fn screen(&self, pane_id: &str) -> Option<String> {
        self.read_agent_styled(pane_id).ok().map(|read| read.text)
    }

    fn release(&self, pane_id: &str, kind: &str) {
        let _ = self.release_agent(pane_id, restart::SOURCE, kind);
    }
}

/// Stops every agent of `agents` that [`plan`] picks, reporting each step
/// to `progress`. Refuses when there is none, or no launch record to keep
/// the pause in.
pub fn pause_all(
    client: &HerdrClient,
    agents: &[DashboardAgent],
    progress: &mut dyn Progress,
) -> Result<Outcome> {
    let path =
        restart::record_path(client).context("Corgi keeps no record of its launches here")?;
    let plan = plan(agents);
    ensure!(!plan.targets.is_empty(), "No agent to pause");
    pause_with(client, &path, &plan.targets, unix_now(), progress)
}

fn pause_with(
    herdr: &dyn Panes,
    path: &Path,
    targets: &[Target],
    now: u64,
    progress: &mut dyn Progress,
) -> Result<Outcome> {
    // Marked before any key goes out, so the hooks' passes while they stop
    // already hold their wakes.
    restart::update(path, |record| {
        for target in targets {
            mark(record, target, now);
        }
    })?;
    let mut outcome = Outcome::default();
    let failed = |target: &Target, why: String, outcome: &mut Outcome| {
        outcome.failed.push((target.name.clone(), why));
    };
    let live = herdr.agents()?;
    let state_of = |agents: &[AgentInfo], pane: &str| {
        agents
            .iter()
            .find(|agent| agent.pane_id == pane && restart::is_agent(agent))
            .map(|agent| agent.state)
    };
    let mut stopping: Vec<&Target> = Vec::new();
    let mut interrupted: Vec<&Target> = Vec::new();
    for target in targets {
        match state_of(&live, &target.pane_id) {
            None => failed(target, "is no longer running".into(), &mut outcome),
            Some(state) if is_busy(state) => match herdr.send_keys(&target.pane_id, &["esc"]) {
                Ok(()) => interrupted.push(target),
                Err(error) => failed(
                    target,
                    format!("could not be interrupted: {error:#}"),
                    &mut outcome,
                ),
            },
            Some(_) => stopping.push(target),
        }
    }
    if !interrupted.is_empty() {
        progress.report(format!(
            "Interrupting {} working…",
            count(interrupted.len(), "agent")
        ));
        let mut still = interrupted.clone();
        for _ in 0..INTERRUPT_POLLS {
            if still.is_empty() {
                break;
            }
            herdr.wait();
            if let Ok(agents) = herdr.agents() {
                still.retain(|target| state_of(&agents, &target.pane_id).is_some_and(is_busy));
            }
        }
        for target in interrupted {
            if still.contains(&target) {
                failed(target, "kept working after Esc".into(), &mut outcome);
            } else {
                stopping.push(target);
            }
        }
    }
    progress.report(format!("Stopping {}…", count(stopping.len(), "agent")));
    let mut exiting: Vec<&Target> = Vec::new();
    for target in stopping {
        let harness = Harness::from_kind(&target.kind);
        let drafted = herdr
            .screen(&target.pane_id)
            .and_then(|screen| draft::holds_draft(&harness, &screen))
            .unwrap_or(false);
        if drafted {
            failed(
                target,
                "has unsent text in its input box".into(),
                &mut outcome,
            );
            continue;
        }
        match herdr.prompt(&target.pane_id, harness.exit_command()) {
            Ok(()) => exiting.push(target),
            Err(error) => failed(
                target,
                format!("could not be asked to exit: {error:#}"),
                &mut outcome,
            ),
        }
    }
    let mut running = exiting.clone();
    for _ in 0..EXIT_POLLS {
        if running.is_empty() {
            break;
        }
        herdr.wait();
        if let Ok(agents) = herdr.agents() {
            running.retain(|target| state_of(&agents, &target.pane_id).is_some());
        }
    }
    let mut stopped: HashSet<&str> = HashSet::new();
    for target in exiting {
        if running.contains(&target) {
            failed(target, "did not exit".into(), &mut outcome);
        } else {
            // Herdr forgets the command with the agent; withdrawn all the
            // same, so nothing but Resume brings it back.
            herdr.release(&target.pane_id, &target.kind);
            outcome.done.push(target.name.clone());
            stopped.insert(target.pane_id.as_str());
        }
    }
    restart::update(path, |record| {
        for target in targets {
            let Some(entry) = record.panes.get_mut(&target.pane_id) else {
                continue;
            };
            if stopped.contains(target.pane_id.as_str()) {
                entry.reported = None;
            } else {
                // Still running, or gone: nothing for Resume all to bring
                // back, and its wakes are no longer held.
                entry.paused = None;
            }
        }
    })?;
    Ok(outcome)
}

/// Marks `target`'s pane paused, taking in an agent Corgi did not launch
/// (it comes back with its session alone, as Herdr would resume it).
fn mark(record: &mut Record, target: &Target, now: u64) {
    let entry = record
        .panes
        .entry(target.pane_id.clone())
        .or_insert_with(|| fresh(target));
    if entry.kind != target.kind {
        *entry = fresh(target);
    }
    entry.session = Some(target.session.clone());
    entry.paused = Some(Paused {
        at: now,
        project_root: target.project_root.clone(),
        supervisor: target.supervisor,
        resuming: false,
    });
}

fn fresh(target: &Target) -> Launched {
    Launched {
        kind: target.kind.clone(),
        name: target.herdr_name.clone(),
        cwd: PathBuf::from(&target.cwd),
        ..Launched::default()
    }
}

/// The panes of `snapshot` paused and back at their shell, which Resume
/// all starts again.
pub fn paused<'a>(
    record: &'a Record,
    snapshot: &crate::herdr::SessionSnapshot,
) -> Vec<(&'a str, &'a Launched)> {
    restart::resumable(record, snapshot)
        .into_iter()
        .filter(|(_, entry)| entry.paused.is_some())
        .collect()
}

/// Starts every paused agent again in its own pane with its session and
/// options (`restart::resume`), and lets go of the marks of any whose agent
/// runs already. Agents that exited some other way are not touched.
pub fn resume_all(client: &HerdrClient, progress: &mut dyn Progress) -> Result<Outcome> {
    let path =
        restart::record_path(client).context("Corgi keeps no record of its launches here")?;
    let record = restart::read_at(&path);
    let snapshot = client.snapshot()?;
    let waiting: Vec<(String, String)> = paused(&record, &snapshot)
        .into_iter()
        .map(|(pane, entry)| (pane.to_string(), entry.name.clone()))
        .collect();
    let running: HashSet<&str> = snapshot
        .agents
        .iter()
        .filter(|agent| restart::is_agent(agent))
        .map(|agent| agent.pane_id.as_str())
        .collect();
    let back: Vec<String> = record
        .panes
        .iter()
        .filter(|(pane, entry)| entry.paused.is_some() && running.contains(pane.as_str()))
        .map(|(pane, _)| pane.clone())
        .collect();
    ensure!(
        !waiting.is_empty() || !back.is_empty(),
        "No agent is paused"
    );
    let mut outcome = Outcome::default();
    for (index, (pane, name)) in waiting.iter().enumerate() {
        progress.report(format!(
            "Resuming {name} ({}/{})…",
            index + 1,
            waiting.len()
        ));
        match restart::resume(client, pane) {
            Ok(_) => outcome.done.push(name.clone()),
            Err(error) => outcome.failed.push((name.clone(), format!("{error:#}"))),
        }
    }
    if !back.is_empty() {
        restart::update(&path, |record| {
            for pane in &back {
                if let Some(paused) = record.panes.get_mut(pane).and_then(|e| e.paused.as_mut()) {
                    paused.resuming = true;
                }
            }
        })?;
    }
    Ok(outcome)
}

/// What the pass holding the wake lock holds back for paused agents: the
/// panes whose stops wake no one and whose supervisor gets nothing typed
/// in, and the projects whose supervisor's handover record is kept while
/// it is away.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Holds {
    pub panes: HashSet<String>,
    pub roots: HashSet<String>,
}

impl Holds {
    pub fn of(record: &Record) -> Self {
        let mut holds = Self::default();
        for (pane, entry) in &record.panes {
            let Some(paused) = &entry.paused else {
                continue;
            };
            holds.panes.insert(pane.clone());
            if paused.supervisor {
                holds.roots.insert(paused.project_root.clone());
            }
        }
        holds
    }

    pub fn read(client: &HerdrClient) -> Self {
        Self::of(&restart::read(client))
    }

    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }
}

/// Drops the marks of panes where `running` (the agents a pass just
/// followed, with their stops held) shows the agent back after Resume, or
/// still running long after its pause. Called after the pass has recorded
/// what it saw, so the next one takes the agent up from there and wakes
/// no one for its return.
pub fn settle(client: &HerdrClient, running: &[DashboardAgent], now: u64) {
    let Some(path) = restart::record_path(client) else {
        return;
    };
    let live: HashSet<&str> = running
        .iter()
        .filter(|agent| !agent.exited)
        .map(|agent| agent.info.pane_id.as_str())
        .collect();
    if !settle_record(&mut restart::read_at(&path), &live, now) {
        return;
    }
    let _ = restart::update(&path, |record| settle_record(record, &live, now));
}

fn settle_record(record: &mut Record, live: &HashSet<&str>, now: u64) -> bool {
    let mut changed = false;
    for (pane, entry) in &mut record.panes {
        let back = entry.paused.as_ref().is_some_and(|paused| {
            live.contains(pane.as_str())
                && (paused.resuming || now.saturating_sub(paused.at) > STALE_SECS)
        });
        if back {
            entry.paused = None;
            changed = true;
        }
    }
    changed
}

/// How many agents are paused and back at their shell, by pane, from a
/// refresh's rows.
pub fn paused_rows(agents: &[DashboardAgent]) -> usize {
    agents.iter().filter(|agent| agent.paused).count()
}

#[cfg(test)]
mod tests;
