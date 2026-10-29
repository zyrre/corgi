//! A saved snapshot of two supervisors, one per harness, with their native
//! conversations, memory and roles in a scratch directory, for the recovery
//! tests and the dashboard's.

use super::*;
use crate::test_support::ScratchDir;
use serde_json::json;
use std::fs;

pub(crate) const CODEX_ID: &str = "00000000-0000-4000-8000-000000000001";
pub(crate) const CLAUDE_ID: &str = "00000000-0000-4000-8000-000000000002";

pub(crate) struct Fixture {
    pub(crate) dir: ScratchDir,
    pub(crate) client: HerdrClient,
    pub(crate) snapshot: Snapshot,
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "only the recovery tests read it")
    )]
    pub(crate) runtime: Runtime,
}
impl Fixture {
    pub(crate) fn new() -> Self {
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
    #[cfg(test)]
    pub(crate) fn store(&self) -> Store {
        Store::for_client(&self.client).unwrap()
    }
}

pub(crate) fn dashboard_agents(snapshot: &Snapshot) -> Vec<DashboardAgent> {
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

pub fn hold_fixture() -> (ScratchDir, HerdrClient, Snapshot, Vec<DashboardAgent>) {
    let f = Fixture::new();
    let agents = dashboard_agents(&f.snapshot);
    (f.dir, f.client, f.snapshot, agents)
}
