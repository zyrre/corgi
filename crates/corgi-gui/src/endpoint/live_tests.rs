//! The adapter against a real Herdr: a throwaway server with a private home
//! and config directory and a session of its own, whose panes run `/bin/sh`
//! with only the system's `PATH`, so no agent CLI can start. It is ignored by
//! default, since it starts a server; run it with
//!
//! ```text
//! cargo test -p corgi-gui -- --ignored live_herdr
//! ```
//!
//! `CORGI_GUI_TEST_HERDR` names the `herdr` to run, else `herdr` on `PATH`.

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Mutex,
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

use corgi_core::herdr::HerdrClient;
use serde_json::json;

use super::{
    Connection, Event, Geometry,
    protocol::{
        ClientKeyCode, ClientKeyKind, ClientPaneInputEvent, ClientSurfaceSize, PaneSurfaceFrame,
    },
    viewer::{PARK_LABEL, Status, Viewer},
};

/// A Herdr server of its own, stopped and deleted on drop.
struct Throwaway {
    herdr: String,
    root: PathBuf,
    config: PathBuf,
    server: Child,
}

impl Throwaway {
    fn start() -> Self {
        // Found on this process's `PATH`: the server's own is the system's.
        let herdr = std::env::var("CORGI_GUI_TEST_HERDR").unwrap_or_else(|_| {
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|dir| dir.join("herdr"))
                .find(|path| path.is_file())
                .expect("herdr on PATH, or CORGI_GUI_TEST_HERDR")
                .to_string_lossy()
                .into_owned()
        });
        let root = std::env::temp_dir().join(format!("corgi-gui-live-{}", std::process::id()));
        // Short: macOS limits a socket path to 104 bytes.
        let config = PathBuf::from(format!("/tmp/cgl{}", std::process::id()));
        for dir in [root.join("home"), root.join("state"), config.clone()] {
            std::fs::create_dir_all(&dir).unwrap();
        }
        let mut command = Command::new(&herdr);
        command.args(["--session", "live", "server"]);
        let server = Self::private(&mut command, &root, &config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("herdr runs; set CORGI_GUI_TEST_HERDR to its path");
        let throwaway = Self {
            herdr,
            root,
            config,
            server,
        };
        let socket = throwaway.json_socket();
        let started = Instant::now();
        while !socket.exists() {
            assert!(started.elapsed() < Duration::from_secs(10), "no socket");
            std::thread::sleep(Duration::from_millis(50));
        }
        throwaway
    }

    /// `command` with nothing of the user's environment.
    fn private<'a>(command: &'a mut Command, root: &Path, config: &Path) -> &'a mut Command {
        command
            .env_clear()
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", config)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_DATA_HOME", root.join("home/.local/share"))
            .env("XDG_CACHE_HOME", root.join("home/.cache"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("SHELL", "/bin/sh")
            .env("TERM", "xterm-256color")
            .env("LANG", "en_US.UTF-8")
    }

    fn json_socket(&self) -> PathBuf {
        self.config.join("herdr/sessions/live/herdr.sock")
    }

    fn herdr(&self) -> HerdrClient {
        HerdrClient::from_socket_path(self.json_socket())
            .with_marker_record(self.root.join("markers.json"))
    }
}

impl Drop for Throwaway {
    fn drop(&mut self) {
        let mut stop = Command::new(&self.herdr);
        stop.args(["--session", "live", "server", "stop"]);
        let _ = Self::private(&mut stop, &self.root, &self.config)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            if let Ok(Some(_)) = self.server.try_wait() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.server.kill();
        let _ = self.server.wait();
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(&self.config);
    }
}

/// The connection, its events, and the viewer the window would drive.
struct Driver {
    connection: Connection,
    events: Receiver<Event>,
    viewer: Viewer,
    surface: Option<std::sync::Arc<PaneSurfaceFrame>>,
}

