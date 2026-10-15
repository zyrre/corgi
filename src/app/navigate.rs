//! `corgi --bar-navigate`: how the Omarchy popup shows a Herdr pane. It
//! focuses the pane's workspace, tab and pane in Herdr, then brings the
//! terminal window attached to that Herdr session into view with Hyprland.

use std::{
    collections::HashSet,
    fmt, fs,
    io::{self, Write},
    path::Path,
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{
    herdr::{HerdrClient, PaneInfo, SessionSnapshot},
    supervisor::PLUGIN_ID,
};

/// What `corgi --bar-navigate` prints for wrong arguments.
const NAVIGATE_USAGE: &str = "\
Usage: corgi --bar-navigate SOCKET agent PANE
       corgi --bar-navigate SOCKET focus PANE
       corgi --bar-navigate SOCKET open";

/// The workspace `scripts/herdr-corgi-dashboard` keeps the dashboard in, and
/// the label Herdr gives the dashboard's pane. Temporary previews use other
/// workspace labels, so they are never mistaken for the dashboard.
const DASHBOARD_WORKSPACE_LABEL: &str = "Corgi dashboard";
const DASHBOARD_PANE_LABEL: &str = "Corgi";

/// The exit code for a pane that is no longer in the session, which tells
/// the popup to forget the pane rather than retry it.
const PANE_GONE_EXIT: i32 = 3;

/// What to bring into view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target<'a> {
    /// The agent in this pane.
    Agent(&'a str),
    /// This plugin pane, such as a dashboard the popup opened earlier.
    Pane(&'a str),
    /// The stable dashboard, opened when there is none.
    Dashboard,
}

impl<'a> Target<'a> {
    fn parse(args: &'a [String]) -> Option<Self> {
        match args {
            [action, pane] if action == "agent" => Some(Self::Agent(pane)),
            [action, pane] if action == "focus" => Some(Self::Pane(pane)),
            [action] if action == "open" => Some(Self::Dashboard),
            _ => None,
        }
    }
}

/// The pane asked for is no longer in the Herdr session.
#[derive(Debug)]
struct PaneGone;

impl fmt::Display for PaneGone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("That pane is no longer in the Herdr session")
    }
}

impl std::error::Error for PaneGone {}

/// Runs `--bar-navigate` with the arguments after it and returns its exit
/// code: 0, [`PANE_GONE_EXIT`], 1 for any other failure (its reason on
/// stderr) or 2 for wrong arguments. Once the pane is shown in Herdr, its
/// location is printed as `{"result":{"plugin_pane":{"pane":…}}}`, even if
/// the terminal window cannot be focused, so that the popup can reuse it.
pub fn bar_navigate(args: &[String]) -> i32 {
    let (Some(socket), Some(target)) = (args.first(), args.get(1..).and_then(Target::parse)) else {
        eprintln!("{NAVIGATE_USAGE}");
        return 2;
    };
    let client = HerdrClient::from_socket_path(socket);
    let mut desktop = System { socket };
    match navigate(&client, target, &mut desktop, &mut io::stdout().lock()) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error:#}");
            if error.is::<PaneGone>() {
                PANE_GONE_EXIT
            } else {
                1
            }
        }
    }
}

fn navigate(
    client: &HerdrClient,
    target: Target<'_>,
    desktop: &mut impl Desktop,
    output: &mut impl Write,
) -> Result<()> {
    let pane = match target {
        Target::Agent(pane_id) => {
            let pane = find_pane(client, pane_id)?;
            client.go_to_agent_pane(&pane)?;
            pane
        }
        Target::Pane(pane_id) => {
            let pane = find_pane(client, pane_id)?;
            client.go_to_plugin_pane(&pane)?;
            pane
        }
        Target::Dashboard => {
            let pane_id = match stable_dashboard(&client.snapshot()?) {
                Some(pane_id) => pane_id,
                None => client.open_plugin_tab(PLUGIN_ID, "dashboard")?.pane_id,
            };
            let pane = find_pane(client, &pane_id)?;
            client.go_to_plugin_pane(&pane)?;
            pane
        }
    };
    let location = json!({ "result": { "plugin_pane": { "pane": {
        "pane_id": pane.pane_id,
        "workspace_id": pane.workspace_id,
        "tab_id": pane.tab_id,
    } } } });
    writeln!(output, "{location}")?;
    output.flush()?;
    focus_terminal(desktop)
}

