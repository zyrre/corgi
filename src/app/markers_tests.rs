//! The tests of Corgi's marks in Herdr (`corgi_core::markers`) that run
//! through the dashboard: its refresh putting them back after a Herdr
//! restart, and the merge tag set and cleared with `corgi tag` and the
//! merge dialog.

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};

use crate::{
    app::launch::CORGI_REQUEST_TOKEN,
    app::{
        App,
        cli::{fleet_rows, requested_agent},
        merge::{MergePhase, MergeWorktreeForm},
        overlay::Overlay,
        project_main::project_root_digest,
        waker::SupervisorWaker,
    },
    herdr::HerdrClient,
    supervisor::{self, HANDOVER_TOKEN, MERGE_TOKEN, OLD_SUPERVISOR_TOKEN, SUPERVISOR_TOKEN},
    tag::{TagOptions, tag_agent},
    test_support::{ScratchDir, fake_herdr, test_app},
};

use crate::app::markers::*;

const ROOT: &str = "/repos/corgi-markers";
const SESSION: &str = "5e55-corgi";

/// A Herdr that keeps a session's panes, agents and workspaces, sets the
/// tokens reported to it (a report replaces the Corgi tokens it had, as
/// Herdr replaces a source's), and logs every request, until dropped.
struct StatefulHerdr {
    socket: PathBuf,
    state: Arc<Mutex<Value>>,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}

impl StatefulHerdr {
    fn new(label: &str, snapshot: Value) -> Self {
        let state = Arc::new(Mutex::new(snapshot));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (served, logged, stopped) = (state.clone(), requests.clone(), stop.clone());
        let (socket, server) = fake_herdr(label, move |listener| {
            serve(&listener, &served, &logged, &stopped);
        });
        Self {
            socket,
            state,
            requests,
            stop,
            server: Some(server),
        }
    }

    fn client(&self, record: &Path) -> HerdrClient {
        HerdrClient::from_socket_path(&self.socket).with_marker_record(record)
    }

    /// What a Herdr restart leaves: the same panes and workspaces, and
    /// every token but Corgi's.
    fn restart(&self) {
        let mut state = self.state.lock().expect("state");
        for list in ["agents", "panes", "workspaces"] {
            for item in state[list].as_array_mut().into_iter().flatten() {
                strip_corgi_tokens(item);
            }
        }
    }

    fn reports(&self) -> Vec<Value> {
        self.requests
            .lock()
            .expect("requests")
            .iter()
            .filter(|request| {
                request["method"]
                    .as_str()
                    .is_some_and(|method| method.ends_with(".report_metadata"))
            })
            .cloned()
            .collect()
    }

    fn tokens(&self, list: &str, key: &str, id: &str) -> Value {
        let state = self.state.lock().expect("state");
        state[list]
            .as_array()
            .into_iter()
            .flatten()
            .find(|item| item[key] == id)
            .map(|item| item["tokens"].clone())
            .unwrap_or(Value::Null)
    }
}

impl Drop for StatefulHerdr {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
        let _ = fs::remove_file(&self.socket);
    }
}

fn strip_corgi_tokens(item: &mut Value) {
    if let Some(tokens) = item["tokens"].as_object_mut() {
        tokens.retain(|key, _| !key.starts_with("corgi_"));
    }
}

