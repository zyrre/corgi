//! The daemon's decisions: which terminals it resumes, its lock, and the
//! events it follows.

use std::collections::BTreeMap;

use super::*;
use crate::{
    hc::{Foreground, ROLE_LABEL, Status},
    test_support::ScratchDir,
};

fn kept(address: &str, pid: u64, front: &str, labels: &[(&str, &str)]) -> Terminal {
    Terminal {
        address: address.into(),
        name: Some(format!("{address}-name")),
        state: "live".into(),
        labels: labels
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
        foreground: Some(Foreground {
            name: front.into(),
            ..Foreground::default()
        }),
        pid: Some(pid),
        keep: true,
        ..Terminal::default()
    }
}

const RESUME: &str = "claude --resume s-1";

#[test]
fn a_kept_terminal_whose_program_the_core_replaced_with_a_shell_is_resumed() {
    let lost = kept(
        "t3",
        200,
        "-zsh",
        &[(RESUME_LABEL, RESUME), (PID_LABEL, "100")],
    );
    assert_eq!(needs_resume(&lost), Some(RESUME));
    // A worker's agent, its own program, also came back as a shell.
    let worker = kept(
        "t4",
        201,
        "zsh",
        &[(RESUME_LABEL, RESUME), (PID_LABEL, "101")],
    );
    assert_eq!(needs_resume(&worker), Some(RESUME));
}

#[test]
fn a_supervisor_that_exited_into_its_own_shell_and_a_running_agent_are_left_alone() {
    // The user's /exit: the same shell, the same pid; corgi supervisor
    // starts it again.
    let exited = kept(
        "t3",
        100,
        "zsh",
        &[(RESUME_LABEL, RESUME), (PID_LABEL, "100")],
    );
    assert_eq!(needs_resume(&exited), None);
    // Already resumed: its agent is in front.
    let running = kept(
        "t3",
        200,
        "claude",
        &[(RESUME_LABEL, RESUME), (PID_LABEL, "100")],
    );
    assert_eq!(needs_resume(&running), None);
    // Nothing to resume with, or nothing to tell a lost program by.
    let no_line = kept("t3", 200, "zsh", &[(PID_LABEL, "100")]);
    assert_eq!(needs_resume(&no_line), None);
    let no_pid = kept("t3", 200, "zsh", &[(RESUME_LABEL, RESUME)]);
    assert_eq!(needs_resume(&no_pid), None);
    let mut finished = kept(
        "t3",
        200,
        "zsh",
        &[(RESUME_LABEL, RESUME), (PID_LABEL, "1")],
    );
    finished.state = "finished".into();
    assert_eq!(needs_resume(&finished), None);
}

#[test]
fn one_daemon_holds_a_cores_lock_and_says_its_pid() {
    let scratch = ScratchDir::new("daemon-lock");
    let path = scratch.join("daemon").join("core.lock");
    let socket = Path::new("/run/hc.sock");
    assert!(!running_at(Some(path.clone())).running);
    let build = r#"{"version":"0.2.3","commit":"5b50895"}"#;
    let held = take_lock_at(&path, socket, build).unwrap();
    let running = running_at(Some(path.clone()));
    assert!(running.running);
    assert_eq!(running.pid, Some(std::process::id()));
    assert_eq!(running.build.as_deref(), Some(build));
    let refused = take_lock_at(&path, socket, "another build")
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains(&format!("(pid {})", std::process::id())),
        "{refused}"
    );
    // The refused try left the holder's pid and build in place.
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("{}\n{build}\n", std::process::id())
    );
    // Unlocked, not only closed: a process another test spawns may hold a
    // copy of the descriptor for a moment.
    held.unlock().unwrap();
    drop(held);
    // What the last daemon wrote is nobody's once it is gone.
    let gone = running_at(Some(path.clone()));
    assert_eq!((gone.running, gone.pid, gone.build), (false, None, None));

    // A daemon from before the lock held a build: its pid alone.
    fs::write(&path, format!("{}\n", std::process::id())).unwrap();
    let old = fs::File::open(&path).unwrap();
    old.lock().unwrap();
    let running = running_at(Some(path));
    assert_eq!(
        (running.running, running.pid, running.build),
        (true, Some(std::process::id()), None)
    );
    old.unlock().unwrap();
}

#[test]
fn the_log_says_the_local_date_and_time_to_the_second() {
    let at = LocalTime {
        secs: 1_794_222_540,
        offset: 3_600,
    };
    assert_eq!(log_stamp(at), "2026-11-09 12:09:00");
}