/// The pane `pane_id` as the session has it now.
fn find_pane(client: &HerdrClient, pane_id: &str) -> Result<PaneInfo> {
    client
        .snapshot()?
        .panes
        .into_iter()
        .find(|pane| pane.pane_id == pane_id)
        .ok_or_else(|| PaneGone.into())
}

/// The dashboard pane in the workspace Corgi keeps it in: a pane Herdr
/// labels `Corgi` that is not an agent.
fn stable_dashboard(snapshot: &SessionSnapshot) -> Option<String> {
    let workspaces: HashSet<&str> = snapshot
        .workspaces
        .iter()
        .filter(|workspace| workspace.label == DASHBOARD_WORKSPACE_LABEL)
        .map(|workspace| workspace.workspace_id.as_str())
        .collect();
    let agents: HashSet<&str> = snapshot
        .agents
        .iter()
        .map(|agent| agent.pane_id.as_str())
        .collect();
    snapshot
        .panes
        .iter()
        .find(|pane| {
            pane.label.as_deref() == Some(DASHBOARD_PANE_LABEL)
                && workspaces.contains(pane.workspace_id.as_str())
                && !agents.contains(pane.pane_id.as_str())
        })
        .map(|pane| pane.pane_id.clone())
}

/// The commands and processes outside Herdr's socket that navigation needs,
/// apart so the tests can stand in for them.
trait Desktop {
    /// Runs `program` with `args` and returns its stdout, or fails with its
    /// stderr.
    fn run(&mut self, program: &str, args: &[&str]) -> Result<String>;

    /// The processes from every local interactive Herdr client attached to
    /// `session` (`None` for the default one) up through their ancestors,
    /// among them the terminals those clients run in.
    fn attached_terminal_pids(&mut self, session: Option<&str>) -> HashSet<u32>;
}

/// This machine.
struct System<'a> {
    socket: &'a str,
}

impl Desktop for System<'_> {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<String> {
        let output = Command::new(program)
            .args(args)
            .env("HERDR_SOCKET_PATH", self.socket)
            .output()
            .with_context(|| format!("cannot run {program}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            match stderr.trim() {
                "" => bail!("{program} {} failed", args.join(" ")),
                stderr => bail!("{stderr}"),
            }
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn attached_terminal_pids(&mut self, session: Option<&str>) -> HashSet<u32> {
        let mut terminals = HashSet::new();
        let Ok(processes) = fs::read_dir("/proc") else {
            return terminals;
        };
        for process in processes.flatten() {
            let Some(mut pid) = process.file_name().to_str().and_then(|n| n.parse().ok()) else {
                continue;
            };
            let Ok(cmdline) = fs::read(process.path().join("cmdline")) else {
                continue;
            };
            let Ok(cmdline) = String::from_utf8(cmdline) else {
                continue;
            };
            let args: Vec<&str> = cmdline.trim_end_matches('\0').split('\0').collect();
            if client_session(&args) != Some(session) {
                continue;
            }
            // Walk from Herdr through its shell to the compositor's terminal.
            while pid > 1 && terminals.insert(pid) {
                match fs::read_to_string(format!("/proc/{pid}/stat")) {
                    Ok(stat) => match parent_pid(&stat) {
                        Some(parent) => pid = parent,
                        None => break,
                    },
                    Err(_) => break,
                }
            }
        }
        terminals
    }
}

