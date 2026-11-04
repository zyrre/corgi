//! `corgi serve` against a fake Herdr that answers every request from a
//! snapshot the test changes as it goes, and records what it was asked. The
//! server runs as `corgi serve` does, its lines read back as the protocol's
//! types; nothing touches the developer's own Corgi state, network or
//! harnesses, and no agent CLI starts.

use std::{
    collections::HashMap,
    fs,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use corgi_core::{
    harness::Harness,
    herdr::HerdrClient,
    inbox::{Inbox, Kind},
    projects::ProjectMemory,
    protocol::{
        BacklogEntry, Change, Command as Wire, MergeOutcome, PaletteSet, Question, Reply,
        ReplyResult, ServerMessage, State, ThemeColors,
    },
    test_support::{ScratchDir, fake_herdr},
    waker::SupervisorWaker,
};
use serde_json::{Value, json};

use super::{
    Event, Options, Output, Server,
    forge::{self, ForgeCli},
};

/// A Herdr that answers every connection until it is dropped.
struct FakeHerdr {
    socket: PathBuf,
    snapshot: Arc<Mutex<Value>>,
    /// Methods made to fail, with the error code they fail with.
    failing: Arc<Mutex<HashMap<String, String>>>,
    calls: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeHerdr {
    fn start(label: &str, snapshot: Value) -> Self {
        let snapshot = Arc::new(Mutex::new(snapshot));
        let failing: Arc<Mutex<HashMap<String, String>>> = Arc::default();
        let calls: Arc<Mutex<Vec<Value>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (socket, thread) = fake_herdr(label, {
            let (snapshot, failing, calls, stop) = (
                snapshot.clone(),
                failing.clone(),
                calls.clone(),
                stop.clone(),
            );
            move |listener: UnixListener| {
                listener
                    .set_nonblocking(true)
                    .expect("non-blocking listener");
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            // macOS hands on the listener's O_NONBLOCK.
                            stream.set_nonblocking(false).expect("blocking stream");
                            let mut reader = BufReader::new(stream);
                            let mut line = String::new();
                            if reader.read_line(&mut line).is_err() || line.is_empty() {
                                continue;
                            }
                            let request: Value = serde_json::from_str(&line).expect("request");
                            calls.lock().unwrap().push(request.clone());
                            let method = request["method"].as_str().unwrap_or_default();
                            let mut response = match failing.lock().unwrap().get(method) {
                                Some(code) => json!({"error": {"code": code, "message": code}}),
                                None => {
                                    respond(method, &request["params"], &snapshot.lock().unwrap())
                                }
                            };
                            response["id"] = request["id"].clone();
                            let stream = reader.get_mut();
                            let _ = serde_json::to_writer(&mut *stream, &response);
                            let _ = stream.write_all(b"\n");
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("accept: {error}"),
                    }
                }
            }
        });
        Self {
            socket,
            snapshot,
            failing,
            calls,
            stop,
            thread: Some(thread),
        }
    }

    fn set_snapshot(&self, snapshot: Value) {
        *self.snapshot.lock().unwrap() = snapshot;
    }

    fn fail(&self, method: &str, code: &str) {
        self.failing
            .lock()
            .unwrap()
            .insert(method.into(), code.into());
    }

    fn heal(&self, method: &str) {
        self.failing.lock().unwrap().remove(method);
    }

    /// The parameters of every request for `method`, oldest first.
    fn calls(&self, method: &str) -> Vec<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request["method"] == method)
            .map(|request| request["params"].clone())
            .collect()
    }
}

impl Drop for FakeHerdr {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.socket);
    }
}

/// What the fake Herdr answers `method` with: the snapshot, or the result
/// each of Corgi's calls expects.
fn respond(method: &str, params: &Value, snapshot: &Value) -> Value {
    let result = |result: Value| json!({ "result": result });
    let agent = |pane: &Value| json!({"pane_id": pane, "workspace_id": "w", "tab_id": "t"});
    match method {
        "session.snapshot" => result(json!({"type": "session_snapshot", "snapshot": snapshot})),
        "agent.prompt" => {
            result(json!({"type": "agent_prompted", "agent": agent(&params["target"])}))
        }
        "agent.focus" => result(json!({"type": "agent_info", "agent": agent(&params["target"])})),
        "workspace.focus" => result(json!({"type": "workspace_info"})),
        "tab.focus" => result(json!({"type": "tab_info"})),
        "pane.report_metadata" => result(json!({"type": "pane_metadata_updated"})),
        "workspace.report_metadata" => result(json!({"type": "workspace_metadata_updated"})),
        "workspace.create" => result(json!({
            "type": "workspace_created",
            "workspace": {"workspace_id": "w9", "label": "scratch 2"},
            "tab": {"tab_id": "w9:t1"},
            "root_pane": {"pane_id": "w9:p1"}
        })),
        "agent.start" => result(json!({
            "type": "agent_started",
            "agent": {
                "agent": "claude", "name": "scratch", "pane_id": "w9:p1",
                "workspace_id": "w9", "tab_id": "w9:t1", "cwd": params["cwd"]
            },
            "argv": ["claude"]
        })),
        "tab.create" => result(json!({
            "type": "tab_created", "tab": {"tab_id": "w2:t9"}, "root_pane": {"pane_id": "w2:p9"}
        })),
        "pane.send_input" => result(json!({"type": "pane_input_sent"})),
        "worktree.remove" => result(json!({"type": "worktree_removed"})),
        "workspace.close" => result(json!({"type": "workspace_closed"})),
        _ => json!({"error": {"code": "unsupported", "message": format!("fake Herdr: {method}")}}),
    }
}

/// A server on the fake Herdr, its lines read back as messages.
struct Session {
    input: Sender<Event>,
    output: Receiver<ServerMessage>,
    thread: Option<JoinHandle<()>>,
    next_id: u64,
}

/// Collects the server's output into lines, each parsed as a message.
struct Lines {
    buffer: Vec<u8>,
    sender: Sender<ServerMessage>,
}

impl Write for Lines {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        while let Some(end) = self.buffer.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            let message = serde_json::from_slice(&line).unwrap_or_else(|error| {
                panic!(
                    "not a protocol line ({error}): {}",
                    String::from_utf8_lossy(&line)
                )
            });
            let _ = self.sender.send(message);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn fixed_palette() -> PaletteSet {
    let colors = ThemeColors {
        foreground: "#ffffff".into(),
        background: "#000000".into(),
        ..ThemeColors::default()
    };
    PaletteSet {
        source: "test".into(),
        light: colors.clone(),
        dark: colors,
    }
}

impl Session {
    /// A server on `herdr` keeping its state, inboxes and selection in
    /// `scratch`, refreshing only when asked.
    fn start(herdr: &FakeHerdr, scratch: &Path) -> Self {
        Self::start_with_forge(herdr, scratch, None)
    }

    /// The same, asking the forges through `forge`.
    fn start_with_forge(herdr: &FakeHerdr, scratch: &Path, forge: Option<ForgeCli>) -> Self {
        let client = HerdrClient::from_socket_path(&herdr.socket)
            .with_marker_record(scratch.join("markers.json"));
        let inboxes = SupervisorWaker::in_dir(&scratch.join("state")).inboxes();
        let options = Options {
            client,
            waker: None,
            inboxes,
            providers: Vec::new(),
            codex_titles: false,
            projects: Some(ProjectMemory::default()),
            installed: vec![Harness::Claude, Harness::Codex],
            discover_models: false,
            selection_file: Some(scratch.join("selection")),
            palette: fixed_palette,
            log: None,
            refresh_interval: Duration::from_secs(3600),
            forge,
        };
        let (events, receiver) = mpsc::channel();
        let (sender, output) = mpsc::channel();
        let writer = Output::new(Box::new(Lines {
            buffer: Vec::new(),
            sender,
        }));
        let server = Server::new(options, writer, events.clone(), receiver);
        Self {
            input: events,
            output,
            thread: Some(thread::spawn(move || server.run())),
            next_id: 0,
        }
    }

