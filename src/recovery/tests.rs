use super::*;
use crate::test_support::ScratchDir;
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    },
    sync::{Arc, Mutex},
    time::Instant,
};

const CODEX_ID: &str = "00000000-0000-4000-8000-000000000001";
const CLAUDE_ID: &str = "00000000-0000-4000-8000-000000000002";

struct Fixture {
    dir: ScratchDir,
    client: HerdrClient,
    snapshot: Snapshot,
    runtime: Runtime,
}
impl Fixture {
    fn new() -> Self {
        let dir = ScratchDir::new("recovery");
        let client = HerdrClient::from_socket_path(dir.join("socket"))
            .with_marker_record(dir.join("state/markers/session.json"));
        let mut entries = Vec::new();
        let mut stores = BTreeMap::new();
        for (kind, id, n) in [("codex", CODEX_ID, 1), ("claude", CLAUDE_ID, 2)] {
            let root = dir.join(kind);
            let conversation = root.join(if kind == "codex" {
                format!("sessions/2026/10/21/rollout-fixture-{id}.jsonl")
            } else {
                format!("projects/fixture/{id}.jsonl")
            });
            fs::create_dir_all(conversation.parent().unwrap()).unwrap();
            let content = if kind == "codex" {
                format!(
                    "{}\n{}\n",
                    json!({"type":"session_meta","payload":{"id":id}}),
                    json!({"type":"turn_context","payload":{"model":"gpt-test","effort":"high"}})
                )
            } else {
                format!(
                    "{}\n{}\n{}\n",
                    json!({"type":"file-history-snapshot"}),
                    json!({"type":"user","sessionId":id,"message":{"content":"PRIVATE_TASK_MUST_NOT_BE_SAVED"}}),
                    json!({"type":"assistant","sessionId":id,"effortLevel":"high","message":{"model":"claude-test","content":[{"text":"PRIVATE_REPLY"}]}})
                )
            };
            fs::write(&conversation, content).unwrap();
            let cwd = dir.join(format!("project-{n}"));
            fs::create_dir_all(&cwd).unwrap();
            let state = dir.join(format!("memory-{n}"));
            fs::create_dir_all(&state).unwrap();
            let role = state.join("ROLE.md");
            fs::write(
                &role,
                "Synthetic supervisor role. State access must survive resume.",
            )
            .unwrap();
            entries.push(Entry {
                name: format!("fixture-{kind}"),
                kind: kind.into(),
                native_id: Some(id.into()),
                native_store: Some(root.clone()),
                native_file: Some(conversation),
                cwd: cwd.clone(),
                shell_cwd: cwd.clone(),
                checkout: cwd.clone(),
                project: cwd.to_string_lossy().into_owned(),
                role: "supervisor".into(),
                supervisor: Some(Supervisor {
                    state_dir: state,
                    role_fingerprint: native::fingerprint(&role).unwrap(),
                    role_file: role,
                    command: None,
                }),
                workspace: format!("w{n}"),
                workspace_label: format!("project {n}"),
                tab: format!("t{n}"),
                tab_label: "Supervisor".into(),
                pane: format!("w{n}:p1"),
                linked_checkout: false,
                git_common: None,
                branch: None,
                head: None,
                previous_merge: false,
                previous_handover: false,
                pending_inbox_ids: Vec::new(),
                configs: Vec::new(),
                request_id: None,
                workspace_tokens: BTreeMap::new(),
                state: AgentState::Idle,
                settings: Settings {
                    model: if kind == "codex" {
                        "gpt-test"
                    } else {
                        "claude-test"
                    }
                    .into(),
                    effort: "high".into(),
                    sandbox: (kind == "codex").then(|| "workspace-write".into()),
                    ..Settings::default()
                },
                warnings: Vec::new(),
            });
            stores.insert(kind.into(), root);
        }
        let snapshot = Snapshot {
            version: 1,
            id: "20261021-000000-fixture".into(),
            created: unix_now(),
            sequence: 0,
            scope: client.socket_path().to_string_lossy().into_owned(),
            uid: unsafe { libc::geteuid() },
            host: store::host().unwrap(),
            boot: "prior-boot".into(),
            manual_prepared: true,
            entries,
        };
        let runtime = Runtime {
            boot: store::boot().unwrap(),
            manual: true,
            stores,
            clis: BTreeSet::from(["codex".into(), "claude".into()]),
        };
        Self {
            dir,
            client,
            snapshot,
            runtime,
        }
    }
    fn store(&self) -> Store {
        Store::for_client(&self.client).unwrap()
    }
}

