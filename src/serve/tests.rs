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
        Change, Command as Wire, MergeOutcome, PaletteSet, Reply, ReplyResult, ServerMessage,
        State, ThemeColors,
    },
    test_support::{ScratchDir, fake_herdr},
    waker::SupervisorWaker,
};
use serde_json::{Value, json};

use super::{Event, Options, Output, Server};

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
