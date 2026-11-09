//! Which terminals Corgi runs its agents in: the Herdr TUI's panes, through
//! its socket ([`crate::herdr`]), or herdr-core's terminals, the backend of
//! the Herdr app ([`crate::hc`]).
//!
//! The command-line tools pick one from where they run:
//!
//! - inside an hc terminal (`HC_TERMINAL_ID` set), hc, even when a Herdr
//!   pane's variables leaked into the terminal's environment;
//! - inside a Herdr TUI pane (`HERDR_SOCKET_PATH` set), the TUI, exactly as
//!   before hc;
//! - anywhere else, the commands that only read see the agents of both, and
//!   the ones that start agents use hc when no Herdr TUI is running.
//!
//! `--backend` chooses where `corgi supervisor` and `corgi spawn` start an
//! agent wherever they run.

use std::{env, os::unix::net::UnixStream, path::PathBuf};

use crate::{hc, herdr::HerdrClient, paths::home};

/// The terminals an agent runs in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Backend {
    /// A Herdr TUI pane.
    #[default]
    Herdr,
    /// A herdr-core terminal.
    Hc,
}

impl Backend {
    /// The name `--backend` takes.
    pub fn name(self) -> &'static str {
        match self {
            Self::Herdr => "herdr",
            Self::Hc => "hc",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Herdr, Self::Hc]
            .into_iter()
            .find(|backend| backend.name() == name.trim())
    }
}

/// Where the calling process runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    /// In an hc terminal.
    Hc,
    /// In a Herdr TUI pane.
    Herdr,
    /// In neither, such as a plain terminal or a launchd job.
    Outside,
}

impl Caller {
    /// Where this process runs, from its environment.
    pub fn from_env() -> Self {
        let set = |name: &str| env::var_os(name).is_some_and(|value| !value.is_empty());
        Self::of(set(hc::TERMINAL_ID_ENV), set("HERDR_SOCKET_PATH"))
    }

    /// Where a process runs that has `HC_TERMINAL_ID` set (`in_hc`) and
    /// `HERDR_SOCKET_PATH` set (`in_herdr`). hc wins: a core started from a
    /// Herdr pane hands that pane's variables to every terminal it starts.
    pub fn of(in_hc: bool, in_herdr: bool) -> Self {
        match (in_hc, in_herdr) {
            (true, _) => Self::Hc,
            (false, true) => Self::Herdr,
            (false, false) => Self::Outside,
        }
    }

    /// The backend that starts agents for a caller here: `requested` (by
    /// `--backend`), else the one it runs in, else hc unless a Herdr TUI is
    /// running (which `herdr_running` tells, asked only then).
    pub fn launch_backend(
        self,
        requested: Option<Backend>,
        herdr_running: impl FnOnce() -> bool,
    ) -> Backend {
        match (self, requested) {
            (_, Some(backend)) => backend,
            (Self::Hc, None) => Backend::Hc,
            (Self::Herdr, None) => Backend::Herdr,
            (Self::Outside, None) if herdr_running() => Backend::Herdr,
            (Self::Outside, None) => Backend::Hc,
        }
    }
}

/// The socket a Herdr TUI started without a session name listens on:
/// `herdr.sock` in its configuration directory (`$XDG_CONFIG_HOME/herdr`,
/// default `~/.config/herdr`).
pub fn herdr_default_socket() -> Option<PathBuf> {
    let config = env::var_os("XDG_CONFIG_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|home| home.join(".config")))?;
    Some(config.join("herdr").join("herdr.sock"))
}

/// A client for the Herdr TUI a process outside it can reach: the one its
/// environment names, else the default session's, when that accepts a
/// connection.
pub fn reachable_herdr() -> Option<HerdrClient> {
    if let Ok(client) = HerdrClient::from_env() {
        return Some(client);
    }
    let socket = herdr_default_socket()?;
    UnixStream::connect(&socket)
        .is_ok()
        .then(|| HerdrClient::from_socket_path(socket))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hc_wins_over_a_leaked_herdr_pane_and_outside_both_reads_neither() {
        assert_eq!(Caller::of(true, true), Caller::Hc);
        assert_eq!(Caller::of(true, false), Caller::Hc);
        assert_eq!(Caller::of(false, true), Caller::Herdr);
        assert_eq!(Caller::of(false, false), Caller::Outside);
    }

    #[test]
    fn agents_start_as_asked_else_where_the_caller_runs_else_on_hc_without_a_tui() {
        let never = || -> bool { panic!("asked whether Herdr runs") };
        assert_eq!(Caller::Hc.launch_backend(None, never), Backend::Hc);
        assert_eq!(Caller::Herdr.launch_backend(None, never), Backend::Herdr);
        assert_eq!(
            Caller::Herdr.launch_backend(Some(Backend::Hc), never),
            Backend::Hc
        );
        assert_eq!(
            Caller::Outside.launch_backend(Some(Backend::Hc), never),
            Backend::Hc
        );
        assert_eq!(
            Caller::Outside.launch_backend(Some(Backend::Herdr), never),
            Backend::Herdr
        );
        assert_eq!(
            Caller::Outside.launch_backend(None, || true),
            Backend::Herdr
        );
        assert_eq!(Caller::Outside.launch_backend(None, || false), Backend::Hc);
    }

    #[test]
    fn backend_names_round_trip() {
        for backend in [Backend::Herdr, Backend::Hc] {
            assert_eq!(Backend::from_name(backend.name()), Some(backend));
        }
        assert_eq!(Backend::from_name("tmux"), None);
    }
}
