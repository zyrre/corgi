use std::path::Path;

use serde_json::{Value, json};

use super::*;
use crate::{
    project_main::{
        CORGI_AGENT_WORKSPACE_ROLE, CORGI_PROJECT_MAIN_ROLE, CORGI_PROJECT_MAIN_TAB_TOKEN,
        CORGI_PROJECT_ROOT_TOKEN, CORGI_WORKSPACE_ROLE_TOKEN,
    },
    refresh::{self, Inputs},
    test_support::{FakeHerdr, ScratchDir, git_repo, git_worktree},
    waker::SupervisorWaker,
};

/// An agent as Herdr's snapshot lists it.
fn agent(pane: &str, kind: &str, name: &str, cwd: &str, session: Option<&str>) -> Value {
    let workspace = pane.split(':').next().unwrap();
    let mut agent = json!({
        "pane_id": pane, "workspace_id": workspace, "tab_id": format!("{workspace}:t1"),
        "agent": kind, "name": name, "cwd": cwd, "agent_status": "idle", "tokens": {},
    });
    if let Some(session) = session {
        agent["agent_session"] = json!({"agent": kind, "source": "herdr:claude", "value": session});
    }
    agent
}

fn with_tokens(mut agent: Value, tokens: Value) -> Value {
    agent["tokens"] = tokens;
    agent
}

/// A plain workspace, the user's own.
fn workspace(id: &str, label: &str) -> Value {
    json!({"workspace_id": id, "label": label})
}

/// A workspace Corgi created for an agent.
fn agent_workspace(id: &str, label: &str) -> Value {
    json!({"workspace_id": id, "label": label,
           "tokens": {CORGI_WORKSPACE_ROLE_TOKEN: CORGI_AGENT_WORKSPACE_ROLE}})
}

/// The snapshot of `workspaces` and `agents`, with a pane and a tab for
/// each agent, and `panes` besides.
fn snapshot(workspaces: Vec<Value>, agents: Vec<Value>, panes: Vec<Value>) -> Value {
    let mut all_panes: Vec<Value> = agents
        .iter()
        .map(|agent| {
            json!({"pane_id": agent["pane_id"], "workspace_id": agent["workspace_id"],
                   "tab_id": agent["tab_id"], "cwd": agent["cwd"], "tokens": agent["tokens"]})
        })
        .collect();
    all_panes.extend(panes);
    let tabs: Vec<Value> = all_panes
        .iter()
        .map(|pane| json!({"tab_id": pane["tab_id"], "workspace_id": pane["workspace_id"], "number": 1}))
        .collect();
    json!({"workspaces": workspaces, "agents": agents, "panes": all_panes, "tabs": tabs})
}

fn process(program: &str, cwd: &str, args: &[&str], pid: u32) -> Value {
    let mut argv = vec![program.to_string()];
    argv.extend(args.iter().map(|arg| arg.to_string()));
    json!({"name": Path::new(program).file_name().unwrap().to_str().unwrap(),
           "cwd": cwd, "argv": argv, "pid": pid})
}

/// A test's Herdr, state and options.
struct World {
    scratch: ScratchDir,
    herdr: FakeHerdr,
    client: HerdrClient,
    options: Options,
}

impl World {
    fn new(label: &str, snapshot: Value) -> Self {
        let scratch = ScratchDir::new(label);
        let herdr = FakeHerdr::start(label, snapshot);
        let client = HerdrClient::from_socket_path(&herdr.socket)
            .with_marker_record(scratch.join("markers.json"));
        let state = scratch.join("state");
        fs::create_dir_all(&state).unwrap();
        let options = Options {
            dashboard_pane_id: None,
            state: SupervisorWaker::in_dir(&state).inboxes(),
            left_file: Some(scratch.join("left.json")),
        };
        Self {
            scratch,
            herdr,
            client,
            options,
        }
    }

