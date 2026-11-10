use std::{cell::RefCell, collections::BTreeMap};

use serde_json::{Value, json};

use super::*;
use crate::{
    herdr::{PaneInfo, PaneProcess},
    model::AgentSession,
    recovery::{native::Profile, store},
    test_support::{ScratchDir, answer, fake_herdr},
};

const OLD: &str = "11111111-2222-4333-8444-555555555555";
const NEW: &str = "99999999-8888-4777-a666-555555555555";

fn strings(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

fn worker(kind: &str, options: &[&str]) -> Launched {
    Launched {
        kind: kind.into(),
        name: "fix-cache".into(),
        cwd: "/work/repo".into(),
        options: strings(options),
        ..Launched::default()
    }
}

fn supervisor_of(kind: &str) -> Launched {
    Launched {
        name: "supervisor-repo".into(),
        supervisor: Some(SupervisorRole {
            role_file: "/state/repo/ROLE.md".into(),
            state_dir: "/state/repo".into(),
        }),
        ..worker(kind, &["--model", "opus"])
    }
}

#[test]
fn a_claude_worker_resumes_with_its_model_effort_and_extra_arguments() {
    let launched = worker(
        "claude",
        &["--model", "opus", "--effort", "high", "--verbose"],
    );
    assert_eq!(
        launched.argv(OLD).unwrap(),
        strings(&[
            "claude",
            "--resume",
            OLD,
            "--model",
            "opus",
            "--effort",
            "high",
            "--verbose"
        ])
    );
}

#[test]
fn a_claude_supervisor_resumes_with_its_role_file_and_memory() {
    assert_eq!(
        supervisor_of("claude").argv(OLD).unwrap(),
        strings(&[
            "claude",
            "--resume",
            OLD,
            "--model",
            "opus",
            "--append-system-prompt-file",
            "/state/repo/ROLE.md",
            "--add-dir",
            "/state/repo",
        ])
    );
}

#[test]
fn a_codex_worker_resumes_in_its_directory_with_its_options_and_trust() {
    let launched = Launched {
        trust: strings(&["-c", "projects={\"/work/repo\"={trust_level=\"trusted\"}}"]),
        ..worker(
            "codex",
            &[
                "--model",
                "gpt-6-sol",
                "-c",
                "model_reasoning_effort=high",
                "--search",
            ],
        )
    };
    assert_eq!(
        launched.argv(OLD).unwrap(),
        strings(&[
            "codex",
            "resume",
            OLD,
            "--cd",
            "/work/repo",
            "--model",
            "gpt-6-sol",
            "-c",
            "model_reasoning_effort=high",
            "--search",
            "-c",
            "projects={\"/work/repo\"={trust_level=\"trusted\"}}",
        ])
    );
}

#[test]
fn a_codex_supervisor_resumes_with_its_flags_and_a_short_instruction_to_read_its_role() {
    let argv = supervisor_of("codex").argv(OLD).unwrap();
    assert_eq!(
        argv[..13],
        strings(&[
            "codex",
            "resume",
            OLD,
            "--cd",
            "/work/repo",
            "--model",
            "opus",
            "--sandbox",
            "workspace-write",
            "--ask-for-approval",
            "on-request",
            "-c",
            "sandbox_workspace_write.network_access=true",
        ])
    );
    assert_eq!(argv[13..15], strings(&["--add-dir", "/state/repo"]));
    assert_eq!(argv[15], "-c");
    let instruction = &argv[16];
    assert!(instruction.starts_with("developer_instructions=\""));
    assert!(instruction.contains("read your role instructions in /state/repo/ROLE.md"));
    assert!(!instruction.contains('\'') && !instruction.contains('\n'));
    assert_eq!(argv.len(), 17);
    native::check_resume_argv(&argv).unwrap();
    // A normal launch keeps the whole role inline.
    let role = "You are the supervisor.\nIt's a long role.";
    let launch = Harness::Codex.supervisor_args(
        Path::new("/state/repo/ROLE.md"),
        role,
        Path::new("/state/repo"),
    );
    assert_eq!(
        launch.last().unwrap(),
        &format!(
            "developer_instructions={}",
            crate::harness::toml_string(role)
        )
    );
}

#[test]
fn the_command_starts_with_the_plain_cli_name_whatever_binary_corgi_uses() {
    // `CORGI_CLAUDE_BIN` names the binary Corgi runs itself; Herdr runs a
    // resume command through the pane's shell, by name.
    for kind in ["claude", "codex"] {
        assert_eq!(worker(kind, &[]).argv(OLD).unwrap()[0], kind);
    }
    assert!(worker("gemini", &[]).argv(OLD).is_err());
    assert!(worker("claude", &[]).argv("not-a-session").is_err());
}

#[test]
fn herdrs_limits_are_kept() {
    native::check_resume_argv(&strings(&["claude", "--resume", OLD])).unwrap();
    for refused in [
        strings(&["/opt/bin/claude", "--resume", OLD]),
        strings(&["", "--resume", OLD]),
        strings(&["claude", "--append-system-prompt", "it's"]),
        strings(&["claude", "--append-system-prompt", "two\nlines"]),
        strings(&["claude", "--append-system-prompt", "tab\there"]),
        vec!["claude".into(), "x".repeat(native::RESUME_ARGV_LIMIT)],
        vec![],
    ] {
        assert!(native::check_resume_argv(&refused).is_err(), "{refused:?}");
    }
    let mut apostrophe = worker("claude", &[]);
    apostrophe.cwd = "/work/it's".into();
    assert!(worker("codex", &[]).argv(OLD).is_ok());
    apostrophe.kind = "codex".into();
    assert!(apostrophe.argv(OLD).is_err());
    let long = worker("claude", &["--append-system-prompt", &"x".repeat(9000)]);
    assert!(long.argv(OLD).is_err());
}

#[test]
fn what_a_process_lacks_is_named_by_option() {
    let launched = worker(
        "claude",
        &["--model", "opus", "--effort", "high", "--verbose"],
    );
    let full = strings(&[
        "claude",
        "--verbose",
        "--resume",
        OLD,
        "--effort",
        "high",
        "--model",
        "opus",
    ]);
    assert!(launched.lacks(&full).is_empty());
    assert_eq!(
        launched.lacks(&strings(&["claude", "--resume", OLD])),
        strings(&["model", "effort", "--verbose"])
    );
    assert_eq!(
        launched.lacks(&strings(&[
            "claude",
            "--model",
            "sonnet",
            "--effort",
            "high",
            "--verbose"
        ])),
        strings(&["model"])
    );
    let supervisor = supervisor_of("claude");
    assert_eq!(
        supervisor.lacks(&strings(&["claude", "--resume", OLD, "--model", "opus"])),
        strings(&["role", "memory"])
    );
    let codex = supervisor_of("codex");
    assert!(codex.lacks(&codex.argv(OLD).unwrap()).is_empty());
    assert_eq!(
        codex.lacks(&strings(&["codex", "resume", OLD, "--model", "opus"])),
        strings(&[
            "--sandbox",
            "--ask-for-approval",
            "network access",
            "memory",
            "role"
        ])
    );
}

#[test]
fn the_warning_says_what_is_missing_and_what_to_do() {
    assert_eq!(
        warning_text(&strings(&["role", "memory"])),
        "Running without its supervisor role and memory access: exit it, then Resume"
    );
    assert_eq!(
        warning_text(&strings(&["role"])),
        "Running without its supervisor role: exit it, then Resume"
    );
    assert_eq!(
        warning_text(&strings(&["model", "effort"])),
        "Running without its launch options (model, effort): exit it, then Resume"
    );
}

#[test]
fn model_and_effort_are_read_back_from_the_options() {
    assert_eq!(
        worker("claude", &["--model", "opus", "--effort", "high"]).model_and_effort(),
        (Some("opus".into()), Some("high".into()))
    );
    assert_eq!(
        worker("codex", &["-c", "model_reasoning_effort=low"]).model_and_effort(),
        (None, Some("low".into()))
    );
}

/// A Herdr that records what [`reconcile_record`] asks of it.
#[derive(Default)]
struct Fake {
    processes: RefCell<BTreeMap<String, PaneProcesses>>,
    calls: RefCell<Vec<String>>,
}

impl Fake {
    fn running(&self, pane: &str, pids: &[u32], argv: &[&str]) {
        self.processes.borrow_mut().insert(
            pane.into(),
            PaneProcesses {
                shell_pid: Some(10),
                foreground_process_group_id: pids.first().copied(),
                foreground_processes: pids
                    .iter()
                    .map(|pid| PaneProcess {
                        pid: *pid,
                        name: argv[0].into(),
                        argv: Some(strings(argv)),
                        argv0: Some(argv[0].into()),
                    })
                    .collect(),
            },
        );
    }

    /// The pane's processes as Herdr lists them, newest first, with the
    /// foreground group's leader if Herdr names one.
    fn listing(&self, pane: &str, leader: Option<u32>, processes: Vec<PaneProcess>) {
        self.processes.borrow_mut().insert(
            pane.into(),
            PaneProcesses {
                shell_pid: Some(10),
                foreground_process_group_id: leader,
                foreground_processes: processes,
            },
        );
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.calls.borrow_mut())
    }
}

