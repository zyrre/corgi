//! The Herdr app (herdr-app) that `corgi app-layout` asks: its binary and
//! its home, which names its window, and the commands it runs there.

use std::{
    io::Read,
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::paths::home;

/// How long one `herdr-app` command may take.
const APP_TIMEOUT: Duration = Duration::from_secs(20);

/// The Herdr app to ask: its binary and its home (`HERDR_APP_HOME`), which
/// also names the window it asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HerdrApp {
    pub bin: PathBuf,
    pub home: PathBuf,
}

impl HerdrApp {
    pub fn new(bin: impl Into<PathBuf>, home: impl Into<PathBuf>) -> Self {
        Self {
            bin: bin.into(),
            home: home.into(),
        }
    }

    /// The app's home when none is given: `$HERDR_APP_HOME`, else
    /// `~/.config/herdr-app`.
    pub fn default_home() -> Option<PathBuf> {
        std::env::var_os("HERDR_APP_HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .or_else(|| home().map(|home| home.join(".config/herdr-app")))
    }

    /// Whether the app's window is open: its socket takes a connection.
    pub fn window_open(&self) -> bool {
        UnixStream::connect(self.home.join("herdr-app.sock")).is_ok()
    }

    /// Runs `herdr-app` with `args` and returns what it printed, or `None`
    /// when no window is open to answer.
    fn call(&self, args: &[&str]) -> Result<Option<String>> {
        if !self.window_open() {
            return Ok(None);
        }
        let (code, stdout, stderr) = run(
            Command::new(&self.bin)
                .args(args)
                .env("HERDR_APP_HOME", &self.home),
            APP_TIMEOUT,
        )
        .with_context(|| format!("run {} {}", self.bin.display(), args.join(" ")))?;
        if code == Some(0) {
            return Ok(Some(stdout));
        }
        if stderr.contains("no Herdr window") {
            return Ok(None);
        }
        let said = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        bail!(
            "herdr-app {} failed ({}): {}",
            args.first().copied().unwrap_or_default(),
            code.map_or("timed out".to_string(), |code| format!("exit {code}")),
            said.trim()
        )
    }

    /// The layout the window shows (`herdr-app status`), or `None` when no
    /// window is open.
    pub fn shown_layout(&self) -> Result<Option<String>> {
        let Some(out) = self.call(&["status"])? else {
            return Ok(None);
        };
        let status: Value =
            serde_json::from_str(&out).context("read what herdr-app status printed")?;
        Ok(status
            .get("layout")
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    /// Runs app command `name` with `args`; `None` when no window is open.
    pub(crate) fn command(&self, name: &str, args: Value) -> Result<Option<Value>> {
        let Some(out) = self.call(&["do", name, &args.to_string()])? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(&out).unwrap_or(Value::Null)))
    }
}

/// Runs `command` with no input, gathering its output, for at most
/// `timeout`: its exit code (`None` when killed), stdout and stderr.
fn run(command: &mut Command, timeout: Duration) -> Result<(Option<i32>, String, String)> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let gather = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            text
        })
    };
    let stdout = gather(Box::new(child.stdout.take().expect("piped stdout")));
    let stderr = gather(Box::new(child.stderr.take().expect("piped stderr")));
    let deadline = Instant::now() + timeout;
    let code = loop {
        match child.try_wait()? {
            Some(status) => break status.code(),
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    Ok((
        code,
        stdout.join().unwrap_or_default(),
        stderr.join().unwrap_or_default(),
    ))
}
