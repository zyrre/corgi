//! The real `corgi hook` binary, as Herdr runs it: many at once, one past
//! its time limit, and the startup hook after a restart. Each test gives it a
//! fake Herdr socket and a private home and state directory; nothing here
//! reaches a live Herdr or the developer's own state.

use std::{
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

use corgi_core::{supervisor::SUPERVISOR_TOKEN, test_support::FakeHerdr};
use serde_json::{Value, json};

const ROOT: &str = "/repos/hook-weather";

/// A home and state directory of the hook's own, under Cargo's target
/// directory rather than the temporary one: Corgi's recovery refuses to
/// coordinate deliveries from state kept in a volatile directory.
struct Private(PathBuf);

impl Private {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "hook-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create the hook's home");
        Self(dir)
    }
}

impl std::ops::Deref for Private {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Private {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// `corgi hook` for `event` on the Herdr at `socket`, with `scratch` as its
/// home and state, and only the variables Herdr would set.
fn hook(socket: &Path, scratch: &Path, event: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corgi"));
    command
        .arg("hook")
        .env_clear()
        // No /usr/sbin, as a Herdr server's PATH may lack it: recovery's boot
        // check once ran `sysctl` from PATH, failed, and held every wake.
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", scratch.join("home"))
        .env("XDG_STATE_HOME", scratch.join("state"))
        .env("CLAUDE_CONFIG_DIR", scratch.join("claude"))
        .env("HERDR_SOCKET_PATH", socket)
        .env("HERDR_PLUGIN_EVENT", event)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn run(command: &mut Command) -> Output {
    command.output().expect("run corgi hook")
}

fn agent(pane: u8, name: &str, state: &str, change: u64, tokens: Value) -> Value {
    json!({
        "pane_id": format!("w1:p{pane}"),
        "workspace_id": "w1",
        "tab_id": "w1:t1",
        "agent": "claude",
        "agent_status": state,
        "state_change_seq": change,
        "name": name,
        "cwd": ROOT,
        "tokens": tokens,
        "agent_session": { "value": format!("{name}-session") }
    })
}

/// The weather project's idle supervisor, marked as Corgi marks it when
/// `marked`, and a worker in `state` after `change` state changes.
fn weather(marked: bool, state: &str, change: u64) -> Value {
    let tokens = if marked {
        json!({ (SUPERVISOR_TOKEN): "session:supervisor-weather-session" })
    } else {
        json!({})
    };
    json!({
        "workspaces": [{"workspace_id": "w1", "label": "weather"}],
        "panes": [
            {"pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1", "tokens": tokens},
            {"pane_id": "w1:p2", "workspace_id": "w1", "tab_id": "w1:t1", "tokens": {}}
        ],
        "agents": [
            agent(1, "supervisor-weather", "idle", 1, tokens),
            agent(2, "w-forecast", state, change, json!({})),
        ]
    })
}

#[test]
fn hooks_started_together_deliver_a_wake_once() {
    let scratch = Private::new("hook-burst");
    let herdr = FakeHerdr::start("hook-burst", weather(true, "working", 2));
    let first = run(&mut hook(&herdr.socket, &scratch, "pane.agent_detected"));
    assert!(first.status.success(), "{first:?}");
    assert!(herdr.prompts().is_empty());

    // The worker stops, and a burst of hooks starts at once, as Herdr
    // starts them for events that come together.
    herdr.set_snapshot(weather(true, "idle", 3));
    let started = Instant::now();
    let children: Vec<_> = (0..16)
        .map(|_| {
            hook(&herdr.socket, &scratch, "pane.agent_status_changed")
                .spawn()
                .expect("spawn corgi hook")
        })
        .collect();
    let outputs: Vec<Output> = children
        .into_iter()
        .map(|child| child.wait_with_output().expect("wait for corgi hook"))
        .collect();
    assert!(started.elapsed() < Duration::from_secs(10));
    let lines: Vec<String> = outputs
        .iter()
        .map(|output| {
            assert!(output.status.success(), "{output:?}");
            String::from_utf8_lossy(&output.stdout).into_owned()
        })
        .collect();
    let prompts = herdr.prompts();
    assert_eq!(prompts.len(), 1, "{prompts:?}\n{lines:#?}");
    assert!(
        prompts[0].starts_with("supervisor-weather: [Corgi] w-forecast is idle."),
        "{prompts:?}"
    );
    assert!(
        lines
            .iter()
            .all(|line| line.starts_with("corgi hook pane.agent_status_changed: ")),
        "{lines:#?}"
    );

    // Later hooks, a startup's among them, find nothing left to deliver.
    for event in ["pane.exited", "startup"] {
        assert!(
            run(&mut hook(&herdr.socket, &scratch, event))
                .status
                .success()
        );
    }
    assert_eq!(herdr.prompts().len(), 1);
}

#[test]
fn a_hook_past_its_time_limit_ends_itself_and_frees_the_lock() {
    let scratch = Private::new("hook-limit");
    // A Herdr that takes connections and never answers.
    let socket = std::env::temp_dir().join(format!("corgi-hook-hung-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).expect("bind hung socket");
    let held = thread::spawn(move || {
        let mut streams = Vec::new();
        for stream in listener.incoming().take(1) {
            streams.push(stream);
        }
        thread::sleep(Duration::from_secs(3));
        drop(streams);
    });
    let started = Instant::now();
    let output =
        run(hook(&socket, &scratch, "pane.agent_status_changed").env("CORGI_HOOK_LIMIT_MS", "400"));
    let took = started.elapsed();
    assert_eq!(output.status.code(), Some(124), "{output:?}");
    assert!(
        took >= Duration::from_millis(400) && took < Duration::from_secs(3),
        "{took:?}"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("time limit"));
    std::fs::remove_file(&socket).ok();
    held.join().ok();

    // The lock went with the process: the next hook takes it.
    let herdr = FakeHerdr::start("hook-limit-after", weather(true, "working", 2));
    let next = run(&mut hook(&herdr.socket, &scratch, "pane.exited"));
    let line = String::from_utf8_lossy(&next.stdout);
    assert!(line.contains(": 1 pass in "), "{line}");
}

#[test]
fn the_startup_hook_puts_back_the_marks_a_herdr_restart_lost() {
    let scratch = Private::new("hook-startup");
    let herdr = FakeHerdr::start("hook-startup", weather(true, "idle", 1));
    // A hook before the restart records the supervisor's mark and session.
    assert!(
        run(&mut hook(&herdr.socket, &scratch, "pane.agent_detected"))
            .status
            .success()
    );
    let record = scratch.join("state/corgi/markers").join(format!(
        "{}.json",
        herdr
            .socket
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect::<String>()
    ));
    let recorded = std::fs::read_to_string(&record).expect("the marks are recorded");
    assert!(
        recorded.contains("supervisor-weather-session"),
        "{recorded}"
    );

    // Herdr restarts: the same panes and sessions, none of Corgi's tokens.
    herdr.set_snapshot(weather(false, "idle", 1));
    let output = run(&mut hook(&herdr.socket, &scratch, "startup"));
    assert!(output.status.success(), "{output:?}");
    let reports = herdr.requests("pane.report_metadata");
    assert!(
        reports
            .iter()
            .any(|report| report["params"]["pane_id"] == "w1:p1"
                && report["params"]["tokens"][SUPERVISOR_TOKEN]
                    == "session:supervisor-weather-session"),
        "{reports:#?}"
    );
    let line = String::from_utf8_lossy(&output.stdout);
    assert!(line.starts_with("corgi hook startup: 1 pass"), "{line}");
    assert!(line.contains("Reconciled 1 of Corgi's"), "{line}");
}
