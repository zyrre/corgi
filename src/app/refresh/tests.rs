//! The refresh in the background: keys and frames go on while Herdr is slow,
//! one refresh at a time, each wake typed once, and a quit that lets a
//! refresh finish.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crossterm::event::{KeyCode, KeyModifiers};
use serde_json::{Value, json};

use super::*;
use crate::{
    app::test_helpers::{poll_until, press},
    inbox::{Item, Kind},
    supervisor::{SUPERVISOR_TOKEN, marker},
    test_support::{ScratchDir, buffer_text, fake_herdr, test_app, test_terminal},
    ui,
};

const ROOT: &str = "/repos/refresh-weather";

/// A Herdr whose snapshots wait until the test opens them, and that counts
/// them and keeps every prompt typed into an agent, until dropped.
struct SlowHerdr {
    socket: PathBuf,
    open: Arc<AtomicBool>,
    snapshots: Arc<AtomicUsize>,
    prompts: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}

impl SlowHerdr {
    fn new(label: &str, snapshot: Value) -> Self {
        let open = Arc::new(AtomicBool::new(false));
        let snapshots = Arc::new(AtomicUsize::new(0));
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let served = (
            open.clone(),
            snapshots.clone(),
            prompts.clone(),
            stop.clone(),
        );
        let (socket, server) = fake_herdr(label, move |listener| {
            let (open, snapshots, prompts, stop) = served;
            serve(&listener, &snapshot, &open, &snapshots, &prompts, &stop);
        });
        Self {
            socket,
            open,
            snapshots,
            prompts,
            stop,
            server: Some(server),
        }
    }

    fn client(&self) -> HerdrClient {
        HerdrClient::from_socket_path(&self.socket)
    }

    /// Lets the snapshot asked for, and every later one, through.
    fn open(&self) {
        self.open.store(true, Ordering::Relaxed);
    }

    fn snapshots(&self) -> usize {
        self.snapshots.load(Ordering::Relaxed)
    }

    fn prompts(&self) -> Vec<String> {
        self.prompts.lock().expect("prompts").clone()
    }

