use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::*;
use crate::{
    model::AgentSession,
    test_support::{Recorded, ScratchDir, answer, fake_herdr},
};

const S1: &str = "11111111-2222-4333-8444-555555555555";
const S2: &str = "22222222-2222-4333-8444-555555555555";
const S3: &str = "33333333-2222-4333-8444-555555555555";
const S4: &str = "44444444-2222-4333-8444-555555555555";

fn row(
    pane: &str,
    kind: &str,
    state: AgentState,
    project: &str,
    session: Option<&str>,
) -> DashboardAgent {
    DashboardAgent {
        info: AgentInfo {
            pane_id: pane.into(),
            name: Some(pane.replace(":p", "-")),
            agent: Some(kind.into()),
            state,
            cwd: Some(format!("/repos/{project}")),
            agent_session: session.map(|value| AgentSession {
                value: value.into(),
                ..AgentSession::default()
            }),
            ..AgentInfo::default()
        },
        project_group: project.into(),
        project_root: format!("/repos/{project}"),
        ..DashboardAgent::default()
    }
}

fn supervisor_row(
    pane: &str,
    kind: &str,
    state: AgentState,
    project: &str,
    session: &str,
) -> DashboardAgent {
    DashboardAgent {
        supervisor: true,
        ..row(pane, kind, state, project, Some(session))
    }
}

/// Two projects and the scratch sessions: a Claude supervisor and a Codex
/// worker at work, an idle Claude worker, a Codex supervisor waiting on a
/// question, and a scratch session; then what is not paused.
fn rows() -> Vec<DashboardAgent> {
    use AgentState::{Blocked, Done, Idle, Unknown, Working};
    let mut exited = row("w9:p1", "claude", Unknown, "corgi", Some(S1));
    exited.exited = true;
    vec![
        supervisor_row("w1:p1", "claude", Working, "corgi", S1),
        row("w2:p1", "codex", Working, "corgi", Some(S2)),
        row("w3:p1", "claude", Idle, "corgi", Some(S3)),
        supervisor_row("w4:p1", "codex", Blocked, "weather", S4),
        row("w5:p1", "claude", Done, SCRATCH_GROUP, Some(S1)),
        exited,
        row("w6:p1", "claude", Unknown, "weather", Some(S2)),
        row("w7:p1", "opencode", Idle, "weather", Some(S3)),
        row("w8:p1", "claude", Idle, "weather", None),
    ]
}

#[test]
fn pause_all_takes_every_running_claude_and_codex_agent_in_every_project() {
    let plan = plan(&rows());
    let panes: Vec<&str> = plan.targets.iter().map(|t| t.pane_id.as_str()).collect();
    assert_eq!(panes, ["w1:p1", "w2:p1", "w3:p1", "w4:p1", "w5:p1"]);
    let busy: Vec<&str> = plan
        .targets
        .iter()
        .filter(|t| t.busy)
        .map(|t| t.pane_id.as_str())
        .collect();
    assert_eq!(
        busy,
        ["w1:p1", "w2:p1", "w4:p1"],
        "working or on a question"
    );
    assert!(plan.targets[0].supervisor && plan.targets[3].supervisor);
    assert_eq!(plan.targets[1].kind, "codex");
    assert_eq!(
        plan.left,
        [
            ("w6-1".to_string(), "Herdr does not know its state"),
            ("w7-1".to_string(), "Corgi cannot resume its harness"),
            ("w8-1".to_string(), "Corgi has seen no session of it"),
        ],
        "the exited row has nothing to stop"
    );
}

#[test]
fn the_confirmation_names_how_many_stop_per_project_and_which_are_working() {
    let form = plan(&rows()).form();
    assert_eq!((form.agents, form.working), (5, 3));
    assert_eq!(
        confirmation(&form),
        [
            "Stops 5 agents to their shell: each one's conversation is saved, and Resume all brings them back with their session and options.",
            "corgi: its supervisor and 2 workers · working: w1-1, w2-1",
            "weather: its supervisor · working: w4-1",
            "Scratch: 1 session",
            "The 3 agents working now are interrupted with Esc first: each one's current turn is lost.",
            "Left running: w6-1: Herdr does not know its state; w7-1: Corgi cannot resume its harness; w8-1: Corgi has seen no session of it.",
        ]
    );
    let one = plan(&rows()[2..3]).form();
    assert_eq!(
        confirmation(&one),
        [
            "Stops 1 agent to its shell: its conversation is saved, and Resume all brings it back with its session and options.",
            "corgi: 1 worker"
        ]
    );
}