fn serve(
    listener: &UnixListener,
    state: &Mutex<Value>,
    requests: &Mutex<Vec<Value>>,
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
        requests.lock().expect("requests").push(request.clone());
        let mut state = state.lock().expect("state");
        let params = &request["params"];
        let mut report = |list: &str, key: &str| {
            for item in state[list].as_array_mut().into_iter().flatten() {
                if item[key] == params[key] {
                    for (name, value) in params["tokens"].as_object().expect("tokens") {
                        if value.is_null() {
                            item["tokens"].as_object_mut().expect("tokens").remove(name);
                        } else {
                            item["tokens"][name] = value.clone();
                        }
                    }
                }
            }
        };
        let result = match request["method"].as_str() {
            Some("session.snapshot") => {
                json!({ "type": "session_snapshot", "snapshot": state.clone() })
            }
            Some("pane.report_metadata") => {
                report("agents", "pane_id");
                report("panes", "pane_id");
                json!({ "type": "ok" })
            }
            Some("workspace.report_metadata") => {
                report("workspaces", "workspace_id");
                json!({ "type": "ok" })
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

/// A session like the one of 2026-09-27: the corgi project's supervisor in its
/// project workspace, which it was asked to hand over from, and one
/// worker in a Corgi worktree workspace.
fn marked_session() -> Value {
    let corgi_tokens = json!({
        "corgi_handler": "supervisor-corgi",
        "corgi_handover": format!("1790000000 {SESSION}"),
        "corgi_baseline": format!("200000 {SESSION}"),
        "session": SESSION,
        "model": "Opus 5.5"
    });
    let worker_tokens = json!({ "session": "5e55-worker" });
    json!({
        "workspaces": [
            {
                "workspace_id": "w70",
                "label": "corgi-markers supervisor",
                "tokens": {
                    "corgi_workspace_role": "project-main",
                    "corgi_project_main_tab": "w70:t1",
                    "corgi_project_root": ROOT,
                    "corgi_project_root_hash": project_root_digest(ROOT)
                },
                "worktree": {
                    "repo_name": "corgi-markers", "repo_root": ROOT,
                    "checkout_path": ROOT, "is_linked_worktree": false
                }
            },
            {
                "workspace_id": "w8Z",
                "label": "worktree-brave-stone",
                "tokens": { "corgi_workspace_role": "agent-workspace" },
                "worktree": {
                    "repo_name": "corgi-markers", "repo_root": ROOT,
                    "checkout_path": "/wt/corgi-markers/brave-stone",
                    "is_linked_worktree": true
                }
            }
        ],
        "tabs": [
            { "tab_id": "w70:t1", "workspace_id": "w70", "label": "1", "number": 1 },
            { "tab_id": "w8Z:t1", "workspace_id": "w8Z", "label": "1", "number": 1 }
        ],
        "panes": [
            { "pane_id": "w70:p1", "workspace_id": "w70", "tab_id": "w70:t1", "tokens": corgi_tokens },
            { "pane_id": "w8Z:p1", "workspace_id": "w8Z", "tab_id": "w8Z:t1", "tokens": worker_tokens }
        ],
        "agents": [
            {
                "pane_id": "w70:p1", "workspace_id": "w70", "tab_id": "w70:t1",
                "agent": "claude", "name": "supervisor-corgi", "agent_status": "working",
                "cwd": ROOT,
                "agent_session": { "agent": "claude", "kind": "id", "value": SESSION },
                "tokens": corgi_tokens
            },
            {
                "pane_id": "w8Z:p1", "workspace_id": "w8Z", "tab_id": "w8Z:t1",
                "agent": "claude", "name": "w-brave", "agent_status": "working",
                "cwd": "/wt/corgi-markers/brave-stone",
                "agent_session": { "agent": "claude", "kind": "id", "value": "5e55-worker" },
                "tokens": worker_tokens
            }
        ]
    })
}

/// The dashboard that wakes supervisors, on `client`.
fn leading_dashboard(client: HerdrClient, scratch: &Path) -> App {
    let mut app = test_app();
    app.client = client;
    let mut waker = SupervisorWaker::in_dir(scratch);
    assert!(waker.lead(&scratch.join("wake.lock")));
    app.wake_with(waker);
    app
}

fn roles(app: &App) -> Vec<String> {
    fleet_rows(app, ROOT)
        .iter()
        .map(|row| {
            let mut fields = row.split('\t');
            format!(
                "{} {}",
                fields.next().unwrap_or_default(),
                fields.next().unwrap_or_default()
            )
        })
        .collect()
}

#[test]
fn one_refresh_after_a_herdr_restart_puts_corgis_marks_back() {
    let scratch = ScratchDir::new("markers-restart");
    let record = scratch.join("markers").join("herdr.json");
    let herdr = StatefulHerdr::new("markers-restart", marked_session());
    let mut dashboard = leading_dashboard(herdr.client(&record), &scratch);

    // The initial refresh migrates the old name marker to a session.
    dashboard.refresh();
    assert_eq!(herdr.reports().len(), 1);
    assert_eq!(
        herdr.tokens("agents", "pane_id", "w70:p1")[SUPERVISOR_TOKEN],
        supervisor::marker("", Some(SESSION))
    );
    let before = [
        herdr.tokens("agents", "pane_id", "w70:p1"),
        herdr.tokens("workspaces", "workspace_id", "w70"),
        herdr.tokens("workspaces", "workspace_id", "w8Z"),
    ];
    assert_eq!(
        roles(&dashboard),
        ["supervisor-corgi supervisor", "w-brave worker"]
    );

    herdr.restart();
    let mut fleet = test_app();
    fleet.client = herdr.client(&record);
    fleet.refresh();
    assert_eq!(
        roles(&fleet),
        ["supervisor-corgi worker", "w-brave worker"],
        "without its marks the supervisor is a worker"
    );

    dashboard.refresh();
    assert_eq!(
        [
            herdr.tokens("agents", "pane_id", "w70:p1"),
            herdr.tokens("workspaces", "workspace_id", "w70"),
            herdr.tokens("workspaces", "workspace_id", "w8Z"),
        ],
        before
    );
    assert_eq!(
        herdr.tokens("panes", "pane_id", "w70:p1"),
        before[0],
        "the pane has them too"
    );
    // The refresh that put them back already sees the supervisor.
    assert_eq!(
        roles(&dashboard),
        ["supervisor-corgi supervisor", "w-brave worker"]
    );
    assert_eq!(herdr.reports().len(), 4);

    // And so does `corgi fleet`.
    fleet.refresh();
    assert_eq!(
        roles(&fleet),
        ["supervisor-corgi supervisor", "w-brave worker"]
    );

    // A later refresh finds everything in place and writes nothing.
    dashboard.refresh();
    assert_eq!(herdr.reports().len(), 4);
    let leftovers: Vec<_> = fs::read_dir(record.parent().expect("record dir"))
        .expect("record dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// A worker `corgi spawn --request-id` started keeps its id while it
/// runs, gets it back after a Herdr restart, and never becomes the
/// supervisor on the way; a retry of the spawn finds it by that id.
#[test]
fn a_spawned_workers_request_id_survives_a_herdr_restart() {
    const ID: &str = "20261002-w-brave";
    let scratch = ScratchDir::new("markers-request");
    let record = scratch.join("markers").join("herdr.json");
    let herdr = StatefulHerdr::new("markers-request", marked_session());
    let client = herdr.client(&record);
    // As the launch tags it: before the agent starts, with no session.
    mark_pane(
        &client,
        "w8Z:p1",
        "w-brave",
        None,
        &[(CORGI_REQUEST_TOKEN, ID)],
    )
    .expect("tag the worker");
    let mut dashboard = leading_dashboard(client.clone(), &scratch);
    let mut fleet = test_app();
    fleet.client = client;
    let worker_tokens = || herdr.tokens("agents", "pane_id", "w8Z:p1");

    dashboard.refresh();
    assert_eq!(worker_tokens()[CORGI_REQUEST_TOKEN], ID);
    assert!(worker_tokens().get(SUPERVISOR_TOKEN).is_none());
    let marks = &read(&record).panes["w8Z:p1"];
    assert_eq!(marks.session.as_deref(), Some("5e55-worker"));
    assert_eq!(marks.tokens, owned(&[(CORGI_REQUEST_TOKEN, ID)]));

    fleet.refresh();
    let found = requested_agent(&fleet, ROOT, ID).expect("the retry finds the worker");
    assert_eq!(found["name"], "w-brave");
    assert_eq!(found["pane_id"], "w8Z:p1");
    assert_eq!(found["checkout"], "worktree");
    assert_eq!(found["existing"], true);
    assert!(requested_agent(&fleet, ROOT, "20261002-w-other").is_none());
    assert!(
        requested_agent(&fleet, "/repos/elsewhere", ID).is_none(),
        "only the same project's agents count"
    );

    herdr.restart();
    fleet.refresh();
    assert!(requested_agent(&fleet, ROOT, ID).is_none());
    dashboard.refresh();
    assert_eq!(worker_tokens()[CORGI_REQUEST_TOKEN], ID);
    assert!(worker_tokens().get(SUPERVISOR_TOKEN).is_none());
    fleet.refresh();
    assert!(requested_agent(&fleet, ROOT, ID).is_some());
    assert_eq!(
        roles(&fleet),
        ["supervisor-corgi supervisor", "w-brave worker"]
    );
}

/// A repository at `root` on `main`, with a worktree on its own branch at
/// `checkout`, which is one commit ahead of `main` when `ahead`.
fn tag_repo(scratch: &Path, ahead: bool) -> (String, String) {
    let root = scratch.join("repo");
    let checkout = scratch.join("worktree");
    fs::create_dir_all(&root).expect("create repo");
    let git = |dir: &Path, args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=Corgi test",
                "-c",
                "user.email=corgi@example.test",
            ])
            .args(args)
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {args:?}: {output:?}");
    };
    git(&root, &["init", "-b", "main"]);
    git(&root, &["commit", "--allow-empty", "-m", "initial"]);
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "worktree/brave",
            checkout.to_str().unwrap(),
        ],
    );
    if ahead {
        git(&checkout, &["commit", "--allow-empty", "-m", "the work"]);
    }
    let text = |path: PathBuf| path.to_string_lossy().into_owned();
    (text(root), text(checkout))
}