    /// The supervisor state directory of the project named `name`.
    fn state_dir(&self, name: &str) -> PathBuf {
        let dir = self.scratch.join("state").join(name);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn dir(&self, name: &str) -> String {
        let dir = self.scratch.join(name);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap().to_string_lossy().into_owned()
    }

    fn detect(&self) -> Vec<Candidate> {
        detect(&self.client, &self.options).expect("detect")
    }

    fn snapshot(&self) -> SessionSnapshot {
        self.client.snapshot().expect("snapshot")
    }

    fn retag(&self, snapshot: &mut SessionSnapshot) -> Option<String> {
        retag_proven(
            &self.client,
            snapshot,
            &mut RepoCache::default(),
            &self.options.state,
            &mut RetagCache::default(),
        )
    }

    /// The tokens each `pane.report_metadata` set, in order.
    fn reported(&self) -> Vec<(String, Value)> {
        self.herdr
            .requests("pane.report_metadata")
            .iter()
            .map(|request| {
                (
                    request["params"]["pane_id"].as_str().unwrap().to_string(),
                    request["params"]["tokens"].clone(),
                )
            })
            .collect()
    }
}

fn supervisor_argv(dir: &Path) -> Vec<String> {
    vec![
        "--append-system-prompt-file".into(),
        dir.join("ROLE.md").to_string_lossy().into_owned(),
        "--add-dir".into(),
        dir.to_string_lossy().into_owned(),
    ]
}

fn argv_refs(argv: &[String]) -> Vec<&str> {
    argv.iter().map(String::as_str).collect()
}

#[test]
fn a_hidden_codex_is_listed_with_the_integration_to_install_and_a_shell_is_not() {
    let world = World::new("orphans-hidden", Value::Null);
    let cwd = world.dir("scratchpad");
    world.herdr.set_snapshot(snapshot(
        vec![workspace("wR", "scratchpad"), workspace("wS", "shell")],
        vec![
            agent("wR:p2", "codex", "codex", &cwd, None),
            // Herdr's detector took a shell for an agent.
            agent("wS:p1", "codex", "codex", &cwd, None),
        ],
        vec![],
    ));
    world.herdr.set_processes(
        "wR:p2",
        json!([process("/opt/homebrew/bin/codex", &cwd, &[], 41)]),
    );
    world
        .herdr
        .set_processes("wS:p1", json!([process("/bin/zsh", &cwd, &[], 42)]));

    let found = world.detect();
    assert_eq!(found.len(), 1, "{found:#?}");
    let hidden = &found[0];
    assert_eq!(hidden.cause, Cause::Hidden);
    assert_eq!(hidden.key, "pane:wR:p2");
    assert_eq!(hidden.harness, "codex");
    assert!(
        hidden
            .fix
            .as_deref()
            .unwrap()
            .contains("herdr integration install codex")
    );
    assert!(hidden.evidence[0].contains("codex runs in the foreground"));
    // The user's own workspace: closing it closes the pane alone.
    assert_eq!(
        hidden.actions,
        vec![
            Action::Leave,
            Action::Close {
                target: "pane".into()
            }
        ]
    );
    assert_eq!(hidden.actions[hidden.suggested], Action::Leave);
}

#[test]
fn a_codex_without_a_session_is_a_row_in_every_workspace_corgi_created_while_it_runs() {
    let world = World::new("orphans-codex-row", Value::Null);
    let cwd = world.dir("webshop-pay");
    world.herdr.set_snapshot(snapshot(
        vec![agent_workspace("wQ", "worktree-pay")],
        vec![agent("wQ:p1", "codex", "w-pay", &cwd, None)],
        vec![],
    ));
    let rows = |world: &World| {
        RefreshState::default()
            .view(&Inputs::new(world.client.clone()), world.snapshot())
            .agents
    };
    // Not running there: no row, and nothing to list (a stale detection).
    assert!(rows(&world).is_empty());
    assert!(world.detect().is_empty());

    world
        .herdr
        .set_processes("wQ:p1", json!([process("codex", &cwd, &[], 7)]));
    let shown = rows(&world);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].info.pane_id, "wQ:p1");
    // It is a row now, not a candidate.
    assert!(
        world
            .detect()
            .iter()
            .all(|candidate| candidate.cause != Cause::Hidden)
    );
}