/// A Herdr whose agents stop on Esc and leave on their exit command, as
/// told, and that records what it was asked.
struct Sim {
    agents: RefCell<Vec<AgentInfo>>,
    calls: RefCell<Vec<String>>,
    /// Panes whose agent ignores Esc, ignores its exit, or shows a draft.
    stubborn: Vec<&'static str>,
    staying: Vec<&'static str>,
    drafts: Vec<&'static str>,
    /// The record, to check every mark is set before any key goes out.
    record: PathBuf,
    marked_first: RefCell<bool>,
}

impl Sim {
    fn new(rows: &[DashboardAgent], record: &Path) -> Self {
        Self {
            agents: RefCell::new(
                rows.iter()
                    .filter(|r| !r.exited)
                    .map(|r| r.info.clone())
                    .collect(),
            ),
            calls: RefCell::default(),
            stubborn: Vec::new(),
            staying: Vec::new(),
            drafts: Vec::new(),
            record: record.to_path_buf(),
            marked_first: RefCell::new(true),
        }
    }

    fn check_marks(&self) {
        let marked = restart::read_at(&self.record)
            .panes
            .values()
            .filter(|entry| entry.paused.is_some())
            .count();
        if marked < 5 {
            *self.marked_first.borrow_mut() = false;
        }
    }
}

impl Panes for Sim {
    fn agents(&self) -> Result<Vec<AgentInfo>> {
        Ok(self.agents.borrow().clone())
    }

    fn send_keys(&self, pane_id: &str, keys: &[&str]) -> Result<()> {
        self.check_marks();
        self.calls
            .borrow_mut()
            .push(format!("{pane_id} keys {}", keys.join(" ")));
        if keys == ["esc"] && !self.stubborn.contains(&pane_id) {
            for agent in self.agents.borrow_mut().iter_mut() {
                if agent.pane_id == pane_id {
                    agent.state = AgentState::Idle;
                }
            }
        }
        Ok(())
    }

    fn prompt(&self, pane_id: &str, text: &str) -> Result<()> {
        self.check_marks();
        self.calls
            .borrow_mut()
            .push(format!("{pane_id} types {text}"));
        let exits = (text == "/exit" || text == "/quit") && !self.staying.contains(&pane_id);
        if exits {
            // For a moment the agent stays listed with no harness.
            for agent in self.agents.borrow_mut().iter_mut() {
                if agent.pane_id == pane_id {
                    agent.agent = None;
                    agent.agent_session = None;
                }
            }
        }
        Ok(())
    }

    fn screen(&self, pane_id: &str) -> Option<String> {
        let typed = if self.drafts.contains(&pane_id) {
            "half a thought"
        } else {
            ""
        };
        Some(format!("────\n❯ {typed}\n────\n"))
    }

    fn release(&self, pane_id: &str, kind: &str) {
        self.calls
            .borrow_mut()
            .push(format!("{pane_id} released {kind}"));
    }

    fn wait(&self) {}
}

#[test]
fn working_agents_get_esc_first_then_each_exits_through_its_harness_and_is_kept_paused() {
    let scratch = ScratchDir::new("pause-all");
    let record = scratch.join("markers.launches.json");
    let rows = rows();
    let plan = plan(&rows);
    let sim = Sim::new(&rows, &record);
    let mut progress = Recorded(Vec::new());
    let outcome = pause_with(&sim, &record, &plan.targets, 1_000, &mut progress).unwrap();
    assert!(
        *sim.marked_first.borrow(),
        "every mark is set before any key"
    );
    assert_eq!(
        *sim.calls.borrow(),
        [
            "w1:p1 keys esc",
            "w2:p1 keys esc",
            "w4:p1 keys esc",
            "w3:p1 types /exit",
            "w5:p1 types /exit",
            "w1:p1 types /exit",
            "w2:p1 types /quit",
            "w4:p1 types /quit",
            "w3:p1 released claude",
            "w5:p1 released claude",
            "w1:p1 released claude",
            "w2:p1 released codex",
            "w4:p1 released codex",
        ]
    );
    assert_eq!(outcome.failed, []);
    assert_eq!(outcome.message("Paused"), "Paused 5 agents");
    let saved = restart::read_at(&record);
    let supervisor = saved.panes["w1:p1"].paused.clone().unwrap();
    assert_eq!(
        supervisor,
        Paused {
            at: 1_000,
            project_root: "/repos/corgi".into(),
            supervisor: true,
            resuming: false
        }
    );
    // Taken in though Corgi did not launch it: it resumes its session.
    let scratch_session = &saved.panes["w5:p1"];
    assert_eq!(
        (
            scratch_session.kind.as_str(),
            scratch_session.session.as_deref()
        ),
        ("claude", Some(S1))
    );
    assert_eq!(scratch_session.cwd, Path::new("/repos/Scratch"));
    assert_eq!(saved.panes.len(), 5, "nothing else is marked");
}

