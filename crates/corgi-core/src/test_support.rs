//! Helpers the test suites share: scratch directories that clean up after
//! themselves and a fake Herdr socket. The frontends' tests use them too,
//! through the `test-support` feature.

use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    ops::Deref,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

use crate::progress::Progress;

/// A socket no Herdr listens on, for a client the test never lets connect.
pub const UNUSED_SOCKET: &str = "/tmp/corgi-unused-test.sock";

/// A directory of its own for one test, removed with everything in it when
/// the value is dropped. The process id, the clock and a counter go into its
/// name, so parallel tests and parallel test runs never share one.
pub struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    pub fn new(label: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "corgi-{label}-{}-{nanos}-{count}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create scratch dir");
        Self { path }
    }
}

/// A scratch directory is used as the path it is.
impl Deref for ScratchDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for ScratchDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).ok();
    }
}

/// A fake Herdr on a socket of its own under the temporary directory. `serve`
/// runs on a thread with the listener and answers Corgi's connections, each
/// with [`answer`]; what it returns comes back through the join handle.
pub fn fake_herdr<T: Send + 'static>(
    label: &str,
    serve: impl FnOnce(UnixListener) -> T + Send + 'static,
) -> (PathBuf, thread::JoinHandle<T>) {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos();
    let socket_path =
        std::env::temp_dir().join(format!("corgi-{label}-{}-{nanos}.sock", std::process::id()));
    let listener = UnixListener::bind(&socket_path).expect("bind fake Herdr socket");
    (socket_path, thread::spawn(move || serve(listener)))
}

/// Answers the next connection to a fake Herdr: reads its one request and
/// sends back `reply(&request)`, a `{"result": …}` or `{"error": …}` body, as
/// the newline-terminated response to that request's `id`.
pub fn answer(listener: &UnixListener, reply: impl FnOnce(&Value) -> Value) {
    let (stream, _) = listener.accept().expect("accept Corgi connection");
    let mut reader = BufReader::new(stream);
    let mut request = String::new();
    reader.read_line(&mut request).expect("read request");
    let request: Value = serde_json::from_str(&request).expect("parse request");
    let mut response = reply(&request);
    response["id"] = request["id"].clone();
    serde_json::to_writer(reader.get_mut(), &response).expect("write response");
    reader.get_mut().write_all(b"\n").expect("finish response");
}

/// A fake Herdr that answers any number of connections at once, as many
/// short-lived processes make them, from one session snapshot the test can
/// change. It records every request, answers `agent.prompt` as delivered,
/// reads with an empty screen, and the methods it does not know with an
/// empty result. It stops, and removes its socket, when dropped.
pub struct FakeHerdr {
    pub socket: PathBuf,
    snapshot: Arc<Mutex<Value>>,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}

impl FakeHerdr {
    pub fn start(label: &str, snapshot: Value) -> Self {
        let snapshot = Arc::new(Mutex::new(snapshot));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (socket, server) = {
            let (snapshot, requests, stop) = (snapshot.clone(), requests.clone(), stop.clone());
            fake_herdr(label, move |listener| {
                listener
                    .set_nonblocking(true)
                    .expect("non-blocking listener");
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let (snapshot, requests) = (snapshot.clone(), requests.clone());
                            thread::spawn(move || serve_one(stream, &snapshot, &requests));
                        }
                        Err(_) => thread::sleep(std::time::Duration::from_millis(2)),
                    }
                }
            })
        };
        Self {
            socket,
            snapshot,
            requests,
            stop,
            server: Some(server),
        }
    }

    /// Replaces the session snapshot Herdr gives from now on.
    pub fn set_snapshot(&self, snapshot: Value) {
        *self.snapshot.lock().expect("snapshot") = snapshot;
    }

    /// The requests of `method` made so far.
    pub fn requests(&self, method: &str) -> Vec<Value> {
        self.requests
            .lock()
            .expect("requests")
            .iter()
            .filter(|request| request["method"] == method)
            .cloned()
            .collect()
    }

    /// The texts typed into agents so far, as `target: text`.
    pub fn prompts(&self) -> Vec<String> {
        self.requests("agent.prompt")
            .iter()
            .map(|request| {
                let params = &request["params"];
                format!(
                    "{}: {}",
                    params["target"].as_str().unwrap_or_default(),
                    params["text"].as_str().unwrap_or_default()
                )
            })
            .collect()
    }
}

impl Drop for FakeHerdr {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
        let _ = fs::remove_file(&self.socket);
    }
}

/// Answers one connection to a [`FakeHerdr`].
fn serve_one(
    stream: std::os::unix::net::UnixStream,
    snapshot: &Mutex<Value>,
    requests: &Mutex<Vec<Value>>,
) {
    // A stream accepted from a non-blocking listener is non-blocking on
    // macOS, though not on Linux.
    let _ = stream.set_nonblocking(false);
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let Ok(request) = serde_json::from_str::<Value>(&line) else {
        return;
    };
    requests.lock().expect("requests").push(request.clone());
    let params = &request["params"];
    let result = match request["method"].as_str().unwrap_or_default() {
        "session.snapshot" => serde_json::json!({
            "type": "session_snapshot",
            "snapshot": snapshot.lock().expect("snapshot").clone(),
        }),
        "agent.prompt" => serde_json::json!({
            "type": "agent_prompted",
            "agent": {
                "pane_id": params["target"], "workspace_id": "w1", "tab_id": "w1:t1",
                "agent": "claude", "agent_status": "working",
            },
        }),
        "agent.read" | "pane.read" => serde_json::json!({
            "type": "pane_read",
            "read": {"pane_id": params["target"], "text": ""},
        }),
        "plugin.list" => serde_json::json!({"type": "plugin_list", "plugins": []}),
        _ => serde_json::json!({"type": "ok"}),
    };
    let mut response = serde_json::json!({"id": request["id"], "result": result});
    response["id"] = request["id"].clone();
    let stream = reader.get_mut();
    let _ = serde_json::to_writer(&mut *stream, &response);
    let _ = stream.write_all(b"\n");
}

/// Writes `lines` to `path` as a JSON Lines file, one record per line.
pub fn write_jsonl(path: &Path, lines: &[&str]) {
    let mut file = File::create(path).expect("create JSONL file");
    for line in lines {
        writeln!(file, "{line}").expect("write JSONL record");
    }
}

/// Keeps the status lines a launch step reports.
#[derive(Default)]
pub struct Recorded(pub Vec<String>);

impl Progress for Recorded {
    fn report(&mut self, message: String) {
        self.0.push(message);
    }
}
