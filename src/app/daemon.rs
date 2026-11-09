//! `corgi daemon`: runs [`corgi_core::hc::daemon`] in the foreground, or
//! installs it as a service of the user's session: a LaunchAgent on macOS,
//! a systemd user unit on Linux, restarted whenever it ends and started at
//! login, with the user's login-shell `PATH`, logging to a file in Corgi's
//! state directory.

use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};

use corgi_core::{
    hc::{
        self, HcClient,
        daemon::{self, Options},
    },
    paths::home,
    supervisor,
};

pub const DAEMON_USAGE: &str = "\
Usage: corgi daemon
       corgi daemon install [--label LABEL]
       corgi daemon uninstall [--label LABEL]
       corgi daemon status [--label LABEL] [--json]

Wakes supervisors on herdr-core (hc), the Herdr app's backend, whether the
app's window is open or not: when a worker stops or is blocked, the stop goes
into its supervisor's inbox with its report, and the inbox is typed into the
supervisor once it rests with its agent in front and nothing in its input box.
It also hands a supervisor whose context is full, or whose prompt cache is
about to go cold, over to a fresh session in the same terminal, and gives
each kept terminal whose program the core lost (a core stop, a reboot) its
resume command, once. It acts only on the terminals Corgi labelled on hc,
never on the Herdr TUI, whose plugin hooks do the same there. One daemon runs
per core; it waits for the core to start and starts none itself.

  (no command)  run in the foreground, logging to stderr
  install       run it as a service: a LaunchAgent on macOS, a systemd user
                unit on Linux, started at login and again whenever it ends.
                Run again to update: it replaces the service and restarts it.
  uninstall     stop the service and remove it
  status        whether the service is installed, and whether a daemon runs

The service runs this binary with the PATH of your login shell and the
HC_SESSION, HC_SOCKET, XDG_*, HERDR_APP_* and CORGI_* settings of the
environment install runs in, and logs to daemon/daemon.log in Corgi's state
directory. --label names the service (default io.github.zyrre.corgi.daemon),
for a test of its own.";

/// The service's name, unless `--label` gives another.
pub const SERVICE_LABEL: &str = "io.github.zyrre.corgi.daemon";

/// The variables the service takes from the environment `install` runs in,
/// when they are set: where the core and Corgi's state are, the Herdr app,
/// Corgi's own settings.
const KEPT_VARIABLES: [&str; 15] = [
    "HC_SESSION",
    "HC_SOCKET",
    "XDG_STATE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_RUNTIME_DIR",
    "HERDR_APP_HOME",
    "HERDR_APP_BIN",
    "CORGI_HC_BIN",
    "CORGI_BIN",
    "CORGI_CLAUDE_BIN",
    "CORGI_CODEX_BIN",
    "CORGI_DEBUG_HANDOVER_PERCENT",
    "CORGI_DEBUG_HANDOVER_IDLE_SECS",
    "LANG",
];

/// `corgi daemon`.
pub fn daemon_command(args: &[String]) -> Result<()> {
    let (command, rest) = match args.split_first() {
        Some((command, rest)) if !command.starts_with('-') => (command.as_str(), rest),
        _ => ("run", args),
    };
    let mut label = SERVICE_LABEL.to_string();
    let mut json = false;
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--label" if command != "run" => {
                label = rest.next().context("--label needs a name")?.clone();
                anyhow::ensure!(
                    !label.is_empty()
                        && label
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
                    "--label takes letters, digits, '.', '_' and '-'"
                );
            }
            "--json" if command == "status" => json = true,
            other => bail!("unknown option {other:?}\n\n{DAEMON_USAGE}"),
        }
    }
    match command {
        "run" => run(),
        "install" => install(&label),
        "uninstall" => uninstall(&label),
        "status" => status(&label, json),
        other => bail!("unknown daemon command {other:?}\n\n{DAEMON_USAGE}"),
    }
}