#[test]
fn an_agent_that_does_not_stop_is_left_running_unmarked_and_named() {
    let scratch = ScratchDir::new("pause-stubborn");
    let record = scratch.join("markers.launches.json");
    // One Corgi launched, whose launch options must survive the pause.
    restart::update(&record, |r| {
        r.panes.insert(
            "w3:p1".into(),
            Launched {
                kind: "claude".into(),
                name: "fix-cache".into(),
                cwd: "/repos/corgi".into(),
                options: vec!["--model".into(), "opus".into()],
                session: Some(S3.into()),
                ..Launched::default()
            },
        );
    })
    .unwrap();
    let rows = rows();
    let plan = plan(&rows);
    let mut sim = Sim::new(&rows, &record);
    sim.stubborn = vec!["w1:p1"];
    sim.staying = vec!["w2:p1"];
    sim.drafts = vec!["w5:p1"];
    let outcome = pause_with(
        &sim,
        &record,
        &plan.targets,
        1_000,
        &mut Recorded(Vec::new()),
    )
    .unwrap();
    assert_eq!(
        outcome.failed,
        [
            ("w1-1".to_string(), "kept working after Esc".to_string()),
            (
                "w5-1".to_string(),
                "has unsent text in its input box".to_string()
            ),
            ("w2-1".to_string(), "did not exit".to_string()),
        ]
    );
    assert!(
        !sim.calls
            .borrow()
            .iter()
            .any(|call| call.starts_with("w1:p1 types") || call.starts_with("w5:p1 types")),
        "nothing typed into a working agent or a draft: {:?}",
        sim.calls.borrow()
    );
    let saved = restart::read_at(&record);
    let paused: Vec<&str> = saved
        .panes
        .iter()
        .filter(|(_, entry)| entry.paused.is_some())
        .map(|(pane, _)| pane.as_str())
        .collect();
    assert_eq!(paused, ["w3:p1", "w4:p1"]);
    assert_eq!(saved.panes["w3:p1"].options, ["--model", "opus"]);
    assert_eq!(saved.panes["w3:p1"].name, "fix-cache");
    assert_eq!(
        outcome.message("Paused"),
        "Paused 2 agents; not 3 agents: w1-1 kept working after Esc; w5-1 has unsent text in its input box; w2-1 did not exit"
    );
}

fn marked(resuming: bool, at: u64) -> Launched {
    Launched {
        kind: "claude".into(),
        session: Some(S1.into()),
        paused: Some(Paused {
            at,
            project_root: "/repos/corgi".into(),
            supervisor: false,
            resuming,
        }),
        ..Launched::default()
    }
}

#[test]
fn a_mark_goes_once_its_agent_is_back_after_resume_or_long_after_a_pause_that_missed_it() {
    let mut record = Record::default();
    record.panes.insert("back".into(), marked(true, 1_000));
    record.panes.insert("starting".into(), marked(true, 1_000));
    record.panes.insert("shell".into(), marked(false, 1_000));
    record.panes.insert("exiting".into(), marked(false, 1_000));
    record
        .panes
        .insert("missed".into(), marked(false, 1_000 - STALE_SECS - 1));
    let live: HashSet<&str> = ["back", "exiting", "missed"].into();
    assert!(settle_record(&mut record, &live, 1_000));
    let left: Vec<&str> = record
        .panes
        .iter()
        .filter(|(_, entry)| entry.paused.is_some())
        .map(|(pane, _)| pane.as_str())
        .collect();
    assert_eq!(left, ["exiting", "shell", "starting"]);
    assert!(
        !settle_record(&mut record, &live, 1_000),
        "nothing more to write"
    );
    let holds = Holds::of(&record);
    assert_eq!(holds.panes.len(), 3);
    assert!(holds.roots.is_empty(), "no supervisor among them");
}