#[derive(Default)]
struct RuntimeState {
    agents: Vec<Value>,
    methods: Vec<String>,
    starts: usize,
    unknown_start: bool,
    occupied: bool,
    fail_metadata: bool,
}
struct Server {
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
    state: Arc<Mutex<RuntimeState>>,
}
impl Server {
    fn new(f: &Fixture) -> Self {
        let listener = UnixListener::bind(f.client.socket_path()).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let state = Arc::new(Mutex::new(RuntimeState::default()));
        let current = state.clone();
        let entries = f.snapshot.entries.clone();
        let join = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let (stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                // On macOS/BSD an accepted stream inherits O_NONBLOCK from the
                // listener (unlike Linux), so read_line below would fail with
                // WouldBlock instead of honoring the read timeout set here.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                let method = request["method"].as_str().unwrap();
                let mut s = current.lock().unwrap();
                s.methods.push(method.into());
                let result = match method {
                    "session.snapshot" => {
                        json!({"type":"session_snapshot","snapshot":{"version":"test","protocol":22,"agents":s.agents,"workspaces":entries.iter().map(|e|json!({"workspace_id":e.workspace,"label":e.workspace_label})).collect::<Vec<_>>(),"tabs":entries.iter().map(|e|json!({"workspace_id":e.workspace,"tab_id":e.tab,"label":e.tab_label})).collect::<Vec<_>>(),"panes":entries.iter().map(|e|json!({"workspace_id":e.workspace,"tab_id":e.tab,"pane_id":e.pane,"cwd":e.cwd,"agent_session":{"agent":e.kind,"kind":"id","value":e.native_id,"source":"herdr:test"}})).collect::<Vec<_>>()}})
                    }
                    "pane.process_info" => {
                        let e = entries
                            .iter()
                            .find(|e| e.pane == request["params"]["pane_id"].as_str().unwrap())
                            .unwrap();
                        json!({"type":"pane_process_info","process_info":{"pane_id":e.pane,"shell_pid":100,"foreground_process_group_id":if s.occupied{200}else{100},"foreground_processes":[{"pid":100,"name":"zsh","cwd":e.cwd}]}})
                    }
                    "agent.start" => {
                        s.starts += 1;
                        let params = &request["params"];
                        let e = entries
                            .iter()
                            .find(|e| e.pane == params["pane_id"].as_str().unwrap())
                            .unwrap();
                        assert_eq!(params["kind"], e.kind);
                        let args = params["args"].as_array().unwrap();
                        assert!(args.contains(&json!(e.native_id)));
                        assert!(!args.contains(&json!("--last")));
                        assert!(!args.contains(&json!("--continue")));
                        assert!(!args.contains(&json!("--worktree")));
                        if e.kind == "codex" && e.settings.sandbox.as_deref() == Some("read-only") {
                            assert!(!args.contains(&json!("--add-dir")));
                            assert!(args.iter().any(|arg| {
                                arg.as_str()
                                    .is_some_and(|s| s.starts_with("developer_instructions="))
                            }));
                        } else {
                            assert!(args.contains(&json!("--add-dir")));
                            assert!(
                                args.contains(&json!(e.supervisor.as_ref().unwrap().state_dir))
                            );
                        }
                        let agent = json!({"agent":e.kind,"name":params["name"],"workspace_id":e.workspace,"tab_id":e.tab,"pane_id":e.pane,"cwd":e.cwd,"agent_status":"idle","interactive_ready":true,"launch_pending":false,"agent_session":{"source":"herdr:test","agent":e.kind,"kind":"id","value":e.native_id}});
                        if !s.unknown_start {
                            s.agents.push(agent.clone());
                        }
                        json!({"type":"agent_started","agent":agent,"argv":params["args"]})
                    }
                    "pane.report_metadata" => json!({"type":"pane_metadata_reported"}),
                    "server.reload_config" => {
                        json!({"type":"config_reload","status":"applied","diagnostics":[]})
                    }
                    _ => panic!(
                        "unexpected method {method}; normal recovery must not create placements or prompt"
                    ),
                };
                let response = if (method == "agent.start" && s.unknown_start)
                    || (method == "pane.report_metadata" && s.fail_metadata)
                {
                    json!({"id":request["id"],"error":{"code":"delivery_unknown","message":"connection lost after queueing"}})
                } else {
                    json!({"id":request["id"],"result":result})
                };
                drop(s);
                serde_json::to_writer(reader.get_mut(), &response).unwrap();
                reader.get_mut().write_all(b"\n").unwrap();
            }
        });
        Self {
            stop,
            join: Some(join),
            state,
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.join.take().unwrap().join().unwrap();
    }
}

#[test]
fn both_harnesses_resume_in_original_terminals_with_no_placement_or_prompt() {
    let f = Fixture::new();
    let server = Server::new(&f);
    f.store().save(&f.snapshot).unwrap();
    let rows = plan_in(&f.client, &f.snapshot, &f.runtime).unwrap();
    let selected = rows
        .into_iter()
        .enumerate()
        .map(|(i, r)| (i, r.target.unwrap(), r.differences))
        .collect::<Vec<_>>();
    let mut outcomes = Vec::new();
    restore_in(
        &f.client,
        &f.snapshot,
        &selected,
        &f.runtime,
        &AtomicBool::new(false),
        |m| outcomes.push(m),
    )
    .unwrap();
    assert_eq!(
        outcomes
            .iter()
            .filter(|m| m.contains("same conversation ready"))
            .count(),
        2
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.starts, 2);
    for (e, agent) in f.snapshot.entries.iter().zip(&state.agents) {
        assert_eq!(agent["workspace_id"], e.workspace);
        assert_eq!(agent["tab_id"], e.tab);
        assert_eq!(agent["pane_id"], e.pane);
        assert_eq!(
            fs::read_to_string(&e.supervisor.as_ref().unwrap().role_file).unwrap(),
            "Synthetic supervisor role. State access must survive resume."
        );
    }
    assert!(
        state
            .methods
            .iter()
            .all(|m| !m.contains("create") && !m.contains("prompt"))
    );
    drop(state);
    let again = plan_in(&f.client, &f.snapshot, &f.runtime).unwrap();
    assert!(
        again
            .iter()
            .all(|r| r.target.is_none() && r.reason.contains("Already running"))
    );
    assert_eq!(server.state.lock().unwrap().starts, 2);
}

