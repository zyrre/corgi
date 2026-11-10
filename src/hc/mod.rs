//! herdr-core (`hc`), the backend of the Herdr app: the client Corgi runs its
//! agents through when it is not on the Herdr TUI.
//!
//! The client shells out to `hc … --json` and reads the one JSON value it
//! prints, the simpler of the two ways `docs/herdr-app-port.md` weighs (the
//! other being hc's JSON-RPC socket, whose framed attach is not documented).
//! Every call starts one short-lived `hc` process, as [`crate::herdr`] opens
//! one socket connection per request.
//!
//! Corgi finds its agents among hc's terminals by their labels, which take
//! the place of the Herdr TUI's pane tokens (see the constants below and
//! [`agents`]). [`launch`] starts supervisors and workers there.

pub mod agents;
pub mod close;
pub mod daemon;
pub mod events;
pub mod launch;
mod observe;
pub mod pause;

use std::{
    collections::BTreeMap,
    env, fmt,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};
use serde::{Deserialize, de::DeserializeOwned};

use crate::{harness::find_executable, paths::home};

/// Chooses the `hc` binary, for a test's fake or a build of its own.
pub const HC_BIN_ENV: &str = "CORGI_HC_BIN";
/// The address hc gives every program in one of its terminals.
pub const TERMINAL_ID_ENV: &str = "HC_TERMINAL_ID";

/// The primary checkout of the project an agent works for.
pub const PROJECT_LABEL: &str = "corgi.project";
/// [`SUPERVISOR_ROLE`] or [`WORKER_ROLE`].
pub const ROLE_LABEL: &str = "corgi.role";
pub const SUPERVISOR_ROLE: &str = "supervisor";
pub const WORKER_ROLE: &str = "worker";
/// The `corgi spawn --request-id` a worker was started with; the hc
/// counterpart of [`crate::launch::CORGI_REQUEST_TOKEN`].
pub const REQUEST_LABEL: &str = "corgi.request";
/// What the dashboard's TASK column shows for the agent.
pub const TASK_LABEL: &str = "corgi.task";
/// The harness's kind: `claude`, `codex` or `opencode`.
pub const HARNESS_LABEL: &str = "corgi.harness";
/// The harness's own session id, once known.
pub const SESSION_LABEL: &str = "corgi.session";
/// The command line that resumes the session, one shell-quoted string to type
/// into a POSIX-quoting shell (see [`launch::resume_line`]).
pub const RESUME_LABEL: &str = "corgi.resume";
/// [`MERGE_TAG`] while a supervisor has tagged the agent ready to merge;
/// absent otherwise. The hc counterpart of [`crate::supervisor::MERGE_TOKEN`].
pub const TAG_LABEL: &str = "corgi.tag";
pub const MERGE_TAG: &str = "merge";
/// Where the agent rested when it was tagged, as
/// [`crate::supervisor::merge_tag_value`] writes it (`done 7`, the state and
/// hc's turn), so a tag it has since worked past no longer shows.
pub const TAG_AT_LABEL: &str = "corgi.tag.at";
/// The Herdr app's space that shows the terminal; one it does not know
/// becomes a new space.
pub const SPACE_LABEL: &str = "herdr.space";
/// A supervisor session's handover request, `<unix seconds> <session>`: the
/// hc counterpart of [`crate::supervisor::HANDOVER_TOKEN`].
pub const HANDOVER_LABEL: &str = "corgi.handover";
/// A supervisor session's context baseline, `<tokens> <session>`: the hc
/// counterpart of [`crate::supervisor::BASELINE_TOKEN`].
pub const BASELINE_LABEL: &str = "corgi.baseline";
/// The process id of the program Corgi started its agent from: the
/// supervisor's shell, a worker's agent. A kept terminal whose program has
/// another pid lost it (a core stop, a reboot) and got a shell in its place,
/// which the daemon gives the [`RESUME_LABEL`] line, once.
pub const PID_LABEL: &str = "corgi.pid";

/// An error `hc` reported: `{error: {kind, message, hint}}` on stderr, with
/// its exit code (3 not found, 9 blocked, 10 status mismatch, 11 finished).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HcError {
    pub kind: String,
    pub message: String,
    pub hint: String,
    pub code: Option<i32>,
}

impl HcError {
    /// Whether `error` is hc's answer that no terminal has that address or
    /// name.
    pub fn is_not_found(error: &anyhow::Error) -> bool {
        error
            .downcast_ref::<HcError>()
            .is_some_and(|error| error.kind == "not_found")
    }
}

impl fmt::Display for HcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "hc: {}", self.message)?;
        if !self.hint.is_empty() {
            write!(formatter, " ({})", self.hint)?;
        }
        Ok(())
    }
}

