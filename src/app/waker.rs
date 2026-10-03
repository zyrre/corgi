//! Waking each project's corgi about its agents, and handing a corgi
//! whose context is filling up, or whose prompt cache is about to go cold,
//! over to a fresh session.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};

use crate::{
    corgi::{self, CORGI_TOKEN},
    harness::Harness,
    herdr::HerdrClient,
    job::Job,
    model::{AgentState, DashboardAgent},
    paths::socket_state_file,
    time::unix_now,
};

use super::{
    App, draft,
    form::Checkout,
    launch::{LaunchPlan, Role, launch_agent},
    markers,
    progress::Silent,
};

/// How long a corgi that handed over has to exit, polled this often.
const CORGI_EXIT_POLLS: usize = 60;
const CORGI_EXIT_POLL_DELAY: Duration = Duration::from_millis(250);

/// Wakes each project's corgi when another agent of that project stops:
/// every agent whose project has a running corgi counts, whoever started
/// it. Every dashboard observes, so one that takes over waking has a baseline
/// and never mistakes an old state for news; only the dashboard holding the
/// lock for its Herdr socket sends, so a corgi hears each stop once. The
/// same dashboard hands corgis over to fresh sessions.
#[derive(Debug, Default)]
pub(super) struct CorgiWaker {
    /// The wake lock, while this dashboard holds it.
    lock: Option<fs::File>,
    /// The Corgi binary the wake messages name.
    corgi_bin: Option<PathBuf>,
    /// Each agent's transitions, by the name its messages use.
    agents: HashMap<String, corgi::Transitions>,
    /// Messages not sent yet, by project root, keeping each agent's newest.
    /// They are for whichever corgi the project has when they go out, so
    /// the ones a handing-over corgi did not get reach its successor.
    pending: HashMap<String, BTreeMap<String, String>>,
    /// Each project's corgi handover, by project root.
    handovers: HashMap<String, corgi::Handover>,
    /// Replacements running in the background, by project root, each ending
    /// with its status line or error.
    replacements: Vec<(String, Job<Result<String>>)>,
}

/// What a handover at one refresh asks the dashboard to do for one corgi.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HandoverAction {
    /// `None` when there is only a new baseline to keep on the pane.
    step: Option<corgi::HandoverStep>,
    /// The session's baseline, once recorded.
    baseline: Option<u64>,
    /// The session's handover request token, kept alongside the baseline.
    token: Option<String>,
    root: String,
    name: String,
    pane_id: String,
    session: String,
    harness: Harness,
    change: u64,
}

impl CorgiWaker {
    /// Whether this dashboard wakes corgis, taking the lock at `path` if
    /// no other dashboard holds it. The lock goes with the process.
    pub(super) fn lead(&mut self, path: &Path) -> bool {
        if self.lock.is_none() {
            self.lock = fs::create_dir_all(path.parent().unwrap_or(path))
                .ok()
                .and_then(|()| fs::File::create(path).ok())
                .filter(|file| file.try_lock().is_ok());
        }
        self.lock.is_some()
    }

    /// Whether this dashboard wakes corgis of `client`'s Herdr session,
    /// taking the lock if no other dashboard holds it.
    pub(super) fn leads(&mut self, client: &HerdrClient) -> bool {
        wake_lock_path(client.socket_path()).is_some_and(|lock| self.lead(&lock))
    }

    /// Whether `root`'s corgi is handing over, so wakes for it wait for the
    /// corgi that takes over.
    fn handing_over(&self, root: &str) -> bool {
        self.handovers
            .get(root)
            .is_some_and(corgi::Handover::in_progress)
    }

    /// Records the agents' states from one refresh, queueing a message for
    /// each one that stopped.
    fn observe(&mut self, agents: &[DashboardAgent], corgi_bin: &Path) {
        let corgis: HashSet<&str> = agents
            .iter()
            .filter(|agent| agent.corgi)
            .map(|agent| agent.project_root.as_str())
            .collect();
        let mut seen = HashSet::new();
        // A scratch agent belongs to no project, so no corgi hears of it.
        for agent in agents.iter().filter(|agent| !agent.corgi && !agent.scratch) {
            let name = agent.info.name.as_deref().unwrap_or(&agent.info.pane_id);
            seen.insert(name.to_string());
            let woke = self
                .agents
                .entry(name.to_string())
                .or_default()
                .observe(agent.info.state, agent.info.state_change_seq);
            let root = agent.project_root.as_str();
            if let Some(state) = woke
                && (corgis.contains(root) || self.handing_over(root))
            {
                self.pending.entry(root.to_string()).or_default().insert(
                    name.to_string(),
                    corgi::wake_message(corgi_bin, name, state),
                );
            }
        }
        self.agents.retain(|name, _| seen.contains(name));
    }

    /// Queues `message` for `root`'s corgi under `key`, to go out with the
    /// wakes at a refresh once the corgi is between turns. Only the
    /// dashboard that wakes corgis delivers; any other drops its queue.
    pub(super) fn queue(&mut self, root: &str, key: String, message: String) {
        self.pending
            .entry(root.to_string())
            .or_default()
            .insert(key, message);
    }