/// The session a local interactive Herdr client with command line `args`
/// shows: `Some(None)` for the default session, `None` when `args` are not
/// such a client (a server, a CLI command, a remote client).
fn client_session<'a>(args: &[&'a str]) -> Option<Option<&'a str>> {
    let (program, tail) = args.split_first()?;
    if Path::new(program).file_name()? != "herdr" {
        return None;
    }
    if let ["session", "attach", name] = tail {
        return Some(Some(name));
    }
    let mut session = None;
    let mut rest = tail.iter();
    while let Some(arg) = rest.next() {
        if *arg == "--session" {
            session = Some(*rest.next()?);
        } else if let Some(name) = arg.strip_prefix("--session=") {
            session = Some(name);
        } else if *arg != "--handoff" {
            return None;
        }
    }
    Some(session)
}

/// The parent process ID in a `/proc/<pid>/stat` line, read after the
/// command name, which may itself hold spaces and parentheses.
fn parent_pid(stat: &str) -> Option<u32> {
    let (_, after_name) = stat.rsplit_once(')')?;
    after_name.split_whitespace().nth(1)?.parse().ok()
}

/// Focuses the most recently used terminal window attached to the Herdr
/// session that answers on the socket.
fn focus_terminal(desktop: &mut impl Desktop) -> Result<()> {
    let status: Value =
        serde_json::from_str(&desktop.run("herdr", &["status", "server", "--json"])?)
            .context("herdr status returned invalid JSON")?;
    let pids = desktop.attached_terminal_pids(status["session"].as_str());
    let windows: Vec<Value> = serde_json::from_str(&desktop.run("hyprctl", &["clients", "-j"])?)
        .context("hyprctl clients returned invalid JSON")?;
    let window = windows
        .iter()
        .filter(|window| {
            window["mapped"] == true
                && window["pid"]
                    .as_u64()
                    .and_then(|pid| u32::try_from(pid).ok())
                    .is_some_and(|pid| pids.contains(&pid))
        })
        .min_by_key(|window| window["focusHistoryID"].as_i64().unwrap_or(i64::MAX))
        .context("Herdr selected the pane, but no attached terminal window was found")?;
    let address = window["address"]
        .as_str()
        .filter(|address| {
            address
                .strip_prefix("0x")
                .is_some_and(|hex| hex.chars().all(|c| c.is_ascii_hexdigit()))
        })
        .context("Hyprland returned an invalid window address")?;
    // Omarchy's Lua Hyprland and the older dispatcher use different syntax.
    let lua = format!("hl.dsp.focus({{ window = \"address:{address}\" }})");
    if desktop.run("hyprctl", &["dispatch", &lua]).is_err() {
        desktop.run(
            "hyprctl",
            &["dispatch", "focuswindow", &format!("address:{address}")],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, path::PathBuf, thread};

    use super::*;
    use crate::test_support::{answer, fake_herdr};

    /// A fake Herdr that answers `replies` in order, one connection each,
    /// and checks each request's method and parameters.
    fn herdr_replying(
        label: &str,
        replies: Vec<(&'static str, Value, Value)>,
    ) -> (PathBuf, thread::JoinHandle<()>) {
        fake_herdr(label, move |listener| {
            for (method, params, reply) in replies {
                answer(&listener, |request| {
                    assert_eq!(request["method"], method);
                    for (key, value) in params.as_object().into_iter().flatten() {
                        assert_eq!(&request["params"][key], value, "{method} {key}");
                    }
                    reply
                });
            }
        })
    }

    fn snapshot(snapshot: Value) -> Value {
        json!({ "result": { "type": "session_snapshot", "snapshot": snapshot } })
    }

    fn result(kind: &str) -> Value {
        json!({ "result": { "type": kind } })
    }

    /// A desktop that answers each command with the next of `replies` and
    /// remembers what was run and which session was asked about.
    #[derive(Default)]
    struct FakeDesktop {
        replies: VecDeque<Result<String, String>>,
        attached: HashSet<u32>,
        runs: Vec<Vec<String>>,
        sessions: Vec<Option<String>>,
    }

    impl Desktop for FakeDesktop {
        fn run(&mut self, program: &str, args: &[&str]) -> Result<String> {
            let mut run = vec![program.to_string()];
            run.extend(args.iter().map(|arg| arg.to_string()));
            self.runs.push(run);
            self.replies
                .pop_front()
                .unwrap_or_else(|| Err("unexpected command".into()))
                .map_err(anyhow::Error::msg)
        }

        fn attached_terminal_pids(&mut self, session: Option<&str>) -> HashSet<u32> {
            self.sessions.push(session.map(str::to_string));
            self.attached.clone()
        }
    }

    #[test]
    fn agent_navigation_crosses_workspace_and_tab_before_pane() {
        let pane = json!({ "pane_id": "w9:p2", "workspace_id": "w9", "tab_id": "w9:t3" });
        let (socket, server) = herdr_replying(
            "navigate-agent",
            vec![
                (
                    "session.snapshot",
                    json!({}),
                    snapshot(json!({ "panes": [pane] })),
                ),
                (
                    "workspace.focus",
                    json!({ "workspace_id": "w9" }),
                    result("workspace_info"),
                ),
                (
                    "tab.focus",
                    json!({ "tab_id": "w9:t3" }),
                    result("tab_info"),
                ),
                (
                    "agent.focus",
                    json!({ "target": "w9:p2" }),
                    json!({ "result": { "type": "agent_info", "agent": pane } }),
                ),
            ],
        );
        let mut desktop = FakeDesktop::default();
        let mut output = Vec::new();
        let client = HerdrClient::from_socket_path(&socket);
        let error = navigate(&client, Target::Agent("w9:p2"), &mut desktop, &mut output)
            .expect_err("the fake desktop has no terminal");
        server.join().expect("fake Herdr");
        fs::remove_file(socket).ok();

        let printed: Value = serde_json::from_slice(&output).expect("location JSON");
        assert_eq!(printed["result"]["plugin_pane"]["pane"]["pane_id"], "w9:p2");
        assert!(!error.is::<PaneGone>());
        assert_eq!(desktop.runs[0], ["herdr", "status", "server", "--json"]);
    }

    #[test]
    fn stale_pane_does_not_change_focus() {
        let (socket, server) = herdr_replying(
            "navigate-stale",
            vec![(
                "session.snapshot",
                json!({}),
                snapshot(json!({ "panes": [] })),
            )],
        );
        let mut desktop = FakeDesktop::default();
        let mut output = Vec::new();
        let client = HerdrClient::from_socket_path(&socket);
        let error = navigate(&client, Target::Pane("w9:p2"), &mut desktop, &mut output)
            .expect_err("the pane is gone");
        server.join().expect("fake Herdr");
        fs::remove_file(socket).ok();

        assert!(error.is::<PaneGone>());
        assert!(output.is_empty());
        assert!(desktop.runs.is_empty());
    }

    #[test]
    fn only_local_interactive_clients_match() {
        for (args, expected) in [
            (&["herdr"][..], Some(None)),
            (&["/usr/bin/herdr", "--session", "dev"], Some(Some("dev"))),
            (&["herdr", "--session=dev", "--handoff"], Some(Some("dev"))),
            (&["herdr", "session", "attach", "dev"], Some(Some("dev"))),
            (&["herdr", "server"], None),
            (&["herdr", "--session", "dev", "api", "snapshot"], None),
            (&["herdr", "--remote", "host"], None),
            (&["herdr", "--no-session"], None),
            (&["herdr", "--session"], None),
            (&["/usr/bin/herdr-helper"], None),
            (&[], None),
        ] {
            assert_eq!(client_session(args), expected, "{args:?}");
        }
    }

    #[test]
    fn parent_pid_is_read_after_the_command_name() {
        assert_eq!(parent_pid("4242 (herdr) S 4100 4242 4100 0"), Some(4100));
        assert_eq!(parent_pid("4243 (a) b (c) R 77 1"), Some(77));
        assert_eq!(parent_pid("garbage"), None);
    }

    #[test]
    fn focus_selects_attached_session_and_falls_back_to_older_hyprland() {
        let windows = json!([
            { "pid": 11, "mapped": true, "address": "0xabc", "focusHistoryID": 2 },
            { "pid": 12, "mapped": true, "address": "0xdef", "focusHistoryID": 0 },
            { "pid": 11, "mapped": false, "address": "0x123", "focusHistoryID": 1 },
        ]);
        let mut desktop = FakeDesktop {
            replies: VecDeque::from([
                Ok(r#"{"session":"dev"}"#.to_string()),
                Ok(windows.to_string()),
                Err("invalid dispatcher".to_string()),
                Ok(String::new()),
            ]),
            attached: HashSet::from([11]),
            ..FakeDesktop::default()
        };
        focus_terminal(&mut desktop).expect("focus the terminal");

        assert_eq!(desktop.sessions, [Some("dev".to_string())]);
        assert_eq!(
            desktop.runs[2],
            [
                "hyprctl",
                "dispatch",
                r#"hl.dsp.focus({ window = "address:0xabc" })"#
            ]
        );
        assert_eq!(
            desktop.runs.last().expect("a dispatch"),
            &["hyprctl", "dispatch", "focuswindow", "address:0xabc"]
        );
    }

    #[test]
    fn focus_refuses_a_window_address_that_is_not_hex() {
        let mut desktop = FakeDesktop {
            replies: VecDeque::from([
                Ok(r#"{"session":null}"#.to_string()),
                Ok(json!([{ "pid": 11, "mapped": true, "address": "0xab; rm" }]).to_string()),
            ]),
            attached: HashSet::from([11]),
            ..FakeDesktop::default()
        };
        let error = focus_terminal(&mut desktop).expect_err("a bad address");

        assert_eq!(desktop.sessions, [None]);
        assert!(error.to_string().contains("invalid window address"));
        assert_eq!(desktop.runs.len(), 2);
    }

    #[test]
    fn open_reuses_stable_dashboard_and_ignores_previews() {
        let pane = json!({ "pane_id": "w9:p2", "workspace_id": "w9", "tab_id": "w9:t3", "label": "Corgi" });
        let preview = json!({ "pane_id": "w8:p1", "workspace_id": "w8", "tab_id": "w8:t1", "label": "Corgi" });
        let agent = json!({ "pane_id": "w9:p1", "workspace_id": "w9", "tab_id": "w9:t1", "label": "Corgi" });
        let session = snapshot(json!({
            "panes": [preview, agent, pane],
            "agents": [agent],
            "workspaces": [
                { "workspace_id": "w8", "label": "🧪 TEMP CORGI PREVIEW — test" },
                { "workspace_id": "w9", "label": "Corgi dashboard" },
            ],
        }));
        let (socket, server) = herdr_replying(
            "navigate-open",
            vec![
                ("session.snapshot", json!({}), session.clone()),
                ("session.snapshot", json!({}), session),
                (
                    "workspace.focus",
                    json!({ "workspace_id": "w9" }),
                    result("workspace_info"),
                ),
                (
                    "tab.focus",
                    json!({ "tab_id": "w9:t3" }),
                    result("tab_info"),
                ),
                (
                    "plugin.pane.focus",
                    json!({ "pane_id": "w9:p2" }),
                    result("plugin_pane_focused"),
                ),
            ],
        );
        let mut desktop = FakeDesktop::default();
        let mut output = Vec::new();
        let client = HerdrClient::from_socket_path(&socket);
        navigate(&client, Target::Dashboard, &mut desktop, &mut output).ok();
        server.join().expect("fake Herdr");
        fs::remove_file(socket).ok();

        let printed: Value = serde_json::from_slice(&output).expect("location JSON");
        assert_eq!(printed["result"]["plugin_pane"]["pane"]["pane_id"], "w9:p2");
        assert_eq!(desktop.runs.len(), 1, "then focuses the terminal");
    }

    #[test]
    fn open_starts_a_dashboard_tab_when_there_is_none() {
        let opened = json!({ "pane_id": "w1:p5", "workspace_id": "w1", "tab_id": "w1:t4" });
        let (socket, server) = herdr_replying(
            "navigate-open-new",
            vec![
                (
                    "session.snapshot",
                    json!({}),
                    snapshot(json!({ "panes": [] })),
                ),
                (
                    "plugin.pane.open",
                    json!({
                        "plugin_id": PLUGIN_ID,
                        "entrypoint": "dashboard",
                        "placement": "tab",
                        "focus": false,
                    }),
                    json!({ "result": {
                        "type": "plugin_pane_opened",
                        "plugin_pane": { "plugin_id": PLUGIN_ID, "entrypoint": "dashboard", "pane": opened },
                    } }),
                ),
                (
                    "session.snapshot",
                    json!({}),
                    snapshot(json!({ "panes": [opened] })),
                ),
                (
                    "workspace.focus",
                    json!({ "workspace_id": "w1" }),
                    result("workspace_info"),
                ),
                (
                    "tab.focus",
                    json!({ "tab_id": "w1:t4" }),
                    result("tab_info"),
                ),
                (
                    "plugin.pane.focus",
                    json!({ "pane_id": "w1:p5" }),
                    result("plugin_pane_focused"),
                ),
            ],
        );
        let mut desktop = FakeDesktop::default();
        let mut output = Vec::new();
        let client = HerdrClient::from_socket_path(&socket);
        navigate(&client, Target::Dashboard, &mut desktop, &mut output).ok();
        server.join().expect("fake Herdr");
        fs::remove_file(socket).ok();

        let printed: Value = serde_json::from_slice(&output).expect("location JSON");
        assert_eq!(printed["result"]["plugin_pane"]["pane"]["pane_id"], "w1:p5");
    }

    #[test]
    fn a_restored_dashboard_herdr_forgot_is_focused_as_a_plain_pane() {
        let pane = json!({ "pane_id": "w9:p2", "workspace_id": "w9", "tab_id": "w9:t3" });
        let (socket, server) = herdr_replying(
            "navigate-orphan",
            vec![
                (
                    "session.snapshot",
                    json!({}),
                    snapshot(json!({ "panes": [pane] })),
                ),
                ("workspace.focus", json!({}), result("workspace_info")),
                ("tab.focus", json!({}), result("tab_info")),
                (
                    "plugin.pane.focus",
                    json!({ "pane_id": "w9:p2" }),
                    json!({ "error": { "code": "plugin_pane_not_found", "message": "gone" } }),
                ),
                (
                    "pane.focus",
                    json!({ "pane_id": "w9:p2" }),
                    result("pane_info"),
                ),
            ],
        );
        let mut desktop = FakeDesktop::default();
        let mut output = Vec::new();
        let client = HerdrClient::from_socket_path(&socket);
        navigate(&client, Target::Pane("w9:p2"), &mut desktop, &mut output).ok();
        server.join().expect("fake Herdr");
        fs::remove_file(socket).ok();

        assert!(!output.is_empty(), "the pane was shown");
    }

    #[test]
    fn arguments_name_one_target() {
        let args = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(
            Target::parse(&args(&["agent", "w1:p1"])),
            Some(Target::Agent("w1:p1"))
        );
        assert_eq!(
            Target::parse(&args(&["focus", "w1:p1"])),
            Some(Target::Pane("w1:p1"))
        );
        assert_eq!(Target::parse(&args(&["open"])), Some(Target::Dashboard));
        assert_eq!(Target::parse(&args(&["open", "w1:p1"])), None);
        assert_eq!(Target::parse(&args(&["agent"])), None);
        assert_eq!(Target::parse(&args(&["close", "w1:p1"])), None);
    }
}
