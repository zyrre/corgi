//! Helpers the test suites share: scratch directories that clean up after
//! themselves, a fake Herdr socket, a dashboard that never touches the
//! developer's own state, and the ways a rendered screen is read back as
//! text.

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

use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, text::Line};
use serde_json::Value;

use crate::{
    app::{App, AppInputs},
    herdr::HerdrClient,
};

/// A socket no Herdr listens on, for a client the test never lets connect.
pub(crate) const UNUSED_SOCKET: &str = "/tmp/corgi-unused-test.sock";

/// A directory of its own for one test, removed with everything in it when
/// the value is dropped. The process id, the clock and a counter go into its
/// name, so parallel tests and parallel test runs never share one.
pub(crate) struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    pub(crate) fn new(label: &str) -> Self {
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
pub(crate) fn fake_herdr<T: Send + 'static>(
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
pub(crate) fn answer(listener: &UnixListener, reply: impl FnOnce(&Value) -> Value) {
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
#[allow(
    dead_code,
    reason = "for the session.rs tests, which adopt it separately"
)]
pub(crate) fn write_jsonl(path: &Path, lines: &[&str]) {
    let mut file = File::create(path).expect("create JSONL file");
    for line in lines {
        writeln!(file, "{line}").expect("write JSONL record");
    }
}

/// A dashboard for tests: its Herdr client points at a socket nobody listens
/// on, its project memory starts empty and is never saved, and it shows no
/// usage cards, so neither the developer's remembered projects nor the
/// provider CLIs on their PATH change what a test sees.
pub(crate) fn test_app() -> App {
    App::with_inputs(
        HerdrClient::from_socket_path(UNUSED_SOCKET),
        false,
        AppInputs::default(),
    )
}

/// The text of one line, its spans joined.
pub(crate) fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// The text of each line.
pub(crate) fn lines_text(lines: &[Line<'_>]) -> Vec<String> {
    lines.iter().map(line_text).collect()
}

/// Every row of a rendered buffer as text.
pub(crate) fn buffer_rows(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (area.top()..area.bottom())
        .map(|y| {
            (area.left()..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

/// A rendered buffer as one string, row after row with nothing between them,
/// for tests that only ask whether some text is on screen.
pub(crate) fn buffer_text(buffer: &Buffer) -> String {
    buffer.content().iter().map(|cell| cell.symbol()).collect()
}

/// A terminal of `width` by `height` cells for rendering into.
pub(crate) fn test_terminal(width: u16, height: u16) -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(width, height)).expect("build test terminal")
}