    fn next(&self) -> ServerMessage {
        self.output
            .recv_timeout(Duration::from_secs(20))
            .expect("a message from corgi serve")
    }

    /// Sends `command` (its fields, without an id) and returns its reply and
    /// what came before it.
    fn ask(&mut self, mut command: Value) -> (Reply, Vec<ServerMessage>) {
        self.next_id += 1;
        command["id"] = json!(self.next_id);
        self.input
            .send(Event::Line(command.to_string()))
            .expect("server running");
        let mut before = Vec::new();
        loop {
            match self.next() {
                ServerMessage::Reply(reply) if reply.id == Some(self.next_id) => {
                    return (reply, before);
                }
                other => before.push(other),
            }
        }
    }

    /// The reply's result, or a panic with its error.
    fn ok(&mut self, command: Value) -> (ReplyResult, Vec<ServerMessage>) {
        let (reply, before) = self.ask(command);
        assert!(reply.ok, "refused: {reply:?}");
        (reply.result.expect("a result"), before)
    }

    /// The reply's error.
    fn refused(&mut self, command: Value) -> Reply {
        let (reply, _) = self.ask(command);
        assert!(!reply.ok, "not refused: {reply:?}");
        reply
    }

    /// Asks for a refresh, and returns the delta it brings, if any.
    fn refresh(&mut self) -> Option<Vec<Change>> {
        self.ok(json!({"command": "refresh"}));
        loop {
            match self.output.recv_timeout(Duration::from_millis(1500)) {
                Ok(ServerMessage::Delta(delta)) => return Some(delta.changes),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
    }

    /// Waits for the delta whose changes `wanted` accepts.
    fn delta_with(&self, wanted: impl Fn(&[Change]) -> bool) -> Vec<Change> {
        loop {
            if let ServerMessage::Delta(delta) = self.next()
                && wanted(&delta.changes)
            {
                return delta.changes;
            }
        }
    }

    fn close(mut self) {
        let _ = self.input.send(Event::InputClosed);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("server panicked");
        }
    }
}

/// Reads the hello and the first snapshot.
fn opening(session: &Session) -> State {
    let ServerMessage::Hello(hello) = session.next() else {
        panic!("the first line is not the hello")
    };
    assert_eq!(hello.protocol, corgi_core::protocol::PROTOCOL_VERSION);
    assert_eq!(hello.commands, Wire::catalog());
    let ServerMessage::Snapshot(snapshot) = session.next() else {
        panic!("the second line is not the snapshot")
    };
    assert_eq!(snapshot.seq, 0);
    snapshot.state
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A repository `weather` on `main`, pushed to a bare remote, and a linked
/// worktree on `worktree/fix` with one commit the agent made.
fn repository(scratch: &Path) -> (PathBuf, PathBuf) {
    let repo = scratch.join("weather");
    let worktree = scratch.join("weather-fix");
    let remote = scratch.join("remote.git");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "-q", "--bare"]);
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "Corgi test"]);
    git(&repo, &["config", "user.email", "corgi@example.test"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    fs::write(repo.join("status.txt"), "base\n").unwrap();
    git(&repo, &["add", "status.txt"]);
    git(&repo, &["commit", "-q", "-m", "initial"]);
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "--set-upstream", "origin", "main"]);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "worktree/fix",
            worktree.to_str().unwrap(),
        ],
    );
    fs::write(worktree.join("status.txt"), "fixed\n").unwrap();
    git(&worktree, &["commit", "-q", "-am", "Fix the weather"]);
    (repo, worktree)
}

/// Herdr's snapshot: the weather project's supervisor and its worker in
/// `worker_state`, and a scratch agent in the home directory.
fn herd(repo: &Path, worktree: &Path, worker_state: &str) -> Value {
    let home = corgi_core::paths::home().expect("HOME");
    let repo = repo.to_str().unwrap();
    let worktree = worktree.to_str().unwrap();
    let session = |value: &str| json!({"agent": "claude", "kind": "session", "source": "herdr", "value": value});
    json!({
        "workspaces": [
            {"workspace_id": "w1", "label": "weather", "worktree": {
                "repo_name": "weather", "repo_root": repo, "checkout_path": repo,
                "is_linked_worktree": false}},
            {"workspace_id": "w2", "label": "weather-fix", "worktree": {
                "repo_name": "weather", "repo_root": repo, "checkout_path": worktree,
                "is_linked_worktree": true}},
            {"workspace_id": "w3", "label": "scratch"}
        ],
        "agents": [
            {"agent": "claude", "name": "supervisor-weather", "agent_status": "idle",
             "pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1", "cwd": repo,
             "state_change_seq": 2, "tokens": {"corgi_handler": "supervisor-weather"},
             "agent_session": session("s-sup")},
            {"agent": "codex", "name": "w-fix", "agent_status": worker_state,
             "pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t1", "cwd": worktree,
             "state_change_seq": 5, "agent_session": session("s-fix")},
            {"agent": "claude", "name": "scratch", "agent_status": "idle",
             "pane_id": "w3:p1", "workspace_id": "w3", "tab_id": "w3:t1",
             "cwd": home.to_str().unwrap(), "state_change_seq": 1,
             "agent_session": session("s-scratch")}
        ]
    })
}

/// The ledger of the weather supervisor, with one question open.
fn ledger(scratch: &Path) {
    let dir = scratch.join("state").join("weather");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("ledger.jsonl"),
        concat!(
            r#"{"id":"20261028-w-fix","agent":"w-fix","status":"needs-answer","questions":["Q4: Keep the old flag?"]}"#,
            "\n"
        ),
    )
    .unwrap();
}

#[test]
fn the_first_lines_are_the_hello_and_a_snapshot_of_rows_cards_and_questions() {
    let scratch = ScratchDir::new("serve-snapshot");
    let (repo, worktree) = repository(&scratch);
    ledger(&scratch);
    let herdr = FakeHerdr::start("serve-snap", herd(&repo, &worktree, "blocked"));
    let session = Session::start(&herdr, &scratch);
    let state = opening(&session);

    assert!(state.herdr.connected);
    assert_eq!(state.palette, fixed_palette());
    let panes: Vec<&str> = state.agents.iter().map(|a| a.pane_id.as_str()).collect();
    // The supervisor leads its project; scratch sessions come last.
    assert_eq!(panes, ["w1:p1", "w2:p1", "w3:p1"]);
    let worker = serde_json::to_value(&state.agents[1]).unwrap();
    assert_eq!(
        worker,
        json!({
            "pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t1",
            "name": "w-fix", "herdr_name": "w-fix", "harness": "codex",
            "state": "blocked", "state_change_seq": 5, "ready_to_merge": false,
            "focused": false, "supervisor": false, "scratch": false,
            "project_group": "weather", "project": "weather/weather-fix",
            "project_root": repo.to_str().unwrap(),
            "worktree": worktree.to_str().unwrap(), "checkout": "weather-fix",
            "task": worker["task"], "model": null, "effort": null,
            "context_percent": null, "context_tokens": null, "cache": null,
            "last_activity": null,
            "message": {"kind": "question", "text": "Waiting for your input"},
            "tool": {"kind": "ready", "text": "No command yet"},
            "can_prompt": false, "can_merge": true
        })
    );
    assert!(state.agents[0].supervisor);
    assert!(state.agents[2].scratch);

    let projects = serde_json::to_value(&state.projects).unwrap();
    assert_eq!(
        projects[0],
        json!({
            "group": "weather", "root": repo.to_str().unwrap(), "card": true,
            "scratch": false, "supervisor": "w1:p1", "agents": ["w1:p1", "w2:p1"],
            "workers": {"total": 1, "blocked": 1, "merge": 0, "working": 0,
                        "done": 0, "idle": 0, "unknown": 0},
            "blocked": [{"pane_id": "w2:p1", "task": worker["task"],
                         "waiting_on": "Waiting for your input"}],
            "questions": [{"id": "Q4", "title": "Keep the old flag?", "agent": "w-fix"}],
            "held": 0
        })
    );
    assert_eq!(projects[1]["scratch"], true);
    assert_eq!(projects[1]["card"], false);
    assert_eq!(projects[1]["root"], "");
    session.close();
}