impl Herdr for Fake {
    fn process_info(&self, pane_id: &str) -> Result<PaneProcesses> {
        self.calls
            .borrow_mut()
            .push(format!("process_info {pane_id}"));
        self.processes
            .borrow()
            .get(pane_id)
            .cloned()
            .context("no such pane")
    }

    fn report(&self, pane_id: &str, agent: &str, session: &str, argv: &[String]) -> Result<()> {
        self.calls.borrow_mut().push(format!(
            "report {pane_id} {agent} {session} {}",
            argv.join(" ")
        ));
        Ok(())
    }

    fn release(&self, pane_id: &str, agent: &str) -> Result<()> {
        self.calls
            .borrow_mut()
            .push(format!("release {pane_id} {agent}"));
        Ok(())
    }
}

fn agent(pane: &str, kind: &str, session: Option<&str>, terminal: &str) -> AgentInfo {
    AgentInfo {
        agent: Some(kind.into()),
        name: Some("fix-cache".into()),
        pane_id: pane.into(),
        workspace_id: "w1".into(),
        tab_id: "w1:t1".into(),
        cwd: Some("/work/repo".into()),
        terminal_id: Some(terminal.into()),
        agent_session: session.map(|value| AgentSession {
            agent: kind.into(),
            kind: "id".into(),
            source: format!("herdr:{kind}"),
            value: value.into(),
        }),
        ..AgentInfo::default()
    }
}

