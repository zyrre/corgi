//! The Corgi space of the Herdr app (herdr-app): a space named "Corgi" on
//! this computer, first in the Spaces list, whose only tab shows the `corgi`
//! mod, the overview the person starts from and jumps to agents from.
//!
//! It is the one space Corgi names. A space is recognised by its name,
//! [`SPACE_NAME`], on this computer: Herdr calls a space nobody named after
//! the repo or folder of its first terminal, and Corgi's repo is `corgi`, in
//! lower case, so the name alone tells them apart. The app has no command
//! that makes a space around a mod, so [`ensure`] makes one with a shell,
//! adds the mod's tab, named [`TAB_NAME`], and closes the shell's tab.
//!
//! Spaces are kept in herdr-core per machine, but only the app's window
//! changes them. When the window is not open, `corgi app-space` leaves a note
//! ([`pending_path`]) and `corgi daemon` makes the space once the window is
//! open ([`make_pending`]), then deletes the note: the space is made once, so
//! closing it is the person's choice and sticks.

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
use serde::{Deserialize, Serialize};
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// Where the note that a Corgi space is still to be made is kept: the app
/// to make it in, as JSON.
pub fn pending_path() -> Option<PathBuf> {
    corgi_state_dir().map(|dir| dir.join("app-space.json"))
}

/// Notes that the Corgi space is to be made in `app` once its window opens.
pub fn set_pending(app: &HerdrApp) -> Result<()> {
    let path = pending_path().context("no home folder for Corgi's state")?;
    write_pending(&path, app)
}

fn write_pending(path: &Path, app: &HerdrApp) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    fs::write(path, serde_json::to_string(app)?)
        .with_context(|| format!("write {}", path.display()))
}

/// Forgets a pending Corgi space.
pub fn clear_pending() {
    if let Some(path) = pending_path() {
        let _ = fs::remove_file(path);
    }
}

/// For `corgi daemon`'s passes: makes the pending Corgi space once the
/// app's window is open, and then forgets it. What it did or why it could
/// not, to log; nothing when nothing is pending or the window is closed.
pub fn make_pending() -> Option<String> {
    make_pending_at(&pending_path()?)
}

fn make_pending_at(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let Ok(app) = serde_json::from_str::<HerdrApp>(&text) else {
        let _ = fs::remove_file(path);
        return Some(format!(
            "forgot the pending Corgi space: {} is not what corgi app-space writes",
            path.display()
        ));
    };
    if !app.window_open() {
        return None;
    }
    match ensure(&app) {
        Ok(Ensured::NoWindow) => None,
        Ok(done) => {
            let _ = fs::remove_file(path);
            Some(match done {
                Ensured::Made(id) => format!("made the Corgi space {id} in the Herdr app"),
                _ => "the Herdr app has a Corgi space already".to_string(),
            })
        }
        Err(error) => Some(format!("could not make the Corgi space: {error:#}")),
    }
}

#[cfg(test)]
mod tests;