/// [`marked_session`] moved to the repository at `root` and its worktree
/// at `checkout`, with the worker done at state change 7 and carrying
/// the request id it was spawned with.
fn tag_session(root: &str, checkout: &str) -> Value {
    let text = marked_session()
        .to_string()
        .replace(&project_root_digest(ROOT), &project_root_digest(root))
        .replace("/wt/corgi-markers/brave-stone", checkout)
        .replace(ROOT, root);
    let mut live: Value = serde_json::from_str(&text).expect("session");
    live["agents"][1]["agent_status"] = json!("done");
    live["agents"][1]["state_change_seq"] = json!(7);
    for list in ["agents", "panes"] {
        live[list][1]["tokens"][CORGI_REQUEST_TOKEN] = json!("20261003-w-brave");
    }
    live
}

impl StatefulHerdr {
    /// Puts the worker in `state`, as of state change `change`.
    fn set_worker(&self, state: &str, change: u64) {
        let mut live = self.state.lock().expect("state");
        live["agents"][1]["agent_status"] = json!(state);
        live["agents"][1]["state_change_seq"] = json!(change);
    }
}

fn tag(merge: bool) -> TagOptions {
    TagOptions {
        name: "w-brave".into(),
        merge,
        project: None,
    }
}

fn worker(app: &App) -> &crate::model::DashboardAgent {
    app.agents
        .iter()
        .find(|agent| agent.info.name.as_deref() == Some("w-brave"))
        .expect("the worker")
}