#[test]
fn an_agent_cli_herdr_does_not_detect_is_likely_by_name_and_a_guess_under_node() {
    let world = World::new("orphans-undetected", Value::Null);
    let cwd = world.dir("repos");
    let pane = |id: &str, label: &str| {
        let workspace = id.split(':').next().unwrap();
        json!({"pane_id": id, "workspace_id": workspace, "tab_id": format!("{workspace}:t1"),
               "cwd": cwd, "label": label})
    };
    world.herdr.set_snapshot(snapshot(
        vec![
            workspace("wA", "a"),
            workspace("wB", "b"),
            workspace("wC", "c"),
        ],
        vec![],
        vec![
            pane("wA:p1", "wrapped"),
            pane("wB:p1", "npx"),
            pane("wC:p1", "shell"),
            pane("wD:p1", "Corgi"),
        ],
    ));
    world.herdr.set_processes(
        "wA:p1",
        json!([process("/usr/local/bin/claude", &cwd, &[], 1)]),
    );
    world.herdr.set_processes(
        "wB:p1",
        json!([process(
            "node",
            &cwd,
            &["/x/node_modules/@anthropic-ai/claude-code/cli.js"],
            2
        )]),
    );
    world
        .herdr
        .set_processes("wC:p1", json!([process("-zsh", &cwd, &[], 3)]));
    // A dashboard pane is never one, whatever runs in it.
    world
        .herdr
        .set_processes("wD:p1", json!([process("claude", &cwd, &[], 4)]));

    let found = world.detect();
    let by_pane = |pane: &str| found.iter().find(|c| c.pane_id.as_deref() == Some(pane));
    assert_eq!(found.len(), 2, "{found:#?}");
    let wrapped = by_pane("wA:p1").unwrap();
    assert_eq!(
        (wrapped.cause, wrapped.confidence, wrapped.harness.as_str()),
        (Cause::NotDetected, Confidence::Likely, "claude")
    );
    assert!(
        wrapped
            .fix
            .as_deref()
            .unwrap()
            .contains("herdr agent explain wA:p1")
    );
    let npx = by_pane("wB:p1").unwrap();
    assert_eq!(
        (npx.confidence, npx.harness.as_str()),
        (Confidence::Guess, "claude")
    );
}

/// A world with an unmarked agent in `webshop` whose process names the
/// supervisor's files, and `others` beside it.
fn unmarked_supervisor(label: &str, session: Option<&str>, others: Vec<Value>) -> (World, String) {
    let world = World::new(label, Value::Null);
    let root = world.dir("webshop");
    let dir = world.state_dir("webshop");
    let mut agents = vec![agent(
        "w1:p1",
        "claude",
        "webshop-supervisor",
        &root,
        session,
    )];
    agents.extend(others);
    world.herdr.set_snapshot(snapshot(
        vec![workspace("w1", "webshop"), workspace("w2", "webshop 2")],
        agents,
        vec![],
    ));
    world.herdr.set_processes(
        "w1:p1",
        json!([process(
            "claude",
            &root,
            &argv_refs(&supervisor_argv(&dir)),
            11
        )]),
    );
    (world, root)
}

#[test]
fn a_supervisor_that_lost_its_mark_is_certain_and_offered_back() {
    let (world, root) = unmarked_supervisor("orphans-supervisor", Some("s-1"), vec![]);
    let found = world.detect();
    assert_eq!(found.len(), 1, "{found:#?}");
    let lost = &found[0];
    assert_eq!(
        (lost.cause, lost.confidence),
        (Cause::Supervisor, Confidence::Certain)
    );
    assert_eq!(
        lost.actions[lost.suggested],
        Action::Supervisor {
            project: "webshop".into(),
            project_root: root,
        }
    );
    assert!(lost.evidence[0].contains("ROLE.md"));
}

