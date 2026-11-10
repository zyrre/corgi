//! The Corgi space of the Herdr app (herdr-app): a space named "Corgi" on
//! this computer, first in the list of spaces, whose only tab shows the
//! `corgi` mod, an overview of every project. Nothing makes it any more but
//! a person running `corgi app-space`: the install and the daemon did until
//! the space was retired, when Corgi's place in the app became the Projects
//! list of its layout ([`crate::app_layout`]).
//!
//! It is the one space Corgi names. A space is recognised by its name,
//! [`SPACE_NAME`], on this computer: Herdr calls a space nobody named after
//! the repo or folder of its first terminal, and Corgi's repo is `corgi`, in
//! lower case, so the name alone tells them apart. The app has no command
//! that makes a space around a mod, so [`ensure`] makes one with a shell,
//! adds the mod's tab, named [`TAB_NAME`], and closes the shell's tab.
//!
//! Spaces are kept in herdr-core per machine, but only the app's window
//! changes them, so with the window closed there is nothing to make one in.
//! An earlier Corgi then left a note for `corgi daemon` to make the space
//! once the window opened; [`forget_pending`] deletes such a note, and
//! nothing reads one.

use std::{
    fs,
    io::Read,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::paths::{corgi_state_dir, home};

/// The Corgi space's name, which is how it is recognised.
pub const SPACE_NAME: &str = "Corgi";
/// The name of the tab that shows the mod.
pub const TAB_NAME: &str = "Corgi";
/// The mod the tab shows: `herdr-app/mods/corgi`.
pub const MOD: &str = "corgi";
/// How long one `herdr-app` command may take.
const APP_TIMEOUT: Duration = Duration::from_secs(20);

/// The Herdr app to ask: its binary and its home (`HERDR_APP_HOME`), which
/// also names the window it asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HerdrApp {
    pub bin: PathBuf,
    pub home: PathBuf,
}

/// What [`ensure`] found or did.
#[derive(Debug, PartialEq, Eq)]
pub enum Ensured {
    /// A Corgi space was there already: its id.
    Found(String),
    /// It made one: its id.
    Made(String),
    /// The app's window is not open, so nothing can be asked or changed.
    NoWindow,
}

/// What [`remove`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Removed {
    /// It closed this many Corgi tabs (a space closes with its last tab).
    Closed(usize),
    NoWindow,
}

/// One space as `herdr-app ls` lists it.
#[derive(Debug, Deserialize)]
struct Space {
    id: String,
    name: String,
    #[serde(default)]
    machine: Option<String>,
    #[serde(default)]
    tabs: Vec<Tab>,
}

#[derive(Debug, Deserialize)]
struct Tab {
    id: String,
    name: String,
    #[serde(default)]
    terminals: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct Listing {
    spaces: Vec<Space>,
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

    fn spaces(&self) -> Result<Option<Vec<Space>>> {
        let Some(out) = self.call(&["ls"])? else {
            return Ok(None);
        };
        let listing: Listing =
            serde_json::from_str(&out).context("read the spaces herdr-app ls printed")?;
        Ok(Some(listing.spaces))
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

    /// Like [`Self::command`], where a closed window is an error: the
    /// window went in the middle of [`ensure`] or [`remove`].
    fn must(&self, name: &str, args: Value) -> Result<Value> {
        self.command(name, args)?
            .with_context(|| format!("the Herdr app's window closed before {name}"))
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

/// Whether `space` is a Corgi space: [`SPACE_NAME`], on this computer.
fn is_corgi(space: &Space) -> bool {
    space.name == SPACE_NAME && space.machine.is_none()
}

/// Finds the Corgi space, or makes it: named [`SPACE_NAME`] on this
/// computer, first in the list, with the mod's tab as its only one. A space
/// that is there is left as it is, wherever it is in the list.
pub fn ensure(app: &HerdrApp) -> Result<Ensured> {
    let Some(spaces) = app.spaces()? else {
        return Ok(Ensured::NoWindow);
    };
    if let Some(space) = spaces.iter().find(|space| is_corgi(space)) {
        return Ok(Ensured::Found(space.id.clone()));
    }
    let folder = home().unwrap_or_else(|| PathBuf::from("/"));
    let Some(made) = app.command(
        "space.new",
        json!({ "name": SPACE_NAME, "folder": folder, "machine": "" }),
    )?
    else {
        return Ok(Ensured::NoWindow);
    };
    let space = field(&made, "space")?;
    let shell_tab = field(&made, "tab")?;
    let finish = || -> Result<()> {
        app.must(
            "tab.new",
            json!({ "space": space, "mod": MOD, "name": TAB_NAME }),
        )?;
        app.must("tab.close", json!({ "id": shell_tab, "confirmed": true }))?;
        app.must("space.move", json!({ "id": space, "index": 1 }))?;
        Ok(())
    };
    if let Err(error) = finish() {
        // A space with only a shell in it is not the Corgi space: leave none
        // rather than one that would be found next time.
        let _ = app.command("space.close", json!({ "id": space, "confirmed": true }));
        return Err(error.context("make the Corgi space's tab"));
    }
    Ok(Ensured::Made(space))
}

fn field(value: &Value, name: &str) -> Result<String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("herdr-app did not say the new {name}: {value}"))
}

/// Closes the Corgi tabs: those named [`TAB_NAME`] with no terminal in them,
/// in Corgi spaces. A space closes with its last tab; one where the person
/// opened terminals of their own keeps them.
pub fn remove(app: &HerdrApp) -> Result<Removed> {
    let Some(spaces) = app.spaces()? else {
        return Ok(Removed::NoWindow);
    };
    let mut closed = 0;
    for space in spaces.iter().filter(|space| is_corgi(space)) {
        for tab in &space.tabs {
            if tab.name == TAB_NAME && tab.terminals.is_empty() {
                app.must("tab.close", json!({ "id": tab.id, "confirmed": true }))?;
                closed += 1;
            }
        }
    }
    Ok(Removed::Closed(closed))
}

/// Where an earlier Corgi noted that a Corgi space was still to be made.
fn pending_path() -> Option<PathBuf> {
    corgi_state_dir().map(|dir| dir.join("app-space.json"))
}

/// Deletes the note an earlier Corgi left for its daemon to make the Corgi
/// space once the app's window opened: whether there was one. The space is
/// only ever made by a person now, so a note left behind must not be acted
/// on by a daemon of that earlier version started later either.
pub fn forget_pending() -> bool {
    pending_path().is_some_and(|path| forget_pending_at(&path))
}

fn forget_pending_at(path: &Path) -> bool {
    fs::remove_file(path).is_ok()
}

#[cfg(test)]
mod tests;
