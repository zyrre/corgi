//! Pause and Resume against a fake core whose terminals act as hc's do:
//! Esc ends a turn, `/exit` leaves a supervisor's kept shell in front and
//! finishes a worker's terminal, a resume line typed into a shell brings
//! the agent back.

use std::{cell::RefCell, collections::BTreeMap};

use super::*;
use crate::{
    hc::{HARNESS_LABEL, SPACE_LABEL, Status, TASK_LABEL, WORKER_ROLE},
    model::AgentInfo,
    progress::Silent,
    test_support::ScratchDir,
};

const ROOT: &str = "/repos/weather";

/// How a fake agent takes Esc and its exit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Acts {
    #[default]
    Well,
    /// Keeps working after Esc.
    Stubborn,
    /// Takes no exit.
    Stays,
    /// Holds text in its input box.
    Drafting,
}

#[derive(Default)]
struct State {
    terminals: BTreeMap<String, (Terminal, Acts)>,
    calls: Vec<String>,
    next: u32,
}

#[derive(Default)]
struct FakeCore {
    state: RefCell<State>,
}

fn terminal(address: &str, name: &str, supervisor: bool, status: &str) -> Terminal {
    let role = if supervisor {
        SUPERVISOR_ROLE
    } else {
        WORKER_ROLE
    };
    let labels: BTreeMap<String, String> = [
        (PROJECT_LABEL, ROOT),
        (ROLE_LABEL, role),
        (HARNESS_LABEL, "claude"),
        (SESSION_LABEL, &format!("session-{name}")),
        (
            RESUME_LABEL,
            &format!("/bin/sh -c 'agent' corgi-agent /w '' claude --resume session-{name}"),
        ),
        (SPACE_LABEL, &format!("space-{name}")),
        (TASK_LABEL, "Fix the cache"),
        (PID_LABEL, "100"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    Terminal {
        address: address.into(),
        name: Some(name.into()),
        state: "live".into(),
        status: Some(Status {
            state: status.into(),
            ..Status::default()
        }),
        labels,
        cwd: format!("/w/{name}"),
        foreground: Some(crate::hc::Foreground {
            name: "claude".into(),
            ..Default::default()
        }),
        pid: Some(100),
        keep: true,
        ..Terminal::default()
    }
}

impl FakeCore {
    fn with(terminals: Vec<(Terminal, Acts)>) -> Self {
        let core = Self::default();
        {
            let mut state = core.state.borrow_mut();
            for (terminal, acts) in terminals {
                state
                    .terminals
                    .insert(terminal.address.clone(), (terminal, acts));
            }
            state.next = 50;
        }
        core
    }

    fn calls(&self) -> Vec<String> {
        self.state.borrow().calls.clone()
    }

    fn terminal(&self, address: &str) -> Option<Terminal> {
        self.state
            .borrow()
            .terminals
            .get(address)
            .map(|(terminal, _)| terminal.clone())
    }

    fn find_name(&self, name: &str) -> Vec<Terminal> {
        self.state
            .borrow()
            .terminals
            .values()
            .filter(|(terminal, _)| terminal.display_name() == name)
            .map(|(terminal, _)| terminal.clone())
            .collect()
    }

    /// A core stop and start: every kept terminal still live gets a shell
    /// of another pid; finished ones stay finished.
    fn restart(&self) {
        for (terminal, _) in self.state.borrow_mut().terminals.values_mut() {
            if terminal.is_live() {
                terminal.foreground = Some(crate::hc::Foreground {
                    name: "zsh".into(),
                    ..Default::default()
                });
                terminal.status = None;
                terminal.pid = terminal.pid.map(|pid| pid + 1000);
            }
        }
    }
}

/// The program a resume line brings back in `terminal`: its harness.
fn harness_of(terminal: &Terminal) -> String {
    terminal.label(HARNESS_LABEL).unwrap_or("agent").to_string()
}

/// `terminal` running `kind` (`codex`, `opencode`), with the resume line
/// Corgi gives that harness.
fn on(kind: &str, mut terminal: Terminal) -> Terminal {
    let session = terminal.label(SESSION_LABEL).unwrap().to_string();
    let resume = match kind {
        "codex" => format!("codex --no-daemon resume {session}"),
        "opencode" => format!("opencode --session {session}"),
        _ => format!("claude --resume {session}"),
    };
    for (key, value) in [
        (HARNESS_LABEL, kind.to_string()),
        (
            RESUME_LABEL,
            format!("/bin/sh -c 'agent' corgi-agent /w '' {resume}"),
        ),
    ] {
        terminal.labels.insert(key.to_string(), value);
    }
    terminal.foreground = Some(crate::hc::Foreground {
        name: kind.into(),
        ..Default::default()
    });
    terminal
}

/// [`CARRY_ON`] as a resume line ends with it.
fn carry_on_words() -> String {
    format!("-- '{CARRY_ON}'")
}

fn set_status(terminal: &mut Terminal, status: &str) {
    terminal.status = Some(Status {
        state: status.into(),
        ..Status::default()
    });
}

impl Core for FakeCore {
    fn terminals(&self) -> Result<Vec<Terminal>> {
        Ok(self
            .state
            .borrow()
            .terminals
            .values()
            .filter(|(terminal, _)| terminal.is_live())
            .map(|(terminal, _)| terminal.clone())
            .collect())
    }

    fn get(&self, address: &str) -> Result<Option<Terminal>> {
        Ok(self.terminal(address))
    }

    fn keys(&self, address: &str, keys: &[&str]) -> Result<()> {
        let mut state = self.state.borrow_mut();
        state
            .calls
            .push(format!("keys {address} {}", keys.join(" ")));
        if let Some((terminal, acts)) = state.terminals.get_mut(address)
            && keys == ["esc"]
            && *acts != Acts::Stubborn
            && terminal.status_word() == Some("working")
        {
            set_status(terminal, "idle");
        }
        Ok(())
    }

    fn send(&self, address: &str, text: &str) -> Result<()> {
        let mut state = self.state.borrow_mut();
        state.calls.push(format!("send {address} {text}"));
        let (terminal, acts) = state
            .terminals
            .get_mut(address)
            .context("no such terminal")?;
        if text == "/exit" || text == "/quit" {
            if *acts == Acts::Stays {
                return Ok(());
            }
            if terminal.label(ROLE_LABEL) == Some(SUPERVISOR_ROLE) {
                terminal.foreground = Some(crate::hc::Foreground {
                    name: "-zsh".into(),
                    ..Default::default()
                });
                terminal.status = None;
            } else {
                terminal.state = "finished".into();
                terminal.foreground = None;
                terminal.pid = None;
                terminal.status = None;
            }
        } else if shell_in_front(terminal) && text.starts_with("/bin/sh -c ") {
            terminal.foreground = Some(crate::hc::Foreground {
                name: harness_of(terminal),
                ..Default::default()
            });
            set_status(terminal, "idle");
        }
        Ok(())
    }

    fn holds_draft(&self, terminal: &Terminal) -> Option<bool> {
        let state = self.state.borrow();
        let (_, acts) = state.terminals.get(&terminal.address)?;
        Some(*acts == Acts::Drafting)
    }

    fn run(&self, spec: &RunSpec) -> Result<Terminal> {
        let mut state = self.state.borrow_mut();
        state.next += 1;
        let address = format!("t{}", state.next);
        state.calls.push(format!(
            "run {address} {} {} {}",
            spec.name,
            spec.cwd.display(),
            spec.argv.join(" ")
        ));
        let terminal = Terminal {
            address: address.clone(),
            name: Some(spec.name.clone()),
            state: "live".into(),
            status: Some(Status {
                state: "idle".into(),
                ..Status::default()
            }),
            labels: spec.labels.iter().cloned().collect(),
            cwd: spec.cwd.to_string_lossy().into_owned(),
            ..Terminal::default()
        };
        let terminal = Terminal {
            foreground: Some(crate::hc::Foreground {
                name: harness_of(&terminal),
                ..Default::default()
            }),
            pid: Some(u64::from(state.next) * 10),
            keep: spec.keep,
            ..terminal
        };
        state
            .terminals
            .insert(address, (terminal.clone(), Acts::Well));
        Ok(terminal)
    }

    fn remove(&self, address: &str) -> Result<()> {
        let mut state = self.state.borrow_mut();
        state.calls.push(format!("rm {address}"));
        let finished = state
            .terminals
            .get(address)
            .is_some_and(|(terminal, _)| !terminal.is_live());
        anyhow::ensure!(finished, "hc rm refuses a live terminal");
        state.terminals.remove(address);
        Ok(())
    }

    fn label(&self, address: &str, labels: &[(&str, &str)]) -> Result<()> {
        let mut state = self.state.borrow_mut();
        let pairs: Vec<String> = labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
        state
            .calls
            .push(format!("label {address} {}", pairs.join(" ")));
        if let Some((terminal, _)) = state.terminals.get_mut(address) {
            for (key, value) in labels {
                terminal.labels.insert(key.to_string(), value.to_string());
            }
        }
        Ok(())
    }

    fn wait(&self) {}
}

/// A fleet: the supervisor and `w-radar` work, `w-cache` rests (idle),
/// `w-docs` finished its turn (done), `w-ask` waits on a question.
fn fleet() -> FakeCore {
    FakeCore::with(vec![
        (terminal("t1", "supervisor", true, "working"), Acts::Well),
        (terminal("t2", "w-radar", false, "working"), Acts::Well),
        (terminal("t3", "w-cache", false, "idle"), Acts::Well),
        (terminal("t4", "w-docs", false, "done"), Acts::Well),
        (terminal("t5", "w-ask", false, "blocked"), Acts::Well),
    ])
}

fn paused(path: &Path) -> Vec<(String, Phase)> {
    read(path)
        .agents
        .into_iter()
        .map(|agent| (agent.name, agent.phase))
        .collect()
}

fn row(terminal: &Terminal) -> Agent {
    Agent {
        info: AgentInfo {
            address: terminal.address.clone(),
            name: terminal.name.clone(),
            state: agent_state(terminal),
            ..AgentInfo::default()
        },
        project_root: ROOT.into(),
        supervisor: terminal.label(ROLE_LABEL) == Some(SUPERVISOR_ROLE),
        exited: shell_in_front(terminal) || !terminal.is_live(),
        ..Agent::default()
    }
}

#[test]
fn pause_stops_only_the_working_agents_with_esc_then_their_exit_and_marks_them() {
    let scratch = ScratchDir::new("hc-pause");
    let path = scratch.join("pause.json");
    let core = fleet();
    let outcome = pause_with(&core, &path, None, 1_000, &mut Silent).unwrap();
    assert_eq!(outcome.done, ["supervisor", "w-radar"]);
    assert!(outcome.failed.is_empty() && outcome.left.is_empty());
    assert_eq!(outcome.result("paused"), "paused");
    assert_eq!(
        core.calls(),
        [
            "keys t1 esc",
            "keys t2 esc",
            "send t1 /exit",
            "send t2 /exit",
        ],
        "Esc, then the harness's own exit; nothing for the agents at rest"
    );
    // The supervisor's shell is in front; the worker's terminal finished.
    assert!(shell_in_front(&core.terminal("t1").unwrap()));
    assert!(!core.terminal("t2").unwrap().is_live());
    for resting in ["t3", "t4", "t5"] {
        assert!(core.terminal(resting).unwrap().is_live());
    }
    assert_eq!(
        paused(&path),
        [
            ("supervisor".to_string(), Phase::Paused),
            ("w-radar".to_string(), Phase::Paused),
        ]
    );
    let record = read(&path);
    assert!(record.in_force());
    let radar = &record.agents[1];
    assert_eq!(radar.session.as_deref(), Some("session-w-radar"));
    assert!(radar.resume.contains("--resume session-w-radar"));
    assert_eq!(
        radar.labels.get(SPACE_LABEL).map(String::as_str),
        Some("space-w-radar")
    );
}

#[test]
fn with_no_agent_working_a_pause_changes_nothing() {
    let scratch = ScratchDir::new("hc-pause-none");
    let path = scratch.join("pause.json");
    let core = FakeCore::with(vec![
        (terminal("t1", "supervisor", true, "idle"), Acts::Well),
        (terminal("t2", "w-cache", false, "done"), Acts::Well),
    ]);
    let outcome = pause_with(&core, &path, None, 1_000, &mut Silent).unwrap();
    assert_eq!(outcome.result("paused"), "nothing");
    assert_eq!(outcome.message("Paused", NOTHING_WORKING), NOTHING_WORKING);
    assert!(core.calls().is_empty());
    assert!(!path.exists(), "nothing marked");
    assert!(!read(&path).in_force());
}

#[test]
fn an_agent_that_does_not_stop_is_reported_and_left_running_never_killed() {
    let scratch = ScratchDir::new("hc-pause-stubborn");
    let path = scratch.join("pause.json");
    let core = FakeCore::with(vec![
        (
            terminal("t1", "w-stubborn", false, "working"),
            Acts::Stubborn,
        ),
        (terminal("t2", "w-stays", false, "working"), Acts::Stays),
        (terminal("t3", "w-draft", false, "working"), Acts::Drafting),
        (terminal("t4", "w-radar", false, "working"), Acts::Well),
    ]);
    let outcome = pause_with(&core, &path, None, 1_000, &mut Silent).unwrap();
    assert_eq!(outcome.done, ["w-radar"]);
    let failed: Vec<(&str, &str)> = outcome
        .failed
        .iter()
        .map(|(name, why)| (name.as_str(), why.as_str()))
        .collect();
    assert_eq!(
        failed,
        [
            ("w-stubborn", "kept working after Esc; left running"),
            ("w-draft", "has unsent text in its input box; left running"),
            ("w-stays", "did not exit; left running"),
        ]
    );
    assert_eq!(outcome.result("paused"), "partial");
    let calls = core.calls();
    assert!(
        calls
            .iter()
            .all(|call| !call.starts_with("kill") && !call.starts_with("rm")),
        "{calls:?}"
    );
    assert!(
        !calls.contains(&"send t3 /exit".to_string()),
        "a draft is never typed after"
    );
    for running in ["t1", "t2", "t3"] {
        assert!(core.terminal(running).unwrap().is_live());
    }
    // Only the one that stopped stays marked: the others hold no wakes.
    assert_eq!(paused(&path), [("w-radar".to_string(), Phase::Paused)]);
}

#[test]
fn an_agent_without_a_resume_line_or_running_this_pause_is_left_alone() {
    let scratch = ScratchDir::new("hc-pause-left");
    let path = scratch.join("pause.json");
    let mut codex = terminal("t1", "w-codex", false, "working");
    codex.labels.remove(RESUME_LABEL);
    codex.labels.remove(SESSION_LABEL);
    let core = FakeCore::with(vec![
        (codex, Acts::Well),
        (terminal("t2", "supervisor", true, "working"), Acts::Well),
    ]);
    let outcome = pause_with(&core, &path, Some("t2"), 1_000, &mut Silent).unwrap();
    assert_eq!(outcome.result("paused"), "nothing");
    assert_eq!(
        outcome.left,
        [
            (
                "w-codex".to_string(),
                "has no resume line, so Corgi could not bring it back".to_string()
            ),
            ("supervisor".to_string(), "runs this pause".to_string()),
        ]
    );
    assert!(core.calls().is_empty());
    assert!(!path.exists());
}

#[test]
fn resume_brings_back_exactly_the_paused_agents_on_their_own_sessions() {
    let scratch = ScratchDir::new("hc-resume");
    let path = scratch.join("pause.json");
    let core = fleet();
    pause_with(&core, &path, None, 1_000, &mut Silent).unwrap();
    // An agent that exits on its own while they are paused stays exited.
    core.send("t3", "/exit").unwrap();
    let before = core.calls().len();

    let outcome = resume_with(&core, &path, 2_000, &mut Silent).unwrap();
    assert_eq!(outcome.done, ["supervisor", "w-radar"]);
    assert!(outcome.failed.is_empty());
    let calls = core.calls()[before..].to_vec();
    assert_eq!(
        calls[0],
        format!(
            "send t1 /bin/sh -c 'agent' corgi-agent /w '' claude --resume session-supervisor {}",
            carry_on_words()
        ),
        "the supervisor's resume line, typed into its kept shell, told to carry on"
    );
    assert_eq!(
        calls[2],
        format!(
            "run t51 w-radar /w/w-radar /bin/sh -c exec /bin/sh -c 'agent' corgi-agent /w '' \
             claude --resume session-w-radar {}",
            carry_on_words()
        ),
        "the worker, in a new terminal of its name, on its session, told to carry on"
    );
    assert_eq!(calls[3], "rm t2", "its finished terminal is removed");
    assert!(!calls.iter().any(|call| call.contains("t3")), "{calls:?}");
    // The new terminal has the old one's labels.
    let radar = core.terminal("t51").unwrap();
    assert_eq!(radar.label(SPACE_LABEL), Some("space-w-radar"));
    assert_eq!(radar.label(TASK_LABEL), Some("Fix the cache"));
    assert_eq!(radar.label(PID_LABEL), Some("510"));
    assert!(!shell_in_front(&core.terminal("t1").unwrap()));
    // Marked until the daemon sees them back: no pause in force.
    let record = read(&path);
    assert!(!record.in_force());
    assert_eq!(
        record
            .agents
            .iter()
            .map(|agent| (agent.address.as_str(), agent.phase))
            .collect::<Vec<_>>(),
        [("t1", Phase::Resuming), ("t51", Phase::Resuming)]
    );
    // A second Resume has nothing to do.
    let again = resume_with(&core, &path, 2_001, &mut Silent).unwrap();
    assert_eq!(again.result("resumed"), "nothing");
}

#[test]
fn a_core_restart_between_pause_and_resume_keeps_them_paused() {
    let scratch = ScratchDir::new("hc-pause-restart");
    let path = scratch.join("pause.json");
    let core = fleet();
    pause_with(&core, &path, None, 1_000, &mut Silent).unwrap();
    core.restart();
    // Read afresh, as a new process after the restart would.
    let holds = Holds::read(&path);
    assert!(holds.in_force);
    let supervisor = core.terminal("t1").unwrap();
    assert!(
        holds.holds_terminal(&supervisor),
        "the daemon resumes no paused agent"
    );
    assert!(!holds.holds_terminal(&core.terminal("t3").unwrap()));

    let outcome = resume_with(&core, &path, 2_000, &mut Silent).unwrap();
    assert_eq!(outcome.done, ["supervisor", "w-radar"]);
    let calls = core.calls();
    assert!(
        calls.contains(&"label t1 corgi.pid=1100".to_string()),
        "the new shell's pid, so the daemon types the line no second time: {calls:?}"
    );
    assert_eq!(core.find_name("w-radar").len(), 1);
}

#[test]
fn a_supervisor_whose_terminal_is_gone_is_reported_and_holds_nothing() {
    let scratch = ScratchDir::new("hc-resume-gone");
    let path = scratch.join("pause.json");
    let core = fleet();
    pause_with(&core, &path, None, 1_000, &mut Silent).unwrap();
    core.state.borrow_mut().terminals.remove("t1");
    let outcome = resume_with(&core, &path, 2_000, &mut Silent).unwrap();
    assert_eq!(outcome.done, ["w-radar"]);
    assert_eq!(outcome.failed.len(), 1);
    assert!(
        outcome.failed[0]
            .1
            .contains("start it again with corgi supervisor"),
        "{outcome:?}"
    );
    assert_eq!(paused(&path), [("w-radar".to_string(), Phase::Resuming)]);
}

#[test]
fn settle_lets_go_of_agents_seen_back_and_of_stale_marks_only() {
    let scratch = ScratchDir::new("hc-pause-settle");
    let path = scratch.join("pause.json");
    let core = fleet();
    pause_with(&core, &path, None, 1_000, &mut Silent).unwrap();
    // Still paused: nothing is let go of, however long it stays so.
    let rows: Vec<Agent> = core.terminals().unwrap().iter().map(row).collect();
    assert!(settle(&path, &rows, 1_000_000).is_empty());

    resume_with(&core, &path, 2_000, &mut Silent).unwrap();
    // A pass that sees the supervisor back alone lets go of it alone.
    let supervisor = row(&core.terminal("t1").unwrap());
    assert_eq!(settle(&path, &[supervisor], 2_001), ["supervisor"]);
    assert_eq!(paused(&path), [("w-radar".to_string(), Phase::Resuming)]);
    // One that never shows goes once stale.
    assert!(settle(&path, &[], 2_000 + STALE_SECS).is_empty());
    assert_eq!(settle(&path, &[], 2_001 + STALE_SECS), ["w-radar"]);
    assert!(!path.exists(), "an empty pause leaves no file");
}

#[test]
fn holds_name_a_resumed_worker_by_its_name_in_its_new_terminal() {
    let scratch = ScratchDir::new("hc-pause-holds");
    let path = scratch.join("pause.json");
    let core = fleet();
    pause_with(&core, &path, None, 1_000, &mut Silent).unwrap();
    // The worker is started again, by hand, before the record knows its
    // new terminal.
    let mut again = terminal("t9", "w-radar", false, "idle");
    again.pid = Some(900);
    let holds = Holds::read(&path);
    assert!(holds.holds_terminal(&again));
    let rows = vec![row(&again), row(&core.terminal("t3").unwrap())];
    assert_eq!(holds.panes(&rows), HashSet::from(["t9".to_string()]));
    assert_eq!(holds.supervisor_roots(), HashSet::from([ROOT.to_string()]));
}

#[test]
fn status_says_whether_a_pause_is_in_force_and_names_its_agents() {
    let scratch = ScratchDir::new("hc-pause-status");
    let path = scratch.join("pause.json");
    assert_eq!(
        status_value(&read(&path)),
        json!({"any": false, "agents": []})
    );
    pause_with(&fleet(), &path, None, 1_000, &mut Silent).unwrap();
    let value = status_value(&read(&path));
    assert_eq!(value["any"], true);
    assert_eq!(value["agents"][0]["role"], "supervisor");
    assert_eq!(value["agents"][1]["name"], "w-radar");
    assert_eq!(value["agents"][1]["terminal"], "t2");
    assert_eq!(value["agents"][1]["phase"], "paused");
}

#[test]
fn the_json_names_what_was_done_and_what_was_not() {
    let outcome = Outcome {
        done: vec!["w-radar".into()],
        failed: vec![("w-stays".into(), "did not exit; left running".into())],
        left: Vec::new(),
    };
    let value = outcome.json("paused", "Paused", NOTHING_WORKING);
    assert_eq!(value["result"], "partial");
    assert_eq!(value["paused"], json!(["w-radar"]));
    assert_eq!(value["failed"][0]["name"], "w-stays");
    assert_eq!(
        value["message"],
        "Paused 1 agent. Not paused: w-stays did not exit; left running"
    );
}

/// Pauses the one worker `w-radar` of `kind` in `t2` and returns the core
/// and the pause file's path.
fn paused_worker(scratch: &ScratchDir, kind: &str) -> (FakeCore, PathBuf) {
    let path = scratch.join("pause.json");
    let core = FakeCore::with(vec![(
        on(kind, terminal("t2", "w-radar", false, "working")),
        Acts::Well,
    )]);
    pause_with(&core, &path, None, 1_000, &mut Silent).unwrap();
    (core, path)
}

#[test]
fn resume_starts_a_finished_worker_of_each_harness_telling_it_to_carry_on() {
    for kind in ["claude", "codex", "opencode"] {
        let scratch = ScratchDir::new("hc-resume-harness");
        let (core, path) = paused_worker(&scratch, kind);
        let before = core.calls().len();
        let outcome = resume_with(&core, &path, 2_000, &mut Silent).unwrap();
        assert_eq!(outcome.done, ["w-radar"], "{kind}");
        assert!(outcome.left.is_empty(), "{kind}: {outcome:?}");
        let calls = core.calls()[before..].to_vec();
        let run = calls.iter().find(|call| call.starts_with("run ")).unwrap();
        let typed: Vec<&String> = calls
            .iter()
            .filter(|call| call.starts_with("send "))
            .collect();
        if kind == "opencode" {
            // Its `--prompt` is lost on the session `--session` opens: the
            // message is typed once it is in front.
            assert!(run.ends_with("opencode --session session-w-radar"), "{run}");
            assert_eq!(typed, [&format!("send t51 {CARRY_ON}")]);
        } else {
            assert!(
                run.ends_with(&format!("session-w-radar {}", carry_on_words())),
                "{kind}: {run}"
            );
            assert!(typed.is_empty(), "{kind}: {calls:?}");
        }
        // The terminal keeps the plain resume line, which the daemon types
        // after a core restart: those agents were not stopped mid-turn.
        core.restart();
        let restarted = core.terminal("t51").unwrap();
        let line = crate::hc::daemon::needs_resume(&restarted).unwrap();
        assert!(line.ends_with("session-w-radar"), "{kind}: {line}");
        assert!(!line.contains("[Corgi]"), "{kind}: {line}");
    }
}

#[test]
fn resume_types_the_carry_on_into_a_kept_shell_of_each_harness() {
    for kind in ["claude", "codex", "opencode"] {
        let scratch = ScratchDir::new("hc-resume-shell");
        let (core, path) = paused_worker(&scratch, kind);
        // A core restart left a shell in the worker's place.
        {
            let mut state = core.state.borrow_mut();
            let (terminal, _) = state.terminals.get_mut("t2").unwrap();
            terminal.state = "live".into();
            terminal.pid = Some(2_000);
            terminal.foreground = Some(crate::hc::Foreground {
                name: "zsh".into(),
                ..Default::default()
            });
        }
        let before = core.calls().len();
        let outcome = resume_with(&core, &path, 2_000, &mut Silent).unwrap();
        assert_eq!(outcome.done, ["w-radar"], "{kind}");
        let typed: Vec<String> = core.calls()[before..]
            .iter()
            .filter(|call| call.starts_with("send "))
            .cloned()
            .collect();
        let line = core.terminal("t2").unwrap().labels[RESUME_LABEL].clone();
        if kind == "opencode" {
            assert_eq!(
                typed,
                [format!("send t2 {line}"), format!("send t2 {CARRY_ON}")]
            );
        } else {
            assert_eq!(typed, [format!("send t2 {line} {}", carry_on_words())]);
        }
    }
}

#[test]
fn a_codex_supervisor_is_resumed_in_its_kept_shell_told_to_carry_on() {
    let scratch = ScratchDir::new("hc-resume-codex-supervisor");
    let path = scratch.join("pause.json");
    let core = FakeCore::with(vec![(
        on("codex", terminal("t1", "supervisor", true, "working")),
        Acts::Well,
    )]);
    pause_with(&core, &path, None, 1_000, &mut Silent).unwrap();
    assert!(core.calls().contains(&"send t1 /quit".to_string()));
    let before = core.calls().len();
    let outcome = resume_with(&core, &path, 2_000, &mut Silent).unwrap();
    assert_eq!(outcome.done, ["supervisor"]);
    assert_eq!(
        core.calls()[before],
        format!(
            "send t1 /bin/sh -c 'agent' corgi-agent /w '' codex --no-daemon resume \
             session-supervisor {}",
            carry_on_words()
        )
    );
    assert!(!shell_in_front(&core.terminal("t1").unwrap()));
}

#[test]
fn an_agent_brought_back_by_hand_before_resume_is_told_nothing() {
    for kind in ["claude", "opencode"] {
        let scratch = ScratchDir::new("hc-resume-by-hand");
        let path = scratch.join("pause.json");
        let core = FakeCore::with(vec![(
            on(kind, terminal("t1", "supervisor", true, "working")),
            Acts::Well,
        )]);
        pause_with(&core, &path, None, 1_000, &mut Silent).unwrap();
        // Someone resumed it in its shell already.
        {
            let mut state = core.state.borrow_mut();
            let (terminal, _) = state.terminals.get_mut("t1").unwrap();
            terminal.foreground = Some(crate::hc::Foreground {
                name: kind.into(),
                ..Default::default()
            });
            set_status(terminal, "idle");
        }
        let before = core.calls().len();
        let outcome = resume_with(&core, &path, 2_000, &mut Silent).unwrap();
        assert_eq!(outcome.done, ["supervisor"], "{kind}");
        assert!(
            core.calls()[before..].is_empty(),
            "{kind}: {:?}",
            core.calls()
        );
    }
}

#[test]
fn an_opencode_agent_that_cannot_be_told_is_back_and_named() {
    let scratch = ScratchDir::new("hc-resume-untold");
    let (core, path) = paused_worker(&scratch, "opencode");
    // Its new terminal is gone before the message goes.
    struct Losing<'a>(&'a FakeCore);
    impl Core for Losing<'_> {
        fn terminals(&self) -> Result<Vec<Terminal>> {
            self.0.terminals()
        }
        fn get(&self, address: &str) -> Result<Option<Terminal>> {
            self.0.get(address)
        }
        fn keys(&self, address: &str, keys: &[&str]) -> Result<()> {
            self.0.keys(address, keys)
        }
        fn send(&self, _: &str, _: &str) -> Result<()> {
            anyhow::bail!("t51 is blocked")
        }
        fn holds_draft(&self, terminal: &Terminal) -> Option<bool> {
            self.0.holds_draft(terminal)
        }
        fn run(&self, spec: &RunSpec) -> Result<Terminal> {
            self.0.run(spec)
        }
        fn remove(&self, address: &str) -> Result<()> {
            self.0.remove(address)
        }
        fn label(&self, address: &str, labels: &[(&str, &str)]) -> Result<()> {
            self.0.label(address, labels)
        }
        fn wait(&self) {}
    }
    let outcome = resume_with(&Losing(&core), &path, 2_000, &mut Silent).unwrap();
    assert_eq!(outcome.result("resumed"), "resumed");
    assert_eq!(
        outcome.message("Resumed", NOTHING_PAUSED),
        "Resumed 1 agent. Left running: w-radar was not told to carry on (t51 is blocked); \
         tell it to"
    );
}

#[test]
fn the_carry_on_reaches_the_agent_as_one_argument_through_a_posix_shell() {
    let scratch = ScratchDir::new("hc-resume-quoting");
    let dir = scratch.canonicalize().unwrap();
    // A harness that prints each argument it gets.
    let command: Vec<String> = [
        "/bin/sh",
        "-c",
        r#"for a do printf '[%s]\n' "$a"; done"#,
        "claude",
        "--resume",
        "s-1",
    ]
    .map(String::from)
    .to_vec();
    let mut agent = PausedAgent::of(
        &terminal("t1", "w-radar", false, "working"),
        &shell_line(&launch::agent_argv(&dir, None, &command)),
        1,
    );
    agent.harness = "claude".into();
    let line = agent.resume_carrying_on().unwrap();
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", &format!("exec {line}")])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("[--resume]\n[s-1]\n[--]\n[{CARRY_ON}]\n")
    );
    agent.harness = "opencode".into();
    assert_eq!(agent.resume_carrying_on(), None);
}
