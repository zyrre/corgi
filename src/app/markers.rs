//! Corgi's own record of the metadata tokens it sets in Herdr, and putting
//! back the ones Herdr lost.
//!
//! A Herdr restart or live handoff restores panes and workspaces under the
//! same IDs but without the tokens clients set. Corgi recognises its handler,
//! its project workspaces and its agent workspaces by those tokens alone, so
//! every one of its marks goes through here: it is reported to Herdr and
//! written to a file in Corgi's state directory, `markers/<socket>.json`,
//! together with what identifies the pane's agent or the workspace's
//! checkout. The dashboard that wakes handlers compares the record with each
//! snapshot ([`App::restore_markers`]): where Herdr has Corgi's tokens, Herdr
//! is right and the record follows it; where Herdr has none, they are put
//! back if the pane or workspace still holds the same occupant, and the
//! entry is dropped if it does not.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write,
    path::Path,
    time::Duration,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    handler::{self, BASELINE_TOKEN, CORGI_HANDLER_TOKEN, HANDOVER_TOKEN, OLD_HANDLER_TOKEN},
    herdr::{HerdrClient, SessionSnapshot},
    model::{AgentInfo, WorkspaceInfo},
};

use super::{
    App,
    launch::CORGI_REQUEST_TOKEN,
    project_main::{
        CORGI_METADATA_SOURCE, CORGI_PROJECT_MAIN_TAB_TOKEN, CORGI_PROJECT_ROOT_HASH_TOKEN,
        CORGI_PROJECT_ROOT_TOKEN, CORGI_WORKSPACE_ROLE_TOKEN, project_root_digest,
    },
};

/// The pane tokens Corgi sets: the handler's marks on the handler's pane,
/// and a spawned worker's request id on that worker's. The marker an older
/// Corgi set under its pre-rename key counts among them, so that it is read,
/// recorded under the new key, and cleared whenever Corgi writes.
const PANE_TOKENS: [&str; 5] = [
    CORGI_HANDLER_TOKEN,
    OLD_HANDLER_TOKEN,
    HANDOVER_TOKEN,
    BASELINE_TOKEN,
    CORGI_REQUEST_TOKEN,
];

/// The workspace tokens Corgi sets, on its project and agent workspaces.
const WORKSPACE_TOKENS: [&str; 4] = [
    CORGI_WORKSPACE_ROLE_TOKEN,
    CORGI_PROJECT_MAIN_TAB_TOKEN,
    CORGI_PROJECT_ROOT_TOKEN,
    CORGI_PROJECT_ROOT_HASH_TOKEN,
];

type Tokens = BTreeMap<String, String>;

/// The marks Corgi set in one Herdr session, by pane and workspace ID.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    #[serde(default)]
    panes: BTreeMap<String, PaneMarks>,
    #[serde(default)]
    workspaces: BTreeMap<String, WorkspaceMarks>,
}

/// Corgi's tokens on one pane and the agent they were set for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct PaneMarks {
    /// The last observed Herdr name, used only until a session is known.
    agent: String,
    /// That agent's harness session, once Corgi has seen one. Tokens are
    /// only ever put back on the same session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<String>,
    tokens: Tokens,
}

/// Corgi's tokens on one workspace and the checkout it was open in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct WorkspaceMarks {
    /// The checkout Herdr reports for the workspace; `None` for one Herdr
    /// knows no Git checkout of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkout: Option<String>,
    tokens: Tokens,
}

/// Tokens to put back where Herdr lost them.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Restore {
    Pane(String, Tokens),
    Workspace(String, Tokens),
}

/// Marks the pane of the agent named `agent`, whose harness session is
/// `session` when it is known yet, with Corgi's `tokens`, and records them.
/// `tokens` is the complete set of Corgi pane tokens to keep. Herdr patches
/// metadata, so omitted Corgi keys are explicitly cleared with JSON null.
pub(super) fn mark_pane(
    client: &HerdrClient,
    pane_id: &str,
    agent: &str,
    session: Option<&str>,
    tokens: &[(&str, &str)],
) -> Result<()> {
    let mut tokens = owned(tokens);
    if tokens.contains_key(CORGI_HANDLER_TOKEN) {
        tokens.insert(CORGI_HANDLER_TOKEN.into(), handler::marker(agent, session));
    }
    client.report_pane_metadata(pane_id, CORGI_METADATA_SOURCE, &pane_patch(&tokens))?;
    remember(client, |record| {
        record.panes.insert(
            pane_id.to_string(),
            PaneMarks {
                agent: agent.to_string(),
                session: session.map(str::to_string),
                tokens,
            },
        );
    });
    Ok(())
}

/// Marks a workspace with Corgi's `tokens`, and records them with the
/// workspace's `checkout`. An unknown checkout keeps the one already
/// recorded; the next snapshot with the tokens in place records Herdr's own.
pub(super) fn mark_workspace(
    client: &HerdrClient,
    workspace_id: &str,
    checkout: Option<&str>,
    tokens: &[(&str, &str)],
) -> Result<()> {
    client.report_workspace_metadata(workspace_id, CORGI_METADATA_SOURCE, tokens)?;
    remember(client, |record| {
        let checkout = checkout.map(str::to_string).or_else(|| {
            record
                .workspaces
                .get(workspace_id)
                .and_then(|marks| marks.checkout.clone())
        });
        record.workspaces.insert(
            workspace_id.to_string(),
            WorkspaceMarks {
                checkout,
                tokens: owned(tokens),
            },
        );
    });
    Ok(())
}