fn snapshot(version: &str, agents: Vec<AgentInfo>, panes: &[&str]) -> SessionSnapshot {
    SessionSnapshot {
        version: version.into(),
        agents,
        panes: panes
            .iter()
            .map(|pane| PaneInfo {
                pane_id: pane.to_string(),
                workspace_id: "w1".into(),
                tab_id: "w1:t1".into(),
                ..PaneInfo::default()
            })
            .collect(),
        ..SessionSnapshot::default()
    }
}

fn launched_in(pane: &str, launched: Launched) -> Record {
    Record {
        panes: BTreeMap::from([(pane.to_string(), launched)]),
    }
}

#[test]
fn a_launch_is_reported_once_and_again_after_a_clear_in_the_same_process() {
    let herdr = Fake::default();
    let mut record = launched_in("w1:p1", worker("claude", &["--model", "opus"]));
    herdr.running("w1:p1", &[100], &["claude", "--model", "opus"]);
    // Before Herdr knows the session there is nothing to resume.
    let waiting = snapshot(
        "0.9.5",
        vec![agent("w1:p1", "claude", None, "t1")],
        &["w1:p1"],
    );
    assert_eq!(reconcile_record(&herdr, &mut record, &waiting), None);
    assert!(herdr.take().is_empty());

    let live = snapshot(
        "0.9.5",
        vec![agent("w1:p1", "claude", Some(OLD), "t1")],
        &["w1:p1"],
    );
    let status = reconcile_record(&herdr, &mut record, &live);
    assert_eq!(
        status.as_deref(),
        Some("Gave Herdr the resume command of 1 agent")
    );
    assert_eq!(
        herdr.take(),
        [
            "process_info w1:p1".to_string(),
            format!("report w1:p1 claude {OLD} claude --resume {OLD} --model opus"),
        ]
    );
    let entry = &record.panes["w1:p1"];
    assert_eq!(entry.session.as_deref(), Some(OLD));
    assert_eq!(entry.processes, [100]);
    assert!(entry.missing.is_empty());

    // Nothing changed: no request at all.
    assert_eq!(reconcile_record(&herdr, &mut record, &live), None);
    assert!(herdr.take().is_empty());

    // `/clear`: a new session, the same process.
    let cleared = snapshot(
        "0.9.5",
        vec![agent("w1:p1", "claude", Some(NEW), "t1")],
        &["w1:p1"],
    );
    reconcile_record(&herdr, &mut record, &cleared);
    assert_eq!(
        herdr.take(),
        [
            "process_info w1:p1".to_string(),
            format!("report w1:p1 claude {NEW} claude --resume {NEW} --model opus"),
        ]
    );
    assert_eq!(record.panes["w1:p1"].session.as_deref(), Some(NEW));
}