    /// Waits until a refresh has asked for its snapshot.
    fn wait_for_snapshots(&self, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.snapshots() < count {
            assert!(Instant::now() < deadline, "no refresh asked for a snapshot");
            thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Drop for SlowHerdr {
    fn drop(&mut self) {
        self.open();
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn serve(
    listener: &UnixListener,
    snapshot: &Value,
    open: &AtomicBool,
    snapshots: &AtomicUsize,
    prompts: &Mutex<Vec<String>>,
    stop: &AtomicBool,
) {
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    while !stop.load(Ordering::Relaxed) {
        let Ok((stream, _)) = listener.accept() else {
            thread::sleep(Duration::from_millis(2));
            continue;
        };
        stream.set_nonblocking(false).expect("blocking stream");
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            continue;
        }
        let request: Value = serde_json::from_str(&line).expect("parse request");
        let params = &request["params"];
        let result = match request["method"].as_str() {
            Some("session.snapshot") => {
                snapshots.fetch_add(1, Ordering::Relaxed);
                while !open.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(2));
                }
                json!({ "type": "session_snapshot", "snapshot": snapshot })
            }
            Some("agent.read") => json!({ "type": "pane_read", "read": {
                "text": "", "pane_id": params["target"]
            }}),
            Some("agent.prompt") => {
                prompts.lock().expect("prompts").push(format!(
                    "{}: {}",
                    params["target"].as_str().unwrap_or_default(),
                    params["text"].as_str().unwrap_or_default()
                ));
                json!({ "type": "agent_prompted", "agent": {
                    "pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1"
                }})
            }
            Some("plugin.list") => json!({ "type": "plugin_list", "plugins": [] }),
            _ => Value::Null,
        };
        let mut response = if result.is_null() {
            json!({ "error": { "code": "unsupported", "message": "not faked" } })
        } else {
            json!({ "result": result })
        };
        response["id"] = request["id"].clone();
        let stream = reader.get_mut();
        let _ = serde_json::to_writer(&mut *stream, &response);
        let _ = stream.write_all(b"\n");
    }
}

/// An idle agent of the weather project in pane `w1:p<pane>`.
fn agent(pane: u8, name: &str, tokens: Value) -> Value {
    json!({
        "pane_id": format!("w1:p{pane}"),
        "workspace_id": "w1",
        "tab_id": "w1:t1",
        "agent": "claude",
        "agent_status": "idle",
        "name": name,
        "cwd": ROOT,
        "tokens": tokens,
        "agent_session": { "value": format!("{name}-session") }
    })
}

/// The weather project's supervisor and two of its workers, all idle.
fn weather_session() -> Value {
    let mut supervisor_tokens = json!({});
    supervisor_tokens[SUPERVISOR_TOKEN] = json!(marker(
        "supervisor-weather",
        Some("supervisor-weather-session")
    ));
    json!({
        "agents": [
            agent(1, "supervisor-weather", supervisor_tokens),
            agent(2, "w-forecast", json!({})),
            agent(3, "w-radar", json!({})),
        ]
    })
}

/// Three workers of the weather project, all idle, and no supervisor, so
/// each has a row of its own.
fn weather_workers() -> Value {
    json!({
        "agents": [
            agent(1, "w-forecast", json!({})),
            agent(2, "w-radar", json!({})),
            agent(3, "w-tides", json!({})),
        ]
    })
}

/// The pane IDs of the agents on the dashboard, in its order.
fn panes(app: &App) -> Vec<&str> {
    app.agents
        .iter()
        .map(|agent| agent.info.pane_id.as_str())
        .collect()
}

#[test]
fn keys_and_frames_go_on_while_a_slow_refresh_runs() {
    let herdr = SlowHerdr::new("refresh-slow", weather_workers());
    let mut app = test_app();
    app.client = herdr.client();
    // A first refresh, let through at once, gives the list its rows.
    herdr.open();
    app.refresh();
    assert_eq!(panes(&app), ["w1:p1", "w1:p2", "w1:p3"]);
    herdr.open.store(false, Ordering::Relaxed);

    app.request_refresh();
    app.tick();
    herdr.wait_for_snapshots(2);
    assert!(app.refreshing());

    // Herdr holds the snapshot back until the test lets it through, so a
    // key or a frame that waited for the refresh would never come back. Keys
    // move the selection over the rows last applied, and frames are drawn.
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    let mut terminal = test_terminal(100, 30);
    for _ in 0..3 {
        app.tick();
        terminal
            .draw(|frame| ui::draw(frame, &mut app))
            .expect("draw");
    }
    assert!(app.refreshing(), "the refresh is still waiting on Herdr");
    assert_eq!(app.selected, 2);
    assert!(buffer_text(terminal.backend().buffer()).contains("3 IDLE"));

    // Once Herdr answers, the rows are put in place and the selection stays
    // on its row.
    herdr.open();
    poll_until(&mut app, App::poll_refresh, |app| !app.refreshing());
    assert_eq!(panes(&app), ["w1:p1", "w1:p2", "w1:p3"]);
    assert_eq!(app.selected, 2);
    assert_eq!(app.status, "3 agents");
}

#[test]
fn a_refresh_asked_for_while_one_runs_waits_for_it_and_wakes_nobody_twice() {
    let scratch = ScratchDir::new("refresh-wake");
    let herdr = SlowHerdr::new("refresh-wake", weather_session());
    let mut app = test_app();
    app.client = herdr.client();
    let mut waker = SupervisorWaker::in_dir(&scratch);
    assert!(waker.lead(&scratch.join("wake.lock")));
    app.wake_with(waker);
    app.inboxes
        .as_ref()
        .expect("the interactive dashboard's inboxes")
        .notify(Item {
            project: ROOT.into(),
            source: "test".into(),
            kind: Kind::Note,
            key: "forecast".into(),
            text: "The forecast is ready".into(),
            ..Item::default()
        })
        .expect("notify");

    app.tick();
    herdr.wait_for_snapshots(1);
    // The waker went with the refresh: nothing on the UI thread holds it.
    assert!(app.refresh_state.is_none());
    // A refresh asked for meanwhile starts no second one beside it.
    app.request_refresh();
    for _ in 0..5 {
        app.tick();
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(herdr.snapshots(), 1);

    herdr.open();
    poll_until(&mut app, App::poll_refresh, |app| !app.refreshing());
    assert!(app.refresh_state.is_some(), "the state came back");
    let prompts = herdr.prompts();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(
        prompts[0].starts_with("supervisor-weather: ") && prompts[0].contains("forecast is ready"),
        "{prompts:?}"
    );

    // The refresh asked for runs right after, and types nothing again.
    assert!(app.refresh_due());
    app.tick();
    assert!(app.refreshing());
    poll_until(&mut app, App::poll_refresh, |app| !app.refreshing());
    assert_eq!(herdr.snapshots(), 2);
    assert_eq!(herdr.prompts().len(), 1);
    assert!(!app.refresh_due(), "the next waits its interval");
}

#[test]
fn quitting_lets_a_refresh_in_flight_finish_but_waits_no_longer_than_its_bound() {
    let herdr = SlowHerdr::new("refresh-quit", weather_session());
    let mut app = test_app();
    app.client = herdr.client();

    app.tick();
    herdr.wait_for_snapshots(1);
    let waited = Instant::now();
    app.wait_for_refresh(Some(Duration::from_millis(150)));
    assert!(waited.elapsed() >= Duration::from_millis(150));
    assert!(app.refreshing(), "a refresh past the bound is left running");

    let opener = {
        let open = herdr.open.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            open.store(true, Ordering::Relaxed);
        })
    };
    app.wait_for_refresh(Some(QUIT_WAIT));
    assert!(!app.refreshing());
    assert_eq!(panes(&app), ["w1:p1", "w1:p2", "w1:p3"]);
    opener.join().expect("opener");
}

#[test]
fn a_refresh_that_panics_is_reported_and_the_next_starts_afresh() {
    let scratch = ScratchDir::new("refresh-panic");
    let mut app = test_app();
    app.wake_with(SupervisorWaker::in_dir(&scratch));
    app.refresh_state = None;
    app.refresh_job = Job::spawn(|_| panic!("refresh failed"));
    poll_until(&mut app, App::poll_refresh, |app| !app.refreshing());
    assert_eq!(app.status, "The refresh failed; trying again");
    assert!(!app.refresh_due(), "the next waits its interval as usual");

    app.request_refresh();
    assert!(app.refresh_due());
    // The state the panic took is started again, waker included.
    assert!(app.refresh_state().waker.is_some());
}

#[test]
fn every_refresh_is_logged_with_how_long_each_step_took() {
    let scratch = ScratchDir::new("refresh-log");
    let herdr = SlowHerdr::new("refresh-log", weather_session());
    herdr.open();
    let mut app = test_app();
    app.client = herdr.client();
    app.refresh_log = Some(scratch.join("refresh.log"));
    app.tick();
    poll_until(&mut app, App::poll_refresh, |app| !app.refreshing());
    app.client = HerdrClient::from_socket_path(scratch.join("gone.sock"));
    app.request_refresh();
    app.tick();
    poll_until(&mut app, App::poll_refresh, |app| !app.refreshing());

    let log = std::fs::read_to_string(scratch.join("refresh.log")).expect("log");
    let lines: Vec<_> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    assert!(lines[0].contains(" refresh "), "{log}");
    assert!(lines[0].contains("(herdr "), "{log}");
    assert!(lines[0].ends_with(": 3 agents"), "{log}");
    assert!(lines[1].contains(": Herdr unavailable: "), "{log}");
}