#[test]
fn what_an_event_stream_says_and_which_of_it_calls_for_a_pass() {
    use crate::hc::events::{Message, concerns_corgi, parse};
    use serde_json::json;
    let snapshot = parse(
        r#"{"id":1,"jsonrpc":"2.0","result":{"channel":null,"seq":2,"terminals":[{"address":"t1","labels":{},"argv":[],"cwd":"/","state":"live","created_ms":1,"screen":"exact","cols":80,"rows":24}]}}"#,
    )
    .unwrap();
    assert!(matches!(&snapshot, Message::Snapshot(terminals) if terminals[0].address == "t1"));
    let status = parse(
        r#"{"jsonrpc":"2.0","method":"event","params":{"type":"status","terminal":"t3","seq":9,"data":{"status":{"state":"done"}}}}"#,
    )
    .unwrap();
    assert_eq!(
        parse(r#"{"jsonrpc":"2.0","method":"heartbeat","params":{"seq":10}}"#),
        Some(Message::Heartbeat)
    );
    assert_eq!(
        parse(r#"{"jsonrpc":"2.0","method":"missed","params":{}}"#),
        Some(Message::Missed)
    );
    assert_eq!(parse(r#"{"error":{"message":"no"}}"#), None);
    assert_eq!(parse("not json"), None);

    let known: HashSet<String> = ["t3".to_string()].into();
    assert!(concerns_corgi(&snapshot, &known));
    assert!(concerns_corgi(&status, &known));
    assert!(
        !concerns_corgi(&status, &HashSet::new()),
        "another terminal"
    );
    assert!(!concerns_corgi(&Message::Heartbeat, &known));
    let event = |kind: &str, data| Message::Event {
        kind: kind.into(),
        terminal: Some("t9".into()),
        data,
    };
    // A terminal Corgi starts, or labels, is known from then on.
    assert!(concerns_corgi(
        &event("created", json!({"labels": {ROLE_LABEL: "worker"}})),
        &known
    ));
    assert!(concerns_corgi(
        &event("labels", json!({ROLE_LABEL: "supervisor"})),
        &known
    ));
    assert!(!concerns_corgi(
        &event("labels", json!({"herdr.space": "s1"})),
        &known
    ));
}

#[test]
fn the_event_stream_subscribes_on_the_socket_and_ends_with_it() {
    use std::{
        io::{BufRead, BufReader},
        os::unix::net::UnixListener,
    };
    let scratch = ScratchDir::new("hc-events");
    let socket = scratch.join("x.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let core = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request = String::new();
        reader.read_line(&mut request).unwrap();
        let mut stream = stream;
        writeln!(
            stream,
            r#"{{"id":1,"jsonrpc":"2.0","result":{{"channel":null,"seq":1,"terminals":[]}}}}"#
        )
        .unwrap();
        writeln!(
            stream,
            r#"{{"jsonrpc":"2.0","method":"event","params":{{"type":"turn","terminal":"t2","seq":2,"data":{{"turn":3}}}}}}"#
        )
        .unwrap();
        request
    });
    let (mut events, terminals) = crate::hc::events::EventStream::connect(&socket).unwrap();
    assert!(terminals.is_empty());
    assert!(matches!(
        events.next_message().unwrap(),
        Some(Message::Event { kind, terminal: Some(t), .. }) if kind == "turn" && t == "t2"
    ));
    assert_eq!(events.next_message().unwrap(), None, "the core closed it");
    let request: serde_json::Value = serde_json::from_str(&core.join().unwrap()).unwrap();
    assert_eq!(request["method"], "events.subscribe");
}

#[test]
fn a_status_record_of_a_resting_agent_with_its_agent_in_front_is_typeable() {
    use crate::waker::hc::resting;
    let mut terminal = kept("t3", 1, "claude", &[]);
    terminal.status = Some(Status {
        state: "done".into(),
        ..Status::default()
    });
    assert!(resting(&terminal));
    terminal.status.as_mut().unwrap().state = "idle".into();
    assert!(resting(&terminal));
    for state in ["working", "blocked", "error"] {
        terminal.status.as_mut().unwrap().state = state.into();
        assert!(!resting(&terminal), "{state}");
    }
    terminal.status.as_mut().unwrap().state = "done".into();
    terminal.foreground.as_mut().unwrap().name = "zsh".into();
    assert!(!resting(&terminal), "its shell is in front");
}