impl std::error::Error for HcError {}

/// One hc terminal, as `hc get` describes it. `hc ls` and `hc run` print
/// only some of these fields; the others keep their defaults then.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Terminal {
    /// `t7`: the terminal for as long as the core keeps it.
    pub address: String,
    #[serde(default)]
    pub name: Option<String>,
    /// `live` or `finished`.
    #[serde(default)]
    pub state: String,
    /// The program's newest OSC 7501 report; `None` while it never sent one.
    /// `hc ls` gives only its word.
    #[serde(default, deserialize_with = "status_record_or_word")]
    pub status: Option<Status>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// The folder it was started in.
    #[serde(default)]
    pub cwd: String,
    /// What runs in it now; `None` once it has finished, and in `ls`.
    #[serde(default)]
    pub foreground: Option<Foreground>,
    /// Goes up whenever its status leaves `working`.
    #[serde(default)]
    pub turn: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub created_ms: u64,
    /// The process id of its program, while it runs.
    #[serde(default)]
    pub pid: Option<u64>,
    /// Whether a lost program (a core stop, a reboot, death by a signal) is
    /// replaced with a shell in place; an exit with a code and a kill stay
    /// final.
    #[serde(default)]
    pub keep: bool,
    /// Its size in cells.
    #[serde(default)]
    pub cols: u16,
    #[serde(default)]
    pub rows: u16,
}

impl Terminal {
    pub fn is_live(&self) -> bool {
        self.state == "live"
    }

    pub fn label(&self, key: &str) -> Option<&str> {
        self.labels
            .get(key)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    /// The program's status word (`idle`, `working`, `done`, `blocked` or
    /// `error`), if it ever reported one.
    pub fn status_word(&self) -> Option<&str> {
        self.status.as_ref().map(|status| status.state.as_str())
    }

    /// The name it is shown and addressed by: its name, else its address.
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.address)
    }
}

/// One OSC 7501 record. The words are the program's own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Status {
    pub state: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub msg: String,
    #[serde(default)]
    pub app: String,
}

/// A terminal's status as `hc get` gives it (a record) or `hc ls` does (its
/// word alone).
fn status_record_or_word<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Status>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Given {
        Record(Status),
        Word(String),
    }
    Ok(match Option::<Given>::deserialize(deserializer)? {
        Some(Given::Record(status)) => Some(status),
        Some(Given::Word(state)) => Some(Status {
            state,
            ..Status::default()
        }),
        None => None,
    })
}

/// The foreground process group of a terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Foreground {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub pid: i64,
}

/// What `hc send` answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Sent {
    /// Whether the program went `working` within a few seconds; `None` when
    /// it reports no status.
    #[serde(default)]
    pub started: Option<bool>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub turn: u64,
}

#[derive(Debug, Deserialize)]
struct Listed {
    #[serde(default)]
    terminals: Vec<Terminal>,
}

#[derive(Debug, Deserialize)]
struct Read {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct ErrorReply {
    error: ErrorBody,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    hint: String,
}

/// Whether an `hc` call says which hc terminal it is made from (hc gives
/// every program in one [`TERMINAL_ID_ENV`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Caller {
    Known,
    Unnamed,
}

/// A terminal for [`HcClient::run`] to start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunSpec {
    pub name: String,
    pub cwd: PathBuf,
    pub labels: Vec<(String, String)>,
    /// Replace a lost program with a shell in place (`--keep`).
    pub keep: bool,
    /// The program and its arguments; empty starts the machine's own shell.
    pub argv: Vec<String>,
}

/// A client for one `hc` binary. Calls do not share state, so a clone is as
/// good as the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HcClient {
    bin: PathBuf,
}

impl HcClient {
    pub fn new(bin: impl Into<PathBuf>) -> Self {
        Self { bin: bin.into() }
    }

    /// The `hc` binary to use: [`HC_BIN_ENV`], else `hc` on `PATH` or in the
    /// usual per-user directories (`~/.local/bin` is where hc installs
    /// itself), if there is one.
    pub fn locate() -> Option<Self> {
        if let Some(bin) = env::var_os(HC_BIN_ENV).filter(|bin| !bin.is_empty()) {
            return Some(Self::new(bin));
        }
        find_executable("hc").map(Self::new)
    }

    /// [`HcClient::locate`], or why there is none.
    pub fn from_env() -> Result<Self> {
        Self::locate().context("hc (herdr-core) is not installed: no hc on PATH or in ~/.local/bin")
    }

    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// Runs `hc args… --json` and returns what it printed on stdout. A
    /// failure is the [`HcError`] hc reported, when it reported one.
    fn output(&self, args: &[&str]) -> Result<Vec<u8>> {
        self.output_as(args, Caller::Known)
    }