#[test]
fn a_project_carries_why_its_supervisor_is_refused_wakes_and_what_waits() {
    let scratch = ScratchDir::new("serve-refused");
    let state = scratch.join("state");
    let inboxes = SupervisorWaker::in_dir(&state).inboxes();
    let client = HerdrClient::from_socket_path(scratch.join("no-socket"))
        .with_marker_record(scratch.join("markers.json"));
    inboxes
        .notify(corgi_core::inbox::Item {
            project: "/repos/weather".into(),
            source: "notify".into(),
            kind: corgi_core::inbox::Kind::Note,
            key: "a-wake".into(),
            text: "[Corgi] a wake".into(),
            ..Default::default()
        })
        .unwrap();
    let mut agents = vec![corgi_core::model::DashboardAgent {
        project_group: "weather".into(),
        project_root: "/repos/weather".into(),
        supervisor: true,
        ..Default::default()
    }];

    let projects = super::state::projects(&agents, &inboxes, &client);
    assert_eq!(projects[0].refused, None);
    assert!(
        serde_json::to_value(&projects[0])
            .unwrap()
            .get("refused")
            .is_none(),
        "an older window reads the same project as ever"
    );

    agents[0].delivery_refused = Some("held until continued".into());
    let projects = super::state::projects(&agents, &inboxes, &client);
    assert_eq!(
        projects[0].refused,
        Some(corgi_core::protocol::Refused {
            reason: "held until continued".into(),
            waiting: 1,
        })
    );
}

#[test]
fn a_question_about_a_plan_is_listed_with_its_text_and_no_worker() {
    let scratch = ScratchDir::new("serve-plan-question");
    let (repo, worktree) = repository(&scratch);
    let dir = scratch.join("state").join("weather");
    fs::create_dir_all(dir.join("questions")).unwrap();
    fs::write(
        dir.join("ledger.jsonl"),
        concat!(
            r#"{"id":"20261102-plan-mrs","agent":"","status":"needs-answer","questions":["Q52: Which MRs?"],"summary":"Plan: MRs panel"}"#,
            "\n"
        ),
    )
    .unwrap();
    let asked = "### Q52: Which MRs?   [blocks: plan \"MRs panel\"]\n\
                 Context: none yet.\n\
                 Options:\n  A. Mine.\n  B. All.\n\
                 Recommendation: A.\n\
                 Until answered: no brief.";
    fs::write(dir.join("questions").join("Q52.md"), asked).unwrap();
    let herdr = FakeHerdr::start("serve-planq", herd(&repo, &worktree, "idle"));
    let session = Session::start(&herdr, &scratch);
    let state = opening(&session);
    assert_eq!(
        state.projects[0].questions,
        [Question {
            id: "Q52".into(),
            title: "Which MRs?".into(),
            agent: String::new(),
            text: asked.into(),
        }]
    );
    session.close();
}

#[test]
fn an_open_questions_file_is_its_text_and_others_are_ignored() {
    let scratch = ScratchDir::new("serve-question-text");
    let (repo, worktree) = repository(&scratch);
    ledger(&scratch);
    let questions = scratch.join("state").join("weather").join("questions");
    fs::create_dir_all(&questions).unwrap();
    let asked = "### Q4: Keep the old flag?   [blocks: plan \"flags\"]\n\
                 Context: the flag is read in one place.\n\
                 Options:\n  A. Keep it.\n  B. Drop it.\n\
                 Recommendation: B.\n\
                 Until answered: no brief.";
    fs::write(questions.join("Q4.md"), format!("{asked}\n\n")).unwrap();
    // Q9 is not open in the ledger: its file is left alone.
    fs::write(questions.join("Q9.md"), "### Q9: Gone?").unwrap();
    let herdr = FakeHerdr::start("serve-qtext", herd(&repo, &worktree, "idle"));
    let mut session = Session::start(&herdr, &scratch);
    let state = opening(&session);
    let open = &state.projects[0].questions;
    assert_eq!(open.len(), 1);
    assert_eq!((open[0].id.as_str(), open[0].text.as_str()), ("Q4", asked));

    // The supervisor deletes the file: the question keeps only its title.
    fs::remove_file(questions.join("Q4.md")).unwrap();
    let changes = session.refresh().expect("a delta");
    let project = changes.iter().find_map(|change| match change {
        Change::Project(project) => Some(project),
        _ => None,
    });
    let open = &project.expect("the project's card").questions;
    assert_eq!(open[0].title, "Keep the old flag?");
    assert_eq!(open[0].text, "");
    session.close();
}

#[test]
fn the_supervisors_backlog_is_sent_with_its_project_and_follows_the_file() {
    let scratch = ScratchDir::new("serve-backlog");
    let (repo, worktree) = repository(&scratch);
    let dir = scratch.join("state").join("weather");
    fs::create_dir_all(&dir).unwrap();
    let herdr = FakeHerdr::start("serve-backlog", herd(&repo, &worktree, "idle"));
    let mut session = Session::start(&herdr, &scratch);
    let state = opening(&session);
    // No file: no backlog, and none on the wire.
    assert!(state.projects[0].backlog.is_empty());
    let project = serde_json::to_value(&state.projects[0]).unwrap();
    assert!(project.get("backlog").is_none(), "{project}");

    fs::write(
        dir.join("backlog.md"),
        "# Backlog: things to build later\nRaised by the user.\n\n\
         ## 1. \"Stop all agents\" button\nFor when the user *leaves*.\n\n\
         ## Merge wake after a handover (parked)\nSeen once.\n",
    )
    .unwrap();
    let changes = session.refresh().expect("a delta");
    let project = changes
        .iter()
        .find_map(|change| match change {
            Change::Project(project) => Some(project),
            _ => None,
        })
        .expect("the project's card");
    assert_eq!(
        project.backlog,
        [
            BacklogEntry {
                heading: "1. \"Stop all agents\" button".into(),
                text: "For when the user *leaves*.".into(),
            },
            BacklogEntry {
                heading: "Merge wake after a handover (parked)".into(),
                text: "Seen once.".into(),
            },
        ]
    );
    session.close();
}

#[test]
fn a_backlog_message_queues_one_line_per_message_for_the_supervisor() {
    let scratch = ScratchDir::new("serve-backlog-message");
    let (repo, worktree) = repository(&scratch);
    let herdr = FakeHerdr::start("serve-blmsg", herd(&repo, &worktree, "idle"));
    let mut session = Session::start(&herdr, &scratch);
    opening(&session);
    let root = repo.to_str().unwrap();
    let pending = || {
        let inbox = Inbox::read(&scratch.join("state").join("weather"));
        let items = inbox.undelivered_for(root);
        assert!(items.iter().all(|item| item.kind == Kind::Note));
        corgi_core::inbox::prompt_text(&items)
    };
    let send = |text: &str| {
        json!({"command": "backlog_message", "project_root": root,
               "heading": "1. Stop all agents", "text": text})
    };

    let error = session.refused(send("  \n ")).error.unwrap();
    assert_eq!(error, "The message cannot be empty");
    assert_eq!(pending(), "");

    // Through the inbox, which the waker types in between turns.
    let (result, _) = session.ok(send(" Do this one next. "));
    assert_eq!(
        result,
        ReplyResult::Done {
            message: "Sent to supervisor-weather".into()
        }
    );
    assert!(herdr.calls("agent.prompt").is_empty());
    let first =
        "[Corgi] The user wrote about backlog entry \"1. Stop all agents\": Do this one next.";
    assert_eq!(pending(), first);

    // Without a supervisor it waits for the next one, and a second message
    // about the same entry does not replace the first.
    let mut alone = herd(&repo, &worktree, "idle");
    alone["agents"].as_array_mut().unwrap().remove(0);
    herdr.set_snapshot(alone);
    session.refresh();
    let (result, _) = session.ok(send("And keep it small."));
    assert_eq!(
        result,
        ReplyResult::Done {
            message: "Sent; the supervisor reads it when it starts".into()
        }
    );
    assert_eq!(
        pending(),
        format!(
            "{first}\n[Corgi] The user wrote about backlog entry \"1. Stop all agents\": And keep it small."
        )
    );
    assert!(herdr.calls("agent.prompt").is_empty());
    session.close();
}

