//! The real `corgi open`, `corgi projects`, `corgi supervisor --terminal`,
//! `corgi app-layout` and `corgi close`, as the Herdr app's mods and installer run them:
//! against a fake `hc`, with a private home and state directory. Nothing
//! here reaches a live herdr-core, a real agent CLI or the developer's own
//! state.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

use corgi_core::test_support::{ScratchDir, git_repo, git_worktree};
use serde_json::{Value, json};

/// A fake `hc`, a private home and state, and a folder for projects. `hc CMD
/// TARGET …` prints `CMD.TARGET.N.out` on its `N`th call for that target,
/// else `CMD.TARGET.out`, else `CMD.out`, with the matching `.err` and
/// `.code`; `hc ls` goes by the role it filters on (`ls.supervisor`,
/// `ls.worker`). Each call is one line of `calls`, its arguments separated
/// by `|`, and one of `callers`: the command, and the hc terminal it was told
/// it is made from.
struct World {
    /// Under Cargo's target directory, as the other binary tests keep state.
    private: PathBuf,
    /// Outside any repository: a project inside this checkout would be
    /// taken for Corgi's own repository.
    projects: ScratchDir,
}

const FAKE_HC: &str = r#"#!/bin/sh
d=$(dirname "$0")
(IFS='|'; printf '%s\n' "$*") >> "$d/calls"
printf '%s %s\n' "$1" "${HC_TERMINAL_ID:-nobody}" >> "$d/callers"
key="$1.$2"
if [ "$1" = ls ]; then
  key=ls
  for a; do case $a in corgi.role=*) key="ls.${a#corgi.role=}" ;; esac; done