#[test]
fn another_agent_in_the_pane_has_the_command_released() {
    let herdr = Fake::default();
    let mut record = launched_in("w1:p1", worker("claude", &["--model", "opus"]));
    herdr.running("w1:p1", &[100], &["claude", "--model", "opus"]);
    let live = snapshot(
        "0.9.5",
        vec![agent("w1:p1", "claude", Some(OLD), "t1")],
        &["w1:p1"],
    );
    reconcile_record(&herdr, &mut record, &live);
    herdr.take();
    // A new process with another session: the user's own `claude`.
    herdr.running("w1:p1", &[200], &["claude"]);
    let other = snapshot(
        "0.9.5",
        vec![agent("w1:p1", "claude", Some(NEW), "t1")],
        &["w1:p1"],
    );
    assert_eq!(
        reconcile_record(&herdr, &mut record, &other).as_deref(),
        Some("Gave Herdr the resume command of 0 agents, withdrew 1")
    );
    assert_eq!(herdr.take(), ["process_info w1:p1", "release w1:p1 claude"]);
    assert!(record.panes.is_empty());

    // Another harness in the pane: released at once.
    let mut record = launched_in(
        "w1:p1",
        Launched {
            session: Some(OLD.into()),
            reported: Some(Reported::default()),
            processes: vec![100],
            terminal: Some("t1".into()),
            ..worker("claude", &[])
        },
    );
    let codex = snapshot(
        "0.9.5",
        vec![agent("w1:p1", "codex", Some(NEW), "t1")],
        &["w1:p1"],
    );
    reconcile_record(&herdr, &mut record, &codex);
    assert_eq!(herdr.take(), ["release w1:p1 claude"]);
    assert!(record.panes.is_empty());
}

