//! The hc client and launch against a fake `hc`: a shell script that records
//! its arguments and answers each command with canned output.

use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use super::{
    agents::{agent_state, dashboard_agents, shell_in_front, tag},
    launch::{
        AGENT_SCRIPT, Start, agent_argv, answer_folder_trust, create_worktree_in, harness_command,
        harness_refusal, launch_worker, new_session_id, project_space, shell_line, task_title,
    },
    *,
};
use crate::{
    harness::Harness,
    launch::{CORGI_REQUEST_TOKEN, Checkout, LaunchPlan, Role},
    model::AgentState,
    progress::Silent,
    supervisor::MERGE_TOKEN,
    tag::TagOptions,
    test_support::Recorded,
    test_support::{ScratchDir, git, git_repo},
};

/// A fake `hc` in a scratch directory. `hc CMD TARGET …` prints the file
/// `CMD.N.out` on its `N`th call of `CMD`, else `CMD.TARGET.out`, else
/// `CMD.out`, writes the matching `.err` to stderr,
/// and exits with the code in the matching `.code` (0 without one). Every
/// call's arguments are recorded, separated by `|`, each call ended by an
/// ASCII record separator (a prompt has lines of its own).
struct FakeHc {
    dir: ScratchDir,
}

impl FakeHc {
    fn new() -> Self {
        let dir = ScratchDir::new("fake-hc");
        let script = dir.join("hc");
        fs::write(
            &script,
            r#"#!/bin/sh
d=$(dirname "$0")
(IFS='|'; printf '%s\036' "$*") >> "$d/calls"
n=$(( $(cat "$d/count.$1" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$d/count.$1"
f="$d/$1.$2"
[ -e "$f.out" ] || [ -e "$f.code" ] || f="$d/$1"
[ -e "$d/$1.$n.out" ] && f="$d/$1.$n"
[ -e "$f.out" ] && cat "$f.out"
[ -e "$f.err" ] && cat "$f.err" >&2
exit "$(cat "$f.code" 2>/dev/null || echo 0)"
"#,
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    fn client(&self) -> HcClient {
        HcClient::new(self.dir.join("hc"))
    }

    /// `hc reply`'s output (`reply` is `CMD` or `CMD.TARGET`).
    fn answer(&self, reply: &str, out: &str) {
        fs::write(self.dir.join(format!("{reply}.out")), out).unwrap();
    }

    fn fail(&self, reply: &str, code: i32, err: &str) {
        fs::write(self.dir.join(format!("{reply}.code")), code.to_string()).unwrap();
        fs::write(self.dir.join(format!("{reply}.err")), err).unwrap();
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.dir.join("calls"))
            .unwrap_or_default()
            .split('\x1e')
            .filter(|call| !call.is_empty())
            .map(str::to_string)
            .collect()
    }
}

fn terminal_json(address: &str, name: &str, labels: &[(&str, &str)], status: &str) -> String {
    let labels: BTreeMap<&str, &str> = labels.iter().copied().collect();
    let status = if status.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::json!({"state": status, "app": "claude-code"})
    };
    serde_json::json!({
        "address": address,
        "name": name,
        "state": "live",
        "status": status,
        "labels": labels,
        "cwd": "/repos/weather",
        "turn": 3,
        "argv": ["/bin/sh"],
        "created_ms": 1,
    })
    .to_string()
}

fn worker_plan(project: &Path, harness: Harness, checkout: Checkout) -> LaunchPlan {
    LaunchPlan {
        name: "weather".into(),
        harness,
        model: "haiku".into(),
        effort: String::new(),
        prompt: "# Fix the cache\n\nDetails.".into(),
        project: project.to_path_buf(),
        new_project: false,
        checkout,
        extra_args: Vec::new(),
        role: Role::Worker,
        request_id: Some("20261109-cache".into()),
    }
}

#[test]
fn run_puts_options_and_json_before_the_program_and_reads_the_terminal() {
    let hc = FakeHc::new();
    hc.answer(
        "run",
        r#"{"address":"t7","agent":"sh","cwd":"/w","name":"w1","state":"live","turn":0}"#,
    );
    let terminal = hc
        .client()
        .run(&RunSpec {
            name: "w1".into(),
            cwd: PathBuf::from("/w"),
            labels: vec![
                ("corgi.role".into(), "worker".into()),
                ("corgi.task".into(), "a b=c".into()),
            ],
            keep: true,
            argv: vec!["claude".into(), "--json".into()],
        })
        .unwrap();
    assert_eq!(terminal.address, "t7");
    assert!(terminal.is_live());
    assert_eq!(
        hc.calls(),
        [
            "run|--bg|--no-parent|--no-notify|--name|w1|--cwd|/w|--keep|--label|corgi.role=worker|\
          --label|corgi.task=a b=c|--json|--|claude|--json"
        ]
    );
}

#[test]
fn a_shell_terminal_is_run_with_no_program() {
    let hc = FakeHc::new();
    hc.answer("run", r#"{"address":"t8"}"#);
    hc.client()
        .run(&RunSpec {
            name: "s".into(),
            cwd: PathBuf::from("/w"),
            ..RunSpec::default()
        })
        .unwrap();
    assert_eq!(
        hc.calls(),
        ["run|--bg|--no-parent|--no-notify|--name|s|--cwd|/w|--json"]
    );
}

#[test]
fn hcs_errors_keep_their_kind_and_a_missing_terminal_is_none() {
    let hc = FakeHc::new();
    hc.fail(
        "get.t9",
        3,
        r#"{"error":{"hint":"see `hc ls --all`","kind":"not_found","message":"no terminal \"t9\""}}"#,
    );
    hc.fail(
        "send",
        9,
        r#"{"error":{"hint":"read its screen first","kind":"blocked","message":"t2 is blocked (permission)"}}"#,
    );
    let client = hc.client();
    assert_eq!(client.find("t9").unwrap(), None);
    let error = client.send("t2", "-v please").unwrap_err();
    let reported = error.downcast_ref::<HcError>().unwrap();
    assert_eq!(reported.kind, "blocked");
    assert_eq!(reported.code, Some(9));
    assert_eq!(
        error.to_string(),
        "hc: t2 is blocked (permission) (read its screen first)"
    );
    // A text that looks like an option is still the text.
    assert_eq!(hc.calls()[1], "send|t2|--json|--|-v please");
}

#[test]
fn list_gets_each_terminal_for_its_labels_and_skips_finished_and_remote_ones() {
    let hc = FakeHc::new();
    // `ls` gives a status as its word, `get` as a record.
    hc.answer(
        "ls",
        r#"{"hidden_finished":0,"terminals":[{"address":"t1","status":"done"},{"address":"t2","status":null},{"address":"vps/t3"}],"machines":[]}"#,
    );
    hc.answer(
        "get.t1",
        &terminal_json("t1", "w1", &[("corgi.role", "worker")], "done"),
    );
    hc.answer(
        "get.t2",
        r#"{"address":"t2","state":"finished","labels":{"corgi.role":"worker"}}"#,
    );
    let listed = hc.client().list(&[("corgi.role", "worker")]).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].labels["corgi.role"], "worker");
    assert_eq!(listed[0].status_word(), Some("done"));
    assert_eq!(
        hc.calls(),
        [
            "ls|--label|corgi.role=worker|--json",
            "get|t1|--json",
            "get|t2|--json"
        ]
    );
}

#[test]
fn read_label_keys_kill_and_rm_send_what_hc_takes() {
    let hc = FakeHc::new();
    hc.answer(
        "read",
        r#"{"complete":true,"how":"history","text":"❯ hello\n"}"#,
    );
    hc.answer("label", &terminal_json("t1", "w1", &[], ""));
    let client = hc.client();
    assert_eq!(client.read("t1", 120).unwrap(), "❯ hello\n");
    client
        .label("t1", &[("corgi.tag", ""), ("corgi.tag.at", "")])
        .unwrap();
    client.keys("t1", &["down", "enter"]).unwrap();
    client.kill("t1").unwrap();
    client.remove("t1").unwrap();
    assert_eq!(
        hc.calls(),
        [
            "read|t1|--lines|120|--json",
            "label|t1|corgi.tag=|corgi.tag.at=|--json",
            "keys|t1|down|enter|--json",
            "kill|t1|--json",
            "rm|t1|--json",
        ]
    );
}

#[test]
fn hc_status_words_become_agent_states_and_a_shell_in_front_is_unknown() {
    let status = |word: &str| Terminal {
        status: Some(Status {
            state: word.into(),
            ..Status::default()
        }),
        ..Terminal::default()
    };
    assert_eq!(agent_state(&status("idle")), AgentState::Idle);
    assert_eq!(agent_state(&status("done")), AgentState::Done);
    assert_eq!(agent_state(&status("working")), AgentState::Working);
    assert_eq!(agent_state(&status("blocked")), AgentState::Blocked);
    assert_eq!(agent_state(&status("error")), AgentState::Blocked);
    assert_eq!(agent_state(&Terminal::default()), AgentState::Unknown);
    let mut exited = status("done");
    exited.foreground = Some(Foreground {
        name: "-zsh".into(),
        ..Foreground::default()
    });
    assert!(shell_in_front(&exited));
    assert_eq!(agent_state(&exited), AgentState::Unknown);
    exited.foreground.as_mut().unwrap().name = "claude".into();
    assert!(!shell_in_front(&exited));
}

#[test]
fn labelled_terminals_become_rows_with_their_sessions_facts() {
    let home = ScratchDir::new("hc-home");
    let session = "6c1d2a6e-0000-4000-8000-000000000001";
    let worktree = "/home/me/.herdr/worktrees/weather/worktree-calm-otter-1f2e";
    let transcripts = home
        .join(".claude/projects")
        .join(worktree.replace(['/', '.'], "-"));
    fs::create_dir_all(&transcripts).unwrap();
    fs::write(
        transcripts.join(format!("{session}.jsonl")),
        concat!(
            r#"{"type":"user","message":{"role":"user","content":"Fix the cache"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"model":"claude-opus-4-6","usage":{"input_tokens":2,"cache_creation_input_tokens":1000,"cache_read_input_tokens":19000,"output_tokens":500},"content":[{"type":"text","text":"Done: the cache is fixed."}]}}"#,
            "\n"
        ),
    )
    .unwrap();
    let worker = Terminal {
        address: "t4".into(),
        name: Some("weather".into()),
        state: "live".into(),
        status: Some(Status {
            state: "done".into(),
            ..Status::default()
        }),
        labels: [
            (PROJECT_LABEL, "/repos/weather"),
            (ROLE_LABEL, WORKER_ROLE),
            (REQUEST_LABEL, "20261109-cache"),
            (TASK_LABEL, "Fix the cache"),
            (HARNESS_LABEL, "claude"),
            (SESSION_LABEL, session),
            (SPACE_LABEL, "corgi-weather"),
            (TAG_LABEL, MERGE_TAG),
            (TAG_AT_LABEL, "done 3"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect(),
        cwd: worktree.into(),
        turn: 3,
        ..Terminal::default()
    };
    let supervisor = Terminal {
        address: "t1".into(),
        name: Some("weather-supervisor".into()),
        state: "live".into(),
        labels: [
            (PROJECT_LABEL, "/repos/weather"),
            (ROLE_LABEL, SUPERVISOR_ROLE),
            (HARNESS_LABEL, "claude"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect(),
        cwd: "/repos/weather".into(),
        foreground: Some(Foreground {
            name: "zsh".into(),
            ..Foreground::default()
        }),
        ..Terminal::default()
    };
    let agents = dashboard_agents(&[worker, supervisor], Some(&home));
    assert_eq!(agents.len(), 2);
    let supervisor = agents.iter().find(|agent| agent.supervisor).unwrap();
    assert_eq!(supervisor.info.pane_id, "t1");
    assert_eq!(supervisor.task, "Supervisor");
    assert!(supervisor.exited);
    let worker = agents.iter().find(|agent| !agent.supervisor).unwrap();
    assert_eq!(worker.project_root, "/repos/weather");
    assert_eq!(worker.project, "weather/calm-otter-1f2e");
    assert_eq!(worker.worktree_checkout.as_deref(), Some(worktree));
    assert_eq!(worker.info.pane_id, "t4");
    assert_eq!(worker.info.workspace_id, "corgi-weather");
    assert_eq!(worker.info.state, AgentState::Done);
    assert_eq!(worker.info.state_change_seq, 3);
    assert_eq!(worker.info.kind(), "claude");
    assert_eq!(worker.task, "Fix the cache");
    assert_eq!(worker.model.as_deref(), Some("Opus 4.6"));
    assert_eq!(worker.context_percent, Some(10));
    assert_eq!(
        worker.info.tokens[CORGI_REQUEST_TOKEN].as_str(),
        "20261109-cache"
    );
    assert_eq!(worker.info.tokens[MERGE_TOKEN].as_str(), "done 3");
    assert!(worker.ready_to_merge());
}

#[test]
fn a_tag_set_without_where_applies_while_the_agent_rests() {
    let mut terminal = Terminal {
        address: "t4".into(),
        state: "live".into(),
        labels: [(PROJECT_LABEL, "/r"), (TAG_LABEL, MERGE_TAG)]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        status: Some(Status {
            state: "idle".into(),
            ..Status::default()
        }),
        turn: 5,
        ..Terminal::default()
    };
    assert!(dashboard_agents(std::slice::from_ref(&terminal), None)[0].ready_to_merge());
    terminal.status.as_mut().unwrap().state = "working".into();
    assert!(!dashboard_agents(&[terminal], None)[0].ready_to_merge());
}

#[test]
fn tag_on_hc_checks_as_on_herdr_and_writes_labels() {
    let scratch = ScratchDir::new("hc-tag");
    let root = git_repo(&scratch.join("weather"));
    let worktree = scratch.join("worktree-a");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "a",
            &worktree.to_string_lossy(),
        ],
    );
    let terminal = |status: &str| Terminal {
        address: "t4".into(),
        name: Some("weather".into()),
        state: "live".into(),
        labels: [(PROJECT_LABEL, root.to_string_lossy().as_ref())]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        status: Some(Status {
            state: status.into(),
            ..Status::default()
        }),
        cwd: worktree.to_string_lossy().into_owned(),
        turn: 2,
        ..Terminal::default()
    };
    let hc = FakeHc::new();
    hc.answer("label", &terminal_json("t4", "weather", &[], "done"));
    let options = |merge| TagOptions {
        name: "weather".into(),
        merge,
        project: None,
    };
    let root = root.to_string_lossy().into_owned();
    let working = dashboard_agents(&[terminal("working")], None);
    let refused = tag(&hc.client(), &working, &root, &options(true)).unwrap_err();
    assert!(
        refused.to_string().contains("weather is working"),
        "{refused}"
    );
    let done = dashboard_agents(&[terminal("done")], None);
    let empty = tag(&hc.client(), &done, &root, &options(true)).unwrap_err();
    assert!(empty.to_string().contains("no commits ahead"), "{empty}");
    git(&worktree, &["commit", "-q", "--allow-empty", "-m", "work"]);
    assert_eq!(
        tag(&hc.client(), &done, &root, &options(true)).unwrap(),
        "Tagged weather ready to merge: 1 commit ahead of main"
    );
    assert_eq!(
        tag(&hc.client(), &done, &root, &options(false)).unwrap(),
        "weather had no merge tag"
    );
    assert_eq!(
        hc.calls(),
        [
            "label|t4|corgi.tag=merge|corgi.tag.at=done 2|--json",
            "label|t4|corgi.tag=|corgi.tag.at=|--json",
        ]
    );
}

#[test]
fn each_harness_starts_with_its_first_prompt_and_resumes_by_its_session() {
    let plan = |harness| worker_plan(Path::new("/repos/weather"), harness, Checkout::Worktree);
    let claude = harness_command(
        &plan(Harness::Claude),
        Some("id-1"),
        &[],
        Start::Task("Do it"),
    );
    assert_eq!(
        claude[1..],
        ["--session-id", "id-1", "--model", "haiku", "--", "Do it"]
    );
    assert!(claude[0].ends_with("claude"), "{}", claude[0]);
    let role = vec!["--add-dir".to_string(), "/state".to_string()];
    let fresh = harness_command(&plan(Harness::Claude), Some("id-1"), &role, Start::Fresh);
    // The prompt the script appends comes after `--`, never as a directory.
    assert_eq!(
        fresh[1..],
        [
            "--session-id",
            "id-1",
            "--model",
            "haiku",
            "--add-dir",
            "/state",
            "--"
        ]
    );
    let resume = harness_command(&plan(Harness::Claude), None, &role, Start::Resume("id-1"));
    assert_eq!(
        resume[1..],
        [
            "--resume",
            "id-1",
            "--model",
            "haiku",
            "--add-dir",
            "/state"
        ]
    );
    let codex = harness_command(&plan(Harness::Codex), None, &[], Start::Task("Do it"));
    assert_eq!(
        codex[1..],
        ["--no-daemon", "--model", "haiku", "--", "Do it"]
    );
    let codex = harness_command(&plan(Harness::Codex), None, &[], Start::Resume("r"));
    assert_eq!(
        codex[1..],
        ["--no-daemon", "resume", "r", "--model", "haiku"]
    );
    let opencode = harness_command(&plan(Harness::OpenCode), None, &[], Start::Task("Do it"));
    assert_eq!(opencode[1..], ["--model", "haiku", "--prompt", "Do it"]);
    assert!(harness_refusal(&Harness::Claude).is_none());
    assert!(harness_refusal(&Harness::Gemini).is_some());
}

#[test]
fn the_agent_script_drops_claude_code_and_herdr_variables_and_runs_in_its_folder() {
    let scratch = ScratchDir::new("hc-script");
    let dir = scratch.canonicalize().unwrap();
    let argv = agent_argv(
        &dir,
        None,
        &["/bin/sh".into(), "-c".into(), "pwd; env".into()],
    );
    let output = Command::new(&argv[0])
        .args(&argv[1..])
        .env("CLAUDE_CODE_CHILD_SESSION", "1")
        .env("CLAUDE_CODE_SESSION_ID", "s")
        .env("CLAUDECODE", "1")
        .env("CLAUDE_PID", "1")
        .env("HERDR_PANE_ID", "w1:p1")
        .env("HERDR_SOCKET_PATH", "/x.sock")
        .env("CORGI_KEPT", "yes")
        .env("CLAUDE_CONFIG_DIR_KEPT", "yes")
        .output()
        .unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some(dir.to_string_lossy().as_ref()));
    let names: Vec<&str> = lines.filter_map(|line| line.split('=').next()).collect();
    for gone in [
        "CLAUDE_CODE_CHILD_SESSION",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDECODE",
        "CLAUDE_PID",
        "HERDR_PANE_ID",
        "HERDR_SOCKET_PATH",
    ] {
        assert!(!names.contains(&gone), "{gone} was kept");
    }
    assert!(names.contains(&"CORGI_KEPT"));
    assert!(names.contains(&"CLAUDE_CONFIG_DIR_KEPT"));
    assert!(AGENT_SCRIPT.starts_with("for v in"));
}

#[test]
fn the_agent_script_passes_a_prompt_file_on_as_the_last_argument() {
    let scratch = ScratchDir::new("hc-prompt");
    let prompt = scratch.join("prompt.txt");
    fs::write(
        &prompt,
        "Start your session.\n\nIt's 'quoted' and $(not run).\n",
    )
    .unwrap();
    let argv = agent_argv(
        &scratch,
        Some(&prompt),
        &[
            "/bin/sh".into(),
            "-c".into(),
            r#"printf '%s|' "$@""#.into(),
            "sh".into(),
            "--".into(),
        ],
    );
    // Typed as one line into a shell, as a supervisor's is.
    let output = Command::new("/bin/sh")
        .args(["-c", &shell_line(&argv)])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "--|Start your session.\n\nIt's 'quoted' and $(not run).|"
    );
}

#[test]
fn a_shell_line_quotes_only_what_needs_it() {
    let words: Vec<String> = ["/bin/claude", "--model", "opus[1m]", "it's", "", "a b"]
        .map(String::from)
        .into();
    assert_eq!(
        shell_line(&words),
        r#"/bin/claude --model 'opus[1m]' 'it'\''s' '' 'a b'"#
    );
    let output = Command::new("/bin/sh")
        .args(["-c", &format!("printf '%s|' {}", shell_line(&words))])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "/bin/claude|--model|opus[1m]|it's||a b|"
    );
}

#[test]
fn a_worker_starts_in_its_checkout_with_corgis_labels() {
    let scratch = ScratchDir::new("hc-worker");
    let root = git_repo(&scratch.join("weather"));
    let hc = FakeHc::new();
    hc.answer(
        "run",
        r#"{"address":"t5","name":"weather","state":"live","turn":0}"#,
    );
    hc.answer("ls", r#"{"terminals":[]}"#);
    // It reports at once, so no folder-trust question is waited for.
    hc.answer("get.t5", &terminal_json("t5", "weather", &[], "idle"));
    let plan = worker_plan(&root, Harness::Claude, Checkout::ProjectRoot);
    let root_text = root.to_string_lossy().into_owned();
    let launched = launch_worker(&hc.client(), &plan, &root_text, &mut Silent).unwrap();
    assert_eq!(launched.terminal.address, "t5");
    assert_eq!(launched.cwd, root);
    assert_eq!(launched.checkout, Checkout::ProjectRoot);
    let session = launched.session.unwrap();
    let calls = hc.calls();
    let run = calls.iter().find(|call| call.starts_with("run|")).unwrap();
    let space = project_space(&root_text);
    for part in [
        format!("|--cwd|{root_text}|"),
        format!("|--label|corgi.project={root_text}|"),
        "|--label|corgi.role=worker|".into(),
        "|--label|corgi.task=Fix the cache|".into(),
        "|--label|corgi.harness=claude|".into(),
        format!("|--label|herdr.space={space}|"),
        format!("|--label|corgi.session={session}|"),
        "|--label|corgi.request=20261109-cache|".into(),
        format!("|--json|--|/bin/sh|-c|{AGENT_SCRIPT}|corgi-agent|{root_text}||"),
        format!("|--session-id|{session}|--model|haiku|--|# Fix the cache\n"),
    ] {
        assert!(run.contains(&part), "{part} not in {run}");
    }
    assert!(
        run.contains(&format!(
            "|--label|corgi.resume=/bin/sh -c '{}' corgi-agent {root_text} '' ",
            AGENT_SCRIPT.replace('\'', r"'\''")
        )),
        "{run}"
    );
    assert!(
        run.contains(&format!(" --resume {session} --model haiku|")),
        "{run}"
    );
}

#[test]
fn worktrees_go_where_herdr_puts_them_on_a_branch_of_their_own() {
    let scratch = ScratchDir::new("hc-worktree");
    let root = git_repo(&scratch.join("weather"));
    let base = scratch.join("worktrees");
    let (path, branch) = create_worktree_in(&base, &root).unwrap();
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    assert!(path.starts_with(base.join("weather")), "{}", path.display());
    let slug = name.strip_prefix("worktree-").unwrap();
    assert_eq!(branch, format!("worktree/{slug}"));
    assert_eq!(slug.split('-').count(), 3, "{slug}");
    assert!(path.join(".git").is_file());
    let (second, _) = create_worktree_in(&base, &root).unwrap();
    assert_ne!(second, path);
}

#[test]
fn small_helpers_name_what_they_should() {
    assert_eq!(
        task_title("\n# Corgi on hc: client\n\nMore"),
        "Corgi on hc: client"
    );
    let long = "x".repeat(100);
    assert_eq!(task_title(&long).chars().count(), 80);
    assert!(task_title(&long).ends_with('…'));
    let id = new_session_id().unwrap();
    assert_eq!(id.len(), 36);
    assert_eq!(&id[14..15], "4");
    assert_ne!(id, new_session_id().unwrap());
    let space = project_space("/repos/weather");
    assert!(space.starts_with("corgi-weather-"), "{space}");
    assert_eq!(space, project_space("/repos/weather"));
    assert_ne!(space, project_space("/other/weather"));
}

#[test]
fn the_cores_socket_is_looked_for_where_hc_puts_it() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| std::ffi::OsString::from(value))
        }
    };
    let home = Some(Path::new("/home/me"));
    assert_eq!(
        socket_candidates(&env(&[]), home),
        [PathBuf::from(
            "/home/me/.local/state/herdr-core/default/default.sock"
        )]
    );
    assert_eq!(
        socket_candidates(
            &env(&[("XDG_RUNTIME_DIR", "/run/u"), ("HC_SESSION", "s")]),
            home
        ),
        [
            PathBuf::from("/run/u/herdr-core/s.sock"),
            PathBuf::from("/home/me/.local/state/herdr-core/s/s.sock")
        ]
    );
    assert_eq!(
        socket_candidates(&env(&[("HC_SOCKET", "/x.sock")]), home),
        [PathBuf::from("/x.sock")]
    );
}