/// A supervisor tags a done worker ready to merge: the tag sits beside its
/// request id, in Herdr and in the record, shows in `corgi fleet`, comes
/// back after a Herdr restart, outlives the user looking at the worker,
/// and goes once the worker works again, cleared by the dashboard that
/// wakes supervisors.
#[test]
fn a_merge_tag_lasts_until_its_worker_works_again() {
    let scratch = ScratchDir::new("markers-tag");
    let (root, checkout) = tag_repo(&scratch, true);
    let record = scratch.join("markers").join("herdr.json");
    let herdr = StatefulHerdr::new("markers-tag", tag_session(&root, &checkout));
    let client = herdr.client(&record);
    let mut fleet = test_app();
    fleet.client = client.clone();
    let mut dashboard = leading_dashboard(client.clone(), &scratch);
    let worker_tokens = || herdr.tokens("agents", "pane_id", "w8Z:p1");

    // The leader records the request id the spawn set.
    dashboard.refresh();
    fleet.refresh();
    assert_eq!(
        tag_agent(&client, &fleet.agents, &root, &tag(true)).expect("tag"),
        "Tagged w-brave ready to merge: 1 commit ahead of main"
    );
    assert_eq!(worker_tokens()[MERGE_TOKEN], "done 7");
    assert_eq!(worker_tokens()[CORGI_REQUEST_TOKEN], "20261003-w-brave");
    assert_eq!(read(&record).panes["w8Z:p1"].tokens[MERGE_TOKEN], "done 7");
    fleet.refresh();
    assert!(worker(&fleet).ready_to_merge());
    let row = fleet_rows(&fleet, &root)
        .into_iter()
        .find(|row| row.starts_with("w-brave\t"))
        .expect("the worker's row");
    let fields: Vec<&str> = row.split('\t').collect();
    assert_eq!(
        (fields[2], fields[6], fields[8]),
        ("done", "-", "merge"),
        "{row}"
    );

    herdr.restart();
    dashboard.refresh();
    assert_eq!(worker_tokens()[MERGE_TOKEN], "done 7");
    assert_eq!(worker_tokens()[CORGI_REQUEST_TOKEN], "20261003-w-brave");

    // Herdr shows a done agent idle once the user has looked at it.
    herdr.set_worker("idle", 8);
    dashboard.refresh();
    assert!(worker(&dashboard).ready_to_merge());
    assert_eq!(worker_tokens()[MERGE_TOKEN], "done 7");

    herdr.set_worker("working", 9);
    fleet.refresh();
    assert!(
        !worker(&fleet).ready_to_merge(),
        "a working worker is shown working"
    );
    assert_eq!(
        worker_tokens()[MERGE_TOKEN],
        "done 7",
        "only the leader clears"
    );
    dashboard.refresh();
    assert!(worker_tokens().get(MERGE_TOKEN).is_none());
    assert_eq!(worker_tokens()[CORGI_REQUEST_TOKEN], "20261003-w-brave");
    assert!(
        !read(&record).panes["w8Z:p1"]
            .tokens
            .contains_key(MERGE_TOKEN)
    );

    herdr.set_worker("done", 10);
    dashboard.refresh();
    assert!(!worker(&dashboard).ready_to_merge());
}