#[test]
fn a_restart_keeps_the_session_and_a_newer_herdr_is_told_again() {
    let herdr = Fake::default();
    let mut record = launched_in("w1:p1", supervisor_of("claude"));
    let argv = supervisor_of("claude").argv(OLD).unwrap();
    herdr.running(
        "w1:p1",
        &[100],
        &argv.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let before = snapshot(
        "0.9.0",
        vec![agent("w1:p1", "claude", Some(OLD), "t1")],
        &["w1:p1"],
    );
    reconcile_record(&herdr, &mut record, &before);
    assert_eq!(herdr.take().len(), 2);
    // Upgraded and restarted: a new terminal and process, the same
    // session, resumed with Herdr's own command, which lacks the role.
    herdr.running("w1:p1", &[300], &["claude", "--resume", OLD]);
    let after = snapshot(
        "0.9.5",
        vec![agent("w1:p1", "claude", Some(OLD), "t2")],
        &["w1:p1"],
    );
    reconcile_record(&herdr, &mut record, &after);
    let calls = herdr.take();
    assert_eq!(calls[0], "process_info w1:p1");
    assert!(calls[1].starts_with(&format!("report w1:p1 claude {OLD} claude --resume {OLD}")));
    let entry = &record.panes["w1:p1"];
    assert_eq!(entry.missing, strings(&["model", "role", "memory"]));
    assert_eq!(entry.terminal.as_deref(), Some("t2"));
    assert_eq!(
        missing(&record, &after.agents[0]),
        Some(&strings(&["model", "role", "memory"])[..])
    );
}

#[test]
fn an_agent_at_its_shell_keeps_its_record_and_whatever_comes_next_is_checked() {
    let herdr = Fake::default();
    let mut record = launched_in("w1:p1", worker("codex", &["--search"]));
    herdr.running("w1:p1", &[100], &["codex", "--search"]);
    let live = snapshot(
        "0.9.5",
        vec![agent("w1:p1", "codex", Some(OLD), "t1")],
        &["w1:p1"],
    );
    reconcile_record(&herdr, &mut record, &live);
    herdr.take();
    let shell = snapshot("0.9.5", Vec::new(), &["w1:p1"]);
    assert_eq!(reconcile_record(&herdr, &mut record, &shell), None);
    assert!(herdr.take().is_empty());
    let entry = &record.panes["w1:p1"];
    assert_eq!(entry.session.as_deref(), Some(OLD));
    assert!(entry.processes.is_empty());
    assert_eq!(resumable(&record, &shell).len(), 1);

    // Resumed by hand without its options: the same session, checked, and
    // its command told again, since Herdr dropped it with the agent.
    herdr.running("w1:p1", &[400], &["codex", "resume", OLD]);
    reconcile_record(&herdr, &mut record, &live);
    let calls = herdr.take();
    assert_eq!(calls[0], "process_info w1:p1");
    assert!(
        calls[1].starts_with(&format!("report w1:p1 codex {OLD} codex resume {OLD}")),
        "{calls:?}"
    );
    assert_eq!(record.panes["w1:p1"].missing, ["--search"]);
    assert_eq!(record.panes["w1:p1"].processes, [400]);
    // While it is flagged, each pass looks again, and tells Herdr nothing
    // new.
    reconcile_record(&herdr, &mut record, &live);
    assert_eq!(herdr.take(), ["process_info w1:p1"]);
    assert_eq!(record.panes["w1:p1"].missing, ["--search"]);
}

/// A process in a pane's listing; `argv: None` where Herdr cannot read it.
fn process(pid: u32, name: &str, argv0: &str, argv: Option<&[&str]>) -> PaneProcess {
    PaneProcess {
        pid,
        name: name.into(),
        argv: argv.map(strings),
        argv0: Some(argv0.into()),
    }
}

/// A supervisor's pane as Herdr lists it: an MCP server and a `node`
/// without a command line, newest first, ahead of the agent itself.
fn supervisor_pane(claude: &[&str]) -> Vec<PaneProcess> {
    vec![
        process(
            39905,
            "node",
            "node",
            Some(&["node", "/npx/.bin/playwright-mcp", "--browser", "chromium"]),
        ),
        process(39755, "node", "playwright-profile", None),
        process(
            39737,
            "caffeinate",
            "caffeinate",
            Some(&["caffeinate", "-i"]),
        ),
        process(39606, "2.1.30", "claude", Some(claude)),
    ]
}

fn supervisor_argv() -> Vec<String> {
    let launched = supervisor_of("claude");
    let role = launched.supervisor.as_ref().unwrap();
    vec![
        "claude".into(),
        "--model".into(),
        "opus".into(),
        "--append-system-prompt-file".into(),
        role.role_file.to_string_lossy().into_owned(),
        "--add-dir".into(),
        role.state_dir.to_string_lossy().into_owned(),
    ]
}

fn live_supervisor() -> SessionSnapshot {
    snapshot(
        "0.9.5",
        vec![agent("w1:p1", "claude", Some(OLD), "t1")],
        &["w1:p1"],
    )
}

#[test]
fn the_agents_own_process_is_checked_not_a_child_listed_ahead_of_it() {
    let argv = supervisor_argv();
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    // By the foreground group's leader, and without one by the name it was
    // started under (Claude Code's own name is its version).
    for leader in [Some(39606), None] {
        let herdr = Fake::default();
        let mut record = launched_in("w1:p1", supervisor_of("claude"));
        herdr.listing("w1:p1", leader, supervisor_pane(&argv));
        reconcile_record(&herdr, &mut record, &live_supervisor());
        let entry = &record.panes["w1:p1"];
        assert!(entry.missing.is_empty(), "{leader:?}: {:?}", entry.missing);
        assert!(!entry.unidentified);
        assert_eq!(entry.processes, [39905, 39755, 39737, 39606]);
    }
}

#[test]
fn the_agents_own_process_without_its_flags_is_still_flagged() {
    for leader in [Some(39606), None] {
        let herdr = Fake::default();
        let mut record = launched_in("w1:p1", supervisor_of("claude"));
        herdr.listing(
            "w1:p1",
            leader,
            supervisor_pane(&["claude", "--resume", OLD, "--model", "opus"]),
        );
        reconcile_record(&herdr, &mut record, &live_supervisor());
        let entry = &record.panes["w1:p1"];
        assert_eq!(entry.missing, strings(&["role", "memory"]), "{leader:?}");
        assert!(!entry.unidentified);
    }
}

#[test]
fn a_pane_without_a_process_that_is_surely_the_agent_warns_of_nothing_and_is_checked_again() {
    let herdr = Fake::default();
    let mut record = launched_in("w1:p1", supervisor_of("claude"));
    // Only children (the agent's own process not listed, or listed without
    // a readable command line), and no leader but the shell.
    let mut processes = supervisor_pane(&[]);
    processes.pop();
    processes.push(process(39606, "2.1.30", "claude", None));
    herdr.listing("w1:p1", Some(10), processes);
    reconcile_record(&herdr, &mut record, &live_supervisor());
    herdr.take();
    let entry = &record.panes["w1:p1"];
    assert!(entry.missing.is_empty());
    assert!(entry.unidentified);
    assert_eq!(missing(&record, &live_supervisor().agents[0]), None);

    // The next pass looks again and, the agent now readable, settles.
    let argv = supervisor_argv();
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    herdr.listing("w1:p1", Some(39606), supervisor_pane(&argv));
    reconcile_record(&herdr, &mut record, &live_supervisor());
    assert_eq!(herdr.take()[0], "process_info w1:p1");
    let entry = &record.panes["w1:p1"];
    assert!(entry.missing.is_empty());
    assert!(!entry.unidentified);
    // Settled: no more requests.
    reconcile_record(&herdr, &mut record, &live_supervisor());
    assert!(herdr.take().is_empty());
}

#[test]
fn a_wrongly_flagged_agent_is_cleared_on_the_next_pass_without_a_restart() {
    // As recorded by a check that read a child: flagged, its processes
    // and session set, its command already given to Herdr.
    let launched = supervisor_of("claude");
    let wanted = Reported {
        argv: launched.argv(OLD).unwrap(),
        herdr: "0.9.5".into(),
    };
    let mut record = launched_in(
        "w1:p1",
        Launched {
            session: Some(OLD.into()),
            terminal: Some("t1".into()),
            processes: vec![39755, 39737, 39606],
            missing: strings(&["role", "memory"]),
            reported: Some(wanted.clone()),
            ..launched
        },
    );
    let live = live_supervisor();
    assert!(missing(&record, &live.agents[0]).is_some());
    let herdr = Fake::default();
    let argv = supervisor_argv();
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    herdr.listing("w1:p1", Some(39606), supervisor_pane(&argv));
    assert_eq!(reconcile_record(&herdr, &mut record, &live), None);
    // One look, and the same process's command is not reported again.
    assert_eq!(herdr.take(), ["process_info w1:p1"]);
    let entry = &record.panes["w1:p1"];
    assert!(entry.missing.is_empty());
    assert_eq!(entry.reported.as_ref(), Some(&wanted));
    assert_eq!(missing(&record, &live.agents[0]), None);
    reconcile_record(&herdr, &mut record, &live);
    assert!(herdr.take().is_empty());
}

#[test]
fn an_agent_that_just_exited_is_not_taken_for_another() {
    // For a moment after an exit, Herdr lists the pane's agent with no
    // harness and no session.
    let herdr = Fake::default();
    let mut record = launched_in(
        "w1:p1",
        Launched {
            session: Some(OLD.into()),
            processes: vec![100],
            terminal: Some("t1".into()),
            reported: Some(Reported::default()),
            ..worker("codex", &["--search"])
        },
    );
    let leaving = AgentInfo {
        agent: None,
        ..agent("w1:p1", "codex", None, "t1")
    };
    let snapshot = snapshot("0.9.5", vec![leaving], &["w1:p1"]);
    assert_eq!(reconcile_record(&herdr, &mut record, &snapshot), None);
    assert!(herdr.take().is_empty());
    assert!(record.panes["w1:p1"].processes.is_empty());
    assert_eq!(resumable(&record, &snapshot).len(), 1);
}

#[test]
fn a_pane_that_is_gone_is_forgotten_without_a_request() {
    let herdr = Fake::default();
    let mut record = launched_in(
        "w1:p1",
        Launched {
            session: Some(OLD.into()),
            reported: Some(Reported::default()),
            ..worker("claude", &[])
        },
    );
    reconcile_record(&herdr, &mut record, &snapshot("0.9.5", Vec::new(), &[]));
    assert!(record.panes.is_empty());
    assert!(herdr.take().is_empty());
}

#[test]
fn a_command_herdr_would_refuse_is_withdrawn_rather_than_kept_stale() {
    let herdr = Fake::default();
    let mut record = launched_in(
        "w1:p1",
        Launched {
            session: Some(OLD.into()),
            processes: vec![100],
            terminal: Some("t1".into()),
            reported: Some(Reported::default()),
            ..worker("claude", &["--append-system-prompt", "it's"])
        },
    );
    herdr.running("w1:p1", &[100], &["claude"]);
    let cleared = snapshot(
        "0.9.5",
        vec![agent("w1:p1", "claude", Some(NEW), "t1")],
        &["w1:p1"],
    );
    reconcile_record(&herdr, &mut record, &cleared);
    assert_eq!(herdr.take(), ["process_info w1:p1", "release w1:p1 claude"]);
    assert_eq!(record.panes["w1:p1"].reported, None);
    assert_eq!(record.panes["w1:p1"].session.as_deref(), Some(NEW));
}

#[test]
fn resume_is_offered_only_where_corgi_saw_a_session_and_the_agent_is_gone() {
    let seen = Launched {
        session: Some(OLD.into()),
        ..worker("claude", &[])
    };
    let record = Record {
        panes: BTreeMap::from([
            ("w1:p1".to_string(), seen.clone()),
            ("w1:p2".to_string(), worker("claude", &[])),
            ("w1:p3".to_string(), seen.clone()),
            ("w1:p4".to_string(), seen),
        ]),
    };
    // p1 at its shell; p2 never had a session; p3 still runs; p4 is gone.
    let live = snapshot(
        "0.9.5",
        vec![agent("w1:p3", "claude", Some(OLD), "t1")],
        &["w1:p1", "w1:p2", "w1:p3", "w9:p9"],
    );
    let offered: Vec<_> = resumable(&record, &live)
        .into_iter()
        .map(|(pane, _)| pane)
        .collect();
    assert_eq!(offered, ["w1:p1"]);
    // A pane Corgi has no record of is never offered.
    assert!(resumable(&Record::default(), &live).is_empty());
}

#[test]
fn the_warning_is_only_for_the_process_the_record_checked() {
    let record = launched_in(
        "w1:p1",
        Launched {
            session: Some(OLD.into()),
            missing: strings(&["role"]),
            ..supervisor_of("claude")
        },
    );
    let running = agent("w1:p1", "claude", Some(OLD), "t1");
    assert_eq!(missing(&record, &running), Some(&strings(&["role"])[..]));
    let other = agent("w1:p1", "claude", Some(NEW), "t1");
    assert_eq!(missing(&record, &other), None);
    let elsewhere = agent("w1:p2", "claude", Some(OLD), "t1");
    assert_eq!(missing(&record, &elsewhere), None);
}

#[test]
fn a_worker_launched_before_the_record_is_taken_in_from_its_recovery_profile() {
    let scratch = ScratchDir::new("restart-profile");
    let profiles = scratch.join("profiles");
    store::private_dir(&profiles).unwrap();
    let mut settings = native::Settings::from_launch("opus", "high", &[]);
    settings.permission = Some("acceptEdits".into());
    let profile = Profile {
        kind: "claude".into(),
        id: OLD.into(),
        cwd: "/work/repo".into(),
        settings,
    };
    store::atomic(
        &profiles.join(format!("claude-{OLD}.json")),
        &serde_json::to_vec(&profile).unwrap(),
    )
    .unwrap();
    let live = snapshot(
        "0.9.5",
        vec![
            agent("w1:p1", "claude", Some(OLD), "t1"),
            agent("w1:p2", "claude", Some(NEW), "t1"),
        ],
        &["w1:p1", "w1:p2"],
    );
    let mut record = Record::default();
    adopt(&mut record, &live, Some(&profiles));
    assert_eq!(record.panes.len(), 1, "no profile, no record: {record:?}");
    let entry = &record.panes["w1:p1"];
    assert_eq!(
        entry.options,
        strings(&[
            "--model",
            "opus",
            "--effort",
            "high",
            "--permission-mode",
            "acceptEdits"
        ])
    );
    assert_eq!(entry.session, None, "taken in at the next check");
}

#[test]
fn a_launch_is_recorded_beside_the_marker_record() {
    let scratch = ScratchDir::new("restart-record");
    let client = HerdrClient::from_socket_path(crate::test_support::UNUSED_SOCKET)
        .with_marker_record(scratch.join("markers.json"));
    assert_eq!(
        record_path(&client),
        Some(scratch.join("markers.launches.json"))
    );
    let started = agent("w1:p1", "claude", Some(OLD), "t1");
    record_launch(
        &client,
        &Harness::Claude,
        &started,
        strings(&["--model", "opus"]),
        Vec::new(),
        Some(SupervisorRole {
            role_file: "/state/repo/ROLE.md".into(),
            state_dir: "/state/repo".into(),
        }),
    )
    .unwrap();
    // `agent.start`'s reply names no harness; the launch's own counts.
    record_launch(
        &client,
        &Harness::Codex,
        &AgentInfo {
            agent: None,
            ..agent("w1:p3", "codex", None, "t1")
        },
        Vec::new(),
        Vec::new(),
        None,
    )
    .unwrap();
    assert_eq!(read(&client).panes["w1:p3"].kind, "codex");
    let record = read(&client);
    let entry = &record.panes["w1:p1"];
    assert_eq!(entry.session.as_deref(), Some(OLD));
    assert_eq!(entry.name, "fix-cache");
    assert!(entry.supervisor.is_some());
    // Another harness is not recorded.
    record_launch(
        &client,
        &Harness::Gemini,
        &agent("w1:p2", "gemini", None, "t1"),
        Vec::new(),
        Vec::new(),
        None,
    )
    .unwrap();
    assert_eq!(read(&client).panes.len(), 2);
    forget(&client, "w1:p1");
    assert_eq!(read(&client).panes.len(), 1);
}

#[test]
fn resume_starts_the_agent_again_in_its_pane_with_its_options() {
    let scratch = ScratchDir::new("restart-resume");
    let cwd = scratch.join("repo");
    fs::create_dir_all(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    let cwd_text = cwd.to_string_lossy().into_owned();
    let (socket, server) = fake_herdr("restart-resume", {
        let cwd_text = cwd_text.clone();
        move |listener| {
            let mut requests = Vec::new();
            let mut record = |request: &Value| requests.push(request.clone());
            answer(&listener, |request| {
                record(request);
                json!({"result": {"type": "session_snapshot", "snapshot": {
                    "panes": [{"pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1"}],
                    "agents": [],
                }}})
            });
            answer(&listener, |request| {
                record(request);
                json!({"result": {"type": "pane_process_info", "process_info": {
                    "pane_id": "w1:p1", "shell_pid": 10, "foreground_process_group_id": 10,
                    "foreground_processes": [{"pid": 10, "name": "bash", "argv": ["bash"], "cwd": cwd_text}],
                }}})
            });
            answer(&listener, |request| {
                record(request);
                json!({"result": {"type": "agent_started", "argv": [], "agent": {
                    "pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1",
                }}})
            });
            requests
        }
    });
    let client =
        HerdrClient::from_socket_path(&socket).with_marker_record(scratch.join("markers.json"));
    record_launch(
        &client,
        &Harness::Claude,
        &AgentInfo {
            cwd: Some(cwd_text.clone()),
            ..agent("w1:p1", "claude", Some(OLD), "t1")
        },
        strings(&["--model", "opus"]),
        Vec::new(),
        None,
    )
    .unwrap();
    resume(&client, "w1:p1").unwrap();
    let requests = server.join().unwrap();
    let start = &requests[2];
    assert_eq!(start["method"], "agent.start");
    assert_eq!(start["params"]["pane_id"], "w1:p1");
    assert_eq!(start["params"]["kind"], "claude");
    assert_eq!(start["params"]["name"], "fix-cache");
    assert_eq!(
        start["params"]["args"],
        json!(["--resume", OLD, "--model", "opus"])
    );
    let _ = fs::remove_file(&socket);
}

#[test]
fn resume_refuses_a_pane_without_a_known_session_or_with_a_busy_shell() {
    let scratch = ScratchDir::new("restart-refuse");
    let client = HerdrClient::from_socket_path(crate::test_support::UNUSED_SOCKET)
        .with_marker_record(scratch.join("markers.json"));
    assert!(resume(&client, "w1:p1").is_err());
    record_launch(
        &client,
        &Harness::Claude,
        &agent("w1:p1", "claude", None, "t1"),
        Vec::new(),
        Vec::new(),
        None,
    )
    .unwrap();
    let error = resume(&client, "w1:p1").unwrap_err();
    assert!(
        format!("{error:#}").contains("never saw a session"),
        "{error:#}"
    );

    let (socket, server) = fake_herdr("restart-busy", |listener| {
        answer(&listener, |_| {
            json!({"result": {"type": "session_snapshot", "snapshot": {
                "panes": [{"pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1"}],
                "agents": [],
            }}})
        });
        answer(&listener, |_| {
            json!({"result": {"type": "pane_process_info", "process_info": {
                "pane_id": "w1:p1", "shell_pid": 10, "foreground_process_group_id": 20,
                "foreground_processes": [{"pid": 20, "name": "vim", "argv": ["vim"], "cwd": "/work/repo"}],
            }}})
        });
    });
    let client =
        HerdrClient::from_socket_path(&socket).with_marker_record(scratch.join("markers.json"));
    record_launch(
        &client,
        &Harness::Claude,
        &agent("w1:p1", "claude", Some(OLD), "t1"),
        Vec::new(),
        Vec::new(),
        None,
    )
    .unwrap();
    let error = resume(&client, "w1:p1").unwrap_err();
    assert!(format!("{error:#}").contains("shell is busy"), "{error:#}");
    server.join().unwrap();
    let _ = fs::remove_file(&socket);
}