#[test]
fn accepted_but_unknown_start_is_not_repeated_even_in_another_snapshot() {
    let f = Fixture::new();
    let server = Server::new(&f);
    server.state.lock().unwrap().unknown_start = true;
    let target = plan_in(&f.client, &f.snapshot, &f.runtime).unwrap()[0]
        .target
        .clone()
        .unwrap();
    restore_in(
        &f.client,
        &f.snapshot,
        &[(0, target.clone(), Vec::new())],
        &f.runtime,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let mut other = f.snapshot.clone();
    other.id.push_str("-retry");
    let rows = plan_in(&f.client, &other, &f.runtime).unwrap();
    assert!(rows[0].reason.contains("outcome unresolved"));
    assert!(
        restore_in(
            &f.client,
            &other,
            &[(0, target, Vec::new())],
            &f.runtime,
            &AtomicBool::new(false),
            |_| {}
        )
        .is_err()
    );
    assert_eq!(server.state.lock().unwrap().starts, 1);
}

#[test]
fn occupied_original_has_only_an_explicit_alternative_and_auto_resume_blocks_missing() {
    let mut f = Fixture::new();
    let server = Server::new(&f);
    server.state.lock().unwrap().occupied = true;
    let rows = plan_in(&f.client, &f.snapshot, &f.runtime).unwrap();
    assert!(
        rows.iter()
            .all(|r| r.target.is_none() && matches!(r.alternative, Some(Target::NewTab { .. })))
    );
    f.runtime.manual = false;
    assert!(
        plan_in(&f.client, &f.snapshot, &f.runtime)
            .unwrap()
            .iter()
            .all(|r| r.target.is_none()
                && r.alternative.is_none()
                && r.reason.contains("automatic resume"))
    );
}

#[test]
fn missing_identity_and_cli_and_changed_supervisor_role_are_skipped() {
    let mut f = Fixture::new();
    let _server = Server::new(&f);
    f.snapshot.entries[0].native_id = None;
    f.runtime.clis.remove("claude");
    let rows = plan_in(&f.client, &f.snapshot, &f.runtime).unwrap();
    assert!(rows[0].reason.contains("missing native ID"));
    assert!(rows[1].reason.contains("CLI missing"));
    f.runtime.clis.insert("claude".into());
    fs::write(
        &f.snapshot.entries[1].supervisor.as_ref().unwrap().role_file,
        "changed",
    )
    .unwrap();
    assert!(
        plan_in(&f.client, &f.snapshot, &f.runtime).unwrap()[1]
            .reason
            .contains("role changed")
    );
}

#[test]
fn exact_lookup_does_not_fall_back_to_a_newer_conversation_or_copy_private_text() {
    let f = Fixture::new();
    for e in &f.snapshot.entries {
        let found = native::locate(
            e.native_store.as_ref().unwrap(),
            &e.kind,
            e.native_id.as_deref().unwrap(),
        )
        .unwrap();
        let metadata = native::metadata(&found, &e.kind, e.native_id.as_deref().unwrap()).unwrap();
        assert_eq!(metadata.effort, "high");
        assert!(
            !serde_json::to_string(&metadata)
                .unwrap()
                .contains("PRIVATE")
        );
        assert!(
            native::locate(
                e.native_store.as_ref().unwrap(),
                &e.kind,
                "ffffffff-ffff-4fff-8fff-ffffffffffff"
            )
            .is_err()
        );
    }
}

#[test]
fn manifests_are_private_durable_and_fail_without_replacing_the_last_good_save() {
    let f = Fixture::new();
    let store = f.store();
    store.save(&f.snapshot).unwrap();
    let file = store
        .0
        .join("snapshots")
        .join(format!("{}.json", f.snapshot.id));
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&store.0).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let bytes = fs::read(&file).unwrap();
    let mut invalid = f.snapshot.clone();
    invalid.version = 99;
    assert!(store.save(&invalid).is_err());
    assert_eq!(fs::read(&file).unwrap(), bytes);
    assert!(!String::from_utf8(bytes).unwrap().contains("PRIVATE"));
    let alias = f.dir.join("alias");
    symlink(&file, &alias).unwrap();
    assert!(store::atomic(&alias, b"bad").is_err());
    assert!(store::read::<Snapshot>(&alias).is_err());
    assert!(store.save(&f.snapshot).is_err());
}

#[test]
fn next_boot_hold_precedes_any_waker_and_save_does_not_pause_current_boot() {
    let mut f = Fixture::new();
    let store = f.store();
    f.snapshot.boot = store::boot().unwrap();
    store.save(&f.snapshot).unwrap();
    let agents = dashboard_agents(&f.snapshot);
    assert!(held_projects(&f.client, &agents).unwrap().is_empty());
    let mut next = f.snapshot.clone();
    next.id.push('z');
    next.boot = "previous-host-boot".into();
    store.save(&next).unwrap();
    assert_eq!(held_projects(&f.client, &agents).unwrap().len(), 2);
    continue_coordination(&f.client, &held_projects(&f.client, &agents).unwrap()).unwrap();
    assert!(held_projects(&f.client, &agents).unwrap().is_empty());
}

fn dashboard_agents(snapshot: &Snapshot) -> Vec<DashboardAgent> {
    snapshot
        .entries
        .iter()
        .map(|e| DashboardAgent {
            info: AgentInfo {
                agent: Some(e.kind.clone()),
                pane_id: e.pane.clone(),
                cwd: Some(e.cwd.to_string_lossy().into_owned()),
                agent_session: Some(crate::model::AgentSession {
                    value: e.native_id.clone().unwrap(),
                    kind: "id".into(),
                    agent: e.kind.clone(),
                    source: "test".into(),
                }),
                ..AgentInfo::default()
            },
            project_root: e.project.clone(),
            supervisor: true,
            ..DashboardAgent::default()
        })
        .collect()
}

#[test]
fn setup_preserves_unrelated_toml_and_comments_without_touching_installed_config() {
    for original in [
        "# hello\n[theme]\nname='light'\n",
        "# keep\n[session]\n# existing\nresume_agents_on_restore = true # explanation\n[other]\nvalue = 5\n",
        "session.resume_agents_on_restore = true\n# rest\n[other]\ntext='keep'\n",
        "[session]\nother=123\n[remote]\nmanage_ssh_config=false\n",
    ] {
        let changed = setup::edit(original).unwrap();
        assert!(changed.contains("false"));
        for line in original.lines().filter(|s| {
            s.starts_with('#')
                || s.contains("keep")
                || s.contains("other=123")
                || s.contains("manage_ssh")
        }) {
            assert!(changed.contains(line));
        }
        assert_eq!(setup::edit(&changed).unwrap(), changed);
    }
}