#[test]
fn a_question_file_is_read_up_to_its_cap_and_only_when_plain() {
    use super::state::{QUESTION_TEXT_BYTES, question_text};
    let scratch = ScratchDir::new("serve-question-cap");
    let cap = QUESTION_TEXT_BYTES as usize;
    let file = scratch.join("Q1.md");
    // A two-byte character straddles the cap.
    fs::write(&file, format!("{}é and more", "x".repeat(cap - 1))).unwrap();
    assert_eq!(question_text(&file), format!("{}…", "x".repeat(cap - 1)));
    fs::write(&file, "x".repeat(cap)).unwrap();
    assert_eq!(question_text(&file), "x".repeat(cap));
    assert_eq!(question_text(&scratch.join("missing.md")), "");
    let link = scratch.join("Q2.md");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert_eq!(question_text(&link), "");
    fs::create_dir(scratch.join("Q3.md")).unwrap();
    assert_eq!(question_text(&scratch.join("Q3.md")), "");
}

#[test]
fn a_refresh_sends_only_what_changed_and_resync_the_whole_state() {
    let scratch = ScratchDir::new("serve-delta");
    let (repo, worktree) = repository(&scratch);
    let herdr = FakeHerdr::start("serve-delta", herd(&repo, &worktree, "working"));
    let mut session = Session::start(&herdr, &scratch);
    let state = opening(&session);
    assert_eq!(state.projects[0].workers.working, 1);
    // A frontend's copy, kept by applying every delta.
    let mut copy = state.clone();

    // Nothing changed, nothing is sent.
    assert_eq!(session.refresh(), None);

    // The worker finishes and the scratch agent goes.
    let mut snapshot = herd(&repo, &worktree, "done");
    snapshot["agents"].as_array_mut().unwrap().pop();
    herdr.set_snapshot(snapshot);
    let changes = session.refresh().expect("a delta");
    copy.apply(changes.clone());
    let worker = changes.iter().find_map(|change| match change {
        Change::Agent(agent) => Some(agent),
        _ => None,
    });
    assert_eq!(worker.expect("the worker's row").state.label(), "DONE");
    assert!(changes.contains(&Change::AgentRemoved {
        pane_id: "w3:p1".into()
    }));
    assert!(changes.contains(&Change::AgentOrder {
        pane_ids: vec!["w1:p1".into(), "w2:p1".into()]
    }));
    assert!(changes.contains(&Change::ProjectRemoved {
        group: "Scratch".into()
    }));
    assert!(changes.iter().any(|change| matches!(
        change,
        Change::Project(project) if project.workers.done == 1
    )));

    // Herdr goes away: the rows stay, marked disconnected.
    herdr.fail("session.snapshot", "server_not_running");
    let changes = session.refresh().expect("a delta");
    assert!(matches!(
        changes.as_slice(),
        [Change::Herdr(status)] if !status.connected && status.error.is_some()
    ));
    copy.apply(changes);
    herdr.heal("session.snapshot");
    let changes = session.refresh().expect("a delta");
    assert!(matches!(changes.as_slice(), [Change::Herdr(status)] if status.connected));
    copy.apply(changes);

    // A resync is the whole state, next in sequence.
    let (_, before) = session.ok(json!({"command": "resync"}));
    let snapshot = before
        .into_iter()
        .find_map(|message| match message {
            ServerMessage::Snapshot(snapshot) => Some(snapshot),
            _ => None,
        })
        .expect("a snapshot");
    assert_eq!(snapshot.seq, 4);
    assert_eq!(snapshot.state.agents.len(), 2);
    // The deltas, applied in order, led to the same state.
    assert_eq!(copy, snapshot.state);
    session.close();
}

#[test]
fn select_watch_and_bad_lines_are_answered_in_place() {
    let scratch = ScratchDir::new("serve-select");
    let (repo, worktree) = repository(&scratch);
    let herdr = FakeHerdr::start("serve-select", herd(&repo, &worktree, "idle"));
    let mut session = Session::start(&herdr, &scratch);
    opening(&session);

    session.ok(json!({"command": "select", "pane_id": "w2:p1"}));
    session.delta_with(|changes| {
        changes.contains(&Change::Selection {
            pane_id: Some("w2:p1".into()),
        })
    });
    assert_eq!(
        fs::read_to_string(scratch.join("selection")).unwrap(),
        "w2:p1"
    );

    session.ok(json!({"command": "watch", "pane_id": "w2:p1"}));
    session.delta_with(|changes| {
        changes.iter().any(|change| {
            matches!(change, Change::Transcript { transcript: Some(t) } if t.pane_id == "w2:p1")
        })
    });

    // A line that is no request is answered, with its id when it has one.
    session.input.send(Event::Line("not json".into())).unwrap();
    let ServerMessage::Reply(reply) = session.next() else {
        panic!("no reply")
    };
    assert_eq!((reply.id, reply.ok), (None, false));
    let reply = session.refused(json!({"command": "fly"}));
    assert!(reply.error.unwrap().contains("unreadable request"));
    let reply = session.refused(json!({"command": "prompt", "pane_id": "w9:p9", "text": "x"}));
    assert_eq!(reply.error.unwrap(), "no agent in pane w9:p9");
    session.close();
}

#[test]
fn prompt_and_answer_type_into_the_agent_and_never_echo_the_text() {
    let scratch = ScratchDir::new("serve-prompt");
    let (repo, worktree) = repository(&scratch);
    let herdr = FakeHerdr::start("serve-prompt", herd(&repo, &worktree, "idle"));
    let mut session = Session::start(&herdr, &scratch);
    opening(&session);

    let (result, _) =
        session.ok(json!({"command": "prompt", "pane_id": "w2:p1", "text": " go on \n"}));
    assert_eq!(
        result,
        ReplyResult::Done {
            message: "Prompt sent to w-fix".into()
        }
    );
    session.ok(
        json!({"command": "answer", "project_root": repo.to_str().unwrap(),
                      "question": "q4", "text": "B"}),
    );
    let prompts = herdr.calls("agent.prompt");
    assert_eq!(prompts[0]["target"], "w2:p1");
    assert_eq!(prompts[0]["text"], "go on");
    assert_eq!(prompts[1]["target"], "w1:p1");
    assert_eq!(prompts[1]["text"], "Q4: B");

    herdr.fail("agent.prompt", "agent_blocked");
    let reply = session.refused(json!({"command": "prompt", "pane_id": "w2:p1",
                                       "text": "secret words"}));
    let error = reply.error.unwrap();
    assert_eq!(error, "Blocked agents must be handled in their own pane");
    herdr.fail("agent.prompt", "timeout");
    let error = session
        .refused(json!({"command": "prompt", "pane_id": "w2:p1", "text": "secret words"}))
        .error
        .unwrap();
    assert_eq!(
        error,
        "Could not confirm prompt delivery (timeout). Check the agent before retrying."
    );

    let error = session
        .refused(json!({"command": "answer", "project_root": "/nowhere",
                        "question": "Q1", "text": "A"}))
        .error
        .unwrap();
    assert!(error.contains("No supervisor runs"), "{error}");
    let error = session
        .refused(
            json!({"command": "answer", "project_root": repo.to_str().unwrap(),
                        "question": "four", "text": "A"}),
        )
        .error
        .unwrap();
    assert!(error.contains("not a question number"), "{error}");
    session.close();
}