/// Takes the lock that `corgi spawn --request-id` holds in the project at
/// `root` from its check for a running worker with the id to the end of its
/// launch, so that a retry made while the first spawn still runs waits for
/// it and then finds its worker. It sits beside the client's record; without
/// one there is nothing to lock. `waiting` is called once if another spawn
/// holds it.
pub(super) fn spawn_lock(
    client: &HerdrClient,
    root: &str,
    waiting: impl FnOnce(),
) -> Result<Option<fs::File>> {
    let Some(record) = client.marker_record() else {
        return Ok(None);
    };
    let dir = record
        .parent()
        .context("the marker record has no directory")?;
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let stem = record.file_stem().unwrap_or_default().to_string_lossy();
    let digest = project_root_digest(root);
    let digest = digest.trim_start_matches("fnv1a64:");
    let path = dir.join(format!("{stem}.spawn-{digest}.lock"));
    let lock = fs::File::create(&path).with_context(|| format!("create {}", path.display()))?;
    if lock.try_lock().is_err() {
        waiting();
        lock.lock()
            .with_context(|| format!("lock {}", path.display()))?;
    }
    Ok(Some(lock))
}

/// Only Herdr's live session can establish pane ownership.
pub(super) fn session_of(agent: &AgentInfo) -> Option<&str> {
    handler::native_session(agent)
}

/// The checkout Herdr reports for a workspace.
pub(super) fn checkout_of(workspace: &WorkspaceInfo) -> Option<&str> {
    workspace
        .worktree
        .as_ref()
        .map(|worktree| worktree.checkout_path.as_str())
        .filter(|checkout| !checkout.is_empty())
}

/// Applies `change` to the client's record, if it keeps one. The Herdr mark
/// is what counts; a record that cannot be written only costs putting it
/// back after a restart.
fn remember(client: &HerdrClient, change: impl FnOnce(&mut Record)) {
    if let Some(path) = client.marker_record() {
        let _ = update(path, change);
    }
}

/// Reads the record at `path`, lets `change` edit it, and writes it back if
/// it changed, all under a lock beside it, so that a dashboard and
/// `corgi spawn` never lose each other's entries.
fn update<T>(path: &Path, change: impl FnOnce(&mut Record) -> T) -> Result<T> {
    let dir = path
        .parent()
        .context("the marker record has no directory")?;
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let lock = fs::File::create(path.with_extension("lock"))?;
    lock.lock()?;
    let mut record = read(path);
    let before = record.clone();
    let changed = change(&mut record);
    let written = if record == before {
        Ok(())
    } else {
        write(path, &record)
    };
    // Unlocked here rather than on close: a child process spawned on another
    // thread may briefly hold a copy of the descriptor.
    let _ = lock.unlock();
    written.map(|()| changed)
}

/// The record at `path`; an empty one when there is none or it cannot be
/// read, which the next write replaces. A marker an older Corgi recorded
/// under its pre-rename key reads as the current one.
fn read(path: &Path) -> Record {
    let mut record: Record = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    for marks in record.panes.values_mut() {
        handler::upgrade_marker(&mut marks.tokens);
    }
    record
}

/// Replaces the record in one step: it goes to a temporary file of this
/// process's own, which is then renamed over the record, so a reader sees
/// the old record or the new one and never half of either.
fn write(path: &Path, record: &Record) -> Result<()> {
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let written = fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(&serde_json::to_vec_pretty(record)?)?;
        file.sync_all()
    });
    if let Err(error) = written.and_then(|()| fs::rename(&temporary, path)) {
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("write {}", path.display()));
    }
    Ok(())
}