/// Runs the daemon in the foreground for the core of this environment.
fn run() -> Result<()> {
    let client = HcClient::from_env()?;
    let socket = hc::expected_socket().context("no HOME, so there is no hc socket to wait for")?;
    let corgi_bin = supervisor::corgi_bin(None)?;
    let name_spaces = daemon::names_spaces(
        env::var("HC_SESSION").ok().as_deref(),
        env::var("HC_SOCKET").ok().as_deref(),
    );
    daemon::run(Options {
        client,
        socket,
        corgi_bin,
        name_spaces,
    })
}

/// This binary, by an absolute path.
fn this_binary() -> Result<PathBuf> {
    let exe = env::current_exe().context("find the corgi binary")?;
    Ok(if exe.is_absolute() {
        exe
    } else {
        fs::canonicalize(&exe).with_context(|| format!("resolve {}", exe.display()))?
    })
}

/// What the service runs with: the variables of [`KEPT_VARIABLES`] that are
/// set here, and `PATH`: the login shell's, with the folders of `hc` and of
/// Corgi's own `~/.local/bin` in front when it lacks them.
fn service_environment() -> Vec<(String, String)> {
    let mut path = login_path().unwrap_or_else(|| env::var("PATH").unwrap_or_default());
    let mut front: Vec<PathBuf> = Vec::new();
    if let Some(hc) =
        HcClient::locate().and_then(|client| client.bin().parent().map(Path::to_path_buf))
    {
        front.push(hc);
    }
    if let Some(home) = home() {
        front.push(home.join(".local/bin"));
    }
    for dir in front.into_iter().rev() {
        let dir = dir.to_string_lossy().into_owned();
        if !path.split(':').any(|entry| entry == dir) {
            path = if path.is_empty() {
                dir
            } else {
                format!("{dir}:{path}")
            };
        }
    }
    let mut variables = vec![("PATH".to_string(), path)];
    for name in KEPT_VARIABLES {
        if let Some(value) = env::var_os(name).filter(|value| !value.is_empty()) {
            variables.push((name.to_string(), value.to_string_lossy().into_owned()));
        }
    }
    variables
}

/// The `PATH` the user's login shell sets, asked of it with a time limit (a
/// profile that waits for input must not hang the install).
fn login_path() -> Option<String> {
    let shell = env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| OsString::from("/bin/sh"));
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let output = Command::new(&shell)
            .args(["-l", "-c", "printf '%s' \"$PATH\""])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        let _ = send.send(output);
    });
    let output = receive.recv_timeout(Duration::from_secs(10)).ok()?.ok()?;
    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.lines().last().unwrap_or_default().trim().to_string();
    (output.status.success() && !path.is_empty()).then_some(path)
}

/// Where the service's definition goes.
fn service_file(label: &str) -> Result<PathBuf> {
    if cfg!(target_os = "macos") {
        Ok(home()
            .context("HOME is not set")?
            .join("Library/LaunchAgents")
            .join(format!("{label}.plist")))
    } else {
        let config = env::var_os("XDG_CONFIG_HOME")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .or_else(|| home().map(|home| home.join(".config")))
            .context("HOME is not set")?;
        Ok(config.join("systemd/user").join(format!("{label}.service")))
    }
}