#[test]
fn closing_a_question_queues_one_line_for_the_supervisor_running_or_not() {
    let scratch = ScratchDir::new("serve-close-question");
    let (repo, worktree) = repository(&scratch);
    ledger(&scratch);
    let herdr = FakeHerdr::start("serve-closeq", herd(&repo, &worktree, "idle"));
    let mut session = Session::start(&herdr, &scratch);
    opening(&session);
    let root = repo.to_str().unwrap();
    let pending = || {
        let inbox = Inbox::read(&scratch.join("state").join("weather"));
        let items = inbox.undelivered_for(root);
        assert!(items.iter().all(|item| item.kind == Kind::Note));
        corgi_core::inbox::prompt_text(&items)
    };
    let line = "[Corgi] The user closed Q4 without answering. Drop it from the ledger and delete its questions file.";

    let error = session
        .refused(json!({"command": "close_question", "project_root": root, "question": "four"}))
        .error
        .unwrap();
    assert!(error.contains("not a question number"), "{error}");
    let error = session
        .refused(json!({"command": "close_question", "project_root": root, "question": "Q9"}))
        .error
        .unwrap();
    assert_eq!(error, format!("Q9 is not open for {root}"));
    assert_eq!(pending(), "");

    // Through the inbox, which the waker types in between turns: never
    // straight into the supervisor's box, over the user.
    let (result, _) =
        session.ok(json!({"command": "close_question", "project_root": root, "question": "q4"}));
    assert_eq!(
        result,
        ReplyResult::Done {
            message: "Asked supervisor-weather to drop Q4".into()
        }
    );
    assert!(herdr.calls("agent.prompt").is_empty());
    // A second click before it goes out still sends one line.
    session.ok(json!({"command": "close_question", "project_root": root, "question": "Q4"}));
    assert_eq!(pending(), line);

    // Without a supervisor it waits for the next one.
    let mut alone = herd(&repo, &worktree, "idle");
    alone["agents"].as_array_mut().unwrap().remove(0);
    herdr.set_snapshot(alone);
    session.refresh();
    let (result, _) =
        session.ok(json!({"command": "close_question", "project_root": root, "question": "Q4"}));
    assert_eq!(
        result,
        ReplyResult::Done {
            message: "Q4 is dropped once the supervisor starts".into()
        }
    );
    assert_eq!(pending(), line);
    assert!(herdr.calls("agent.prompt").is_empty());
    session.close();
}

#[test]
fn the_new_agent_form_is_preset_and_spawn_checks_it_then_launches() {
    let scratch = ScratchDir::new("serve-spawn");
    let (repo, worktree) = repository(&scratch);
    let herdr = FakeHerdr::start("serve-spawn", herd(&repo, &worktree, "idle"));
    let mut session = Session::start(&herdr, &scratch);
    opening(&session);

    let (result, _) = session.ok(json!({"command": "new_agent_form", "pane_id": "w2:p1",
                                        "harness": "codex"}));
    let ReplyResult::NewAgentForm(form) = result else {
        panic!("not a form: {result:?}")
    };
    assert_eq!(form.project, repo.to_str().unwrap());
    assert_eq!(form.harness, "codex");
    assert!(form.supports_effort);
    assert!(!form.starts_supervisor, "the project has agents");
    assert!(form.harnesses.iter().any(|choice| choice.value == "claude"));
    assert!(!form.models.is_empty());
    assert_eq!(form.checkout, "worktree");
    assert_eq!(form.checkouts.len(), 2);
    assert!(
        form.projects
            .iter()
            .any(|choice| choice.value == form.project)
    );

    // The form's checks, pointing at the row.
    let reply = session.refused(
        json!({"command": "spawn", "project": repo.to_str().unwrap(),
                                       "harness": "claude"}),
    );
    assert_eq!(reply.field.as_deref(), Some("task"));
    let reply = session.refused(json!({"command": "spawn", "project": "relative",
                                       "harness": "claude", "task": "t"}));
    assert_eq!(reply.field.as_deref(), Some("project"));
    let reply = session.refused(
        json!({"command": "spawn", "project": repo.to_str().unwrap(),
                                       "harness": "claude", "task": "t", "checkout": "nope"}),
    );
    assert!(reply.error.unwrap().contains("checkout"));

    // A scratch agent needs no task, so it starts without a prompt.
    let home = corgi_core::paths::home().expect("HOME");
    let (result, before) = session.ok(
        json!({"command": "spawn", "project": home.to_str().unwrap(),
                                             "harness": "claude"}),
    );
    let ReplyResult::Done { message } = result else {
        panic!("{result:?}")
    };
    assert!(message.contains("started"), "{message}");
    let steps = before.iter().find_map(|message| match message {
        ServerMessage::Progress(progress) => progress.steps.clone(),
        _ => None,
    });
    assert_eq!(
        steps.expect("the steps"),
        ["Create a scratch workspace", "Start claude"]
    );
    assert_eq!(herdr.calls("agent.start").len(), 1);
    assert!(herdr.calls("agent.prompt").is_empty());
    session.close();
}

#[test]
fn start_supervisor_names_the_running_one_and_refuses_the_home_directory() {
    let scratch = ScratchDir::new("serve-supervisor");
    let (repo, worktree) = repository(&scratch);
    let herdr = FakeHerdr::start("serve-sup", herd(&repo, &worktree, "idle"));
    let mut session = Session::start(&herdr, &scratch);
    opening(&session);

    let (result, _) = session.ok(json!({"command": "start_supervisor",
                                        "project_root": format!("{}/", repo.display())}));
    assert_eq!(
        result,
        ReplyResult::Supervisor {
            pane_id: Some("w1:p1".into()),
            started: false,
            message: "The supervisor runs as supervisor-weather".into()
        }
    );
    let home = corgi_core::paths::home().expect("HOME");
    let error = session
        .refused(json!({"command": "start_supervisor", "project_root": home.to_str().unwrap()}))
        .error
        .unwrap();
    assert!(error.contains("not a project"), "{error}");
    session.close();
}

#[test]
fn a_merge_is_checked_confirmed_and_tells_the_supervisor() {
    let scratch = ScratchDir::new("serve-merge");
    let (repo, worktree) = repository(&scratch);
    let herdr = FakeHerdr::start("serve-merge", herd(&repo, &worktree, "done"));
    let mut session = Session::start(&herdr, &scratch);
    opening(&session);

    let error = session
        .refused(json!({"command": "merge", "pane_id": "w2:p1"}))
        .error
        .unwrap();
    assert!(error.contains("merge_check"), "{error}");
    let (result, _) = session.ok(json!({"command": "merge_check", "pane_id": "w2:p1"}));
    let ReplyResult::MergeForm(form) = result else {
        panic!("{result:?}")
    };
    assert_eq!(
        (form.source_branch.as_str(), form.target_branch.as_str()),
        ("worktree/fix", "main")
    );
    assert_eq!(form.upstream.as_deref(), Some("origin/main"));
    assert_eq!(form.commits.len(), 1);
    assert!(form.commits[0].ends_with("Fix the weather"));

    let (result, before) = session.ok(json!({"command": "merge", "pane_id": "w2:p1"}));
    let ReplyResult::MergeOutcome(MergeOutcome::Succeeded { commit, message }) = result else {
        panic!("{result:?}")
    };
    assert_eq!(message, "Merged and pushed worktree/fix into main");
    assert_eq!(git(&repo, &["rev-parse", "--short", "HEAD"]), commit);
    assert!(before.iter().any(|message| matches!(
        message,
        ServerMessage::Progress(progress) if progress.step.is_some()
    )));
    // The supervisor hears of it at its next turn.
    let inbox = Inbox::read(&scratch.join("state").join("weather"));
    let items = inbox.undelivered();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, Kind::Merge);
    // The merge is over: nothing is left to hand over.
    let error = session
        .refused(json!({"command": "merge_handoff", "pane_id": "w2:p1"}))
        .error
        .unwrap();
    assert!(error.contains("merge_check"), "{error}");
    session.close();
}