#[test]
fn resume_all_starts_only_the_agents_pause_all_stopped_each_in_its_pane() {
    let scratch = ScratchDir::new("resume-all");
    let cwd = scratch.join("repo");
    fs::create_dir_all(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    let cwd_text = cwd.to_string_lossy().into_owned();
    let panes = json!([
        {"pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1"},
        {"pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t1"},
    ]);
    let (socket, server) = fake_herdr("resume-all", {
        let cwd_text = cwd_text.clone();
        move |listener| {
            let mut requests: Vec<Value> = Vec::new();
            let snapshot = json!({"result": {"type": "session_snapshot", "snapshot": {
                "panes": panes, "agents": [],
            }}});
            for reply in [
                snapshot.clone(),
                snapshot,
                json!({"result": {"type": "pane_process_info", "process_info": {
                    "pane_id": "w1:p1", "shell_pid": 10, "foreground_process_group_id": 10,
                    "foreground_processes": [{"pid": 10, "name": "zsh", "argv": ["zsh"], "cwd": cwd_text}],
                }}}),
                json!({"result": {"type": "agent_started", "argv": [], "agent": {
                    "pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1",
                }}}),
            ] {
                answer(&listener, |request| {
                    requests.push(request.clone());
                    reply
                });
            }
            requests
        }
    });
    let client =
        HerdrClient::from_socket_path(&socket).with_marker_record(scratch.join("markers.json"));
    let path = restart::record_path(&client).unwrap();
    restart::update(&path, |record| {
        let mut paused = marked(false, 1_000);
        paused.cwd = cwd.clone();
        paused.name = "fix-cache".into();
        paused.options = vec!["--model".into(), "opus".into()];
        record.panes.insert("w1:p1".into(), paused);
        // Exited some other way: Resume all leaves it be.
        record.panes.insert(
            "w2:p1".into(),
            Launched {
                paused: None,
                ..marked(false, 1_000)
            },
        );
    })
    .unwrap();
    let outcome = resume_all(&client, &mut Recorded(Vec::new())).unwrap();
    assert_eq!(outcome.done, ["fix-cache"]);
    assert_eq!(outcome.message("Resumed"), "Resumed 1 agent");
    let requests = server.join().unwrap();
    let starts: Vec<&Value> = requests
        .iter()
        .filter(|request| request["method"] == "agent.start")
        .collect();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0]["params"]["pane_id"], "w1:p1");
    assert_eq!(
        starts[0]["params"]["args"],
        json!(["--resume", S1, "--model", "opus"])
    );
    let saved = restart::read(&client);
    assert!(
        saved.panes["w1:p1"]
            .paused
            .as_ref()
            .is_some_and(|p| p.resuming),
        "held until a pass sees it back"
    );
    assert!(saved.panes["w2:p1"].paused.is_none());
    let _ = fs::remove_file(&socket);
}

#[test]
fn resume_all_refuses_when_nothing_is_paused() {
    let scratch = ScratchDir::new("resume-none");
    let (socket, server) = fake_herdr("resume-none", |listener| {
        answer(
            &listener,
            |_| json!({"result": {"type": "session_snapshot", "snapshot": {"panes": [], "agents": []}}}),
        );
    });
    let client =
        HerdrClient::from_socket_path(&socket).with_marker_record(scratch.join("markers.json"));
    let error = resume_all(&client, &mut Recorded(Vec::new())).unwrap_err();
    assert_eq!(format!("{error:#}"), "No agent is paused");
    server.join().unwrap();
    let _ = fs::remove_file(&socket);
}

#[test]
fn paused_agents_stay_paused_across_a_herdr_restart() {
    let scratch = ScratchDir::new("pause-restart");
    let client = HerdrClient::from_socket_path(crate::test_support::UNUSED_SOCKET)
        .with_marker_record(scratch.join("markers.json"));
    let path = restart::record_path(&client).unwrap();
    restart::update(&path, |record| {
        let mut supervisor = marked(false, 1_000);
        supervisor.terminal = Some("t-before".into());
        supervisor.processes = vec![41];
        if let Some(paused) = supervisor.paused.as_mut() {
            paused.supervisor = true;
        }
        record.panes.insert("w1:p1".into(), supervisor);
        record.panes.insert("w2:p1".into(), marked(false, 1_000));
    })
    .unwrap();
    // The server came back: the same panes, each at its shell, and the
    // startup hook's pass reconciles them. Nothing asks Herdr anything
    // (there is no server here), nothing is resumed, and the marks stay.
    let snapshot: crate::herdr::SessionSnapshot = serde_json::from_value(json!({
        "version": "0.9.5",
        "panes": [
            {"pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1"},
            {"pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t1"},
        ],
        "agents": [],
    }))
    .unwrap();
    assert_eq!(restart::reconcile(&client, &snapshot), None);
    let record = restart::read(&client);
    assert_eq!(
        paused(&record, &snapshot).len(),
        2,
        "Resume all still has both"
    );
    let holds = Holds::of(&record);
    assert_eq!(holds.panes.len(), 2);
    assert!(holds.roots.contains("/repos/corgi"));
    settle(&client, &[], 1_000 + 10 * STALE_SECS);
    assert_eq!(
        Holds::read(&client).panes.len(),
        2,
        "no agent back, no mark gone"
    );
}