#[test]
fn prompt_free_adapters_retain_settings_role_and_memory_access() {
    let f = Fixture::new();
    for e in &f.snapshot.entries {
        let args = native::resume_args(e).unwrap();
        assert_eq!(
            args[0],
            if e.kind == "codex" {
                "resume"
            } else {
                "--resume"
            }
        );
        assert_eq!(args[1], e.native_id.as_deref().unwrap());
        assert!(args.contains(&"--model".into()));
        assert!(
            args.contains(&"high".into()) || args.contains(&"model_reasoning_effort=high".into())
        );
        if e.kind == "codex" {
            assert!(
                args.iter()
                    .any(|a| a.starts_with("developer_instructions="))
            );
        } else {
            assert!(args.contains(&"--append-system-prompt-file".into()));
        }
    }
}

#[test]
fn stopped_restore_schedules_nothing() {
    let f = Fixture::new();
    let server = Server::new(&f);
    let selected = plan_in(&f.client, &f.snapshot, &f.runtime)
        .unwrap()
        .into_iter()
        .enumerate()
        .map(|(i, r)| (i, r.target.unwrap(), r.differences))
        .collect::<Vec<_>>();
    let start = Instant::now();
    restore_in(
        &f.client,
        &f.snapshot,
        &selected,
        &f.runtime,
        &AtomicBool::new(true),
        |_| {},
    )
    .unwrap();
    assert_eq!(server.state.lock().unwrap().starts, 0);
    assert!(start.elapsed() < Duration::from_secs(2));
}

pub(crate) fn hold_fixture() -> (ScratchDir, HerdrClient, Snapshot, Vec<DashboardAgent>) {
    let f = Fixture::new();
    let agents = dashboard_agents(&f.snapshot);
    (f.dir, f.client, f.snapshot, agents)
}