#[test]
fn a_conflicted_merge_is_handed_to_the_supervisor() {
    let scratch = ScratchDir::new("serve-conflict");
    let (repo, worktree) = repository(&scratch);
    fs::write(repo.join("status.txt"), "moved on\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "Main moved on"]);
    git(&repo, &["push", "-q"]);
    let herdr = FakeHerdr::start("serve-conflict", herd(&repo, &worktree, "done"));
    let mut session = Session::start(&herdr, &scratch);
    opening(&session);

    session.ok(json!({"command": "merge_check", "pane_id": "w2:p1"}));
    let (result, _) = session.ok(json!({"command": "merge", "pane_id": "w2:p1",
                                        "upstream": "check"}));
    assert_eq!(
        result,
        ReplyResult::MergeOutcome(MergeOutcome::Conflicted {
            files: vec!["status.txt".into()],
            supervisor: Some("supervisor-weather".into())
        })
    );
    let (result, _) = session.ok(json!({"command": "merge_handoff", "pane_id": "w2:p1"}));
    let ReplyResult::Done { message } = result else {
        panic!("{result:?}")
    };
    assert!(
        message.starts_with("Merge aborted; asked supervisor-weather"),
        "{message}"
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["rev-parse", "-q", "--verify", "MERGE_HEAD"])
            .output()
            .unwrap()
            .stdout
            .is_empty(),
        "the merge was aborted"
    );
    let inbox = Inbox::read(&scratch.join("state").join("weather"));
    assert_eq!(inbox.undelivered()[0].kind, Kind::Conflict);
    session.close();
}

#[test]
fn a_merge_refused_before_it_starts_names_why_and_is_handed_to_the_supervisor() {
    let scratch = ScratchDir::new("serve-refused");
    let (repo, worktree) = repository(&scratch);
    fs::write(repo.join("AGENTS.md"), "a note\n").unwrap();
    let herdr = FakeHerdr::start("serve-refused", herd(&repo, &worktree, "done"));
    let mut session = Session::start(&herdr, &scratch);
    opening(&session);

    let (result, _) = session.ok(json!({"command": "merge_check", "pane_id": "w2:p1"}));
    let ReplyResult::MergeForm(form) = result else {
        panic!("{result:?}")
    };
    let error = "primary checkout has uncommitted or untracked changes: ?? AGENTS.md";
    assert_eq!(form.refused.as_deref(), Some(error));
    assert_eq!(form.supervisor.as_deref(), Some("supervisor-weather"));
    assert_eq!(
        (form.source_branch.as_str(), form.target_branch.as_str()),
        ("worktree/fix", "main")
    );
    // It cannot be run.
    let refused = session
        .refused(json!({"command": "merge", "pane_id": "w2:p1"}))
        .error
        .unwrap();
    assert_eq!(refused, format!("Merge unavailable: {error}"));

    let (result, _) = session.ok(json!({"command": "merge_handoff", "pane_id": "w2:p1"}));
    assert_eq!(
        result,
        ReplyResult::Done {
            message: "Asked supervisor-weather to fix the merge of w-fix".into()
        }
    );
    let inbox = Inbox::read(&scratch.join("state").join("weather"));
    let items = inbox.undelivered();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, Kind::Conflict);
    assert!(
        items[0].text.starts_with("[Corgi] Merging w-fix"),
        "{}",
        items[0].text
    );
    assert!(
        items[0].text.contains(&format!(
            "into main in {} failed: {error}. The user asks you to fix it and report back.",
            repo.display()
        )),
        "{}",
        items[0].text
    );
    session.close();
}

#[test]
fn a_failed_fetch_and_an_error_only_the_window_saw_are_handed_to_the_supervisor() {
    let scratch = ScratchDir::new("serve-fetch-handoff");
    let (repo, worktree) = repository(&scratch);
    git(
        &repo,
        &["remote", "set-url", "origin", "/nonexistent/corgi-remote"],
    );
    let herdr = FakeHerdr::start("serve-fetch-handoff", herd(&repo, &worktree, "done"));
    let mut session = Session::start(&herdr, &scratch);
    opening(&session);

    session.ok(json!({"command": "merge_check", "pane_id": "w2:p1"}));
    let (result, _) = session.ok(json!({"command": "merge", "pane_id": "w2:p1"}));
    let ReplyResult::MergeOutcome(MergeOutcome::FetchFailed { error, supervisor }) = result else {
        panic!("{result:?}")
    };
    assert!(error.contains("git fetch origin"), "{error}");
    assert_eq!(supervisor.as_deref(), Some("supervisor-weather"));
    let (result, _) = session.ok(json!({"command": "merge_handoff", "pane_id": "w2:p1"}));
    assert_eq!(
        result,
        ReplyResult::Done {
            message: "Asked supervisor-weather to fix the merge of w-fix".into()
        }
    );
    let inbox = Inbox::read(&scratch.join("state").join("weather"));
    assert!(
        inbox.undelivered()[0]
            .text
            .contains("failed: could not fetch origin/main: git fetch origin"),
        "{:?}",
        inbox.undelivered()
    );

    // No merge is kept now; the window still hands over the error it shows.
    let lost = "The connection to corgi serve was lost before it replied.";
    let (result, _) = session.ok(json!({"command": "merge_handoff", "pane_id": "w2:p1",
                                        "error": lost}));
    assert!(matches!(result, ReplyResult::Done { .. }), "{result:?}");
    let inbox = Inbox::read(&scratch.join("state").join("weather"));
    let items = inbox.undelivered();
    // The newer line for the same agent stands in for the older one.
    assert!(
        items
            .iter()
            .any(|item| item.text.contains(&format!("failed: {lost} The user asks"))),
        "{items:?}"
    );
    session.close();
}

#[test]
fn close_tag_focus_edit_and_digest_drive_their_flows() {
    let scratch = ScratchDir::new("serve-actions");
    let (repo, worktree) = repository(&scratch);
    ledger(&scratch);
    let mut snapshot = herd(&repo, &worktree, "working");
    snapshot["agents"][1]["tokens"] = json!({"corgi_merge": "done 3"});
    let herdr = FakeHerdr::start("serve-actions", snapshot);
    let mut session = Session::start(&herdr, &scratch);
    opening(&session);

    let (result, _) = session.ok(json!({"command": "close_check", "pane_id": "w2:p1"}));
    let ReplyResult::CloseForm(form) = result else {
        panic!("{result:?}")
    };
    assert_eq!(form.target, "worktree");
    assert_eq!(form.checkout.as_deref(), worktree.to_str());
    assert_eq!(form.confirm, "Remove worktree");

    // The tag goes; a working agent cannot be tagged.
    let (result, _) = session.ok(json!({"command": "tag", "pane_id": "w2:p1", "merge": false}));
    assert_eq!(
        result,
        ReplyResult::Done {
            message: "Cleared the merge tag of w-fix".into()
        }
    );
    assert_eq!(
        herdr.calls("pane.report_metadata")[0]["tokens"]["corgi_merge"],
        Value::Null
    );
    session.refused(json!({"command": "tag", "pane_id": "w2:p1", "merge": true}));

    // Focus and edit are the commands that move Herdr's focus.
    session.ok(json!({"command": "focus", "pane_id": "w2:p1"}));
    assert_eq!(herdr.calls("agent.focus")[0]["target"], "w2:p1");
    let (result, _) = session.ok(json!({"command": "edit", "pane_id": "w2:p1"}));
    assert_eq!(
        result,
        ReplyResult::Done {
            message: "Opened worktree/fix in the editor".into()
        }
    );
    // Git names the checkout by its real path, past /tmp's link on macOS.
    assert_eq!(
        herdr.calls("tab.create")[0]["cwd"],
        worktree.canonicalize().unwrap().to_str().unwrap()
    );

    let (result, _) = session.ok(json!({"command": "digest",
        "project_root": repo.to_str().unwrap(), "lookup": {"kind": "ledger", "id": "20261028-w-fix"}}));
    let ReplyResult::Digest { text } = result else {
        panic!("{result:?}")
    };
    assert!(text.contains("Keep the old flag?"), "{text}");
    let (result, _) = session.ok(json!({"command": "digest",
        "project_root": repo.to_str().unwrap()}));
    let ReplyResult::Digest { text } = result else {
        panic!("{result:?}")
    };
    assert!(
        text.starts_with(&format!("# Project {}", repo.display())),
        "{text}"
    );

    let (result, _) = session.ok(json!({"command": "close", "pane_id": "w2:p1"}));
    assert!(matches!(result, ReplyResult::Done { .. }), "{result:?}");
    assert_eq!(herdr.calls("worktree.remove")[0]["workspace_id"], "w2");
    herdr.fail("worktree.remove", "dirty_worktree_requires_force");
    let (result, _) = session.ok(json!({"command": "close", "pane_id": "w2:p1"}));
    assert!(
        matches!(result, ReplyResult::CloseDirty { .. }),
        "{result:?}"
    );
    herdr.heal("worktree.remove");
    session.ok(json!({"command": "close", "pane_id": "w2:p1", "force": true}));
    let removals = herdr.calls("worktree.remove");
    assert_eq!(
        (removals[1]["force"].clone(), removals[2]["force"].clone()),
        (json!(false), json!(true))
    );
    session.close();
}