fi
n=$(( $(cat "$d/count.$key" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$d/count.$key"
for f in "$d/$key.$n" "$d/$key" "$d/$1"; do
  if [ -e "$f.out" ] || [ -e "$f.code" ]; then
    [ -e "$f.out" ] && cat "$f.out"
    [ -e "$f.err" ] && cat "$f.err" >&2
    exit "$(cat "$f.code" 2>/dev/null || echo 0)"
  fi
done
exit 0
"#;

impl World {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let private = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "open-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(private.join("hc")).unwrap();
        fs::create_dir_all(private.join("home")).unwrap();
        let script = private.join("hc/hc");
        fs::write(&script, FAKE_HC).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let world = Self {
            private,
            projects: ScratchDir::new(&format!("open-{label}")),
        };
        // No agents, unless a test says otherwise.
        world.answer("ls", r#"{"terminals":[]}"#);
        world.answer("send", r#"{"started":null,"status":null,"turn":0}"#);
        world.answer("read", r#"{"text":"jane@mac weather % "}"#);
        world
    }

    fn answer(&self, reply: &str, out: &str) {
        fs::write(self.private.join("hc").join(format!("{reply}.out")), out).unwrap();
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.private.join("hc/calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn state(&self) -> PathBuf {
        self.private.join("state/corgi")
    }

    /// A repository among the projects: its canonical path.
    fn repo(&self, name: &str) -> String {
        git_repo(&self.projects.join(name))
            .to_string_lossy()
            .into_owned()
    }

    /// The canonical folder the projects are in.
    fn parent(&self) -> PathBuf {
        fs::canonicalize(&*self.projects).unwrap()
    }

    fn corgi(&self, args: &[&str]) -> Output {
        self.corgi_in(None, args)
    }

    /// `corgi` run in the hc terminal `terminal`, as an agent there runs it.
    fn corgi_in(&self, terminal: Option<&str>, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corgi"));
        command
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.private.join("home"))
            .env("XDG_STATE_HOME", self.private.join("state"))
            .env("CORGI_HC_BIN", self.private.join("hc/hc"))
            .stdin(Stdio::null());
        if let Some(terminal) = terminal {
            command.env("HC_TERMINAL_ID", terminal);
        }
        command.output().expect("run corgi")
    }

    /// What a command that prints one JSON object printed.
    fn json(&self, args: &[&str]) -> (Value, Output) {
        let output = self.corgi(args);
        let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "corgi {args:?} printed no JSON ({error}): {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (value, output)
    }
}

impl Drop for World {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.private).ok();
    }
}

/// A terminal of the Herdr app with a shell at its prompt in `cwd`.
fn shell(address: &str, cwd: &str) -> Value {
    json!({
        "address": address, "name": null, "state": "live", "status": null,
        "labels": {"herdr.space": "sp1", "herdr.tab": "tb1"},
        "cwd": cwd, "pid": 4242, "keep": true, "turn": 0,
        "foreground": {"name": "zsh", "pid": 4242, "argv": ["/bin/zsh", "-l"], "cwd": cwd},
    })
}

/// `terminal` once an agent runs in it.
fn with_agent(terminal: &Value) -> Value {
    let mut agent = terminal.clone();
    agent["foreground"]["name"] = "claude".into();
    agent["status"] = json!({"state": "idle", "app": "claude-code"});
    agent
}

/// `terminal` as the supervisor of the project at `root`.
fn as_supervisor(terminal: &Value, root: &str) -> Value {
    let mut supervisor = terminal.clone();
    supervisor["labels"]["corgi.role"] = "supervisor".into();
    supervisor["labels"]["corgi.project"] = root.into();
    supervisor["labels"]["corgi.harness"] = "claude".into();
    supervisor
}

fn called<'a>(calls: &'a [String], start: &str) -> Vec<&'a String> {
    calls
        .iter()
        .filter(|call| call.starts_with(start))
        .collect()
}

#[test]
fn open_starts_the_supervisor_in_the_shell_that_waits_in_the_project() {
    let world = World::new("shell");
    let root = world.repo("weather");
    // t5 is a shell elsewhere, t6 runs a command, t7 waits in the project.
    world.answer("get.t5", &shell("t5", "/usr").to_string());
    let mut busy = shell("t6", &root);
    busy["foreground"]["name"] = "vim".into();
    world.answer("get.t6", &busy.to_string());
    let waiting = shell("t7", &root);
    world.answer("get.t7.1", &waiting.to_string());
    world.answer("get.t7.2", &waiting.to_string());
    world.answer("get.t7", &with_agent(&waiting).to_string());
    world.answer("label", &waiting.to_string());

    let (opened, output) = world.json(&[
        "open",
        &root,
        "--json",
        "--harness",
        "claude",
        "--terminal",
        "t5",
        "--terminal",
        "t6",
        "--terminal",
        "t7",
    ]);
    assert!(output.status.success(), "{opened}");
    assert_eq!(opened["result"], "started");
    assert_eq!(opened["place"], "shell");
    assert_eq!(opened["terminal"], "t7");
    assert_eq!(opened["project"], root.as_str());
    assert_eq!(opened["created"], false);

    let calls = world.calls();
    assert!(called(&calls, "run|").is_empty(), "{calls:?}");
    let labels = called(&calls, "label|");
    assert_eq!(labels.len(), 1, "{calls:?}");
    for part in [
        "label|t7|",
        &format!("corgi.project={root}|"),
        "corgi.role=supervisor|",
        "corgi.harness=claude|",
        "corgi.pid=4242|",
    ] {
        assert!(labels[0].contains(part), "{part} not in {}", labels[0]);
    }
    assert!(!labels[0].contains("herdr.space"), "{}", labels[0]);
    let sends = called(&calls, "send|");
    assert_eq!(sends.len(), 1, "{calls:?}");
    assert!(
        sends[0].starts_with("send|t7|--json|--|/bin/sh -c "),
        "{}",
        sends[0]
    );
    assert!(
        sends[0].contains(&format!("corgi-agent {root} ")),
        "{}",
        sends[0]
    );
    // The project is written down, and its supervisor's state is made.
    assert_eq!(
        fs::read_to_string(world.state().join("projects")).unwrap(),
        format!("{root}\n")
    );
    assert!(world.state().join("supervisors/weather/ROLE.md").is_file());

    // From then on that terminal is the project's supervisor: open names it
    // and starts nothing.
    let supervisor = as_supervisor(&with_agent(&waiting), &root);
    world.answer(
        "ls.supervisor",
        &json!({"terminals": [supervisor]}).to_string(),
    );
    world.answer("get.t7", &supervisor.to_string());
    let before = world.calls().len();
    let (opened, output) = world.json(&["open", &root, "--json", "--terminal", "t5"]);
    assert!(output.status.success(), "{opened}");
    assert_eq!(opened["result"], "running");
    assert_eq!(opened["terminal"], "t7");
    assert_eq!(opened["name"], "t7");
    let after = world.calls();
    assert!(
        after[before..]
            .iter()
            .all(|call| call.starts_with("ls|") || call.starts_with("get|")),
        "{:?}",
        &after[before..]
    );
}

#[test]
fn open_starts_a_new_terminal_when_no_shell_waits_and_makes_a_new_project() {
    let world = World::new("new");
    let fresh = world.parent().join("fresh");
    let fresh_text = fresh.to_string_lossy().into_owned();
    let started = json!({"address": "t9", "name": "fresh-supervisor", "state": "live", "turn": 0});
    world.answer("run", &started.to_string());
    let mut kept = shell("t9", &fresh_text);
    kept["labels"] = json!({});
    world.answer("get.fresh-supervisor", &json!({}).to_string());
    fs::write(world.private.join("hc/get.fresh-supervisor.code"), "3").unwrap();
    fs::write(
        world.private.join("hc/get.fresh-supervisor.err"),
        r#"{"error":{"kind":"not_found","message":"no terminal fresh-supervisor","hint":""}}"#,
    )
    .unwrap();
    world.answer("get.t9.1", &kept.to_string());
    world.answer("get.t9", &with_agent(&kept).to_string());
    world.answer("label", &kept.to_string());
    // A shell elsewhere is offered, and not taken.
    world.answer("get.t5", &shell("t5", "/usr").to_string());

    // Without --create a missing directory is refused, as JSON, and stays
    // missing.
    let (refused, output) = world.json(&["open", &fresh_text, "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(refused["result"], "failed");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("--create makes it a new project"),
        "{refused}"
    );
    assert!(!fresh.exists());
    assert!(world.calls().is_empty(), "{:?}", world.calls());

    let (opened, output) = world.json(&[
        "open",
        &fresh_text,
        "--create",
        "--json",
        "--harness",
        "claude",
        "--terminal",
        "t5",
    ]);
    assert!(
        output.status.success(),
        "{opened} {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(opened["result"], "started");
    assert_eq!(opened["place"], "new");
    assert_eq!(opened["terminal"], "t9");
    assert_eq!(opened["name"], "fresh-supervisor");
    assert_eq!(opened["created"], true);
    assert!(fresh.is_dir());
    // Recorded as Corgi's own, so its folder-trust question is answered.
    assert_eq!(
        fs::read_to_string(world.state().join("created-projects")).unwrap(),
        format!("{fresh_text}\n")
    );
    let calls = world.calls();
    let runs = called(&calls, "run|");
    assert_eq!(runs.len(), 1, "{calls:?}");
    for part in [
        "|--name|fresh-supervisor|",
        &format!("|--cwd|{fresh_text}|"),
        "|--keep|",
        "|--label|corgi.role=supervisor|",
        "|--label|herdr.space=corgi-fresh-",
    ] {
        assert!(runs[0].contains(part), "{part} not in {}", runs[0]);
    }
    assert!(called(&calls, "send|t9|").len() == 1, "{calls:?}");
    assert!(called(&calls, "label|t5|").is_empty(), "{calls:?}");
}

#[test]
fn supervisor_with_a_terminal_starts_there_or_nowhere() {
    let world = World::new("named");
    let root = world.repo("weather");
    let mut busy = shell("t6", &root);
    busy["foreground"]["name"] = "vim".into();
    world.answer("get.t6", &busy.to_string());
    let output = world.corgi(&["supervisor", &root, "--terminal", "t6"]);
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains("t6 runs vim, not a shell waiting at its prompt"),
        "{said}"
    );
    let calls = world.calls();
    assert!(
        calls
            .iter()
            .all(|call| call.starts_with("ls|") || call.starts_with("get|")),
        "{calls:?}"
    );

    // A shell that waits anywhere is taken when it is named: the agent goes
    // to the project itself.
    let waiting = shell("t5", "/usr");
    world.answer("get.t5.1", &waiting.to_string());
    world.answer("get.t5.2", &waiting.to_string());
    world.answer("get.t5", &with_agent(&waiting).to_string());
    world.answer("label", &waiting.to_string());
    let (started, output) = world.json(&["supervisor", &root, "--terminal", "t5"]);
    assert!(output.status.success(), "{started}");
    assert_eq!(started["terminal"], "t5");
    assert_eq!(started["supervisor"], true);
    assert_eq!(started["backend"], "hc");
    assert!(
        started["location"]
            .as_str()
            .unwrap()
            .contains("the shell that was open in"),
        "{started}"
    );

    // It is hc's alone, and never a worker's.
    let output = world.corgi(&[
        "supervisor",
        &root,
        "--terminal",
        "t5",
        "--backend",
        "herdr",
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("does not go with --backend herdr"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn projects_lists_what_is_open_and_completes_what_is_typed() {
    let world = World::new("list");
    let weather = world.repo("weather");
    world.repo("zebra");
    let parent = world.parent();

    let (all, output) = world.json(&["projects", "--json", "--open", &weather]);
    assert!(output.status.success());
    let names: Vec<&str> = all["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["weather", "zebra"]);
    assert_eq!(all["projects"][0]["badge"], "open in Herdr");
    assert_eq!(all["projects"][1]["badge"], "nearby");
    assert_eq!(all["parent"], parent.to_string_lossy().as_ref());
    // The core does not run here, and a list starts none.
    assert!(world.calls().is_empty(), "{:?}", world.calls());

    let (typed, _) = world.json(&["projects", "--json", "--open", &weather, "--query", "fresh"]);
    assert_eq!(
        typed["projects"],
        json!([{
            "dir": parent.join("fresh"), "name": "fresh", "badge": "new project",
            "new": true, "supervisor": null,
        }])
    );
    let query = format!("{}/ze", parent.display());
    let (completed, _) = world.json(&["projects", "--json", "--query", &query]);
    assert_eq!(
        completed["projects"][0]["dir"],
        parent.join("zebra").to_string_lossy().as_ref()
    );
    assert_eq!(completed["projects"][0]["badge"], "typed path");
}

#[test]
fn app_layout_builds_the_layout_in_the_home_it_is_given() {
    let world = World::new("layout");
    let home = world.private.join("app");
    fs::create_dir_all(home.join("layouts")).unwrap();
    fs::create_dir_all(home.join("mods/projects")).unwrap();
    fs::write(home.join("mods/projects/main.js"), "// the mod").unwrap();
    let main = json!({ "id": "root", "type": "row", "children": [
        { "id": "sidebar", "type": "column", "children": [
            { "id": "spaces", "type": "block", "content": { "mod": "spaces" } },
            { "id": "agents", "type": "block", "content": { "mod": "agents" } } ] },
        { "id": "main", "type": "block", "content": { "tab": "active" } } ] });
    fs::write(home.join("layouts/main.json"), main.to_string()).unwrap();
    let home_text = home.to_string_lossy().into_owned();

    let (made, output) = world.json(&["app-layout", "--home", &home_text, "--json"]);
    assert!(output.status.success());
    assert_eq!(made["result"], "made");
    assert_eq!(
        made["layout"],
        home.join("layouts/corgi.json").to_string_lossy().as_ref()
    );
    let layout: Value =
        serde_json::from_str(&fs::read_to_string(home.join("layouts/corgi.json")).unwrap())
            .unwrap();
    assert_eq!(
        layout["children"][0]["children"],
        json!([{ "id": "spaces", "type": "block", "content": { "mod": "projects" } }])
    );
    let (again, _) = world.json(&["app-layout", "--home", &home_text, "--json"]);
    assert_eq!(again["result"], "unchanged");

    // A main of another shape keeps the build, exits 0 and says why.
    fs::write(
        home.join("layouts/main.json"),
        r#"{"id":"root","children":[]}"#,
    )
    .unwrap();
    let (kept, output) = world.json(&["app-layout", "--home", &home_text, "--json"]);
    assert!(output.status.success());
    assert_eq!(kept["result"], "kept");
    assert!(
        kept["message"]
            .as_str()
            .unwrap()
            .contains("no node called sidebar"),
        "{kept}"
    );
    let output = world.corgi(&["app-layout", "--home", &home_text]);
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(
        said.starts_with("kept the Corgi layout as it was built last: "),
        "{said}"
    );

    let output = world.corgi(&["app-layout", "remove", "--home", &home_text]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "removed the Corgi layout\n"
    );
    assert!(!home.join("layouts/corgi.json").exists());
    assert!(home.join("layouts/main.json").exists());
}

#[test]
fn what_corgi_types_into_a_terminal_is_never_marked_as_an_agents_message() {
    // hc wraps what an agent's terminal sends to a program that has
    // reported (`<hc from="t9 worker">…</hc>`): a shell cannot run that.
    let world = World::new("caller");
    let root = world.repo("weather");
    let waiting = shell("t5", &root);
    world.answer("get.t5.1", &waiting.to_string());
    world.answer("get.t5.2", &waiting.to_string());
    world.answer("get.t5", &with_agent(&waiting).to_string());
    world.answer("label", &waiting.to_string());
    let output = world.corgi_in(Some("t9"), &["supervisor", &root, "--terminal", "t5"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let callers = fs::read_to_string(world.private.join("hc/callers")).unwrap();
    let callers: Vec<&str> = callers.lines().collect();
    assert!(callers.contains(&"send nobody"), "{callers:?}");
    // Every other call still says where it is made from.
    assert!(callers.contains(&"get t9"), "{callers:?}");
    assert!(
        callers
            .iter()
            .all(|caller| caller.ends_with(" t9") || *caller == "send nobody"),
        "{callers:?}"
    );
}

#[test]
fn app_space_makes_nothing_without_a_window_and_leaves_no_note_for_the_daemon() {
    // The Corgi space is made by hand only: no window, no space, and the
    // note an earlier corgi left for its daemon to make one from is deleted.
    let world = World::new("space");
    let home = world.private.join("app");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(world.state()).unwrap();
    let note = world.state().join("app-space.json");
    fs::write(
        &note,
        json!({ "bin": "/usr/bin/true", "home": home }).to_string(),
    )
    .unwrap();
    let home_text = home.to_string_lossy().into_owned();
    let output = world.corgi(&["app-space", "--home", &home_text, "--app", "/usr/bin/false"]);
    assert_eq!(output.status.code(), Some(1));
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains("the Herdr app's window is not open")
            && said.contains("run corgi app-space again"),
        "{said}"
    );
    assert!(!note.exists());
    let left: Vec<_> = fs::read_dir(world.state())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

/// `corgi close`, as the Projects list runs it after a merge: refused while
/// the worktree has changes, then closed with `--force`, its terminal ended
/// and removed by its address, its worktree removed and its branch kept.
#[test]
fn close_refuses_a_dirty_worktree_then_closes_the_worker_when_forced() {
    let world = World::new("close");
    let root = world.repo("weather");
    let worktree = git_worktree(
        Path::new(&root),
        &world.projects.join("worktree-fix"),
        "worktree/fix",
    );
    fs::write(worktree.join("draft.txt"), "unsaved").unwrap();
    let worker = json!({
        "address": "t5", "name": "w-fix", "state": "live",
        "status": {"state": "done", "app": "claude-code"},
        "labels": {"corgi.role": "worker", "corgi.project": root, "herdr.space": "sp-fix"},
        "cwd": worktree, "keep": true, "turn": 3,
        "foreground": {"name": "claude", "pid": 4242, "argv": [], "cwd": worktree},
    });
    world.answer("ls.worker", r#"{"terminals":[{"address":"t5"}]}"#);
    // Live while it is listed, twice, and looked at before the kill.
    for n in 1..=3 {
        world.answer(&format!("get.t5.{n}"), &worker.to_string());
    }
    let mut finished = worker.clone();
    finished["state"] = "finished".into();
    world.answer("get.t5", &finished.to_string());
    // A core that runs, as far as `corgi close` looks: a socket that takes
    // a connection. Short, for the socket path limit.
    let socket = PathBuf::from(format!("/tmp/corgi-close-{}.sock", std::process::id()));
    fs::remove_file(&socket).ok();
    let _core = UnixListener::bind(&socket).unwrap();
    let close = |extra: &[&str]| {
        let mut args = vec!["close", "w-fix", "--project", &root, "--json"];
        args.extend(extra);
        let output = Command::new(env!("CARGO_BIN_EXE_corgi"))
            .args(&args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", world.private.join("home"))
            .env("XDG_STATE_HOME", world.private.join("state"))
            .env("CORGI_HC_BIN", world.private.join("hc/hc"))
            .env("HC_SOCKET", &socket)
            .stdin(Stdio::null())
            .output()
            .expect("run corgi");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "corgi close printed no JSON ({error}): {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (value, output.status.code())
    };

    let (refused, code) = close(&[]);
    assert_eq!(code, Some(1));
    assert_eq!(refused["result"], "refused", "{refused}");
    assert_eq!(refused["reason"], "dirty");
    assert_eq!(refused["dirty"], json!(["?? draft.txt"]));
    assert!(called(&world.calls(), "kill").is_empty());
    assert!(worktree.exists());

    let (closed, code) = close(&["--force"]);
    fs::remove_file(&socket).ok();
    assert_eq!(code, Some(0), "{closed}");
    assert_eq!(closed["result"], "closed");
    assert_eq!(closed["removed"], true);
    assert_eq!(closed["branch"], "worktree/fix");
    let calls = world.calls();
    assert_eq!(called(&calls, "kill"), ["kill|t5|--json"]);
    assert_eq!(called(&calls, "rm"), ["rm|t5|--json"]);
    assert!(!worktree.exists());
    let branch = Command::new("git")
        .args(["-C", &root, "rev-parse", "--verify", "-q", "worktree/fix"])
        .output()
        .unwrap();
    assert!(branch.status.success(), "the branch stays");
}