/// Claude Code's folder-trust question about `folder`, with Yes or No
/// highlighted.
fn trust_screen(folder: &str, yes: bool) -> String {
    let (no, yes) = if yes { (" ", "❯") } else { ("❯", " ") };
    let screen = format!(
        " Accessing workspace:\n\n {folder}\n\n Quick safety check: Is this a project you created \
         or one you trust? (Like your own code)\n\n {no} No, exit\n {yes} Yes, I trust this folder\n"
    );
    serde_json::json!({"text": screen, "complete": true, "how": "screen"}).to_string()
}

#[test]
fn the_trust_question_is_answered_yes_only_once_yes_shows_and_a_lost_key_is_pressed_again() {
    let scratch = ScratchDir::new("hc-trust");
    let folder = scratch.canonicalize().unwrap();
    let text = folder.to_string_lossy().into_owned();
    let hc = FakeHc::new();
    // The first Down is lost; the second one moves to Yes; Enter answers.
    hc.answer("read.1", &trust_screen(&text, false));
    hc.answer("read.2", &trust_screen(&text, false));
    hc.answer("read.3", &trust_screen(&text, false));
    hc.answer("read.4", &trust_screen(&text, false));
    hc.answer("read.5", &trust_screen(&text, false));
    hc.answer("read.6", &trust_screen(&text, false));
    hc.answer("read.7", &trust_screen(&text, false));
    hc.answer("read.8", &trust_screen(&text, false));
    hc.answer("read.9", &trust_screen(&text, false));
    hc.answer("read.10", &trust_screen(&text, true));
    hc.answer("read", r#"{"text":"❯ ","complete":true,"how":"screen"}"#);
    let mut progress = Recorded(Vec::new());
    answer_folder_trust(&hc.client(), "t5", &folder, "w", &mut progress, &|_| true);
    let keys: Vec<String> = hc
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("keys|"))
        .collect();
    assert_eq!(
        keys,
        [
            "keys|t5|down|--json",
            "keys|t5|down|--json",
            "keys|t5|enter|--json"
        ]
    );
    assert_eq!(progress.0.len(), 1, "{:?}", progress.0);
}

#[test]
fn a_trust_question_corgi_did_not_create_the_project_for_stays_with_the_user() {
    let scratch = ScratchDir::new("hc-trust-not");
    let folder = scratch.canonicalize().unwrap();
    let hc = FakeHc::new();
    hc.answer("read", &trust_screen(&folder.to_string_lossy(), false));
    let mut progress = Recorded(Vec::new());
    answer_folder_trust(&hc.client(), "t5", &folder, "w", &mut progress, &|_| false);
    assert!(hc.calls().iter().all(|call| !call.starts_with("keys|")));
    assert!(
        progress.0[0].contains("asks whether to trust"),
        "{:?}",
        progress.0
    );
    // Nor is another folder's question answered.
    let hc = FakeHc::new();
    hc.answer("read", &trust_screen("/elsewhere", true));
    answer_folder_trust(&hc.client(), "t5", &folder, "w", &mut progress, &|_| true);
    assert!(hc.calls().iter().all(|call| !call.starts_with("keys|")));
}