/// Each call a canned forge took: its program, arguments and directory.
type ForgeCalls = Arc<Mutex<Vec<(String, Vec<String>, PathBuf)>>>;

/// A forge whose `glab` and `gh` answer `answer`, recording each call.
fn canned_forge(
    answer: impl Fn(&str) -> Result<String, String> + Send + Sync + 'static,
) -> (ForgeCli, ForgeCalls) {
    let calls: ForgeCalls = Arc::default();
    let recorded = calls.clone();
    let cli = ForgeCli {
        run: Arc::new(move |program: &str, args: &[&str], cwd: &Path| {
            recorded.lock().unwrap().push((
                program.to_string(),
                args.iter().map(|arg| arg.to_string()).collect(),
                cwd.to_path_buf(),
            ));
            answer(program)
        }),
        glab_hosts: || vec!["git.int.example.com".to_string()],
        debug: false,
    };
    (cli, calls)
}

/// `glab mr list --output json` with one merge request from the worktree's
/// branch, one from a branch the ledger's archive names, and two of
/// someone else's.
const GLAB_LIST: &str = r#"[
  {"iid":1715,"title":"Fix the weather","draft":false,"source_branch":"worktree/fix",
   "state":"opened","web_url":"https://git.int.example.com/team/weather/-/merge_requests/1715"},
  {"iid":1714,"title":"Someone's work","draft":false,"source_branch":"their-branch",
   "state":"opened","web_url":"https://git.int.example.com/team/weather/-/merge_requests/1714"},
  {"iid":1705,"title":"Draft: Whole percents","draft":true,"source_branch":"alarm-percent-whole",
   "state":"opened","web_url":"https://git.int.example.com/team/weather/-/merge_requests/1705"},
  {"iid":1600,"title":"On main","draft":false,"source_branch":"main",
   "state":"opened","web_url":"https://git.int.example.com/team/weather/-/merge_requests/1600"}
]"#;

#[test]
fn a_projects_open_merge_requests_from_its_branches_arrive_after_the_snapshot() {
    let scratch = ScratchDir::new("serve-forge");
    let (repo, worktree) = repository(&scratch);
    ledger(&scratch);
    fs::write(
        scratch.join("state/weather/ledger-archive.jsonl"),
        r#"{"id":"20261020-w-pct","agent":"w-pct","status":"reported","branch":"alarm-percent-whole"}
"#,
    )
    .unwrap();
    git(
        &repo,
        &[
            "remote",
            "set-url",
            "origin",
            "ssh://git@git.int.example.com:7999/team/weather.git",
        ],
    );
    let (cli, calls) = canned_forge(|_| Ok(GLAB_LIST.to_string()));
    let herdr = FakeHerdr::start("serve-forge", herd(&repo, &worktree, "working"));
    let session = Session::start_with_forge(&herdr, &scratch, Some(cli));
    let state = opening(&session);
    assert!(state.projects[0].merge_requests.is_empty());

    let changes = session.delta_with(|changes| {
        changes
            .iter()
            .any(|change| matches!(change, Change::Project(project) if !project.merge_requests.is_empty()))
    });
    let Some(Change::Project(project)) = changes.into_iter().find(
        |change| matches!(change, Change::Project(project) if !project.merge_requests.is_empty()),
    ) else {
        panic!("no project change")
    };
    assert_eq!(project.root, repo.to_str().unwrap());
    assert_eq!(
        serde_json::to_value(&project.merge_requests).unwrap(),
        json!([
            {"id": "!1715", "title": "Fix the weather", "branch": "worktree/fix",
             "draft": false,
             "url": "https://git.int.example.com/team/weather/-/merge_requests/1715"},
            {"id": "!1705", "title": "Draft: Whole percents",
             "branch": "alarm-percent-whole", "draft": true,
             "url": "https://git.int.example.com/team/weather/-/merge_requests/1705"}
        ])
    );
    let calls = calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].0, "glab");
    assert_eq!(
        calls[0].1,
        ["mr", "list", "--output", "json", "--per-page", "100"]
    );
    assert_eq!(calls[0].2, repo);
    session.close();
}