fn owned(tokens: &[(&str, &str)]) -> Tokens {
    tokens
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Corgi's own tokens among `tokens`, those named in `keys`.
fn corgi_tokens(tokens: &Tokens, keys: &[&str]) -> Tokens {
    tokens
        .iter()
        .filter(|(key, _)| keys.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Brings `record` in line with `snapshot` and returns the tokens to put
/// back.
///
/// Live markers win when they identify the current session. Legacy markers
/// migrate by matching the name, or by matching the persisted session after
/// a rename. A recorded session mismatch rejects an unchanged legacy marker.
/// Lost tokens are restored only to the recorded session (or workspace
/// checkout). Missing native identity waits for a later snapshot.
fn reconcile(record: &mut Record, snapshot: &SessionSnapshot) -> Vec<Restore> {
    let mut restores = Vec::new();

    let agents: HashMap<&str, &AgentInfo> = snapshot
        .agents
        .iter()
        .map(|agent| (agent.pane_id.as_str(), agent))
        .collect();
    // What Herdr has on each pane it knows, from the agent in it if any.
    let held: HashMap<&str, Tokens> = snapshot
        .panes
        .iter()
        .map(|pane| (pane.pane_id.as_str(), &pane.tokens))
        .chain(
            snapshot
                .agents
                .iter()
                .map(|agent| (agent.pane_id.as_str(), &agent.tokens)),
        )
        .map(|(pane, tokens)| (pane, corgi_tokens(tokens, &PANE_TOKENS)))
        .collect();
    for (pane, held_tokens) in held.iter().filter(|(_, tokens)| !tokens.is_empty()) {
        // A marker under the pre-rename key is read as the current one; the
        // difference makes the pane's marks be written again, under it.
        let mut upgraded = held_tokens.clone();
        handler::upgrade_marker(&mut upgraded);
        let tokens = &upgraded;
        let Some(agent) = agents.get(pane) else {
            // A native session can be temporarily undiscovered after restart.
            // Keep its record, but never restore until that session returns.
            continue;
        };
        let session = session_of(agent);
        let live_marker = tokens.get(CORGI_HANDLER_TOKEN);
        let legacy = live_marker.is_some_and(|value| !value.starts_with("session:"));
        let previous = record
            .panes
            .get(*pane)
            .filter(|marks| marks.tokens.get(CORGI_HANDLER_TOKEN) == live_marker || legacy);
        let recorded_session = previous.and_then(|marks| marks.session.as_deref());
        let same_session = session.is_some() && session == recorded_session;
        let stale = matches!((session, recorded_session), (Some(now), Some(then)) if now != then);
        // A worker's request id carries no identity of its own: while Herdr
        // has it and no recorded session says otherwise, the agent in the
        // pane is the one it was set for, and its session is bound to it.
        let unbound_worker = live_marker.is_none() && recorded_session.is_none();
        if stale || !(handler::is_handler(agent) || same_session || unbound_worker) {
            // A missing native identity is inconclusive, not permission to
            // fall back to the name on a session-bound marker.
            if session.is_none()
                && (recorded_session.is_some()
                    || live_marker.is_some_and(|value| value.starts_with("session:")))
            {
                continue;
            }
            // Keep the old session until clearing succeeds, so a failed
            // report cannot let a stale legacy name become trusted next time.
            if !stale {
                record.panes.remove(*pane);
            }
            // Remove stale live tokens too: legacy name markers otherwise
            // still match a new session launched under the same name.
            restores.push(Restore::Pane(pane.to_string(), Tokens::new()));
            continue;
        }
        let name = agent.name.as_deref().unwrap_or_default();
        let mut promoted = tokens.clone();
        if session.is_some() && live_marker.is_some() {
            promoted.insert(CORGI_HANDLER_TOKEN.into(), handler::marker(name, session));
        }
        let session = session
            .map(str::to_string)
            .or_else(|| recorded_session.map(str::to_string));
        if promoted != *held_tokens {
            restores.push(Restore::Pane(pane.to_string(), promoted.clone()));
        }
        record.panes.insert(
            pane.to_string(),
            PaneMarks {
                agent: name.to_string(),
                session,
                tokens: promoted,
            },
        );
    }
    record.panes.retain(|pane, marks| {
        let Some(tokens) = held.get(pane.as_str()) else {
            return false;
        };
        if !tokens.is_empty() {
            return true;
        }
        let Some(agent) = agents.get(pane.as_str()) else {
            return true;
        };
        match (session_of(agent), marks.session.as_deref()) {
            (None, _) => true,
            (Some(now), Some(then)) if now == then => {
                // Persisted name markers are safe to migrate even after a
                // rename, because their recorded native session still matches.
                marks.agent = agent.name.clone().unwrap_or_default();
                if marks.tokens.contains_key(CORGI_HANDLER_TOKEN) {
                    marks.tokens.insert(
                        CORGI_HANDLER_TOKEN.into(),
                        handler::marker(&marks.agent, Some(now)),
                    );
                }
                restores.push(Restore::Pane(pane.clone(), marks.tokens.clone()));
                true
            }
            _ => false,
        }
    });

    let workspaces: HashMap<&str, &WorkspaceInfo> = snapshot
        .workspaces
        .iter()
        .map(|workspace| (workspace.workspace_id.as_str(), workspace))
        .collect();
    for workspace in &snapshot.workspaces {
        let tokens = corgi_tokens(&workspace.tokens, &WORKSPACE_TOKENS);
        if !tokens.is_empty() {
            record.workspaces.insert(
                workspace.workspace_id.clone(),
                WorkspaceMarks {
                    checkout: checkout_of(workspace).map(str::to_string),
                    tokens,
                },
            );
        }
    }
    record.workspaces.retain(|id, marks| {
        let Some(workspace) = workspaces.get(id.as_str()) else {
            return false;
        };
        if !corgi_tokens(&workspace.tokens, &WORKSPACE_TOKENS).is_empty() {
            return true;
        }
        if checkout_of(workspace) != marks.checkout.as_deref() {
            return false;
        }
        restores.push(Restore::Workspace(id.clone(), marks.tokens.clone()));
        true
    });

    restores
}

/// Reports `tokens` as Corgi's, for a restore.
fn pairs(tokens: &Tokens) -> Vec<(&str, &str)> {
    tokens
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

/// Replace Corgi's managed pane tokens using Herdr's patch API. Include null
/// for every omitted managed key while leaving other sources' keys alone.
fn pane_patch(tokens: &Tokens) -> Vec<(&str, Option<&str>)> {
    PANE_TOKENS
        .iter()
        .map(|key| (*key, tokens.get(*key).map(String::as_str)))
        .collect()
}

impl App {
    /// Puts back the tokens Corgi set that Herdr lost, as after a restart or
    /// live handoff, and keeps the record in step with `snapshot`. What is
    /// put back goes into `snapshot` too, so the refresh that found the loss
    /// already sees the handler and the project workspaces again. Only the
    /// dashboard that wakes handlers calls this, so one process writes.
    pub(super) fn restore_markers(&mut self, snapshot: &mut SessionSnapshot) {
        let Some(path) = self.client.marker_record() else {
            return;
        };
        let Ok(restores) = update(path, |record| reconcile(record, snapshot)) else {
            return;
        };
        let mut restored = 0;
        for restore in restores {
            match restore {
                Restore::Pane(pane_id, tokens) => {
                    let reported = self
                        .client
                        .report_pane_metadata(&pane_id, CORGI_METADATA_SOURCE, &pane_patch(&tokens))
                        .is_ok();
                    if !reported && !tokens.is_empty() {
                        continue;
                    }
                    // Rejected ownership must also be hidden from this
                    // refresh's waker even if clearing live metadata failed.
                    for agent in snapshot.agents.iter_mut().filter(|a| a.pane_id == pane_id) {
                        agent
                            .tokens
                            .retain(|key, _| !PANE_TOKENS.contains(&key.as_str()));
                        agent.tokens.extend(tokens.clone());
                    }
                    for pane in snapshot.panes.iter_mut().filter(|p| p.pane_id == pane_id) {
                        pane.tokens
                            .retain(|key, _| !PANE_TOKENS.contains(&key.as_str()));
                        pane.tokens.extend(tokens.clone());
                    }
                    if !reported {
                        continue;
                    }
                }
                Restore::Workspace(workspace_id, tokens) => {
                    if self
                        .client
                        .report_workspace_metadata(
                            &workspace_id,
                            CORGI_METADATA_SOURCE,
                            &pairs(&tokens),
                        )
                        .is_err()
                    {
                        continue;
                    }
                    for workspace in snapshot
                        .workspaces
                        .iter_mut()
                        .filter(|w| w.workspace_id == workspace_id)
                    {
                        workspace.tokens.extend(tokens.clone());
                    }
                }
            }
            restored += 1;
        }
        if restored > 0 {
            self.set_status(
                format!("Reconciled {restored} of Corgi's pane and workspace marks"),
                Some(Duration::from_secs(15)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader},
        os::unix::net::UnixListener,
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };

    use serde_json::{Value, json};

    use crate::{
        app::{
            cli::{fleet_rows, requested_agent},
            project_main::project_root_digest,
            waker::HandlerWaker,
        },
        model::{AgentSession, WorkspaceWorktreeInfo},
        test_support::{ScratchDir, fake_herdr, test_app},
    };

    use super::*;

    const ROOT: &str = "/repos/corgi-markers";
    const SESSION: &str = "5e55-handler";

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

    /// A session like the one of 2026-09-27: the corgi handler in its
    /// project workspace, which it was asked to hand over from, and one
    /// worker in a Corgi worktree workspace.
    fn marked_session() -> Value {
        let handler_tokens = json!({
            "corgi_handler": "handler-corgi",
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
                    "label": "corgi-markers handler",
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
                { "pane_id": "w70:p1", "workspace_id": "w70", "tab_id": "w70:t1", "tokens": handler_tokens },
                { "pane_id": "w8Z:p1", "workspace_id": "w8Z", "tab_id": "w8Z:t1", "tokens": worker_tokens }
            ],
            "agents": [
                {
                    "pane_id": "w70:p1", "workspace_id": "w70", "tab_id": "w70:t1",
                    "agent": "claude", "name": "handler-corgi", "agent_status": "working",
                    "cwd": ROOT,
                    "agent_session": { "agent": "claude", "kind": "id", "value": SESSION },
                    "tokens": handler_tokens
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

    /// The dashboard that wakes handlers, on `client`.
    fn leading_dashboard(client: HerdrClient, scratch: &Path) -> App {
        let mut app = test_app();
        app.client = client;
        let mut waker = HandlerWaker::default();
        assert!(waker.lead(&scratch.join("wake.lock")));
        app.handler_waker = Some(waker);
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

    /// A Steward an older Corgi launched carries its marker under the
    /// pre-rename key, in Herdr and in the record. After the dashboard is
    /// rebuilt and restarted, it is still the project's handler, and its
    /// marker moves to the new key.
    #[test]
    fn a_steward_marked_by_an_older_corgi_stays_the_handler_under_the_new_key() {
        let scratch = ScratchDir::new("markers-steward");
        let record = scratch.join("markers").join("herdr.json");
        let marker = handler::marker("", Some(SESSION));
        fs::create_dir_all(record.parent().unwrap()).unwrap();
        let old_record = json!({
            "panes": { "w70:p1": {
                "agent": "steward-corgi", "session": SESSION,
                "tokens": { OLD_HANDLER_TOKEN: marker }
            } }
        });
        fs::write(&record, old_record.to_string()).unwrap();
        assert_eq!(
            read(&record).panes["w70:p1"].tokens,
            owned(&[(CORGI_HANDLER_TOKEN, &marker)]),
            "the old record reads under the new key"
        );
        let mut live = marked_session();
        live["agents"][0]["name"] = json!("steward-corgi");
        for list in ["agents", "panes"] {
            let tokens = live[list][0]["tokens"].as_object_mut().unwrap();
            tokens.remove(CORGI_HANDLER_TOKEN);
            tokens.insert(OLD_HANDLER_TOKEN.into(), json!(marker));
        }
        let herdr = StatefulHerdr::new("markers-steward", live);
        let mut dashboard = leading_dashboard(herdr.client(&record), &scratch);

        dashboard.refresh();
        assert_eq!(
            roles(&dashboard),
            ["steward-corgi handler", "w-brave worker"]
        );
        for list in ["agents", "panes"] {
            let tokens = herdr.tokens(list, "pane_id", "w70:p1");
            assert_eq!(tokens[CORGI_HANDLER_TOKEN], json!(marker), "{list}");
            assert!(tokens.get(OLD_HANDLER_TOKEN).is_none(), "{list}: {tokens}");
            assert_eq!(
                tokens[HANDOVER_TOKEN],
                json!(format!("1790000000 {SESSION}"))
            );
        }
        let marks = &read(&record).panes["w70:p1"];
        assert_eq!(marks.tokens[CORGI_HANDLER_TOKEN], marker);
        assert!(!marks.tokens.contains_key(OLD_HANDLER_TOKEN));

        // The next refresh finds the marks in place and writes nothing.
        let reports = herdr.reports().len();
        dashboard.refresh();
        assert_eq!(herdr.reports().len(), reports);
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
            herdr.tokens("agents", "pane_id", "w70:p1")[CORGI_HANDLER_TOKEN],
            handler::marker("", Some(SESSION))
        );
        let before = [
            herdr.tokens("agents", "pane_id", "w70:p1"),
            herdr.tokens("workspaces", "workspace_id", "w70"),
            herdr.tokens("workspaces", "workspace_id", "w8Z"),
        ];
        assert_eq!(
            roles(&dashboard),
            ["handler-corgi handler", "w-brave worker"]
        );

        herdr.restart();
        let mut fleet = test_app();
        fleet.client = herdr.client(&record);
        fleet.refresh();
        assert_eq!(
            roles(&fleet),
            ["handler-corgi worker", "w-brave worker"],
            "without its marks the Project handler is a worker"
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
        // The refresh that put them back already sees the handler.
        assert_eq!(
            roles(&dashboard),
            ["handler-corgi handler", "w-brave worker"]
        );
        assert_eq!(herdr.reports().len(), 4);

        // And so does `corgi fleet`.
        fleet.refresh();
        assert_eq!(roles(&fleet), ["handler-corgi handler", "w-brave worker"]);

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

    fn handler_agent(
        pane: &str,
        name: &str,
        session: Option<&str>,
        tokens: &[(&str, &str)],
    ) -> AgentInfo {
        AgentInfo {
            pane_id: pane.into(),
            workspace_id: "w70".into(),
            name: Some(name.into()),
            agent_session: session.map(|value| AgentSession {
                value: value.into(),
                ..AgentSession::default()
            }),
            tokens: owned(tokens),
            ..AgentInfo::default()
        }
    }

    fn handler_marks(session: Option<&str>) -> PaneMarks {
        PaneMarks {
            agent: "handler-corgi".into(),
            session: session.map(str::to_string),
            tokens: owned(&[(CORGI_HANDLER_TOKEN, "handler-corgi")]),
        }
    }

    fn pane(id: &str) -> crate::herdr::PaneInfo {
        crate::herdr::PaneInfo {
            pane_id: id.into(),
            ..Default::default()
        }
    }

    fn record_with_handler(session: Option<&str>) -> Record {
        Record {
            panes: BTreeMap::from([("w70:p1".into(), handler_marks(session))]),
            ..Record::default()
        }
    }

    #[test]
    fn a_pane_that_holds_another_session_now_is_not_marked_and_is_forgotten() {
        for agent in [
            // The same name on a new session, as after a handover Corgi did
            // not record, or a resumed conversation.
            handler_agent("w70:p1", "handler-corgi", Some("new-session"), &[]),
            // Another agent altogether.
            handler_agent("w70:p1", "w-other", Some("another-session"), &[]),
        ] {
            let mut record = record_with_handler(Some(SESSION));
            let snapshot = SessionSnapshot {
                agents: vec![agent],
                panes: vec![pane("w70:p1")],
                ..SessionSnapshot::default()
            };
            assert_eq!(reconcile(&mut record, &snapshot), []);
            assert!(record.panes.is_empty());
        }
    }

    #[test]
    fn a_pane_waits_while_herdr_has_not_found_its_agent_or_session_again() {
        for agents in [
            vec![],
            vec![handler_agent("w70:p1", "handler-corgi", None, &[])],
        ] {
            let mut record = record_with_handler(Some(SESSION));
            let snapshot = SessionSnapshot {
                agents,
                panes: vec![pane("w70:p1")],
                ..SessionSnapshot::default()
            };
            assert_eq!(reconcile(&mut record, &snapshot), []);
            assert_eq!(record, record_with_handler(Some(SESSION)));
        }
        // A pane that is gone is forgotten.
        let mut record = record_with_handler(Some(SESSION));
        assert_eq!(reconcile(&mut record, &SessionSnapshot::default()), []);
        assert!(record.panes.is_empty());
    }

    #[test]
    fn a_marked_pane_is_recorded_only_while_its_handler_is_in_it() {
        let marker = [(CORGI_HANDLER_TOKEN, "handler-corgi")];
        // A handler whose session is not known yet keeps the one recorded.
        let mut record = record_with_handler(Some(SESSION));
        let snapshot = SessionSnapshot {
            agents: vec![handler_agent("w70:p1", "handler-corgi", None, &marker)],
            ..SessionSnapshot::default()
        };
        assert_eq!(reconcile(&mut record, &snapshot), []);
        assert_eq!(record, record_with_handler(Some(SESSION)));
        // Herdr can temporarily lose the agent during restart. Retain the
        // record but restore nothing until its native session returns.
        let snapshot = SessionSnapshot {
            panes: vec![crate::herdr::PaneInfo {
                tokens: owned(&marker),
                ..pane("w70:p1")
            }],
            ..SessionSnapshot::default()
        };
        assert_eq!(reconcile(&mut record, &snapshot), []);
        assert_eq!(record, record_with_handler(Some(SESSION)));
    }

    #[test]
    fn what_herdr_has_wins_over_the_record() {
        let mut record = record_with_handler(Some(SESSION));
        let newer = [
            (CORGI_HANDLER_TOKEN, "session:new-session"),
            (BASELINE_TOKEN, "1000 new-session"),
        ];
        let snapshot = SessionSnapshot {
            agents: vec![handler_agent(
                "w70:p1",
                "handler-corgi",
                Some("new-session"),
                &newer,
            )],
            workspaces: vec![WorkspaceInfo {
                workspace_id: "w9".into(),
                tokens: owned(&[(CORGI_WORKSPACE_ROLE_TOKEN, "agent-workspace")]),
                worktree: Some(WorkspaceWorktreeInfo {
                    checkout_path: "/wt/nine".into(),
                    ..WorkspaceWorktreeInfo::default()
                }),
                ..WorkspaceInfo::default()
            }],
            ..SessionSnapshot::default()
        };
        assert_eq!(reconcile(&mut record, &snapshot), []);
        assert_eq!(
            record.panes["w70:p1"],
            PaneMarks {
                agent: "handler-corgi".into(),
                session: Some("new-session".into()),
                tokens: owned(&newer),
            }
        );
        assert_eq!(
            record.workspaces["w9"].checkout.as_deref(),
            Some("/wt/nine")
        );
    }

    #[test]
    fn legacy_markers_migrate_by_recorded_session_even_after_a_rename() {
        for live in [vec![], vec![(CORGI_HANDLER_TOKEN, "handler-corgi")]] {
            let mut record = record_with_handler(Some(SESSION));
            let snapshot = SessionSnapshot {
                agents: vec![handler_agent(
                    "w70:p1",
                    "renamed-task",
                    Some(SESSION),
                    &live,
                )],
                ..Default::default()
            };
            let expected = owned(&[(CORGI_HANDLER_TOKEN, &handler::marker("", Some(SESSION)))]);
            assert_eq!(
                reconcile(&mut record, &snapshot),
                [Restore::Pane("w70:p1".into(), expected.clone())]
            );
            assert_eq!(record.panes["w70:p1"].tokens, expected);
            assert_eq!(record.panes["w70:p1"].agent, "renamed-task");
            // A failed metadata write retries safely from the upgraded record.
            assert_eq!(
                reconcile(&mut record, &snapshot),
                [Restore::Pane("w70:p1".into(), expected)]
            );
        }
    }

    #[test]
    fn a_live_legacy_marker_cannot_override_a_recorded_session_mismatch() {
        let mut record = record_with_handler(Some(SESSION));
        let snapshot = SessionSnapshot {
            agents: vec![handler_agent(
                "w70:p1",
                "handler-corgi",
                Some("new-session"),
                &[
                    (CORGI_HANDLER_TOKEN, "handler-corgi"),
                    (BASELINE_TOKEN, "1000 old-session"),
                ],
            )],
            ..Default::default()
        };
        assert_eq!(
            reconcile(&mut record, &snapshot),
            [Restore::Pane("w70:p1".into(), Tokens::new())]
        );
        assert_eq!(record, record_with_handler(Some(SESSION)));
        assert_eq!(
            reconcile(&mut record, &snapshot),
            [Restore::Pane("w70:p1".into(), Tokens::new())]
        );
        let mut cleared = snapshot;
        cleared.agents[0].tokens.clear();
        assert!(reconcile(&mut record, &cleared).is_empty());
        assert!(record.panes.is_empty());
    }

    #[test]
    fn rejected_ownership_clears_live_tokens_with_null_patches() {
        for marker in [
            "handler-corgi".to_string(),
            handler::marker("", Some(SESSION)),
        ] {
            let scratch = ScratchDir::new("markers-live-clear");
            let path = scratch.join("markers.json");
            update(&path, |record| {
                *record = record_with_handler(Some(SESSION));
                record
                    .panes
                    .get_mut("w70:p1")
                    .unwrap()
                    .tokens
                    .insert(CORGI_HANDLER_TOKEN.into(), marker.clone());
            })
            .unwrap();
            let mut live = marked_session();
            live["agents"][0]["agent_session"]["value"] = json!("replacement-session");
            for list in ["agents", "panes"] {
                live[list][0]["tokens"][CORGI_HANDLER_TOKEN] = json!(marker);
                live[list][0]["tokens"]["task"] = json!("another plugin's title");
                live[list][0]["tokens"]["corgi_unrelated"] = json!("keep me");
            }
            let herdr = StatefulHerdr::new("markers-live-clear", live);
            let mut app = test_app();
            app.client = herdr.client(&path);
            let mut snapshot = app.client.snapshot().unwrap();
            app.restore_markers(&mut snapshot);
            assert!(!handler::is_handler(&snapshot.agents[0]));
            assert_eq!(herdr.reports().len(), 1);
            assert_eq!(
                herdr.reports()[0]["params"]["tokens"],
                // The pre-rename marker key is cleared too.
                json!({
                    "corgi_handler": null, "corgi_steward": null,
                    "corgi_handover": null, "corgi_baseline": null,
                    "corgi_request": null
                })
            );
            for list in ["agents", "panes"] {
                let tokens = herdr.tokens(list, "pane_id", "w70:p1");
                for key in PANE_TOKENS {
                    assert!(
                        tokens.get(key).is_none(),
                        "{list} still holds {key}: {tokens}"
                    );
                }
                assert_eq!(tokens["session"], SESSION);
                assert_eq!(tokens["model"], "Opus 5.5");
                assert_eq!(tokens["task"], "another plugin's title");
                assert_eq!(tokens["corgi_unrelated"], "keep me");
            }
            // Read independently from Herdr, not the locally edited snapshot.
            let mut fresh = app.client.snapshot().unwrap();
            assert!(!handler::is_handler(&fresh.agents[0]));
            app.restore_markers(&mut fresh);
            assert!(read(&path).panes.is_empty());
            assert_eq!(
                herdr.reports().len(),
                1,
                "cleared tokens must not reappear next refresh"
            );
        }
    }

    #[test]
    fn a_failed_clear_does_not_let_the_waker_adopt_a_reused_pane() {
        let scratch = ScratchDir::new("markers-failed-clear");
        let path = scratch.join("markers.json");
        update(&path, |record| *record = record_with_handler(Some(SESSION))).unwrap();
        let (socket, server) = fake_herdr("markers-failed-clear", |listener| {
            crate::test_support::answer(&listener, |request| {
                assert_eq!(request["method"], "pane.report_metadata");
                assert_eq!(
                    request["params"]["tokens"],
                    // The pre-rename marker key is cleared too.
                    json!({
                        "corgi_handler": null, "corgi_steward": null,
                        "corgi_handover": null, "corgi_baseline": null,
                        "corgi_request": null
                    })
                );
                json!({"error": {"code": "unavailable", "message": "try later"}})
            });
        });
        let mut app = test_app();
        app.client = HerdrClient::from_socket_path(&socket).with_marker_record(&path);
        let mut snapshot = SessionSnapshot {
            agents: vec![handler_agent(
                "w70:p1",
                "handler-corgi",
                Some("new-session"),
                &[(CORGI_HANDLER_TOKEN, "handler-corgi")],
            )],
            ..Default::default()
        };
        app.restore_markers(&mut snapshot);
        assert!(!handler::is_handler(&snapshot.agents[0]));
        assert_eq!(read(&path), record_with_handler(Some(SESSION)));
        server.join().unwrap();
        fs::remove_file(socket).unwrap();
    }

    #[test]
    fn legacy_markers_need_a_matching_name_or_a_known_session() {
        let marker = [(CORGI_HANDLER_TOKEN, "handler-corgi")];
        let mut record = Record::default();
        let mut snapshot = SessionSnapshot {
            agents: vec![handler_agent("w70:p1", "handler-corgi", None, &marker)],
            ..Default::default()
        };
        assert!(reconcile(&mut record, &snapshot).is_empty());
        // A bridge token left on the pane must not bind a native identity.
        snapshot.agents[0]
            .tokens
            .insert("session".into(), SESSION.into());
        assert!(reconcile(&mut record, &snapshot).is_empty());
        assert!(record.panes["w70:p1"].session.is_none());
        snapshot.agents[0].agent_session = Some(AgentSession {
            value: SESSION.into(),
            ..Default::default()
        });
        let expected = owned(&[(CORGI_HANDLER_TOKEN, &handler::marker("", Some(SESSION)))]);
        assert_eq!(
            reconcile(&mut record, &snapshot),
            [Restore::Pane("w70:p1".into(), expected)]
        );

        // An already renamed legacy marker with no recorded session cannot
        // safely be distinguished from a pane now running another agent.
        snapshot.agents[0].name = Some("unproven-rename".into());
        record = record_with_handler(None);
        assert_eq!(
            reconcile(&mut record, &snapshot),
            [Restore::Pane("w70:p1".into(), Tokens::new())]
        );
        assert!(record.panes.is_empty());
    }

    #[test]
    fn a_session_marker_waits_for_native_detection_and_rejects_reuse() {
        let marker = handler::marker("", Some(SESSION));
        let mut record = Record::default();
        let mut snapshot = SessionSnapshot {
            agents: vec![handler_agent(
                "w70:p1",
                "renamed-task",
                None,
                &[(CORGI_HANDLER_TOKEN, &marker)],
            )],
            ..Default::default()
        };
        assert!(reconcile(&mut record, &snapshot).is_empty());
        snapshot.agents[0].agent_session = Some(AgentSession {
            value: SESSION.into(),
            ..Default::default()
        });
        assert!(reconcile(&mut record, &snapshot).is_empty());
        assert_eq!(record.panes["w70:p1"].session.as_deref(), Some(SESSION));
        snapshot.agents[0].agent_session.as_mut().unwrap().value = "new-session".into();
        assert_eq!(
            reconcile(&mut record, &snapshot),
            [Restore::Pane("w70:p1".into(), Tokens::new())]
        );
        assert_eq!(record.panes["w70:p1"].session.as_deref(), Some(SESSION));
        snapshot.agents[0].tokens.clear();
        assert!(reconcile(&mut record, &snapshot).is_empty());
        assert!(record.panes.is_empty());
    }

    #[test]
    fn a_workspace_is_marked_again_only_while_it_has_the_same_checkout() {
        let role = owned(&[(CORGI_WORKSPACE_ROLE_TOKEN, "agent-workspace")]);
        let marks = |checkout: &str| WorkspaceMarks {
            checkout: Some(checkout.into()),
            tokens: role.clone(),
        };
        let mut record = Record {
            workspaces: BTreeMap::from([
                ("w1".into(), marks("/wt/one")),
                ("w2".into(), marks("/wt/two")),
                ("w3".into(), marks("/wt/three")),
            ]),
            ..Record::default()
        };
        let open = |id: &str, checkout: &str| WorkspaceInfo {
            workspace_id: id.into(),
            worktree: Some(WorkspaceWorktreeInfo {
                checkout_path: checkout.into(),
                ..WorkspaceWorktreeInfo::default()
            }),
            ..WorkspaceInfo::default()
        };
        let snapshot = SessionSnapshot {
            // w1 is still itself, w2 is another checkout now, w3 is gone.
            workspaces: vec![open("w1", "/wt/one"), open("w2", "/wt/elsewhere")],
            ..SessionSnapshot::default()
        };
        assert_eq!(
            reconcile(&mut record, &snapshot),
            [Restore::Workspace("w1".into(), role.clone())]
        );
        assert_eq!(record.workspaces.keys().collect::<Vec<_>>(), ["w1"]);
    }

    #[test]
    fn the_record_is_replaced_whole_and_survives_a_torn_file() {
        let scratch = ScratchDir::new("markers-write");
        let path = scratch.join("markers").join("herdr.json");
        update(&path, |record| {
            record
                .panes
                .insert("w1:p1".into(), handler_marks(Some(SESSION)));
        })
        .expect("first write");
        assert_eq!(read(&path), record_with_handler_at("w1:p1"));
        // A file that cannot be parsed counts as empty and is replaced.
        fs::write(&path, b"{\"panes\": {").expect("tear the record");
        update(&path, |record| {
            record
                .panes
                .insert("w1:p1".into(), handler_marks(Some(SESSION)));
        })
        .expect("second write");
        assert_eq!(read(&path), record_with_handler_at("w1:p1"));
        let names: Vec<_> = fs::read_dir(path.parent().expect("dir"))
            .expect("dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|name| !name.ends_with(".tmp")),
            "{names:?}"
        );
    }

    #[test]
    fn corgis_marks_are_recorded_as_they_are_set() {
        let scratch = ScratchDir::new("markers-mark");
        let record = scratch.join("markers").join("herdr.json");
        let herdr = StatefulHerdr::new("markers-mark", marked_session());
        let client = herdr.client(&record);
        let marker = [(CORGI_HANDLER_TOKEN, "handler-corgi")];
        mark_pane(&client, "w70:p1", "handler-corgi", None, &marker).expect("mark pane");
        let role = [(CORGI_WORKSPACE_ROLE_TOKEN, "agent-workspace")];
        mark_workspace(&client, "w8Z", Some("/wt/corgi-markers/brave-stone"), &role)
            .expect("mark workspace");
        // A mark that does not know the checkout keeps the recorded one.
        mark_workspace(&client, "w8Z", None, &role).expect("mark workspace again");
        assert_eq!(
            read(&record),
            Record {
                panes: BTreeMap::from([("w70:p1".into(), handler_marks(None))]),
                workspaces: BTreeMap::from([(
                    "w8Z".into(),
                    WorkspaceMarks {
                        checkout: Some("/wt/corgi-markers/brave-stone".into()),
                        tokens: owned(&role),
                    }
                )]),
            }
        );
        assert_eq!(
            herdr.tokens("agents", "pane_id", "w70:p1")["corgi_handler"],
            "handler-corgi"
        );
        assert_eq!(
            herdr.reports()[0]["params"]["tokens"],
            // The pre-rename marker key is cleared too.
            json!({
                "corgi_handler": "handler-corgi", "corgi_steward": null,
                "corgi_handover": null, "corgi_baseline": null,
                "corgi_request": null
            })
        );
        for list in ["agents", "panes"] {
            let tokens = herdr.tokens(list, "pane_id", "w70:p1");
            assert!(tokens.get(HANDOVER_TOKEN).is_none());
            assert!(tokens.get(BASELINE_TOKEN).is_none());
            assert_eq!(tokens["session"], SESSION);
            assert_eq!(tokens["model"], "Opus 5.5");
        }
        let fresh = client
            .snapshot()
            .expect("fresh live tokens deserialize without null values");
        assert!(handler::is_handler(&fresh.agents[0]));
    }

    fn record_with_handler_at(pane: &str) -> Record {
        Record {
            panes: BTreeMap::from([(pane.into(), handler_marks(Some(SESSION)))]),
            ..Record::default()
        }
    }

    /// A worker `corgi spawn --request-id` started keeps its id while it
    /// runs, gets it back after a Herdr restart, and never becomes the
    /// handler on the way; a retry of the spawn finds it by that id.
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
        assert!(worker_tokens().get(CORGI_HANDLER_TOKEN).is_none());
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
        assert!(worker_tokens().get(CORGI_HANDLER_TOKEN).is_none());
        fleet.refresh();
        assert!(requested_agent(&fleet, ROOT, ID).is_some());
        assert_eq!(roles(&fleet), ["handler-corgi handler", "w-brave worker"]);
    }

    /// Another session in a tagged pane, as when the worker exited and
    /// someone started an agent in its shell, does not inherit the id.
    #[test]
    fn a_request_id_is_not_inherited_by_a_later_session_in_its_pane() {
        let tagged = [(CORGI_REQUEST_TOKEN, "20261002-w-brave")];
        let mut record = Record {
            panes: BTreeMap::from([(
                "w8Z:p1".into(),
                PaneMarks {
                    agent: "w-brave".into(),
                    session: Some("5e55-worker".into()),
                    tokens: owned(&tagged),
                },
            )]),
            ..Record::default()
        };
        let snapshot = SessionSnapshot {
            agents: vec![handler_agent("w8Z:p1", "w-next", Some("later"), &tagged)],
            ..SessionSnapshot::default()
        };
        assert_eq!(
            reconcile(&mut record, &snapshot),
            [Restore::Pane("w8Z:p1".into(), Tokens::new())]
        );
    }

    /// A retry made while the first spawn of the project still runs waits
    /// for it; a spawn in another project does not.
    #[test]
    fn a_spawn_with_a_request_id_waits_for_one_still_running_in_its_project() {
        let scratch = ScratchDir::new("markers-spawn-lock");
        let record = scratch.join("markers").join("herdr.json");
        let client =
            HerdrClient::from_socket_path("/tmp/corgi-unused.sock").with_marker_record(&record);
        let first = spawn_lock(&client, ROOT, || panic!("nothing holds it yet"))
            .expect("first lock")
            .expect("a record has a lock");
        spawn_lock(&client, "/repos/elsewhere", || {
            panic!("another project's lock")
        })
        .expect("another project's lock");

        let (waiting, waited) = std::sync::mpsc::channel();
        let retry = thread::spawn(move || {
            spawn_lock(&client, ROOT, || waiting.send(()).expect("say it waits"))
                .expect("retry lock")
                .is_some()
        });
        waited
            .recv_timeout(Duration::from_secs(5))
            .expect("the retry waits for the first spawn");
        assert!(!retry.is_finished());
        drop(first);
        assert!(retry.join().expect("retry thread"));

        let unrecorded = HerdrClient::from_socket_path("/tmp/corgi-unused.sock");
        assert!(spawn_lock(&unrecorded, ROOT, || ()).unwrap().is_none());
    }
}