#[test]
fn a_supervisor_sighting_is_not_offered_beside_a_running_supervisor_or_without_a_session() {
    // The project has a running supervisor: a handover may be under way.
    let (world, root) = unmarked_supervisor(
        "orphans-supervisor-running",
        Some("s-1"),
        vec![with_tokens(
            agent("w2:p1", "claude", "webshop-boss", "", Some("s-2")),
            json!({SUPERVISOR_TOKEN: "session:s-2"}),
        )],
    );
    // Same project as the sighting.
    let mut snapshot_json = snapshot(
        vec![workspace("w1", "webshop"), workspace("w2", "webshop 2")],
        vec![
            agent("w1:p1", "claude", "webshop-supervisor", &root, Some("s-1")),
            with_tokens(
                agent("w2:p1", "claude", "webshop-boss", &root, Some("s-2")),
                json!({SUPERVISOR_TOKEN: "session:s-2"}),
            ),
        ],
        vec![],
    );
    world.herdr.set_snapshot(snapshot_json.clone());
    let found = world.detect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].confidence, Confidence::Likely);
    assert!(
        !found[0]
            .actions
            .iter()
            .any(|action| matches!(action, Action::Supervisor { .. }))
    );
    assert!(
        found[0]
            .evidence
            .iter()
            .any(|line| line.contains("webshop-boss"))
    );
    // Nor is it marked without asking.
    let mut snapshot = world.snapshot();
    assert_eq!(world.retag(&mut snapshot), None);
    assert!(world.reported().is_empty());

    // Without a session there is nothing to bind the mark to.
    snapshot_json["agents"].as_array_mut().unwrap().truncate(1);
    snapshot_json["agents"][0]
        .as_object_mut()
        .unwrap()
        .remove("agent_session");
    // Corgi's mark of a status-line session keeps it a row.
    snapshot_json["agents"][0]["tokens"] = json!({"session": "s-1"});
    world.herdr.set_snapshot(snapshot_json);
    let found = world.detect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        !found[0]
            .actions
            .iter()
            .any(|action| matches!(action, Action::Supervisor { .. }))
    );
    let mut snapshot = world.snapshot();
    assert_eq!(world.retag(&mut snapshot), None);
}

#[test]
fn two_processes_naming_one_supervisors_files_are_a_handover_and_neither_is_marked() {
    let world = World::new("orphans-two-supervisors", Value::Null);
    let root = world.dir("webshop");
    let dir = world.state_dir("webshop");
    world.herdr.set_snapshot(snapshot(
        vec![workspace("w1", "webshop"), workspace("w2", "webshop 2")],
        vec![
            agent("w1:p1", "claude", "old", &root, Some("s-1")),
            agent("w2:p1", "claude", "new", &root, Some("s-2")),
        ],
        vec![],
    ));
    for (pane, pid) in [("w1:p1", 1), ("w2:p1", 2)] {
        world.herdr.set_processes(
            pane,
            json!([process(
                "claude",
                &root,
                &argv_refs(&supervisor_argv(&dir)),
                pid
            )]),
        );
    }
    let found = world.detect();
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(|candidate| {
        candidate.confidence == Confidence::Likely
            && !candidate
                .actions
                .iter()
                .any(|action| matches!(action, Action::Supervisor { .. }))
    }));
    let mut snapshot = world.snapshot();
    assert_eq!(world.retag(&mut snapshot), None);
}

#[test]
fn the_hooks_pass_marks_a_proven_supervisor_and_says_so() {
    let (world, _) = unmarked_supervisor("orphans-retag-supervisor", Some("s-1"), vec![]);
    let mut snapshot = world.snapshot();
    let status = world.retag(&mut snapshot).expect("retagged");
    assert_eq!(
        status,
        "Marked webshop-supervisor as webshop's supervisor again"
    );
    let reported = world.reported();
    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].0, "w1:p1");
    assert_eq!(reported[0].1[SUPERVISOR_TOKEN], "session:s-1");
    // The snapshot the refresh goes on with sees the supervisor already.
    assert!(is_supervisor(&snapshot.agents[0]));
    // And the record keeps it across a restart.
    let record = markers::read(&world.scratch.join("markers.json"));
    assert_eq!(record.panes["w1:p1"].session.as_deref(), Some("s-1"));
}