    /// [`HcClient::output`], with hc told or not which of its terminals
    /// this process runs in.
    fn output_as(&self, args: &[&str], caller: Caller) -> Result<Vec<u8>> {
        let mut command = Command::new(&self.bin);
        if caller == Caller::Unnamed {
            command.env_remove(TERMINAL_ID_ENV);
        }
        let output = command
            .args(args.iter().take_while(|arg| **arg != "--"))
            .arg("--json")
            .args(args.iter().skip_while(|arg| **arg != "--"))
            .output()
            .with_context(|| format!("run {}", self.bin.display()))?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reported = stderr
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str::<ErrorReply>(line.trim()).ok());
        match reported {
            Some(ErrorReply { error }) => Err(HcError {
                kind: error.kind,
                message: error.message,
                hint: error.hint,
                code: output.status.code(),
            }
            .into()),
            None => anyhow::bail!(
                "hc {} failed ({}): {}",
                args.first().copied().unwrap_or_default(),
                output.status,
                stderr.trim()
            ),
        }
    }

    fn json<T: DeserializeOwned>(&self, args: &[&str]) -> Result<T> {
        self.json_as(args, Caller::Known)
    }

    fn json_as<T: DeserializeOwned>(&self, args: &[&str], caller: Caller) -> Result<T> {
        let stdout = self.output_as(args, caller)?;
        serde_json::from_slice(&stdout).with_context(|| {
            format!(
                "hc {} printed unexpected JSON: {}",
                args.first().copied().unwrap_or_default(),
                String::from_utf8_lossy(&stdout).trim()
            )
        })
    }

    /// Starts `spec` in the background, with no parent (so it outlives the
    /// caller's terminal) and no notes typed into anyone, and returns it as
    /// `hc run` describes it.
    pub fn run(&self, spec: &RunSpec) -> Result<Terminal> {
        let cwd = spec.cwd.to_string_lossy();
        let mut args = vec![
            "run",
            "--bg",
            "--no-parent",
            "--no-notify",
            "--name",
            &spec.name,
            "--cwd",
            &cwd,
        ];
        if spec.keep {
            args.push("--keep");
        }
        let labels: Vec<String> = spec
            .labels
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        for label in &labels {
            args.extend(["--label", label]);
        }
        if !spec.argv.is_empty() {
            // Everything after `--` is the program's, `--json` included.
            args.push("--");
            args.extend(spec.argv.iter().map(String::as_str));
        }
        self.json(&args)
    }

    /// The terminal `target` (an address or a name).
    pub fn get(&self, target: &str) -> Result<Terminal> {
        self.json(&["get", target])
    }

    /// [`HcClient::get`], or `None` when hc has no terminal `target`.
    pub fn find(&self, target: &str) -> Result<Option<Terminal>> {
        match self.get(target) {
            Ok(terminal) => Ok(Some(terminal)),
            Err(error) if HcError::is_not_found(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The live terminals carrying every label of `labels`, each as `hc get`
    /// describes it (`hc ls` leaves labels and status out). One that ends
    /// between the two calls is left out.
    pub fn list(&self, labels: &[(&str, &str)]) -> Result<Vec<Terminal>> {
        let filters: Vec<String> = labels
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        let mut args = vec!["ls"];
        for filter in &filters {
            args.extend(["--label", filter]);
        }
        let listed: Listed = self.json(&args)?;
        let mut terminals = Vec::new();
        for listed in listed.terminals {
            // Another machine's terminals come as `machine/t7`; Corgi starts
            // its agents on this one.
            if listed.address.contains('/') {
                continue;
            }
            if let Some(terminal) = self.find(&listed.address)?
                && terminal.is_live()
            {
                terminals.push(terminal);
            }
        }
        Ok(terminals)
    }

    /// The screen of `target`, with `lines` lines of history above it.
    pub fn read(&self, target: &str, lines: u32) -> Result<String> {
        let lines = lines.to_string();
        let read: Read = self.json(&["read", target, "--lines", &lines])?;
        Ok(read.text)
    }

    /// Pastes `text` into `target` and presses Enter, once nobody has typed
    /// there for a few seconds. Never forced: hc refuses a blocked program.
    /// The text arrives as it is: hc wraps what one of its agents' terminals
    /// sends to a program that has reported a status (`<hc from="t7 name">…
    /// </hc>`), which a shell cannot run and a harness would not take for a
    /// command of its own, so the send never says which terminal it is from.
    pub fn send(&self, target: &str, text: &str) -> Result<Sent> {
        // After `--`, a text that starts with `-` is still the text.
        self.json_as(&["send", target, "--", text], Caller::Unnamed)
    }

    /// Presses the named `keys` (`down`, `enter`, `esc`) in `target`.
    pub fn keys(&self, target: &str, keys: &[&str]) -> Result<()> {
        let mut args = vec!["keys", target];
        args.extend(keys);
        self.output(&args).map(drop)
    }

    /// Sets `labels` on `target`; an empty value removes the label.
    pub fn label(&self, target: &str, labels: &[(&str, &str)]) -> Result<Terminal> {
        let pairs: Vec<String> = labels
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        let mut args = vec!["label", target];
        args.extend(pairs.iter().map(String::as_str));
        self.json(&args)
    }

    /// Has the core replace a lost program of `target` (a core stop, a
    /// reboot) with a shell in place, as `hc run --keep` does from the start.
    pub fn keep(&self, target: &str) -> Result<()> {
        self.output(&["keep", target]).map(drop)
    }

    /// Ends the program of `target`.
    pub fn kill(&self, target: &str) -> Result<()> {
        self.output(&["kill", target]).map(drop)
    }

    /// Removes the finished terminal `target`.
    pub fn remove(&self, target: &str) -> Result<()> {
        self.output(&["rm", target]).map(drop)
    }

    /// The first paint a watch-only viewer of `target` gets (`hc attach
    /// --observe`, through a pseudo-terminal of `cols` by `rows`): its whole
    /// screen as ANSI, one row per line with its styling, which is what
    /// [`crate::draft::holds_draft`] reads. The viewer never types or
    /// resizes, and is ended once the paint has gone quiet.
    pub fn observe_paint(&self, target: &str, cols: u16, rows: u16) -> Result<String> {
        observe::paint(&self.bin, target, cols, rows)
    }
}

/// Whether this process runs in an hc terminal, as hc tells every program in
/// one with [`TERMINAL_ID_ENV`].
pub fn in_terminal() -> bool {
    calling_terminal().is_some()
}

/// The address of the hc terminal this process runs in, if it runs in one.
pub fn calling_terminal() -> Option<String> {
    env::var(TERMINAL_ID_ENV)
        .ok()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

/// Whether a herdr-core accepts connections on this machine, found without
/// asking `hc`: any `hc` call starts a stopped core, with the caller's
/// environment, which a command that only reads must not do. It looks where
/// hc puts the socket of the session `HC_SESSION` (default `default`):
/// `HC_SOCKET`, else in an hc terminal `HC_TERMINAL_SOCKET`, else under
/// `$XDG_RUNTIME_DIR/herdr-core/`, else under
/// `$XDG_STATE_HOME/herdr-core/<session>/` (`~/.local/state` by default). A
/// socket moved by hc's own config is not found.
pub fn core_running() -> bool {
    running_socket().is_some()
}

/// The socket of the core [`core_running`] finds, if one accepts
/// connections. Asking it is safe where calling `hc` is not: a connection to
/// a socket nobody listens on starts nothing.
pub fn running_socket() -> Option<PathBuf> {
    socket_candidates(&|name| env::var_os(name), home().as_deref())
        .into_iter()
        .find(|socket| UnixStream::connect(socket).is_ok())
}

/// Where the core of this environment's session listens, or would: the
/// first place [`core_running`] looks. It names the core in Corgi's own
/// records (the daemon's lock, what it saw of each agent), so it must not
/// depend on whether the core runs.
pub fn expected_socket() -> Option<PathBuf> {
    socket_candidates(&|name| env::var_os(name), home().as_deref())
        .into_iter()
        .next()
}

/// Where hc's socket may be, given the environment `var` and the home
/// directory `home`.
fn socket_candidates(
    var: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    let set = |name: &str| var(name).filter(|value| !value.is_empty());
    // hc's own: the socket it chose, then, in one of its terminals, the
    // socket of that terminal's core.
    if let Some(socket) = set("HC_SOCKET").or_else(|| set("HC_TERMINAL_SOCKET")) {
        return vec![PathBuf::from(socket)];
    }
    let session = set("HC_SESSION")
        .map(|session| session.to_string_lossy().into_owned())
        .unwrap_or_else(|| "default".to_string());
    let mut candidates = Vec::new();
    if let Some(runtime) = set("XDG_RUNTIME_DIR") {
        candidates.push(
            PathBuf::from(runtime)
                .join("herdr-core")
                .join(format!("{session}.sock")),
        );
    }
    let state = set("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".local/state")));
    if let Some(state) = state {
        candidates.push(
            state
                .join("herdr-core")
                .join(&session)
                .join(format!("{session}.sock")),
        );
    }
    candidates
}

#[cfg(test)]
mod tests;