/// `corgi daemon install`.
fn install(label: &str) -> Result<()> {
    let program = this_binary()?;
    let log = daemon::log_path().context("Corgi has no state directory for the daemon's log")?;
    if let Some(dir) = log.parent() {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let environment = service_environment();
    let file = service_file(label)?;
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    if cfg!(target_os = "macos") {
        fs::write(&file, launchd_plist(label, &program, &environment, &log))
            .with_context(|| format!("write {}", file.display()))?;
        let domain = launchd_domain();
        // An installed one is replaced, so it runs this binary and settings.
        let _ = Command::new("launchctl")
            .args(["bootout", &format!("{domain}/{label}")])
            .stderr(Stdio::null())
            .status();
        let status = Command::new("launchctl")
            .arg("bootstrap")
            .arg(&domain)
            .arg(&file)
            .status()
            .context("run launchctl bootstrap")?;
        anyhow::ensure!(
            status.success(),
            "launchctl bootstrap {domain} {} failed",
            file.display()
        );
    } else {
        fs::write(&file, systemd_unit(&program, &environment, &log))
            .with_context(|| format!("write {}", file.display()))?;
        let unit = format!("{label}.service");
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", &unit])?;
        systemctl(&["restart", &unit])?;
    }
    eprintln!(
        "corgi daemon: installed {} ({label}), running {} daemon; it logs to {}",
        file.display(),
        program.display(),
        log.display()
    );
    Ok(())
}

/// `corgi daemon uninstall`.
fn uninstall(label: &str) -> Result<()> {
    let file = service_file(label)?;
    if cfg!(target_os = "macos") {
        let _ = Command::new("launchctl")
            .args(["bootout", &format!("{}/{label}", launchd_domain())])
            .stderr(Stdio::null())
            .status();
    } else {
        let _ = systemctl(&["disable", "--now", &format!("{label}.service")]);
    }
    if file.exists() {
        fs::remove_file(&file).with_context(|| format!("remove {}", file.display()))?;
        if !cfg!(target_os = "macos") {
            let _ = systemctl(&["daemon-reload"]);
        }
        eprintln!("corgi daemon: removed {}", file.display());
    } else {
        eprintln!("corgi daemon: no service at {}", file.display());
    }
    Ok(())
}

/// `corgi daemon status`.
fn status(label: &str, json: bool) -> Result<()> {
    let file = service_file(label)?;
    let installed = file.exists();
    let loaded = if cfg!(target_os = "macos") {
        Command::new("launchctl")
            .args(["print", &format!("{}/{label}", launchd_domain())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    } else {
        Command::new("systemctl")
            .args([
                "--user",
                "is-active",
                "--quiet",
                &format!("{label}.service"),
            ])
            .status()
            .is_ok_and(|status| status.success())
    };
    let socket = hc::expected_socket();
    let running = socket.as_deref().map(daemon::running);
    let log = daemon::log_path();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "label": label,
                "service": file,
                "installed": installed,
                "loaded": loaded,
                "socket": socket,
                "running": running.as_ref().is_some_and(|running| running.running),
                "pid": running.as_ref().and_then(|running| running.pid),
                "log": log,
            })
        );
        return Ok(());
    }
    println!(
        "service: {} ({})",
        file.display(),
        match (installed, loaded) {
            (true, true) => "installed, loaded",
            (true, false) => "installed, not loaded",
            (false, true) => "not installed, but loaded",
            (false, false) => "not installed",
        }
    );
    match (&socket, &running) {
        (Some(socket), Some(running)) if running.running => println!(
            "daemon: running (pid {}) for the core at {}",
            running
                .pid
                .map_or_else(|| "unknown".to_string(), |pid| pid.to_string()),
            socket.display()
        ),
        (Some(socket), _) => println!("daemon: not running for the core at {}", socket.display()),
        (None, _) => println!("daemon: no core socket (HOME is not set)"),
    }
    if let Some(log) = log {
        println!("log: {}", log.display());
    }
    Ok(())
}

/// The user's launchd domain: `gui/<uid>`.
fn launchd_domain() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}

fn systemctl(args: &[&str]) -> Result<()> {
    let status = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
        .context("run systemctl --user")?;
    anyhow::ensure!(
        status.success(),
        "systemctl --user {} failed",
        args.join(" ")
    );
    Ok(())
}

/// `text` escaped for an XML text node.
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The LaunchAgent that runs `program daemon` as `label` with
/// `environment`, at load and again whenever it ends, logging to `log`.
pub(crate) fn launchd_plist(
    label: &str,
    program: &Path,
    environment: &[(String, String)],
    log: &Path,
) -> String {
    let variables: String = environment
        .iter()
        .map(|(name, value)| {
            format!(
                "    <key>{}</key>\n    <string>{}</string>\n",
                xml(name),
                xml(value)
            )
        })
        .collect();
    let log = xml(&log.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{program}</string>
    <string>daemon</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
{variables}  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ThrottleInterval</key>
  <integer>10</integer>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
        label = xml(label),
        program = xml(&program.to_string_lossy()),
    )
}