#[test]
fn a_leading_refresh_puts_a_proven_mark_back_before_anything_reads_it() {
    let (world, _) = unmarked_supervisor("orphans-refresh-retag", Some("s-1"), vec![]);
    let waker = SupervisorWaker::in_dir(&world.scratch.join("state"));
    let refreshed = refresh::run(
        &Inputs::new(world.client.clone()),
        RefreshState::new(Some(waker)),
        None,
    );
    let view = refreshed.view.expect("view");
    assert!(view.agents[0].supervisor, "{:?}", view.agents[0]);
    let (status, _) = view.status.expect("status");
    assert!(status.contains("webshop's supervisor again"), "{status}");
}

/// A world with a repository `webshop`, a plain linked worktree `pay` of
/// it in which `restored-w1` runs without a request id, and a ledger whose
/// lines are `ledger`, `{cwd}` standing for the worktree.
fn untracked_worker(label: &str, ledger: &[&str], others: Vec<Value>) -> (World, String) {
    let world = World::new(label, Value::Null);
    let root = git_repo(&world.scratch.join("webshop"));
    let linked = git_worktree(&root, &world.scratch.join("webshop-pay"), "pay");
    let linked = linked.to_string_lossy().into_owned();
    let dir = world.state_dir("webshop");
    let lines: Vec<String> = ledger
        .iter()
        .map(|line| line.replace("{cwd}", &linked))
        .collect();
    fs::write(dir.join("ledger.jsonl"), lines.join("\n") + "\n").unwrap();
    let mut agents = vec![agent(
        "w1:p1",
        "claude",
        "restored-w1",
        &linked,
        Some("s-1"),
    )];
    agents.extend(others.into_iter().map(|mut other| {
        other["cwd"] = json!(linked);
        other
    }));
    world
        .herdr
        .set_snapshot(snapshot(vec![workspace("w1", "pay")], agents, vec![]));
    (world, linked)
}

const PAY: &str =
    r#"{"id":"r-pay","agent":"w-pay","status":"dispatched","branch":"pay","cwd":"{cwd}"}"#;

#[test]
fn a_worker_without_its_request_id_is_matched_by_checkout_and_branch() {
    let (world, _) = untracked_worker("orphans-worker", &[PAY], vec![]);
    let found = world.detect();
    assert_eq!(found.len(), 1, "{found:#?}");
    let worker = &found[0];
    assert_eq!(
        (worker.cause, worker.confidence),
        (Cause::Worker, Confidence::Certain)
    );
    assert!(matches!(
        &worker.actions[worker.suggested],
        Action::Worker { request_id, agent, .. } if request_id == "r-pay" && agent == "w-pay"
    ));

    // The hooks' pass gives it back without asking.
    let mut snapshot = world.snapshot();
    let status = world.retag(&mut snapshot).expect("retagged");
    assert_eq!(status, "Gave restored-w1 back its request id r-pay");
    assert_eq!(world.reported()[0].1[CORGI_REQUEST_TOKEN], "r-pay");
    assert_eq!(snapshot.agents[0].tokens[CORGI_REQUEST_TOKEN], "r-pay");
}

#[test]
fn an_unproven_worker_match_is_listed_but_never_tagged_without_asking() {
    // Another agent works in the same checkout.
    let (world, _) = untracked_worker(
        "orphans-worker-shared",
        &[PAY],
        vec![agent("w2:p1", "claude", "mine", "", Some("s-2"))],
    );
    let found = world.detect();
    let worker = found.iter().find(|c| c.cause == Cause::Worker).unwrap();
    assert_eq!(worker.confidence, Confidence::Likely);
    assert_eq!(worker.actions[worker.suggested], Action::Leave);
    let mut snapshot = world.snapshot();
    assert_eq!(world.retag(&mut snapshot), None);
    assert!(world.reported().is_empty());

    // Another live agent carries the entry's id: it is that one's.
    let (world, _) = untracked_worker(
        "orphans-worker-taken",
        &[PAY],
        vec![with_tokens(
            agent("w2:p1", "claude", "w-pay", "", Some("s-2")),
            json!({CORGI_REQUEST_TOKEN: "r-pay"}),
        )],
    );
    assert!(world.detect().iter().all(|c| c.cause != Cause::Worker));
    let mut snapshot = world.snapshot();
    assert_eq!(world.retag(&mut snapshot), None);

    // Finished work, another branch, or an entry naming it already.
    for ledger in [
        r#"{"id":"r-pay","agent":"w-pay","status":"merged","branch":"pay","cwd":"{cwd}"}"#,
        r#"{"id":"r-pay","agent":"w-pay","status":"dispatched","branch":"other","cwd":"{cwd}"}"#,
        r#"{"id":"r-pay","agent":"restored-w1","status":"dispatched","branch":"pay","cwd":"{cwd}"}"#,
    ] {
        let (world, _) = untracked_worker("orphans-worker-none", &[ledger], vec![]);
        assert!(world.detect().is_empty(), "{ledger}");
        let mut snapshot = world.snapshot();
        assert_eq!(world.retag(&mut snapshot), None, "{ledger}");
    }
}

