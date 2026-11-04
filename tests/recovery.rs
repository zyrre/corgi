//! The real binary's account of a recovery hold: `corgi recovery`, and the
//! footers of `corgi inbox` and `corgi fleet`. A fake Herdr socket and a private
//! home and state directory; nothing here reaches a live Herdr or real state.

use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use corgi_core::{
    recovery::{Store, fixture::hold_fixture},
    supervisor::SUPERVISOR_TOKEN,
    test_support::{FakeHerdr, git_repo},
};
use serde_json::{Value, json};

/// A state directory outside the temporary one, which recovery refuses.
struct Private(PathBuf);

impl Private {
    fn new(label: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("recovery-{label}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("create the private state");
        Self(dir)
    }
}

impl Drop for Private {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn corgi(socket: &Path, private: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_corgi"))
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", private.join("home"))
        .env("XDG_STATE_HOME", private.join("state"))
        .env("CLAUDE_CONFIG_DIR", private.join("claude"))
        .env("HERDR_SOCKET_PATH", socket)
        .stdin(Stdio::null())
        .output()
        .expect("run corgi")
}

fn text(output: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Herdr's snapshot with the project's supervisor running as `session`.
fn running(root: &Path, kind: &str, session: &str) -> Value {
    let tokens = json!({ (SUPERVISOR_TOKEN): format!("session:{session}") });
    json!({
        "workspaces": [{"workspace_id": "w1", "label": "weather"}],
        "panes": [{"pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1", "tokens": tokens}],
        "agents": [{
            "pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1",
            "agent": kind, "agent_status": "idle", "name": "supervisor-weather",
            "cwd": root, "tokens": tokens,
            "agent_session": {"value": session, "kind": "id", "agent": kind, "source": "test"},
        }]
    })
}

#[test]
fn a_hold_refuses_the_session_it_names_visibly_and_never_a_new_session() {
    const NEW_SESSION: &str = "00000000-0000-4000-8000-0000000000aa";
    let private = Private::new("hold");
    let root = git_repo(&private.0.join("weather"));
    let root = root.canonicalize().unwrap();
    let root_text = root.to_str().unwrap();

    let (_scratch, _client, mut saved, _) = hold_fixture();
    let herdr = FakeHerdr::start("recovery-cli", running(&root, "claude", NEW_SESSION));
    saved.scope = herdr.socket.to_string_lossy().into_owned();
    saved.entries.truncate(1);
    saved.entries[0].project = root_text.into();
    saved.entries[0].cwd = root.clone();
    let old_id = saved.entries[0].native_id.clone().unwrap();
    let old_kind = saved.entries[0].kind.clone();
    let key = format!("{old_kind}:{old_id}");
    let state = private.0.join("state/corgi");
    std::fs::create_dir_all(&state).unwrap();
    Store(state.join("recovery")).save(&saved).unwrap();

    // A new supervisor session: the hold is listed and stops nothing.
    let (listed, stderr) = text(&corgi(&herdr.socket, &private.0, &["recovery"]));
    assert!(
        listed.contains(&format!("{key}\t{root_text}\tsaved\t-\n")),
        "{listed}"
    );
    assert!(!stderr.contains("refused"), "{stderr}");
    let (inbox, _) = text(&corgi(&herdr.socket, &private.0, &["inbox", root_text]));
    assert!(!inbox.contains("refused"), "{inbox}");

    // The session the snapshot names, back and not continued: refused, in
    // each command's footer, with the reason.
    herdr.set_snapshot(running(&root, &old_kind, &old_id));
    let (listed, stderr) = text(&corgi(&herdr.socket, &private.0, &["recovery"]));
    assert!(
        listed.contains(&format!("{key}\t{root_text}\tsaved\tw1:p1\n")),
        "{listed}"
    );
    assert!(stderr.contains("Wakes are refused"), "{stderr}");
    let (inbox, _) = text(&corgi(&herdr.socket, &private.0, &["inbox", root_text]));
    assert!(inbox.contains("Wakes are refused"), "{inbox}");
    assert!(inbox.contains("held until continued"), "{inbox}");
    herdr.set_repo_root(root_text);
    let (_, fleet) = text(&corgi(&herdr.socket, &private.0, &["fleet", root_text]));
    assert!(fleet.contains("Wakes are refused"), "{fleet}");

    // It is released on purpose only, and running takes saying so.
    let refused = corgi(&herdr.socket, &private.0, &["recovery", "release", &key]);
    assert!(!refused.status.success());
    assert!(text(&refused).1.contains("--running"));
    assert!(
        text(&corgi(&herdr.socket, &private.0, &["inbox", root_text]))
            .0
            .contains("refused")
    );
    let released = corgi(
        &herdr.socket,
        &private.0,
        &["recovery", "release", &key, "--running"],
    );
    assert!(released.status.success(), "{:?}", text(&released));
    let (listed, _) = text(&corgi(&herdr.socket, &private.0, &["recovery"]));
    assert_eq!(listed, "No recovered session is held.\n");
    let (inbox, _) = text(&corgi(&herdr.socket, &private.0, &["inbox", root_text]));
    assert!(!inbox.contains("refused"), "{inbox}");
}