impl Driver {
    fn open(socket: PathBuf) -> Self {
        let (sender, events) = mpsc::channel();
        let sender = Mutex::new(sender);
        let geometry = Geometry {
            size: ClientSurfaceSize { cols: 80, rows: 24 },
            cell_width_px: 8,
            cell_height_px: 17,
        };
        let connection = Connection::open(socket, geometry, true, move |event| {
            let _ = sender.lock().unwrap().send(event);
        });
        Self {
            connection,
            events,
            viewer: Viewer::new(true),
            surface: None,
        }
    }

    /// Handles events, sending what the viewer asks, until `done` holds.
    fn until(&mut self, what: &str, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(self) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {what}");
            match self.events.recv_timeout(left) {
                Ok(Event::Snapshot(snapshot)) => self.viewer.snapshot(&snapshot),
                Ok(Event::Response { id, result }) => {
                    self.viewer.answered(&id, &result);
                }
                Ok(Event::Surface) => {
                    if let Some((surface, _)) = self.connection.take_surface() {
                        self.surface = Some(surface);
                    }
                }
                Ok(Event::Ended(why)) => panic!("the connection ended: {why}"),
                Ok(_) => {}
                Err(_) => {}
            }
            for step in self.viewer.next() {
                let (method, params) = step.request();
                let id = self.connection.request(method, params);
                self.viewer.sent(id, step);
            }
        }
    }

    /// The text the latest surface shows.
    fn screen(&self) -> String {
        let Some(surface) = &self.surface else {
            return String::new();
        };
        let width = usize::from(surface.frame.width.max(1));
        surface
            .frame
            .cells
            .chunks(width)
            .map(|row| row.iter().map(|c| c.symbol.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[test]
#[ignore = "starts a throwaway Herdr server; run with --ignored"]
fn live_herdr() {
    let herdr = Throwaway::start();
    let client = herdr.herdr();
    let user = client.create_workspace(None, Some("my work")).unwrap();
    let agent = client.create_workspace(None, Some("agent")).unwrap();
    let park = client.create_workspace(None, Some(PARK_LABEL)).unwrap();
    client
        .run_in_pane(&agent.root_pane_id, "echo corgi-live-$((40+2))")
        .unwrap();

    let mut driver = Driver::open(super::client_socket(&herdr.json_socket()));
    driver.viewer.show(Some(agent.tab_id.clone()));
    let tab = agent.tab_id.clone();
    driver.until("the agent's terminal", |d| {
        d.viewer.status() == Status::Showing(tab.clone()) && d.screen().contains("corgi-live-42")
    });
    assert_eq!(
        driver.viewer.park_tab(),
        Some(park.tab_id.as_str()),
        "found by its label"
    );

    // Typing reaches the pane: text, then Enter as a key Herdr encodes.
    let enter = ClientPaneInputEvent::Key {
        code: ClientKeyCode::Enter,
        modifiers: 0,
        kind: ClientKeyKind::Press,
        repeat_count: 1,
        shifted_codepoint: None,
        generated_text: None,
        tracks_release: false,
        physical_key_id: None,
        windows_record: None,
    };
    driver.connection.input(
        &agent.root_pane_id,
        vec![
            ClientPaneInputEvent::TextCommit("echo typed-$((6*7))".into()),
            enter,
        ],
    );
    driver.until("the typed command's output", |d| {
        d.screen().contains("\ntyped-42")
    });

    // Leaving parks the connection, then deactivates it.
    driver.viewer.show(None);
    driver.until("the park", |d| d.viewer.at_rest());
    assert_eq!(driver.viewer.status(), Status::Parked);

    // Back, then someone's JSON focus call moves every client: gone.
    driver.viewer.show(Some(agent.tab_id.clone()));
    driver.until("the agent's terminal again", |d| {
        d.viewer.status() == Status::Showing(tab.clone())
    });
    client
        .request("tab.focus", json!({ "tab_id": user.tab_id }))
        .unwrap();
    driver.until("the loss", |d| {
        matches!(d.viewer.status(), Status::Gone { .. }) && d.viewer.at_rest()
    });
}