#[test]
fn an_agent_in_no_repository_is_a_guess_outside_corgis_workspaces() {
    let world = World::new("orphans-no-project", Value::Null);
    let downloads = world.dir("Downloads");
    let new_project = world.dir("new-project");
    world.herdr.set_snapshot(snapshot(
        vec![workspace("w1", "dl"), agent_workspace("w2", "new-project")],
        vec![
            agent("w1:p1", "claude", "loose", &downloads, Some("s-1")),
            // A new project before `git init`, in a workspace Corgi made.
            agent("w2:p1", "claude", "fresh", &new_project, Some("s-2")),
        ],
        vec![],
    ));
    let found = world.detect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        (found[0].cause, found[0].confidence, found[0].key.as_str()),
        (Cause::NoProject, Confidence::Guess, "pane:w1:p1")
    );
    assert_eq!(found[0].actions[found[0].suggested], Action::Leave);
}

#[test]
fn an_empty_duplicate_project_workspace_is_offered_for_closing() {
    let world = World::new("orphans-duplicate", Value::Null);
    let root = world.dir("webshop");
    let main = |id: &str| {
        json!({"workspace_id": id, "label": "webshop supervisor", "tokens": {
            CORGI_WORKSPACE_ROLE_TOKEN: CORGI_PROJECT_MAIN_ROLE,
            CORGI_PROJECT_MAIN_TAB_TOKEN: format!("{id}:t1"),
            CORGI_PROJECT_ROOT_TOKEN: root,
        }})
    };
    world.herdr.set_snapshot(snapshot(
        vec![main("w45"), main("w9X")],
        vec![with_tokens(
            agent("w45:p1", "claude", "webshop-supervisor", &root, Some("s-1")),
            json!({SUPERVISOR_TOKEN: "session:s-1"}),
        )],
        vec![json!({"pane_id": "w9X:p1", "workspace_id": "w9X", "tab_id": "w9X:t1", "cwd": root})],
    ));
    let found = world.detect();
    assert_eq!(found.len(), 1, "{found:#?}");
    let duplicate = &found[0];
    assert_eq!(duplicate.key, "workspace:w9X");
    assert_eq!(
        duplicate.actions[duplicate.suggested],
        Action::Close {
            target: "workspace".into()
        }
    );
    let status = apply(
        &world.client,
        &world.options,
        &duplicate.key,
        &duplicate.fingerprint,
        &duplicate.actions[duplicate.suggested],
    )
    .expect("closed");
    assert!(status.contains("webshop supervisor"), "{status}");
    let closed = world.herdr.requests("workspace.close");
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0]["params"]["workspace_id"], "w9X");
}