#[test]
fn metadata_failure_retries_metadata_without_starting_the_harness_again() {
    let f = Fixture::new();
    let server = Server::new(&f);
    server.state.lock().unwrap().fail_metadata = true;
    let row = plan_in(&f.client, &f.snapshot, &f.runtime)
        .unwrap()
        .remove(0);
    restore_in(
        &f.client,
        &f.snapshot,
        &[(0, row.target.unwrap(), row.differences)],
        &f.runtime,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(server.state.lock().unwrap().starts, 1);
    server.state.lock().unwrap().fail_metadata = false;
    let row = plan_in(&f.client, &f.snapshot, &f.runtime)
        .unwrap()
        .remove(0);
    assert!(matches!(row.target, Some(Target::Metadata { .. })));
    restore_in(
        &f.client,
        &f.snapshot,
        &[(0, row.target.unwrap(), row.differences)],
        &f.runtime,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(server.state.lock().unwrap().starts, 1);
    assert_eq!(
        f.store().journal().unwrap().attempts[&f.snapshot.entries[0].key()].phase,
        "complete"
    );
}

#[test]
fn unconfirmed_inbox_deliveries_stay_quarantined_after_continue() {
    let mut f = Fixture::new();
    let store = f.store();
    let ids = vec!["pending-1".to_string()];
    begin_delivery(
        &store,
        &f.snapshot.scope,
        &f.snapshot.entries[0].project,
        &ids,
    )
    .unwrap();
    assert!(
        begin_delivery(
            &store,
            &f.snapshot.scope,
            &f.snapshot.entries[0].project,
            &ids
        )
        .is_err()
    );
    finish_delivery(
        &store,
        &f.snapshot.scope,
        &f.snapshot.entries[0].project,
        &ids,
    )
    .unwrap();
    f.snapshot.entries[0].pending_inbox_ids = ids.clone();
    store.save(&f.snapshot).unwrap();
    let agents = dashboard_agents(&f.snapshot);
    assert!(
        begin_delivery(
            &store,
            &f.snapshot.scope,
            &f.snapshot.entries[0].project,
            &["must-not-dispatch-while-held".into()]
        )
        .is_err()
    );
    continue_coordination(&f.client, &held_projects(&f.client, &agents).unwrap()).unwrap();
    assert!(
        begin_delivery(
            &store,
            &f.snapshot.scope,
            &f.snapshot.entries[0].project,
            &ids
        )
        .is_err()
    );
    begin_delivery(
        &store,
        &f.snapshot.scope,
        &f.snapshot.entries[0].project,
        &["new-explicit-item".into()],
    )
    .unwrap();
    let journal = fs::read_to_string(store.0.join("progress.json")).unwrap();
    assert!(!journal.contains("PRIVATE_TASK"));
}

#[test]
fn latest_settings_and_pending_native_work_are_read_as_metadata_only() {
    let f = Fixture::new();
    let _server = Server::new(&f);
    let e = &f.snapshot.entries[0];
    let file = e.native_file.as_ref().unwrap();
    let mut writer = fs::OpenOptions::new().append(true).open(file).unwrap();
    writeln!(writer,"{}",json!({"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"model":"new-model","reasoning_effort":"low","approval_policy":"on-request"}}})).unwrap();
    writeln!(
        writer,
        "{}",
        json!({"type":"event_msg","payload":{"type":"task_started"}})
    )
    .unwrap();
    let metadata = native::metadata(file, "codex", CODEX_ID).unwrap();
    assert_eq!(metadata.model, "new-model");
    assert_eq!(metadata.effort, "low");
    assert!(metadata.native_pending);
    assert!(
        plan_in(&f.client, &f.snapshot, &f.runtime).unwrap()[0]
            .reason
            .contains("pending turn/input")
    );
    writeln!(
        writer,
        "{}",
        json!({"type":"event_msg","payload":{"type":"task_complete"}})
    )
    .unwrap();
    assert!(
        !native::metadata(file, "codex", CODEX_ID)
            .unwrap()
            .native_pending
    );
}

#[test]
fn actual_io_failure_and_wrong_machine_cannot_replace_or_execute_a_snapshot() {
    let f = Fixture::new();
    let store = f.store();
    store.save(&f.snapshot).unwrap();
    let file = store
        .0
        .join("snapshots")
        .join(format!("{}.json", f.snapshot.id));
    let before = fs::read(&file).unwrap();
    assert!(store::atomic(&file.join("not-a-directory"), b"bad").is_err());
    assert_eq!(fs::read(&file).unwrap(), before);
    let mut other = f.snapshot.clone();
    other.host = "different-host".into();
    assert!(plan_in(&f.client, &other, &f.runtime).is_err());
    other = f.snapshot.clone();
    other.version = 999;
    assert!(plan_in(&f.client, &other, &f.runtime).is_err());
}

#[test]
fn setup_checks_and_reloads_only_the_explicit_isolated_config() {
    let f = Fixture::new();
    let _server = Server::new(&f);
    let path = f.dir.join("isolated-herdr.toml");
    fs::write(&path, "# retained\n[theme]\nname='test'\n").unwrap();
    let change = setup::prepare(path.clone()).unwrap();
    assert!(
        setup::apply_with(&f.client, &change, |staged| {
            assert_ne!(staged, path);
            assert!(
                fs::read_to_string(staged)
                    .unwrap()
                    .contains("resume_agents_on_restore = false")
            );
            Ok(false)
        })
        .is_err()
    );
    assert!(!fs::read_to_string(&path).unwrap().contains("resume_agents"));
    setup::apply_with(&f.client, &change, |_| Ok(true)).unwrap();
    assert!(!setup::configured(&path).unwrap());
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("# retained"));
    assert!(text.contains("name='test'"));
    let backups = fs::read_dir(&f.dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|v| v == "bak"))
        .count();
    assert_eq!(backups, 1);
    let proof = f.store().0.join("manual-setup.json");
    let before = fs::read(&proof).unwrap();
    setup::apply_with(&f.client, &setup::prepare(path).unwrap(), |_| Ok(true)).unwrap();
    assert_eq!(
        fs::read(proof).unwrap(),
        before,
        "reconfirmation must retain the original start boundary"
    );
}

#[test]
fn saved_order_survives_a_backward_wall_clock_change() {
    let mut f = Fixture::new();
    let store = f.store();
    store.save(&f.snapshot).unwrap();
    f.snapshot.id = "20000101-000000-later-save".into();
    f.snapshot.created = 1;
    store.save(&f.snapshot).unwrap();
    let saved = store.snapshots().unwrap();
    assert_eq!(saved[0].id, f.snapshot.id);
    assert!(saved[0].sequence > saved[1].sequence);
}

#[test]
fn independent_regression_latest_codex_permission_profile_supersedes_old_turn() {
    let f = Fixture::new();
    let e = &f.snapshot.entries[0];
    let file = e.native_file.as_ref().unwrap();
    let mut writer = fs::OpenOptions::new().append(true).open(file).unwrap();
    writeln!(
        writer,
        "{}",
        json!({
            "type": "turn_context",
            "payload": {"sandbox_policy": {"type": "read-only"}}
        })
    )
    .unwrap();
    // Shape observed in the installed CLI's real synthetic conversation,
    // emitted on interactive resume without a new turn_context/task.
    writeln!(
        writer,
        "{}",
        json!({
            "type": "event_msg",
            "payload": {"type": "thread_settings_applied", "thread_settings": {
                "model": "gpt-6.1-sol", "reasoning_effort": "high",
                "approval_policy": "on-request", "cwd": e.cwd,
                "runtime_workspace_roots": [e.cwd, f.dir.join("extra")],
                "permission_profile": {"type": "managed",
                    "file_system": {"type": "restricted", "entries": [
                        {"path": {"type": "special", "value": {"kind": "root"}}, "access": "read"},
                        {"path": {"type": "path", "path": e.cwd}, "access": "write"},
                        {"path": {"type": "path", "path": f.dir.join("extra")}, "access": "write"}
                    ]}, "network": "enabled"}
            }}
        })
    )
    .unwrap();
    let observed = native::metadata(file, "codex", CODEX_ID).unwrap();
    assert_eq!(observed.sandbox.as_deref(), Some("workspace-write"));
    assert_eq!(observed.network, Some(true));
    assert!(observed.writable.contains(&f.dir.join("extra")));
}

#[test]
fn independent_regression_read_only_supervisor_has_no_writable_root_argument() {
    let mut f = Fixture::new();
    let e = &mut f.snapshot.entries[0];
    e.settings.sandbox = Some("read-only".into());
    let args = native::resume_args(e).unwrap();
    assert!(
        args.iter()
            .any(|arg| arg.starts_with("developer_instructions="))
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--sandbox", "read-only"])
    );
    assert!(
        !args.contains(&"--add-dir".into()),
        "--add-dir is rejected with read-only; retaining read access must not request writable roots"
    );
}

#[test]
fn independent_regression_save_does_not_release_an_active_next_boot_hold() {
    let mut f = Fixture::new();
    let store = f.store();
    store.save(&f.snapshot).unwrap();
    let agents = dashboard_agents(&f.snapshot);
    let held = held_projects(&f.client, &agents).unwrap();
    assert_eq!(held.len(), 2);
    f.snapshot.id.push_str("-new-save");
    f.snapshot.boot = store::boot().unwrap();
    store.save(&f.snapshot).unwrap();
    assert_eq!(
        held_projects(&f.client, &agents).unwrap(),
        held,
        "only deliberate Continue may release an existing recovery hold"
    );
}

fn append_codex(entry: &Entry, record: Value) {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(entry.native_file.as_ref().unwrap())
        .unwrap();
    writeln!(file, "{record}").unwrap();
}

fn native_settings(entry: &Entry) -> Settings {
    native::metadata(entry.native_file.as_ref().unwrap(), "codex", CODEX_ID).unwrap()
}

#[test]
fn chronological_permissions_replace_roots_network_and_launch_profile_defaults() {
    let mut f = Fixture::new();
    let e = &mut f.snapshot.entries[0];
    e.settings.writable = vec![f.dir.to_path_buf()];
    e.settings.network = Some(true);
    append_codex(
        e,
        json!({"type":"turn_context", "payload":{
            "cwd":e.cwd, "sandbox_policy":{"type":"workspace-write", "writable_roots":[f.dir.to_path_buf()], "network_access":true}
        }}),
    );
    append_codex(
        e,
        json!({"type":"event_msg", "payload":{
            "type":"thread_settings_applied", "thread_settings":{
                "approval_policy":"on-request", "approvals_reviewer":"auto_review",
                "permission_profile":{"type":"managed", "network":"restricted", "file_system":{
                    "type":"restricted", "entries":[{"path":{"type":"special","value":{"kind":"root"}},"access":"read"}]
                }}
            }
        }}),
    );
    let observed = native_settings(e);
    assert!(observed.permission_error.is_none());
    e.settings.apply_native(observed);
    assert_eq!(e.settings.sandbox.as_deref(), Some("read-only"));
    assert_eq!(e.settings.network, Some(false));
    assert!(e.settings.writable.is_empty());
    let args = native::resume_args(e).unwrap();
    assert!(!args.contains(&"--add-dir".into()));
    assert!(args.contains(&"approvals_reviewer=\"auto_review\"".into()));
    assert!(
        args.iter()
            .any(|s| s.starts_with("developer_instructions="))
    );

    // A later legacy turn is authoritative too, including its temp controls.
    let memory = e.supervisor.as_ref().unwrap().state_dir.clone();
    append_codex(
        e,
        json!({"type":"turn_context", "payload":{
            "sandbox_policy":{"type":"workspace-write", "writable_roots":[memory],
                "network_access":false, "exclude_tmpdir_env_var":true, "exclude_slash_tmp":false}
        }}),
    );
    e.settings.apply_native(native_settings(e));
    assert_eq!(e.settings.sandbox.as_deref(), Some("workspace-write"));
    assert_eq!(
        e.settings.writable.as_slice(),
        std::slice::from_ref(&memory)
    );
    let args = native::resume_args(e).unwrap();
    for config in [
        "sandbox_workspace_write.network_access=false",
        "sandbox_workspace_write.exclude_tmpdir_env_var=true",
        "sandbox_workspace_write.exclude_slash_tmp=false",
        "sandbox_workspace_write.writable_roots=[]",
    ] {
        assert!(args.contains(&config.into()), "missing {config}");
    }
    assert_eq!(args.iter().filter(|s| *s == "--add-dir").count(), 1);
    assert!(args.contains(&memory.to_string_lossy().into_owned()));
    let roundtrip: Settings =
        serde_json::from_slice(&serde_json::to_vec(&e.settings).unwrap()).unwrap();
    assert_eq!(roundtrip, e.settings);
}

#[test]
fn managed_permissions_preserve_exact_roots_and_do_not_add_supervisor_writes() {
    let mut f = Fixture::new();
    let e = &mut f.snapshot.entries[0];
    let extra = f.dir.join("extra");
    fs::create_dir(&extra).unwrap();
    append_codex(
        e,
        json!({"type":"event_msg","payload":{
            "type":"thread_settings_applied", "thread_settings":{"cwd":e.cwd,
                // Runtime roots are not write grants; only the permission entries are.
                "runtime_workspace_roots":[e.cwd, extra, f.dir.to_path_buf()],
                "permission_profile":{"type":"managed","network":"enabled","file_system":{
                    "type":"restricted","entries":[
                        {"path":{"type":"special","value":{"kind":"root"}},"access":"read"},
                        {"path":{"type":"path","path":e.cwd},"access":"write"},
                        {"path":{"type":"path","path":extra},"access":"write"}
                    ]
                }}
            }
        }}),
    );
    // Match the full installed 0.163 profile, including its automatic guards.
    let mut record = serde_json::json!({"type":"event_msg","payload":{
        "type":"thread_settings_applied", "thread_settings":{"cwd":e.cwd,
            "permission_profile":{"type":"managed","network":"enabled","file_system":{
                "type":"restricted","entries":[
                    {"path":{"type":"special","value":{"kind":"root"}},"access":"read"},
                    {"path":{"type":"path","path":e.cwd},"access":"write"},
                    {"path":{"type":"path","path":extra},"access":"write"}
                ]
            }}
        }
    }});
    for root in [&e.cwd, &extra] {
        for name in [".git", ".agents", ".codex", ".aws"] {
            record["payload"]["thread_settings"]["permission_profile"]["file_system"]["entries"].as_array_mut().unwrap().push(
                json!({"path":{"type":"path","path":root.join(name)}, "access":"read", "missing_path_behavior":"skip"}));
        }
    }
    append_codex(e, record);
    e.settings.apply_native(native_settings(e));
    let args = native::resume_args(e).unwrap();
    assert_eq!(args.iter().filter(|s| *s == "--add-dir").count(), 2);
    assert!(args.contains(&extra.to_string_lossy().into_owned()));
    assert!(
        !args.contains(
            &e.supervisor
                .as_ref()
                .unwrap()
                .state_dir
                .to_string_lossy()
                .into_owned()
        )
    );
    assert!(args.contains(&"sandbox_workspace_write.network_access=true".into()));
    assert!(args.contains(&"sandbox_workspace_write.exclude_tmpdir_env_var=true".into()));
    assert!(args.contains(&"sandbox_workspace_write.exclude_slash_tmp=true".into()));
}

#[test]
fn unsupported_current_permissions_block_instead_of_using_older_policy() {
    let f = Fixture::new();
    let server = Server::new(&f);
    let e = &f.snapshot.entries[0];
    let read_root = json!({"path":{"type":"special","value":{"kind":"root"}}, "access":"read"});
    let deny = json!({"path":{"type":"path","path":e.cwd.join("private")},"access":"deny"});
    for profile in [
        json!({"type":"external","network":"enabled"}),
        json!({"type":"managed","network":"restricted","file_system":{"type":"unrestricted"}}),
        json!({"type":"managed","network":"restricted","file_system":{"type":"restricted","entries":[read_root,deny]}}),
        json!({"type":"managed","network":"enabled","file_system":{"type":"restricted","entries":[read_root]}}),
        json!({"type":"managed","network":"restricted","file_system":{"type":"restricted","entries":[]}}),
    ] {
        append_codex(
            e,
            json!({"type":"event_msg","payload":{"type":"thread_settings_applied", "thread_settings":{"cwd":e.cwd,"permission_profile":profile}}}),
        );
        let observed = native_settings(e);
        assert!(observed.permission_error.is_some(), "{profile}");
        let mut saved = e.clone();
        saved.settings.apply_native(observed);
        assert!(native::resume_args(&saved).is_err());
        assert!(
            plan_in(&f.client, &f.snapshot, &f.runtime).unwrap()[0]
                .reason
                .contains("permissions cannot be restored")
        );
    }
    // A subsequent representable policy clears the blocker, never sticks to it.
    append_codex(
        e,
        json!({"type":"turn_context","payload":{"sandbox_policy":{"type":"read-only"}}}),
    );
    assert!(native_settings(e).permission_error.is_none());
    assert!(
        plan_in(&f.client, &f.snapshot, &f.runtime).unwrap()[0]
            .target
            .is_some()
    );
    assert_eq!(server.state.lock().unwrap().starts, 0);
}

#[test]
fn claude_observations_preserve_registered_permissions_and_role_access() {
    let mut f = Fixture::new();
    let e = &mut f.snapshot.entries[1];
    e.settings.permission = Some("plan".into());
    e.settings.writable = vec![e.supervisor.as_ref().unwrap().state_dir.clone()];
    e.settings.apply_native(
        native::metadata(e.native_file.as_ref().unwrap(), "claude", CLAUDE_ID).unwrap(),
    );
    let args = native::resume_args(e).unwrap();
    assert!(args.windows(2).any(|p| p == ["--permission-mode", "plan"]));
    assert!(args.contains(&"--append-system-prompt-file".into()));
    assert!(
        args.contains(
            &e.supervisor
                .as_ref()
                .unwrap()
                .state_dir
                .to_string_lossy()
                .into_owned()
        )
    );
    assert_eq!(args[1], CLAUDE_ID);
}

#[test]
fn read_only_supervisor_restores_in_original_terminal_with_role_and_no_writes() {
    let mut f = Fixture::new();
    f.snapshot.entries[0].settings.sandbox = Some("read-only".into());
    let server = Server::new(&f);
    let rows = plan_in(&f.client, &f.snapshot, &f.runtime).unwrap();
    let target = rows[0].target.clone().unwrap();
    assert_eq!(
        target,
        Target::Original {
            workspace: f.snapshot.entries[0].workspace.clone(),
            tab: f.snapshot.entries[0].tab.clone(),
            pane: f.snapshot.entries[0].pane.clone()
        }
    );
    restore_in(
        &f.client,
        &f.snapshot,
        &[(0, target, rows[0].differences.clone())],
        &f.runtime,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let state = server.state.lock().unwrap();
    assert_eq!(state.starts, 1);
    assert!(!state.methods.iter().any(|m| m.contains("create")));
    assert_eq!(state.agents[0]["pane_id"], f.snapshot.entries[0].pane);
    assert_eq!(
        state.agents[0]["workspace_id"],
        f.snapshot.entries[0].workspace
    );
    assert_eq!(state.agents[0]["tab_id"], f.snapshot.entries[0].tab);
}

#[test]
fn holds_survive_multiple_saves_partial_restore_continue_and_store_reopen() {
    let mut f = Fixture::new();
    let store = f.store();
    store.save(&f.snapshot).unwrap();
    let agents = dashboard_agents(&f.snapshot);
    let mut older = f.snapshot.clone();
    older.id.push_str("-second");
    store.save(&older).unwrap();
    let server = Server::new(&f);
    let rows = plan_in(&f.client, &f.snapshot, &f.runtime).unwrap();
    restore_in(
        &f.client,
        &f.snapshot,
        &[(
            0,
            rows[0].target.clone().unwrap(),
            rows[0].differences.clone(),
        )],
        &f.runtime,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(server.state.lock().unwrap().starts, 1);

    // Save after the partial restore omits the skipped supervisor. Neither that
    // omission nor replacing latest may release its uncontinued hold.
    f.snapshot.id.push_str("-same-boot");
    f.snapshot.boot = store::boot().unwrap();
    f.snapshot.entries.truncate(1);
    store.save(&f.snapshot).unwrap();
    assert_eq!(held_projects(&f.client, &agents).unwrap().len(), 2);
    assert!(
        begin_delivery(
            &store,
            &f.snapshot.scope,
            &agents[1].project_root,
            &["kept-inbox".into()]
        )
        .is_err()
    );
    let first = BTreeSet::from([agents[0].project_root.clone()]);
    continue_coordination(&f.client, &first).unwrap();
    assert_eq!(
        held_projects(&f.client, &agents).unwrap(),
        BTreeSet::from([agents[1].project_root.clone()])
    );
    let journal = store.journal().unwrap();
    assert!(
        !journal.released.contains_key(&f.snapshot.id),
        "Continue must not disarm a same-boot save for next boot"
    );
    assert_eq!(journal.released.len(), 2);
    assert!(!journal.attempts.is_empty());
    begin_delivery(
        &store,
        &f.snapshot.scope,
        &agents[0].project_root,
        &["still-quarantined".into()],
    )
    .unwrap();

    // Reopen all handles from disk, as a new dashboard process would.
    let client = HerdrClient::from_socket_path(f.client.socket_path().to_path_buf())
        .with_marker_record(f.dir.join("state/markers/session.json"));
    assert_eq!(
        held_projects(&client, &agents).unwrap(),
        BTreeSet::from([agents[1].project_root.clone()])
    );
    continue_coordination(&client, &BTreeSet::from([agents[1].project_root.clone()])).unwrap();
    assert!(held_projects(&client, &agents).unwrap().is_empty());
    assert!(
        begin_delivery(
            &store,
            &f.snapshot.scope,
            &agents[0].project_root,
            &["still-quarantined".into()]
        )
        .is_err()
    );
    assert_eq!(
        store.journal().unwrap().attempts.len(),
        journal.attempts.len()
    );
    // A new prior-boot save arms the identities again, despite older releases.
    older.id.push_str("-next-boot");
    store.save(&older).unwrap();
    assert_eq!(held_projects(&client, &agents).unwrap().len(), 2);
}

#[test]
fn prior_boot_holds_do_not_depend_on_being_observed_before_save() {
    let mut f = Fixture::new();
    let store = f.store();
    store.save(&f.snapshot).unwrap();
    let agents = dashboard_agents(&f.snapshot);
    f.snapshot.id.push_str("-same-boot");
    f.snapshot.boot = store::boot().unwrap();
    store.save(&f.snapshot).unwrap();
    assert_eq!(held_projects(&f.client, &agents).unwrap().len(), 2);
    assert!(
        begin_delivery(
            &store,
            &f.snapshot.scope,
            &agents[0].project_root,
            &["inbox-id".into()]
        )
        .is_err()
    );
    continue_coordination(&f.client, &held_projects(&f.client, &agents).unwrap()).unwrap();
    assert!(held_projects(&f.client, &agents).unwrap().is_empty());
    begin_delivery(
        &store,
        &f.snapshot.scope,
        &agents[0].project_root,
        &["inbox-id".into()],
    )
    .unwrap();
}

#[test]
fn independent_late_identity_after_unknown_start_only_reconciles_metadata() {
    let f = Fixture::new();
    let server = Server::new(&f);
    server.state.lock().unwrap().unknown_start = true;
    let row = plan_in(&f.client, &f.snapshot, &f.runtime)
        .unwrap()
        .remove(0);
    restore_in(
        &f.client,
        &f.snapshot,
        &[(0, row.target.unwrap(), row.differences)],
        &f.runtime,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    // The start was accepted despite its lost acknowledgment. The integration
    // reports exact identity later, after the first restore already returned.
    let e = &f.snapshot.entries[0];
    server.state.lock().unwrap().agents.push(json!({
        "agent": e.kind, "name": e.name, "workspace_id": e.workspace,
        "tab_id": e.tab, "pane_id": e.pane, "cwd": e.cwd,
        "agent_status": "idle", "interactive_ready": true, "launch_pending": false,
        "agent_session": {"agent": e.kind, "kind": "id", "value": e.native_id,
                          "source": "herdr:test"}
    }));
    let row = plan_in(&f.client, &f.snapshot, &f.runtime)
        .unwrap()
        .remove(0);
    assert!(matches!(row.target, Some(Target::Metadata { .. })));
    restore_in(
        &f.client,
        &f.snapshot,
        &[(0, row.target.unwrap(), row.differences)],
        &f.runtime,
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let state = server.state.lock().unwrap();
    assert_eq!(state.starts, 1);
    assert!(
        state
            .methods
            .iter()
            .all(|m| !m.contains("create") && !m.contains("prompt"))
    );
    assert_eq!(
        f.store().journal().unwrap().attempts[&e.key()].phase,
        "complete"
    );
}

#[test]
fn independent_partial_retry_does_not_restart_completed_entries() {
    let f = Fixture::new();
    let server = Server::new(&f);
    for index in [0, 1] {
        let rows = plan_in(&f.client, &f.snapshot, &f.runtime).unwrap();
        if index == 1 {
            assert!(rows[0].target.is_none());
            assert!(rows[0].reason.contains("Already running"));
        }
        let row = &rows[index];
        restore_in(
            &f.client,
            &f.snapshot,
            &[(index, row.target.clone().unwrap(), row.differences.clone())],
            &f.runtime,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    }
    assert_eq!(server.state.lock().unwrap().starts, 2);
    assert!(
        plan_in(&f.client, &f.snapshot, &f.runtime)
            .unwrap()
            .iter()
            .all(|r| r.target.is_none())
    );
}

#[test]
fn independent_identity_elsewhere_is_reused_and_duplicate_live_ids_block() {
    let f = Fixture::new();
    let server = Server::new(&f);
    let e = &f.snapshot.entries[0];
    let elsewhere = json!({
        "agent": e.kind, "name": "different-name", "workspace_id": "elsewhere",
        "tab_id": "elsewhere:t1", "pane_id": "elsewhere:p1", "cwd": e.cwd,
        "agent_status": "idle", "interactive_ready": true,
        "agent_session": {"agent": e.kind, "kind": "id", "value": e.native_id,
                          "source": "herdr:test"}
    });
    server.state.lock().unwrap().agents.push(elsewhere.clone());
    let rows = plan_in(&f.client, &f.snapshot, &f.runtime).unwrap();
    assert!(rows[0].target.is_none() && rows[0].alternative.is_none());
    assert!(rows[0].reason.contains("Already running in elsewhere:p1"));
    let mut duplicate = elsewhere;
    duplicate["pane_id"] = json!(e.pane);
    server.state.lock().unwrap().agents.push(duplicate);
    let rows = plan_in(&f.client, &f.snapshot, &f.runtime).unwrap();
    assert!(
        rows[0]
            .reason
            .contains("duplicate live conversation identities")
    );
    assert_eq!(server.state.lock().unwrap().starts, 0);
}