#[test]
fn a_forge_that_hangs_or_fails_never_holds_up_the_server() {
    let scratch = ScratchDir::new("serve-forge-slow");
    let (repo, worktree) = repository(&scratch);
    git(
        &repo,
        &[
            "remote",
            "set-url",
            "origin",
            "git@gitlab.example.com:team/weather.git",
        ],
    );
    let (release, hold) = mpsc::channel::<()>();
    let hold = Mutex::new(hold);
    let (cli, calls) = canned_forge(move |_| {
        // Hangs until the test lets go, then fails as a logged-out glab does.
        let _ = hold.lock().unwrap().recv_timeout(Duration::from_secs(20));
        Err("glab mr list failed (exit status: 1): not logged in".to_string())
    });
    let herdr = FakeHerdr::start("serve-forge-slow", herd(&repo, &worktree, "working"));
    let mut session = Session::start_with_forge(&herdr, &scratch, Some(cli));
    let state = opening(&session);
    assert!(state.projects.iter().all(|p| p.merge_requests.is_empty()));
    // The forge is asked, and hangs, while the server answers.
    for _ in 0..200 {
        if !calls.lock().unwrap().is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(calls.lock().unwrap().len(), 1);
    session.ok(json!({"command": "refresh"}));
    release.send(()).unwrap();
    let (_, before) = session.ok(json!({"command": "resync"}));
    let Some(ServerMessage::Snapshot(snapshot)) = before
        .into_iter()
        .find(|message| matches!(message, ServerMessage::Snapshot(_)))
    else {
        panic!("no snapshot after resync")
    };
    let projects = serde_json::to_value(&snapshot.state.projects).unwrap();
    assert!(projects[0].get("merge_requests").is_none(), "{projects}");
    session.close();
}

#[test]
fn a_forge_is_asked_only_for_a_forge_remote_with_branches_to_match() {
    let scratch = ScratchDir::new("serve-forge-match");
    let (repo, _worktree) = repository(&scratch);
    let inboxes = SupervisorWaker::in_dir(&scratch.join("state")).inboxes();
    let (cli, calls) = canned_forge(|program| match program {
        "gh" => Ok(
            r#"[{"headRefName":"worktree/fix","isDraft":false,"number":12,
                        "title":"Fix","url":"https://github.com/o/weather/pull/12"}]"#
                .to_string(),
        ),
        _ => Err("unexpected".to_string()),
    });
    let root = repo.to_str().unwrap();

    // The origin is a local path: no forge.
    let refused = forge::open_merge_requests(&cli, &inboxes, root).unwrap_err();
    assert!(refused.contains("has no host"), "{refused}");
    git(
        &repo,
        &[
            "remote",
            "set-url",
            "origin",
            "https://bitbucket.org/o/weather.git",
        ],
    );
    let refused = forge::open_merge_requests(&cli, &inboxes, root).unwrap_err();
    assert!(refused.contains("neither GitLab nor GitHub"), "{refused}");
    assert!(calls.lock().unwrap().is_empty());

    git(
        &repo,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:o/weather.git",
        ],
    );
    let found = forge::open_merge_requests(&cli, &inboxes, root).unwrap();
    assert_eq!(
        found.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["#12"]
    );
    assert_eq!(calls.lock().unwrap()[0].0, "gh");

    // No linked worktree and no ledger: nothing to match, nothing asked.
    let lone = scratch.join("lone");
    fs::create_dir_all(&lone).unwrap();
    git(&lone, &["init", "-q", "-b", "main"]);
    git(
        &lone,
        &["remote", "add", "origin", "git@github.com:o/lone.git"],
    );
    let found = forge::open_merge_requests(&cli, &inboxes, lone.to_str().unwrap()).unwrap();
    assert!(found.is_empty());
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn an_exited_agent_has_a_row_that_takes_resume_and_refuses_prompts() {
    let scratch = ScratchDir::new("serve-resume");
    let (repo, worktree) = repository(&scratch);
    let mut snapshot = herd(&repo, &worktree, "idle");
    snapshot["panes"] = json!([
        {"pane_id": "w2:p2", "workspace_id": "w2", "tab_id": "w2:t2", "cwd": worktree},
    ]);
    let session_id = "11111111-2222-4333-8444-555555555555";
    let record = corgi_core::restart::Record {
        panes: std::collections::BTreeMap::from([(
            "w2:p2".to_string(),
            corgi_core::restart::Launched {
                kind: "codex".into(),
                name: "w-old".into(),
                cwd: worktree.clone(),
                options: vec!["--model".into(), "gpt-6-sol".into()],
                session: Some(session_id.into()),
                ..Default::default()
            },
        )]),
    };
    fs::write(
        scratch.join("markers.launches.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let herdr = FakeHerdr::start("serve-resume", snapshot);
    let mut session = Session::start(&herdr, &scratch);
    let state = opening(&session);
    let exited = state
        .agents
        .iter()
        .find(|agent| agent.pane_id == "w2:p2")
        .expect("a row for the exited agent");
    assert!(exited.exited);
    assert!(!exited.can_prompt);
    assert_eq!(exited.name, "w-old");
    assert_eq!(exited.model.as_deref(), Some("gpt-6-sol"));
    assert_eq!(exited.project_group, state.agents[0].project_group);
    let running = state.agents.iter().find(|a| a.pane_id == "w2:p1").unwrap();
    assert!(!running.exited && running.warning.is_none());

    let reply = session.refused(json!({"command": "prompt", "pane_id": "w2:p2", "text": "hi"}));
    assert!(
        reply
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("exited to its shell"),
        "{reply:?}"
    );
    session.refused(json!({"command": "focus", "pane_id": "w2:p2"}));
    let reply = session.refused(json!({"command": "resume", "pane_id": "w2:p1"}));
    assert!(
        reply
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("nothing to resume"),
        "{reply:?}"
    );
    // The fake Herdr knows no `pane.process_info`, so the shell cannot be
    // checked and nothing starts.
    session.refused(json!({"command": "resume", "pane_id": "w2:p2"}));
    assert!(herdr.calls("agent.start").is_empty());
    assert_eq!(herdr.calls("pane.process_info").len(), 1);
    session.close();
}

#[test]
fn pause_check_names_what_pause_all_stops_and_a_paused_row_says_so() {
    let scratch = ScratchDir::new("serve-pause");
    let (repo, worktree) = repository(&scratch);
    let mut snapshot = herd(&repo, &worktree, "working");
    snapshot["panes"] = json!([
        {"pane_id": "w2:p2", "workspace_id": "w2", "tab_id": "w2:t2", "cwd": worktree},
    ]);
    // Native ids, which Resume needs.
    for (index, agent) in snapshot["agents"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        agent["agent_session"]["value"] =
            json!(format!("{index}1111111-2222-4333-8444-555555555555"));
    }
    let launched = |paused: bool| corgi_core::restart::Launched {
        kind: "claude".into(),
        name: "w-old".into(),
        cwd: worktree.clone(),
        session: Some("11111111-2222-4333-8444-555555555555".into()),
        paused: paused.then(|| corgi_core::restart::Paused {
            at: 1,
            project_root: repo.to_string_lossy().into_owned(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let record = corgi_core::restart::Record {
        panes: std::collections::BTreeMap::from([("w2:p2".to_string(), launched(true))]),
    };
    fs::write(
        scratch.join("markers.launches.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let herdr = FakeHerdr::start("serve-pause", snapshot);
    let mut session = Session::start(&herdr, &scratch);
    let state = opening(&session);
    let row = |pane: &str| state.agents.iter().find(|a| a.pane_id == pane).unwrap();
    assert!(row("w2:p2").exited && row("w2:p2").paused);
    assert!(!row("w2:p1").paused);

    let (result, _) = session.ok(json!({"command": "pause_check"}));
    let ReplyResult::PauseForm(form) = result else {
        panic!("not a pause form: {result:?}")
    };
    assert_eq!((form.agents, form.working), (3, 1));
    assert_eq!(form.projects.len(), 2);
    assert_eq!(
        (form.projects[0].supervisors, form.projects[0].workers),
        (1, 1)
    );
    assert_eq!(form.projects[0].working, ["w-fix"]);
    assert_eq!(form.projects[1].project, "Scratch");
    // Checking changes nothing.
    assert!(herdr.calls("agent.prompt").is_empty());

    // The fake Herdr knows no `pane.process_info`, so the shell cannot be
    // checked: the paused agent is named and nothing starts.
    let (result, _) = session.ok(json!({"command": "resume_all"}));
    let ReplyResult::Done { message } = result else {
        panic!("not done: {result:?}")
    };
    assert!(
        message.starts_with("Resumed 0 agents; not 1 agent: w-old"),
        "{message}"
    );
    assert!(herdr.calls("agent.start").is_empty());
    session.close();
}

#[test]
fn orphans_are_found_on_asking_and_a_choice_is_applied_after_a_fresh_look() {
    let scratch = ScratchDir::new("serve-orphans");
    let root = scratch.join("webshop");
    fs::create_dir_all(&root).unwrap();
    let root = root.to_string_lossy().into_owned();
    let main = |id: &str| {
        json!({"workspace_id": id, "label": "webshop supervisor", "tokens": {
            "corgi_workspace_role": "project-main",
            "corgi_project_main_tab": format!("{id}:t1"),
            "corgi_project_root": root,
        }})
    };
    let herdr = FakeHerdr::start(
        "serve-orphans",
        json!({
            "workspaces": [main("w4"), main("w9")],
            "agents": [{"pane_id": "w4:p1", "workspace_id": "w4", "tab_id": "w4:t1",
                        "agent": "claude", "name": "webshop-supervisor", "cwd": root,
                        "agent_status": "idle", "tokens": {"corgi_handler": "session:s1"},
                        "agent_session": {"value": "s1"}}],
            "tabs": [{"tab_id": "w4:t1", "workspace_id": "w4"},
                     {"tab_id": "w9:t1", "workspace_id": "w9"}],
            "panes": [{"pane_id": "w4:p1", "workspace_id": "w4", "tab_id": "w4:t1"},
                      {"pane_id": "w9:p1", "workspace_id": "w9", "tab_id": "w9:t1"}],
        }),
    );
    let mut session = Session::start(&herdr, &scratch);
    opening(&session);
    // Nothing is looked for until asked.
    assert!(herdr.calls("pane.process_info").is_empty());

    let (result, _) = session.ok(json!({"command": "orphans"}));
    let ReplyResult::Orphans { candidates } = result else {
        panic!("{result:?}")
    };
    assert_eq!(candidates.len(), 1, "{candidates:?}");
    let duplicate = &candidates[0];
    assert_eq!(duplicate.key, "workspace:w9");
    let close = duplicate.actions[duplicate.suggested].clone();

    // A stale fingerprint is refused, and nothing closes.
    let refused = session.refused(json!({"command": "orphan_apply", "key": duplicate.key,
        "fingerprint": "duplicate w1", "action": close}));
    assert!(refused.error.unwrap().contains("changed since"));
    assert!(herdr.calls("workspace.close").is_empty());

    let (result, _) = session.ok(json!({"command": "orphan_apply", "key": duplicate.key,
        "fingerprint": duplicate.fingerprint, "action": close}));
    assert_eq!(
        result,
        ReplyResult::Done {
            message: "Closed duplicate workspace webshop supervisor".into()
        }
    );
    assert_eq!(herdr.calls("workspace.close")[0]["workspace_id"], "w9");
    session.close();
}