/// `value` quoted for a systemd unit's `Environment=` or `ExecStart=`.
fn systemd_quoted(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

/// The systemd user unit that runs `program daemon` with `environment`,
/// at login and again whenever it ends, logging to `log`.
pub(crate) fn systemd_unit(program: &Path, environment: &[(String, String)], log: &Path) -> String {
    let variables: String = environment
        .iter()
        .map(|(name, value)| {
            format!(
                "Environment={}\n",
                systemd_quoted(&format!("{name}={value}"))
            )
        })
        .collect();
    let log = log.to_string_lossy().replace('%', "%%");
    format!(
        "[Unit]\n\
         Description=Corgi daemon: wakes and handovers for supervisors on herdr-core\n\
         \n\
         [Service]\n\
         ExecStart={} daemon\n\
         {variables}\
         Restart=always\n\
         RestartSec=10\n\
         StandardOutput=append:{log}\n\
         StandardError=append:{log}\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        systemd_quoted(&program.to_string_lossy()),
    )
}

/// What `corgi status --json` says of the daemon of the core of this
/// environment: whether one runs, and its pid, and whether the service
/// `corgi daemon install` writes is installed.
pub(crate) fn status_value() -> serde_json::Value {
    let running = hc::expected_socket().map(|socket| daemon::running(&socket));
    let installed = service_file(SERVICE_LABEL).is_ok_and(|file| file.exists());
    serde_json::json!({
        "running": running.as_ref().is_some_and(|running| running.running),
        "pid": running.as_ref().and_then(|running| running.pid),
        "installed": installed,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn environment() -> Vec<(String, String)> {
        vec![
            ("PATH".into(), "/home/me/.local/bin:/usr/bin".into()),
            ("HC_SESSION".into(), "x & <y>".into()),
        ]
    }

    #[test]
    fn the_launch_agent_keeps_the_daemon_running_with_its_environment_and_log() {
        let plist = launchd_plist(
            "io.github.zyrre.corgi-daemon-test",
            Path::new("/opt/corgi/target/release/corgi"),
            &environment(),
            Path::new("/home/me/.local/state/corgi/daemon/daemon.log"),
        );
        for part in [
            "<key>Label</key>\n  <string>io.github.zyrre.corgi-daemon-test</string>",
            "<string>/opt/corgi/target/release/corgi</string>\n    <string>daemon</string>",
            "<key>PATH</key>\n    <string>/home/me/.local/bin:/usr/bin</string>",
            "<key>HC_SESSION</key>\n    <string>x &amp; &lt;y&gt;</string>",
            "<key>RunAtLoad</key>\n  <true/>",
            "<key>KeepAlive</key>\n  <true/>",
            "<key>StandardErrorPath</key>\n  <string>/home/me/.local/state/corgi/daemon/daemon.log</string>",
        ] {
            assert!(plist.contains(part), "{part}\nin\n{plist}");
        }
    }

    #[test]
    fn the_systemd_unit_restarts_the_daemon_and_quotes_what_it_is_given() {
        let unit = systemd_unit(
            Path::new("/opt/my corgi/corgi"),
            &environment(),
            Path::new("/home/me/.local/state/corgi/daemon/daemon.log"),
        );
        for part in [
            "ExecStart=\"/opt/my corgi/corgi\" daemon\n",
            "Environment=\"PATH=/home/me/.local/bin:/usr/bin\"\n",
            "Environment=\"HC_SESSION=x & <y>\"\n",
            "Restart=always\n",
            "StandardError=append:/home/me/.local/state/corgi/daemon/daemon.log\n",
            "WantedBy=default.target\n",
        ] {
            assert!(unit.contains(part), "{part}\nin\n{unit}");
        }
    }

    #[test]
    fn a_label_is_checked_and_options_belong_to_their_command() {
        let args = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert!(daemon_command(&args(&["install", "--label", "a b"])).is_err());
        assert!(daemon_command(&args(&["run", "--json"])).is_err());
        assert!(daemon_command(&args(&["stop"])).is_err());
    }
}