    /// The messages waiting for `root`'s corgi.
    #[cfg(test)]
    pub(super) fn pending_for(&self, root: &str) -> Vec<String> {
        self.pending
            .get(root)
            .map(|messages| messages.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Advances each corgi's handover by one refresh, with `percent` as
    /// the threshold and `idle_secs` giving each harness's idle time, and
    /// returns what to do about it. `note_since(root, at)` says whether
    /// `root`'s corgi wrote its note since `at`, and `drafting(corgi)`
    /// whether the user is writing in its input box, which holds a due
    /// request, or the replacement, back for a later refresh.
    fn hand_over(
        &mut self,
        agents: &[DashboardAgent],
        percent: u8,
        idle_secs: impl Fn(&Harness) -> Option<u64>,
        now: u64,
        mut note_since: impl FnMut(&str, u64) -> bool,
        mut drafting: impl FnMut(&DashboardAgent) -> bool,
    ) -> Vec<HandoverAction> {
        let busy: HashSet<&str> = agents
            .iter()
            .filter(|agent| {
                !agent.corgi
                    && matches!(agent.info.state, AgentState::Working | AgentState::Blocked)
            })
            .map(|agent| agent.project_root.as_str())
            .collect();
        let mut actions = Vec::new();
        // The home directory is never a project, so nothing there is
        // handed over as its corgi.
        for agent in agents.iter().filter(|agent| agent.corgi && !agent.scratch) {
            let (Some(name), Some(session)) = (
                agent.info.name.as_deref(),
                corgi::native_session(&agent.info),
            ) else {
                continue;
            };
            let root = agent.project_root.as_str();
            let token = |name| agent.info.tokens.get(name).map(String::as_str);
            let harness = agent.info.harness();
            let sighting = corgi::Sighting {
                session,
                state: agent.info.state,
                change: agent.info.state_change_seq,
                context_percent: agent.context_percent,
                context_tokens: agent.context_tokens,
                workers_busy: busy.contains(root),
                token: token(corgi::HANDOVER_TOKEN),
                baseline_token: token(corgi::BASELINE_TOKEN),
            };
            let limits = corgi::Thresholds {
                percent,
                idle_secs: idle_secs(&harness),
            };
            let handover = self.handovers.entry(root.to_string()).or_default();
            let step = handover.observe(
                sighting,
                limits,
                now,
                |at| note_since(root, at),
                || !drafting(agent),
            );
            let baseline = handover.baseline();
            let unsaved = baseline.is_some_and(|baseline| {
                sighting.baseline_token != Some(format!("{baseline} {session}").as_str())
            });
            if step.is_some() || unsaved {
                actions.push(HandoverAction {
                    step,
                    baseline,
                    token: sighting.token.map(str::to_string),
                    root: root.to_string(),
                    name: name.to_string(),
                    pane_id: agent.info.pane_id.clone(),
                    session: session.to_string(),
                    harness,
                    change: agent.info.state_change_seq,
                });
            }
        }
        // A project whose corgi is gone keeps its record only while the
        // replacement that removed it runs.
        let live: HashSet<&str> = agents
            .iter()
            .filter(|agent| agent.corgi)
            .map(|agent| agent.project_root.as_str())
            .collect();
        self.handovers
            .retain(|root, handover| live.contains(root.as_str()) || handover.in_progress());
        actions
    }

    /// Records the end of `root`'s replacement.
    fn replaced(&mut self, root: &str, succeeded: bool, agents: &[DashboardAgent]) {
        let change = agents
            .iter()
            .find(|agent| agent.corgi && agent.project_root == root)
            .map_or(0, |agent| agent.info.state_change_seq);
        if let Some(handover) = self.handovers.get_mut(root) {
            handover.replaced(succeeded, change);
        }
    }

    /// Sends each project's corgi its queued messages, one line each in
    /// one prompt, once it is between turns. A corgi that is working or
    /// blocked keeps them until a later refresh, so they arrive after its
    /// turn, and so does one whose input box holds the user's draft, as
    /// `drafting` says, and one handing over, for its successor; a project
    /// without a corgi drops them.
    fn deliver(
        &mut self,
        agents: &[DashboardAgent],
        mut drafting: impl FnMut(&DashboardAgent) -> bool,
        mut prompt: impl FnMut(&str, &str) -> bool,
    ) {
        let handovers = &self.handovers;
        self.pending.retain(|root, messages| {
            if handovers
                .get(root)
                .is_some_and(corgi::Handover::in_progress)
            {
                return true;
            }
            let Some((agent, corgi)) = agents.iter().find_map(|agent| {
                let name = agent.info.name.as_deref()?;
                (agent.corgi && agent.project_root == *root).then_some((agent, name))
            }) else {
                return false;
            };
            if matches!(agent.info.state, AgentState::Working | AgentState::Blocked)
                || drafting(agent)
            {
                return true;
            }
            let text = messages.values().cloned().collect::<Vec<_>>().join("\n");
            !prompt(corgi, &text)
        });
    }

    /// Takes one refresh's agents: tells each project's corgi about its
    /// agents that stopped since the last one, and hands corgis whose
    /// handover is due over to fresh sessions, when this dashboard is
    /// the one that wakes corgis. Returns the status line to show, if any.
    fn tick(&mut self, client: &HerdrClient, agents: &[DashboardAgent]) -> Option<String> {
        let leads = self.leads(client);
        let corgi_bin = self.corgi_bin.get_or_insert_with(|| {
            let installed = client.plugin_root(corgi::PLUGIN_ID).ok().flatten();
            corgi::corgi_bin(installed.as_deref()).unwrap_or_else(|_| PathBuf::from("corgi"))
        });
        let corgi_bin = corgi_bin.clone();
        self.observe(agents, &corgi_bin);
        if !leads {
            self.pending.clear();
            return None;
        }
        let mut status = None;
        let mut finished = Vec::new();
        self.replacements.retain_mut(|(root, replacement)| {
            let Some(outcome) = replacement.outcome() else {
                return true;
            };
            // A panicking replacement still reports, or the handover would
            // stay in progress for good.
            let result =
                outcome.unwrap_or_else(|_| Err(anyhow::anyhow!("the replacement panicked")));
            finished.push((std::mem::take(root), result));
            false
        });
        for (root, result) in finished {
            self.replaced(&root, result.is_ok(), agents);
            status = Some(
                result.unwrap_or_else(|error| format!("The corgi's handover failed: {error:#}")),
            );
        }
        let percent = corgi::handover_percent();
        let actions = self.hand_over(
            agents,
            percent,
            corgi::handover_idle_secs,
            unix_now(),
            |root, at| corgi::state_dir(root).is_ok_and(|dir| corgi::note_written_since(&dir, at)),
            |corgi| holds_draft(client, corgi),
        );
        for action in actions {
            let name = &action.name;
            let Some(step) = action.step else {
                report_tokens(client, &action, action.token.as_deref());
                continue;
            };
            match step {
                corgi::HandoverStep::Ask(at, trigger) => {
                    let asked = corgi::state_dir(&action.root).and_then(|dir| {
                        client.prompt_agent(name, &corgi::handover_request(&dir, trigger))
                    });
                    status = Some(match asked {
                        Ok(_) => {
                            let asked = format!("{at} {}", action.session);
                            report_tokens(client, &action, Some(&asked));
                            match trigger {
                                corgi::Trigger::Full(percent) => format!(
                                    "{name}'s context is past {percent}%: asked for its handover note"
                                ),
                                corgi::Trigger::Idle(minutes) => format!(
                                    "{name} has been idle {minutes} min, its prompt cache about to expire: asked for its handover note"
                                ),
                                corgi::Trigger::Update => format!(
                                    "{name} worked after writing its handover note: asked to bring it up to date"
                                ),
                            }
                        }
                        Err(error) => {
                            if let Some(handover) = self.handovers.get_mut(&action.root) {
                                handover.unsent(action.change);
                            }
                            format!("Could not ask {name} for its handover note: {error:#}")
                        }
                    });
                }
                corgi::HandoverStep::Replace => {
                    status = Some(format!("Replacing {name} with a fresh session…"));
                    let client = client.clone();
                    let root = action.root.clone();
                    let replacement = Job::spawn(move |_| replace_corgi(&client, &action));
                    self.replacements.push((root, replacement));
                }
                corgi::HandoverStep::NoNote => {
                    status = Some(format!(
                        "{name} wrote no handover note: it keeps its session, and is asked again after a later turn"
                    ));
                }
            }
        }
        self.deliver(
            agents,
            |corgi| holds_draft(client, corgi),
            |corgi, text| client.prompt_agent(corgi, text).is_ok(),
        );
        status
    }
}

/// Whether the user is writing in `corgi`'s input box, so that nothing is
/// typed into their draft. A screen that cannot be read, or a harness whose
/// box Corgi does not know, counts as no draft: the text goes out as it did
/// before this check, rather than waiting for good.
fn holds_draft(client: &HerdrClient, corgi: &DashboardAgent) -> bool {
    let screen = client
        .read_agent_styled(&corgi.info.pane_id)
        .map(|read| read.text);
    screen_holds_draft(screen, &corgi.info.harness())
}

/// Whether `screen`, as read from a `harness` corgi's pane, shows a draft.
fn screen_holds_draft(screen: Result<String>, harness: &Harness) -> bool {
    screen
        .ok()
        .and_then(|screen| draft::holds_draft(harness, &screen))
        .unwrap_or(false)
}

/// Keeps a corgi's handover tokens on its pane: its `handover` request,
/// if it has one, and its baseline. The marker goes with them, so the corgi
/// keeps it whether Herdr merges tokens or replaces them, and all of them go
/// into Corgi's record of its marks.
fn report_tokens(client: &HerdrClient, action: &HandoverAction, handover: Option<&str>) {
    let baseline = action
        .baseline
        .map(|baseline| format!("{baseline} {}", action.session));
    let tokens: Vec<(&str, &str)> = [
        Some((CORGI_TOKEN, action.name.as_str())),
        handover.map(|handover| (corgi::HANDOVER_TOKEN, handover)),
        baseline
            .as_deref()
            .map(|baseline| (corgi::BASELINE_TOKEN, baseline)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let _ = markers::mark_pane(
        client,
        &action.pane_id,
        &action.name,
        Some(&action.session),
        &tokens,
    );
}

/// Gives up the wake lock at once. Closing the file alone may not: a child
/// process spawned on another thread at that moment briefly holds a copy of
/// the descriptor, and with it the lock, so the next dashboard would find
/// it still taken.
impl Drop for CorgiWaker {
    fn drop(&mut self) {
        if let Some(lock) = &self.lock {
            let _ = lock.unlock();
        }
    }
}

/// The launch of the corgi that takes over from the one in `action`,
/// started the way `launch` records.
fn successor_plan(action: &HandoverAction, launch: corgi::Launch) -> LaunchPlan {
    LaunchPlan {
        name: action.name.clone(),
        harness: Harness::from_kind(&launch.kind),
        model: launch.model,
        effort: launch.effort,
        prompt: String::new(),
        project: PathBuf::from(&action.root),
        new_project: false,
        checkout: Checkout::ProjectRoot,
        extra_args: launch.extra_args,
        role: Role::Corgi {
            handover_pane: Some(action.pane_id.clone()),
        },
        request_id: None,
    }
}

/// Replaces the corgi that handed over in `action` with a fresh session
/// in the same pane: the old one exits through its harness's own command,
/// which leaves its transcript on disk and its pane at the shell, and the new
/// one starts there under the same name, marked as the corgi, the way it
/// was launched, and with a first prompt that takes up the note.
fn replace_corgi(client: &HerdrClient, action: &HandoverAction) -> Result<String> {
    let HandoverAction {
        root,
        name,
        pane_id,
        harness,
        ..
    } = action;
    client
        .prompt_agent(name, harness.exit_command())
        .with_context(|| format!("ask {name} to exit"))?;
    let exited = (0..CORGI_EXIT_POLLS).any(|_| {
        thread::sleep(CORGI_EXIT_POLL_DELAY);
        client.snapshot().is_ok_and(|snapshot| {
            !snapshot
                .agents
                .iter()
                .any(|agent| agent.pane_id == *pane_id && agent.name.as_deref() == Some(name))
        })
    });
    anyhow::ensure!(
        exited,
        "{name} did not exit; it keeps running, with its handover note in place"
    );
    let plan = successor_plan(action, corgi::launch_of(&corgi::state_dir(root)?, harness));
    launch_agent(client, &plan, &mut Silent)
        .with_context(|| format!("{name} exited, but its successor did not start"))?;
    Ok(format!("{name} handed over to a fresh session"))
}

/// The lock that makes one dashboard per Herdr socket the one that wakes
/// corgis, named after the socket so that dashboards of other Herdr
/// sessions wake their own.
fn wake_lock_path(socket: &Path) -> Option<PathBuf> {
    socket_state_file("wake", socket, "lock")
}

impl App {
    /// Tells each project's corgi about its agents that stopped since the
    /// last refresh, and hands corgis whose handover is due over to
    /// fresh sessions, when this dashboard is the one that wakes corgis.
    pub(super) fn wake_corgis(&mut self) {
        let Some(waker) = self.corgi_waker.as_mut() else {
            return;
        };
        if let Some(status) = waker.tick(&self.client, &self.agents) {
            self.set_status(status, Some(Duration::from_secs(15)));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use crate::{
        app::launch::launch_args,
        model::{AgentInfo, AgentState, DashboardAgent},
    };

    use super::*;

    fn project_agent(name: &str, root: &str, corgi: bool, state: AgentState) -> DashboardAgent {
        DashboardAgent {
            info: AgentInfo {
                pane_id: format!("{name}:p1"),
                name: Some(name.into()),
                state,
                ..AgentInfo::default()
            },
            project_root: root.into(),
            corgi,
            ..DashboardAgent::default()
        }
    }

    #[test]
    fn the_dashboard_wakes_a_corgi_once_per_stop_of_its_projects_agents() {
        use AgentState::{Done, Idle, Working};
        let corgi_bin = Path::new("/opt/corgi");
        let mut waker = CorgiWaker::default();
        let sent = std::cell::RefCell::new(Vec::new());
        let refresh = |waker: &mut CorgiWaker, agents: &[DashboardAgent]| {
            waker.observe(agents, corgi_bin);
            waker.deliver(
                agents,
                |_| false,
                |corgi, text| {
                    sent.borrow_mut().push(format!("{corgi}: {text}"));
                    true
                },
            );
        };
        let agents = |corgi: AgentState, worker: AgentState, other: AgentState| {
            vec![
                project_agent("corgi-weather", "/repos/weather", true, corgi),
                project_agent("w-forecast", "/repos/weather", false, worker),
                project_agent("w-elsewhere", "/repos/corgi", false, other),
            ]
        };
        // The first refresh is a baseline: nothing is news yet.
        refresh(&mut waker, &agents(Idle, Done, Done));
        refresh(&mut waker, &agents(Working, Working, Working));
        // The worker stops while its corgi is mid-turn: the message waits.
        refresh(&mut waker, &agents(Working, Done, Done));
        refresh(&mut waker, &agents(Working, Done, Done));
        refresh(&mut waker, &agents(Done, Done, Done));
        refresh(&mut waker, &agents(Idle, Idle, Idle));
        assert_eq!(
            *sent.borrow(),
            ["corgi-weather: [corgi] w-forecast is done. Run: /opt/corgi report w-forecast"]
        );
        // The corgi's own turns, and another project's agents, wake nobody.
        refresh(&mut waker, &agents(Working, Idle, Working));
        refresh(&mut waker, &agents(Done, Idle, Done));
        assert_eq!(sent.borrow().len(), 1);
    }

    #[test]
    fn a_renamed_corgi_receives_wakes_and_keeps_its_handover_session() {
        use AgentState::{Done, Idle, Working};
        let root = "/repos/weather";
        let mut owner = project_agent("corgi-weather", root, false, Idle);
        owner.info.agent_session = Some(crate::model::AgentSession {
            value: "s1".into(),
            ..Default::default()
        });
        owner
            .info
            .tokens
            .insert(CORGI_TOKEN.into(), corgi::marker("", Some("s1")));
        owner.corgi = corgi::is_corgi(&owner.info);
        let mut waker = CorgiWaker::default();
        waker.observe(
            &[owner.clone(), project_agent("worker", root, false, Working)],
            Path::new("/opt/corgi"),
        );
        owner.info.name = Some("renamed-task".into());
        owner.corgi = corgi::is_corgi(&owner.info);
        let agents = [owner.clone(), project_agent("worker", root, false, Done)];
        waker.observe(&agents, Path::new("/opt/corgi"));
        let mut recipients = Vec::new();
        waker.deliver(
            &agents,
            |_| false,
            |name, _| {
                recipients.push(name.to_string());
                true
            },
        );
        assert_eq!(recipients, ["renamed-task"]);

        owner.context_percent = Some(80);
        let actions = waker.hand_over(&[owner], 50, |_| None, 100, |_, _| false, |_| false);
        assert_eq!(actions.len(), 1);
        let action = &actions[0];
        assert_eq!(action.name, "renamed-task");
        assert_eq!(action.session, "s1");
        assert_eq!(
            action.step,
            Some(corgi::HandoverStep::Ask(100, corgi::Trigger::Full(50)))
        );
        let (socket, server) = crate::test_support::fake_herdr("renamed-handover", |listener| {
            crate::test_support::answer(&listener, |request| {
                assert_eq!(request["method"], "pane.report_metadata");
                assert_eq!(request["params"]["tokens"][CORGI_TOKEN], "session:s1");
                assert_eq!(request["params"]["tokens"][corgi::HANDOVER_TOKEN], "100 s1");
                serde_json::json!({"result": {"type": "ok"}})
            });
        });
        report_tokens(
            &HerdrClient::from_socket_path(&socket),
            action,
            Some("100 s1"),
        );
        server.join().unwrap();
        fs::remove_file(socket).unwrap();
    }

    #[test]
    fn a_scratch_agent_wakes_no_corgi_and_none_is_handed_over_in_home() {
        use AgentState::{Done, Working};
        let scratch = |name: &str, corgi: bool, state: AgentState| DashboardAgent {
            scratch: true,
            info: AgentInfo {
                agent_session: Some(crate::model::AgentSession {
                    value: format!("{name}-session"),
                    ..Default::default()
                }),
                ..project_agent(name, "/home/me", corgi, state).info
            },
            context_percent: Some(99),
            ..project_agent(name, "/home/me", corgi, state)
        };
        let mut waker = CorgiWaker::default();
        // Even beside a corgi someone started in home before it was
        // refused there, a scratch agent that stops is nobody's news.
        let agents = |state| {
            [
                scratch("corgi-me", true, Done),
                scratch("scratch", false, state),
            ]
        };
        waker.observe(&agents(Done), Path::new("/opt/corgi"));
        waker.observe(&agents(Working), Path::new("/opt/corgi"));
        waker.observe(&agents(Done), Path::new("/opt/corgi"));
        assert!(waker.pending.is_empty());
        let actions = waker.hand_over(&agents(Done), 1, |_| Some(0), 1_000, |_, _| true, |_| false);
        assert!(actions.is_empty());
        assert!(waker.handovers.is_empty());
    }

    #[test]
    fn a_wake_waits_while_the_user_writes_in_the_corgis_input_box() {
        use AgentState::{Done, Working};
        let screen = |name: &str| {
            fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("src/app/fixtures/input_box")
                    .join(name),
            )
            .map_err(anyhow::Error::from)
        };
        for (harness, draft, empty) in [
            ("claude", "claude_draft.ansi", "claude_suggestion.ansi"),
            ("claude", "claude_pasted_text.ansi", "claude_empty.ansi"),
            (
                "codex",
                "codex_multiline_draft.ansi",
                "codex_placeholder.ansi",
            ),
        ] {
            let corgi_bin = Path::new("/opt/corgi");
            let mut waker = CorgiWaker::default();
            let mut sent = Vec::new();
            let mut refresh = |worker, shown: Result<String>| {
                let mut corgi = project_agent("corgi-weather", "/repos/weather", true, Done);
                corgi.info.agent = Some(harness.into());
                let agents = [
                    corgi,
                    project_agent("w-forecast", "/repos/weather", false, worker),
                ];
                waker.observe(&agents, corgi_bin);
                let mut shown = Some(shown);
                waker.deliver(
                    &agents,
                    |corgi| {
                        let screen = shown.take().expect("one read per refresh");
                        screen_holds_draft(screen, &corgi.info.harness())
                    },
                    |_, text| {
                        sent.push(text.to_string());
                        true
                    },
                );
                sent.len()
            };
            assert_eq!(refresh(Working, screen(draft)), 0);
            // The worker stops while the user writes: the wake waits.
            assert_eq!(refresh(Done, screen(draft)), 0, "{draft}");
            assert_eq!(refresh(Done, screen(draft)), 0, "{draft}");
            // The box is empty, or shows only what the harness suggests.
            assert_eq!(refresh(Done, screen(empty)), 1, "{empty}");
            assert_eq!(refresh(Done, screen(empty)), 1, "sent once");
            assert_eq!(
                sent,
                ["[corgi] w-forecast is done. Run: /opt/corgi report w-forecast"]
            );
        }
    }

    #[test]
    fn a_screen_that_cannot_be_read_or_understood_holds_nothing_back() {
        let unreadable = Err(anyhow::anyhow!("agent_not_found"));
        assert!(!screen_holds_draft(unreadable, &Harness::Claude));
        let draft = "────\n❯ fix it\n────".to_string();
        assert!(screen_holds_draft(Ok(draft.clone()), &Harness::Claude));
        assert!(!screen_holds_draft(Ok(draft), &Harness::Gemini));
        assert!(!screen_holds_draft(Ok("$ ".into()), &Harness::Codex));
    }

    #[test]
    fn a_handover_request_waits_while_the_user_writes_in_the_corgis_input_box() {
        use crate::corgi::{HandoverStep::Ask, Trigger::Full};
        let mut corgi = project_agent("corgi-weather", "/repos/weather", true, AgentState::Idle);
        corgi.info.agent_session = Some(crate::model::AgentSession {
            value: "s1".into(),
            ..Default::default()
        });
        corgi.context_percent = Some(70);
        let mut waker = CorgiWaker::default();
        let mut steps = |now, drafting: bool| {
            waker
                .hand_over(
                    std::slice::from_ref(&corgi),
                    50,
                    |_| None,
                    now,
                    |_, _| false,
                    |_| drafting,
                )
                .into_iter()
                .filter_map(|action| action.step)
                .collect::<Vec<_>>()
        };
        assert_eq!(steps(1, true), []);
        assert_eq!(steps(2, true), []);
        assert_eq!(steps(3, false), [Ask(3, Full(50))]);
    }

    #[test]
    fn the_replacement_waits_while_the_user_writes_in_the_corgis_input_box() {
        use crate::corgi::{HandoverStep::Replace, Trigger::Full};
        use AgentState::{Done, Idle, Working};
        let screen = |name: &str| {
            fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("src/app/fixtures/input_box")
                    .join(name),
            )
            .map_err(anyhow::Error::from)
        };
        let corgi = |state, change| {
            let mut agent = project_agent("corgi-weather", "/repos/weather", true, state);
            agent.info.agent = Some("claude".into());
            agent.info.state_change_seq = change;
            agent.info.agent_session = Some(crate::model::AgentSession {
                value: "s1".into(),
                ..Default::default()
            });
            agent.context_percent = Some(70);
            agent
        };
        // The screen each refresh shows; `None` means it is never read.
        let run = |screens: Vec<(AgentState, u64, u64, Option<Result<String>>)>| {
            let mut waker = CorgiWaker::default();
            let mut steps = Vec::new();
            for (state, change, now, mut shown) in screens {
                let agents = [corgi(state, change)];
                steps.push(
                    waker
                        .hand_over(
                            &agents,
                            50,
                            |_| None,
                            now,
                            |_, _| true,
                            |corgi| {
                                let screen = shown.take().expect("read only when due");
                                screen_holds_draft(screen, &corgi.info.harness())
                            },
                        )
                        .into_iter()
                        .find_map(|action| action.step),
                );
            }
            steps
        };
        let empty = || Some(screen("claude_empty.ansi"));
        let draft = || Some(screen("claude_draft.ansi"));
        let late = 1_000;
        assert_eq!(
            run(vec![
                (Idle, 1, 10, empty()),
                (Working, 2, 11, None),
                // The note is there, but so is the user's draft, long past
                // the two minutes a request has to be taken up.
                (Done, 3, 12, draft()),
                (Done, 3, late, draft()),
                (Done, 3, late + 1, Some(screen("claude_suggestion.ansi"))),
            ]),
            [
                Some(crate::corgi::HandoverStep::Ask(10, Full(50))),
                None,
                None,
                None,
                Some(Replace)
            ]
        );
        // A screen that cannot be read does not hold the replacement back.
        let unreadable = || Some(Err(anyhow::anyhow!("agent_not_found")));
        assert_eq!(
            run(vec![
                (Idle, 1, 10, empty()),
                (Working, 2, 11, None),
                (Done, 3, 12, unreadable()),
            ])[2],
            Some(Replace)
        );
    }

    #[test]
    fn wakes_a_handing_over_corgi_did_not_get_reach_the_corgi_that_takes_over() {
        use crate::corgi::{
            HandoverStep::{Ask, Replace},
            Trigger::Full,
        };
        use AgentState::{Done, Idle, Working};
        let corgi_bin = Path::new("/opt/corgi");
        let root = "/repos/weather";
        let mut waker = CorgiWaker::default();
        let mut sent = Vec::new();
        let mut steps = Vec::new();
        let corgi = |session: &str, state, change, percent| DashboardAgent {
            info: AgentInfo {
                state_change_seq: change,
                agent_session: Some(crate::model::AgentSession {
                    value: session.into(),
                    ..Default::default()
                }),
                ..project_agent("corgi-weather", root, true, state).info
            },
            context_percent: Some(percent),
            ..project_agent("corgi-weather", root, true, state)
        };
        let worker = |name: &str, state, change| DashboardAgent {
            info: AgentInfo {
                state_change_seq: change,
                ..project_agent(name, root, false, state).info
            },
            ..project_agent(name, root, false, state)
        };
        let mut refresh = |waker: &mut CorgiWaker, agents: &[DashboardAgent], note: bool| {
            waker.observe(agents, corgi_bin);
            steps.extend(
                waker
                    .hand_over(agents, 50, |_| None, 1_000, |_, _| note, |_| false)
                    .into_iter()
                    .filter_map(|action| Some((action.name, action.step?))),
            );
            waker.deliver(
                agents,
                |_| false,
                |corgi, text| {
                    sent.push(format!("{corgi}: {text}"));
                    true
                },
            );
        };
        let forecast = |state, change| worker("w-forecast", state, change);
        refresh(
            &mut waker,
            &[corgi("s1", Idle, 1, 70), forecast(Working, 1)],
            false,
        );
        // The handover turn runs, and a worker stops meanwhile.
        refresh(
            &mut waker,
            &[corgi("s1", Working, 2, 71), forecast(Done, 2)],
            false,
        );
        // The turn ends with a note: the corgi is idle, but the wake waits.
        refresh(
            &mut waker,
            &[corgi("s1", Done, 3, 72), forecast(Done, 2)],
            true,
        );
        // The old corgi has exited, and another worker stops in the gap.
        refresh(
            &mut waker,
            &[forecast(Done, 2), worker("w-radar", Working, 1)],
            true,
        );
        refresh(
            &mut waker,
            &[forecast(Done, 2), worker("w-radar", Done, 2)],
            true,
        );
        // The successor is at its first prompt when the replacement reports.
        let successor = [
            corgi("s2", Working, 1, 3),
            forecast(Done, 2),
            worker("w-radar", Done, 2),
        ];
        refresh(&mut waker, &successor, false);
        waker.replaced(root, true, &successor);
        refresh(&mut waker, &successor, false);
        let rested = [
            corgi("s2", Done, 2, 4),
            forecast(Done, 2),
            worker("w-radar", Done, 2),
        ];
        refresh(&mut waker, &rested, false);
        // One prompt, to the successor, once its first turn is over.
        assert_eq!(
            steps,
            [
                ("corgi-weather".to_string(), Ask(1_000, Full(50))),
                ("corgi-weather".to_string(), Replace)
            ]
        );
        assert_eq!(
            sent,
            [
                "corgi-weather: [corgi] w-forecast is done. Run: /opt/corgi report w-forecast\n\
              [corgi] w-radar is done. Run: /opt/corgi report w-radar"
            ]
        );
    }

    #[test]
    fn a_corgi_whose_turn_ends_without_a_note_keeps_its_session_and_gets_its_wakes() {
        use crate::corgi::{
            HandoverStep::{Ask, NoNote},
            Trigger::Full,
        };
        use AgentState::{Done, Idle, Working};
        let corgi_bin = Path::new("/opt/corgi");
        let root = "/repos/weather";
        let mut waker = CorgiWaker::default();
        let mut sent = Vec::new();
        let mut steps = Vec::new();
        let agents = |state, change, worker| {
            let mut corgi = project_agent("corgi-weather", root, true, state);
            corgi.info.state_change_seq = change;
            corgi.info.agent_session = Some(crate::model::AgentSession {
                value: "s1".into(),
                ..Default::default()
            });
            corgi.context_percent = Some(80);
            let mut forecast = project_agent("w-forecast", root, false, worker);
            forecast.info.state_change_seq = change;
            [corgi, forecast]
        };
        for (state, change, worker) in [(Idle, 1, Working), (Working, 2, Done), (Done, 3, Done)] {
            let agents = agents(state, change, worker);
            waker.observe(&agents, corgi_bin);
            steps.extend(
                waker
                    .hand_over(&agents, 50, |_| None, 1_000, |_, _| false, |_| false)
                    .into_iter()
                    .filter_map(|action| action.step),
            );
            waker.deliver(
                &agents,
                |_| false,
                |corgi, text| {
                    sent.push(format!("{corgi}: {text}"));
                    true
                },
            );
        }
        assert_eq!(steps, [Ask(1_000, Full(50)), NoNote]);
        assert_eq!(
            sent,
            ["corgi-weather: [corgi] w-forecast is done. Run: /opt/corgi report w-forecast"]
        );
    }

    #[test]
    fn an_idle_corgi_is_handed_over_from_the_baseline_its_pane_keeps() {
        use crate::corgi::{HandoverStep::Ask, Trigger::Idle};
        use AgentState::{Done, Idle as Resting, Working};
        let root = "/repos/weather";
        let corgi = |state, change, tokens, baseline: Option<&str>| {
            let mut agent = project_agent("corgi-weather", root, true, state);
            agent.info.agent = Some("codex".into());
            agent.info.state_change_seq = change;
            agent.info.agent_session = Some(crate::model::AgentSession {
                value: "s1".into(),
                ..Default::default()
            });
            if let Some(baseline) = baseline {
                agent
                    .info
                    .tokens
                    .insert(corgi::BASELINE_TOKEN.into(), baseline.into());
            }
            agent.context_percent = Some(10);
            agent.context_tokens = Some(tokens);
            agent
        };
        let mut waker = CorgiWaker::default();
        let mut refresh = |agents: &[DashboardAgent], now| {
            waker
                .hand_over(
                    agents,
                    50,
                    |harness| (*harness == Harness::Codex).then_some(60),
                    now,
                    |_, _| true,
                    |_| false,
                )
                .into_iter()
                .map(|action| (action.step, action.baseline))
                .collect::<Vec<_>>()
        };
        // The first turn ends: its baseline goes on the pane, once.
        assert_eq!(
            refresh(&[corgi(Done, 2, 30_000, None)], 0),
            [(None, Some(30_000))]
        );
        assert_eq!(refresh(&[corgi(Done, 2, 30_000, Some("30000 s1"))], 1), []);
        // A later turn doubles it; a worker of this project, then only one
        // of another project's, is at work while it rests.
        let grown = |state| corgi(state, 4, 60_000, Some("30000 s1"));
        let worker = |root| project_agent("w-forecast", root, false, Working);
        assert_eq!(refresh(&[grown(Done), worker(root)], 10), []);
        assert_eq!(refresh(&[grown(Done), worker(root)], 70), []);
        assert_eq!(
            refresh(&[grown(Resting), worker("/repos/corgi")], 71),
            [(Some(Ask(71, Idle(1))), Some(30_000))]
        );
    }

    #[test]
    fn a_codex_corgi_is_succeeded_in_its_pane_on_the_model_and_effort_it_was_launched_with() {
        let action = HandoverAction {
            step: Some(crate::corgi::HandoverStep::Replace),
            baseline: None,
            token: None,
            root: "/repos/weather".into(),
            name: "corgi-weather".into(),
            pane_id: "w9:p1".into(),
            session: "019a-thread".into(),
            harness: Harness::Codex,
            change: 12,
        };
        let plan = successor_plan(
            &action,
            crate::corgi::Launch {
                kind: "codex".into(),
                model: "gpt-6-astra".into(),
                effort: "high".into(),
                extra_args: vec!["--search".into()],
            },
        );
        assert_eq!(plan.name, "corgi-weather");
        assert_eq!(
            plan.role,
            Role::Corgi {
                handover_pane: Some("w9:p1".into())
            }
        );
        assert_eq!(plan.project, Path::new("/repos/weather"));
        assert!(
            plan.prompt.is_empty(),
            "the takeover prompt replaces any task"
        );
        let args = launch_args(&plan);
        assert_eq!(
            &args[..5],
            [
                "--model",
                "gpt-6-astra",
                "-c",
                "model_reasoning_effort=high",
                "--search"
            ]
        );
        assert_eq!(action.harness.exit_command(), "/quit");
        // A Claude corgi launched on its harness's defaults stays on them.
        let claude = successor_plan(
            &HandoverAction {
                harness: Harness::Claude,
                ..action
            },
            crate::corgi::Launch {
                kind: "claude".into(),
                ..crate::corgi::Launch::default()
            },
        );
        assert!(launch_args(&claude).is_empty());
    }

    #[test]
    fn one_dashboard_per_herdr_socket_wakes_corgis() {
        let lock = std::env::temp_dir()
            .join(format!("corgi-wake-test-{}", std::process::id()))
            .join("herdr.lock");
        let mut first = CorgiWaker::default();
        let mut second = CorgiWaker::default();
        assert!(first.lead(&lock));
        assert!(!second.lead(&lock));
        assert!(first.lead(&lock));
        // When the leader goes, another dashboard takes over.
        drop(first);
        assert!(second.lead(&lock));
        fs::remove_dir_all(lock.parent().expect("lock dir")).ok();
        assert_eq!(
            wake_lock_path(Path::new("/tmp/herdr.sock"))
                .and_then(|path| path.file_name().map(|name| name.to_owned())),
            Some("_tmp_herdr_sock.lock".into())
        );
    }
}