/// A worker that worked and stopped again while no dashboard watched is
/// not shown as tagged, and the first dashboard to lead clears its tag.
#[test]
fn a_tag_its_worker_worked_past_unseen_is_ignored_and_cleared() {
    let scratch = ScratchDir::new("markers-tag-stale");
    let (root, checkout) = tag_repo(&scratch, true);
    let record = scratch.join("markers").join("herdr.json");
    let herdr = StatefulHerdr::new("markers-tag-stale", tag_session(&root, &checkout));
    let client = herdr.client(&record);
    let mut fleet = test_app();
    fleet.client = client.clone();
    fleet.refresh();
    tag_agent(&client, &fleet.agents, &root, &tag(true)).expect("tag");

    herdr.set_worker("done", 12);
    fleet.refresh();
    assert!(!worker(&fleet).ready_to_merge());
    let row = fleet_rows(&fleet, &root).join("\n");
    assert!(!row.contains("\tmerge"), "{row}");
    assert_eq!(
        herdr.tokens("agents", "pane_id", "w8Z:p1")[MERGE_TOKEN],
        "done 7"
    );

    let mut dashboard = leading_dashboard(client, &scratch);
    dashboard.refresh();
    assert!(
        herdr
            .tokens("agents", "pane_id", "w8Z:p1")
            .get(MERGE_TOKEN)
            .is_none()
    );
}

/// `--clear` and a merge with `m` both take the tag off.
#[test]
fn a_merge_tag_is_cleared_on_request_and_by_merging_the_branch() {
    let scratch = ScratchDir::new("markers-tag-clear");
    let (root, checkout) = tag_repo(&scratch, true);
    let record = scratch.join("markers").join("herdr.json");
    let herdr = StatefulHerdr::new("markers-tag-clear", tag_session(&root, &checkout));
    let client = herdr.client(&record);
    let mut app = test_app();
    app.client = client.clone();
    let worker_tokens = || herdr.tokens("agents", "pane_id", "w8Z:p1");

    app.refresh();
    tag_agent(&client, &app.agents, &root, &tag(true)).expect("tag");
    app.refresh();
    assert_eq!(
        tag_agent(&client, &app.agents, &root, &tag(false)).expect("clear"),
        "Cleared the merge tag of w-brave"
    );
    assert!(worker_tokens().get(MERGE_TOKEN).is_none());
    assert_eq!(worker_tokens()[CORGI_REQUEST_TOKEN], "20261003-w-brave");
    app.refresh();
    assert_eq!(
        tag_agent(&client, &app.agents, &root, &tag(false)).expect("clear again"),
        "w-brave had no merge tag"
    );

    tag_agent(&client, &app.agents, &root, &tag(true)).expect("tag again");
    app.refresh();
    assert!(worker(&app).ready_to_merge());
    app.overlay = Overlay::merge(MergeWorktreeForm {
        label: "corgi-markers/brave".into(),
        workspace_id: "w8Z".into(),
        agent: "w-brave".into(),
        project_root: root.clone().into(),
        worktree_checkout: checkout.into(),
        source_branch: "worktree/brave".into(),
        target_branch: "main".into(),
        task: String::new(),
        commits: Vec::new(),
        upstream: Default::default(),
        phase: MergePhase::Running(2),
    });
    app.clear_merged_tag();
    assert!(!worker(&app).ready_to_merge());
    assert!(worker_tokens().get(MERGE_TOKEN).is_none());
    assert!(
        read(&record)
            .panes
            .get("w8Z:p1")
            .is_none_or(|marks| !marks.tokens.contains_key(MERGE_TOKEN))
    );
}

