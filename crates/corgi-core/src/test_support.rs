//! Helpers the test suites share: scratch directories that clean up after
//! themselves and a fake Herdr socket. The frontends' tests use them too,
//! through the `test-support` feature.

use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    ops::Deref,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
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