#[test]
fn applying_marks_the_chosen_role_and_refuses_a_pane_whose_occupant_changed() {
    let (world, root) = unmarked_supervisor("orphans-apply", Some("s-1"), vec![]);
    let found = world.detect();
    let lost = &found[0];
    let mark = Action::Supervisor {
        project: "webshop".into(),
        project_root: root.clone(),
    };
    // Another session now in the pane: refused, nothing written.
    let mut changed = snapshot(
        vec![workspace("w1", "webshop")],
        vec![agent(
            "w1:p1",
            "claude",
            "webshop-supervisor",
            &root,
            Some("s-9"),
        )],
        vec![],
    );
    world.herdr.set_snapshot(changed.clone());
    let refused = apply(
        &world.client,
        &world.options,
        &lost.key,
        &lost.fingerprint,
        &mark,
    )
    .unwrap_err()
    .to_string();
    assert!(
        refused.contains("changed since the list was read"),
        "{refused}"
    );
    assert!(world.reported().is_empty());

    // An action it does not offer is refused too.
    changed["agents"][0]["agent_session"]["value"] = json!("s-1");
    world.herdr.set_snapshot(changed);
    let worker = Action::Worker {
        project: "webshop".into(),
        project_root: root,
        request_id: "r".into(),
        agent: "w".into(),
    };
    assert!(
        apply(
            &world.client,
            &world.options,
            &lost.key,
            &lost.fingerprint,
            &worker
        )
        .is_err()
    );

    let status = apply(
        &world.client,
        &world.options,
        &lost.key,
        &lost.fingerprint,
        &mark,
    )
    .expect("marked");
    assert_eq!(status, "Marked webshop-supervisor as webshop's supervisor");
    assert_eq!(world.reported()[0].1[SUPERVISOR_TOKEN], "session:s-1");
}

#[test]
fn a_left_candidate_stays_away_until_its_evidence_changes() {
    let world = World::new("orphans-leave", Value::Null);
    let cwd = world.dir("scratchpad");
    world.herdr.set_snapshot(snapshot(
        vec![workspace("wR", "scratchpad")],
        vec![agent("wR:p2", "codex", "codex", &cwd, None)],
        vec![],
    ));
    world
        .herdr
        .set_processes("wR:p2", json!([process("codex", &cwd, &[], 41)]));
    let found = world.detect();
    let status = apply(
        &world.client,
        &world.options,
        &found[0].key,
        &found[0].fingerprint,
        &Action::Leave,
    )
    .expect("left");
    assert_eq!(status, "Left codex as it is");
    assert!(world.detect().is_empty());
    // Nothing in Herdr changed.
    assert!(world.reported().is_empty());
    assert!(world.herdr.requests("pane.close").is_empty());

    // Another codex process in the pane is new evidence.
    world
        .herdr
        .set_processes("wR:p2", json!([process("codex", &cwd, &[], 99)]));
    assert_eq!(world.detect().len(), 1);
}

#[test]
fn closing_a_hidden_agent_closes_only_its_pane_in_the_users_own_workspace() {
    let world = World::new("orphans-close-pane", Value::Null);
    let cwd = world.dir("scratchpad");
    world.herdr.set_snapshot(snapshot(
        vec![workspace("wR", "my shells")],
        vec![agent("wR:p2", "codex", "codex", &cwd, None)],
        vec![],
    ));
    world
        .herdr
        .set_processes("wR:p2", json!([process("codex", &cwd, &[], 41)]));
    let found = world.detect();
    let close = Action::Close {
        target: "pane".into(),
    };
    apply(
        &world.client,
        &world.options,
        &found[0].key,
        &found[0].fingerprint,
        &close,
    )
    .expect("closed");
    let closed = world.herdr.requests("pane.close");
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0]["params"]["pane_id"], "wR:p2");
    assert!(world.herdr.requests("workspace.close").is_empty());
    assert!(world.herdr.requests("tab.close").is_empty());
}

#[test]
fn the_report_lists_each_candidate_with_its_evidence_and_choices() {
    assert_eq!(report(&[]), "No orphaned sessions.\n");
    let (world, _) = unmarked_supervisor("orphans-report", Some("s-1"), vec![]);
    let text = report(&world.detect());
    assert!(text.starts_with("1 orphaned session:\n"), "{text}");
    assert!(
        text.contains("supervisor, unmarked — claude webshop-supervisor (w1:p1) [certain]"),
        "{text}"
    );
    assert!(
        text.contains("[s] supervisor of webshop (suggested) · [l] leave · [x] close pane"),
        "{text}"
    );
}