/// Nothing to merge, a worker still at work, and a name the project does
/// not have are refused, and tag nothing.
#[test]
fn a_merge_tag_is_refused_without_commits_ahead_or_while_working() {
    let scratch = ScratchDir::new("markers-tag-refused");
    let (root, checkout) = tag_repo(&scratch, false);
    let record = scratch.join("markers").join("herdr.json");
    let herdr = StatefulHerdr::new("markers-tag-refused", tag_session(&root, &checkout));
    let client = herdr.client(&record);
    let mut app = test_app();
    app.client = client.clone();
    app.refresh();
    let refused = tag_agent(&client, &app.agents, &root, &tag(true)).expect_err("nothing ahead");
    assert!(
        refused.to_string().contains("no commits ahead of main"),
        "{refused}"
    );
    let supervisor = TagOptions {
        name: "supervisor-corgi".into(),
        ..tag(true)
    };
    let refused =
        tag_agent(&client, &app.agents, &root, &supervisor).expect_err("a working supervisor");
    assert!(refused.to_string().contains("is working"), "{refused}");
    let elsewhere = tag_agent(&client, &app.agents, "/repos/elsewhere", &tag(true))
        .expect_err("another project's agent");
    assert!(
        elsewhere.to_string().contains("no agent named w-brave"),
        "{elsewhere}"
    );
    assert!(
        herdr
            .tokens("agents", "pane_id", "w8Z:p1")
            .get(MERGE_TOKEN)
            .is_none()
    );
    assert!(herdr.reports().is_empty());
}

/// A Steward an older Corgi launched carries its marker under the
/// pre-rename key, in Herdr and in the record. After the dashboard is
/// rebuilt and restarted, it is still the project's supervisor, and its
/// marker moves to the new key.
#[test]
fn a_steward_marked_by_an_older_corgi_stays_the_supervisor_under_the_new_key() {
    let scratch = ScratchDir::new("markers-steward");
    let record = scratch.join("markers").join("herdr.json");
    let marker = supervisor::marker("", Some(SESSION));
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    let old_record = json!({
        "panes": { "w70:p1": {
            "agent": "steward-corgi", "session": SESSION,
            "tokens": { OLD_SUPERVISOR_TOKEN: marker }
        } }
    });
    fs::write(&record, old_record.to_string()).unwrap();
    assert_eq!(
        read(&record).panes["w70:p1"].tokens,
        owned(&[(SUPERVISOR_TOKEN, &marker)]),
        "the old record reads under the new key"
    );
    let mut live = marked_session();
    live["agents"][0]["name"] = json!("steward-corgi");
    for list in ["agents", "panes"] {
        let tokens = live[list][0]["tokens"].as_object_mut().unwrap();
        tokens.remove(SUPERVISOR_TOKEN);
        tokens.insert(OLD_SUPERVISOR_TOKEN.into(), json!(marker));
    }
    let herdr = StatefulHerdr::new("markers-steward", live);
    let mut dashboard = leading_dashboard(herdr.client(&record), &scratch);

    dashboard.refresh();
    assert_eq!(
        roles(&dashboard),
        ["steward-corgi supervisor", "w-brave worker"]
    );
    for list in ["agents", "panes"] {
        let tokens = herdr.tokens(list, "pane_id", "w70:p1");
        assert_eq!(tokens[SUPERVISOR_TOKEN], json!(marker), "{list}");
        assert!(
            tokens.get(OLD_SUPERVISOR_TOKEN).is_none(),
            "{list}: {tokens}"
        );
        assert_eq!(
            tokens[HANDOVER_TOKEN],
            json!(format!("1790000000 {SESSION}"))
        );
    }
    let marks = &read(&record).panes["w70:p1"];
    assert_eq!(marks.tokens[SUPERVISOR_TOKEN], marker);
    assert!(!marks.tokens.contains_key(OLD_SUPERVISOR_TOKEN));

    // The next refresh finds the marks in place and writes nothing.
    let reports = herdr.reports().len();
    dashboard.refresh();
    assert_eq!(herdr.reports().len(), reports);
}
