use std::os::unix::{fs::PermissionsExt, net::UnixListener};

use super::*;
use crate::test_support::ScratchDir;

/// A fake herdr-app in a scratch home: `ls` prints `ls.json`, `do NAME`
/// prints `reply-NAME.json` (else `{"ok":true}`) or fails when `fail-NAME`
/// is there, and every call is logged to `calls.log`. The window is open
/// while `window` holds its socket.
struct Fake {
    dir: ScratchDir,
    app: HerdrApp,
    window: Option<UnixListener>,
}

const SCRIPT: &str = r#"#!/bin/sh
h=$HERDR_APP_HOME
printf '%s\n' "$*" >>"$h/calls.log"
case $1 in
  ls) cat "$h/ls.json" ;;
  do)
    if [ -e "$h/fail-$2" ]; then echo "{\"error\": \"$2 refused\"}"; exit 1; fi
    if [ -e "$h/reply-$2.json" ]; then cat "$h/reply-$2.json"; else echo '{"ok": true}'; fi
    ;;
  *) exit 2 ;;
esac
"#;

impl Fake {
    fn new(label: &str, spaces: Value) -> Self {
        let dir = ScratchDir::new(label);
        let bin = dir.join("herdr-app");
        fs::write(&bin, SCRIPT).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let home = dir.join("home");
        fs::create_dir_all(&home).unwrap();
        let fake = Self {
            app: HerdrApp::new(bin, &home),
            window: None,
            dir,
        };
        fake.set_spaces(spaces);
        fake
    }

    fn open(mut self) -> Self {
        self.window = Some(UnixListener::bind(self.app.home.join("herdr-app.sock")).unwrap());
        self
    }

    fn set_spaces(&self, spaces: Value) {
        fs::write(
            self.app.home.join("ls.json"),
            json!({ "spaces": spaces, "recent": [] }).to_string(),
        )
        .unwrap();
    }

    fn file(&self, name: &str, text: &str) {
        fs::write(self.app.home.join(name), text).unwrap();
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.app.home.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn space(id: &str, name: &str, machine: Option<&str>, tabs: Value) -> Value {
    json!({ "id": id, "name": name, "folder": "/x", "machine": machine, "active": false, "tabs": tabs })
}

fn shell_tab(id: &str, name: &str) -> Value {
    json!({ "id": id, "name": name, "active": true, "settled": false,
            "terminals": [{ "terminal": "t1", "pane": "p1", "status": null }] })
}

fn mod_tab(id: &str) -> Value {
    json!({ "id": id, "name": TAB_NAME, "active": true, "settled": false, "terminals": [] })
}

#[test]
fn with_no_window_nothing_is_asked() {
    let fake = Fake::new("app-space-closed", json!([]));
    assert_eq!(ensure(&fake.app).unwrap(), Ensured::NoWindow);
    assert_eq!(remove(&fake.app).unwrap(), Removed::NoWindow);
    assert!(fake.calls().is_empty());
}

#[test]
fn a_corgi_space_is_made_first_with_the_mod_as_its_only_tab() {
    let fake = Fake::new(
        "app-space-make",
        json!([
            // The repo's own space, unnamed, and another machine's "Corgi".
            space("s-repo", "corgi", None, json!([shell_tab("tb-r", "1")])),
            space("vps/s9", "Corgi", Some("vps"), json!([mod_tab("vps/tb9")])),
        ]),
    )
    .open();
    fake.file(
        "reply-space.new.json",
        r#"{"space": "sp-new", "tab": "tb-shell", "terminal": "t9"}"#,
    );
    assert_eq!(ensure(&fake.app).unwrap(), Ensured::Made("sp-new".into()));
    let home = home().unwrap_or_else(|| PathBuf::from("/"));
    let calls = fake.calls();
    assert_eq!(calls[0], "ls");
    let commands: Vec<(String, Value)> = calls[1..]
        .iter()
        .map(|call| {
            let (name, args) = call.strip_prefix("do ").unwrap().split_once(' ').unwrap();
            (name.to_string(), serde_json::from_str(args).unwrap())
        })
        .collect();
    assert_eq!(
        commands,
        vec![
            (
                "space.new".into(),
                json!({ "name": "Corgi", "folder": home, "machine": "" })
            ),
            (
                "tab.new".into(),
                json!({ "space": "sp-new", "mod": "corgi", "name": "Corgi" })
            ),
            (
                "tab.close".into(),
                json!({ "id": "tb-shell", "confirmed": true })
            ),
            ("space.move".into(), json!({ "id": "sp-new", "index": 1 })),
        ]
    );
}

#[test]
fn an_existing_corgi_space_is_found_and_left_where_it_is() {
    let fake = Fake::new(
        "app-space-found",
        json!([
            space("s-repo", "corgi", None, json!([shell_tab("tb-r", "1")])),
            space("s-corgi", "Corgi", None, json!([mod_tab("tb-c")])),
        ]),
    )
    .open();
    assert_eq!(ensure(&fake.app).unwrap(), Ensured::Found("s-corgi".into()));
    assert_eq!(fake.calls(), vec!["ls"]);
}

#[test]
fn a_failed_mod_tab_leaves_no_space_behind() {
    let fake = Fake::new("app-space-tab-fails", json!([])).open();
    fake.file(
        "reply-space.new.json",
        r#"{"space": "sp-new", "tab": "tb-shell", "terminal": "t9"}"#,
    );
    fake.file("fail-tab.new", "");
    let error = ensure(&fake.app).unwrap_err();
    assert!(
        format!("{error:#}").contains("tab.new refused"),
        "{error:#}"
    );
    let calls = fake.calls();
    assert_eq!(
        calls.last().unwrap(),
        r#"do space.close {"confirmed":true,"id":"sp-new"}"#
    );
    assert!(!calls.iter().any(|call| call.starts_with("do tab.close")));
}

#[test]
fn remove_closes_only_corgi_tabs_without_terminals() {
    let fake = Fake::new(
        "app-space-remove",
        json!([
            space("s-repo", "corgi", None, json!([mod_tab("tb-other-mod")])),
            space(
                "s-corgi",
                "Corgi",
                None,
                json!([
                    mod_tab("tb-c"),
                    shell_tab("tb-mine", "Corgi"),
                    shell_tab("tb-2", "2")
                ])
            ),
            space("vps/s9", "Corgi", Some("vps"), json!([mod_tab("vps/tb9")])),
        ]),
    )
    .open();
    assert_eq!(remove(&fake.app).unwrap(), Removed::Closed(1));
    assert_eq!(
        fake.calls(),
        vec!["ls", r#"do tab.close {"confirmed":true,"id":"tb-c"}"#]
    );
}

#[test]
fn a_note_of_a_pending_space_is_deleted_and_nothing_is_made_from_it() {
    let fake = Fake::new("app-space-pending", json!([])).open();
    let note = fake.dir.join("state/app-space.json");
    fs::create_dir_all(note.parent().unwrap()).unwrap();
    // What an earlier Corgi wrote: the app to make the space in.
    fs::write(
        &note,
        json!({ "bin": fake.app.bin, "home": fake.app.home }).to_string(),
    )
    .unwrap();
    assert!(forget_pending_at(&note));
    assert!(!note.exists());
    // The window is open, and still nothing was asked of it.
    assert!(fake.calls().is_empty());
    // With no note there is nothing to forget.
    assert!(!forget_pending_at(&note));
}
