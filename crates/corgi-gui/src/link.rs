//! The connection to `corgi serve`: a child process, local or `ssh`, whose
//! stdout lines are the server's messages and whose stdin takes requests.
//! Each connection has a number, so events from one that ended cannot be
//! taken for the next one's.

use std::{
    collections::VecDeque,
    io::{self, BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use corgi_core::protocol::{Request, ServerMessage};

/// Lines of the server's stderr kept to explain why it ended.
const STDERR_LINES: usize = 6;

/// What a connection's reader saw.
#[derive(Debug)]
pub enum LinkEvent {
    Message(Box<ServerMessage>),
    /// The server's stdout closed: it exited, or `ssh` lost the host. The
    /// text is what it said on stderr last, or how it exited.
    Ended(String),
}

/// Sends a connection's events, with its number, to the window.
pub type EventSender = Box<dyn Fn(u64, LinkEvent) + Send + Sync>;

/// One running `corgi serve`.
pub struct Link {
    stdin: Mutex<Option<ChildStdin>>,
}

impl Link {
    /// Starts `program args`, and a thread that reads its messages until it
    /// ends.
    pub fn start(
        number: u64,
        program: &str,
        args: &[String],
        events: EventSender,
    ) -> io::Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                io::Error::new(error.kind(), format!("could not start {program}: {error}"))
            })?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let stdin = child.stdin.take();
        let child = Arc::new(Mutex::new(child));
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let stderr_thread = {
            let tail = tail.clone();
            thread::Builder::new()
                .name("corgi-serve-stderr".into())
                .spawn(move || keep_tail(stderr, &tail))?
        };
        let reader_child = child;
        thread::Builder::new()
            .name("corgi-serve-reader".into())
            .spawn(move || {
                read_messages(stdout, |message| {
                    events(number, LinkEvent::Message(Box::new(message)))
                });
                let status = exit_status(&reader_child);
                let _ = stderr_thread.join();
                let said: Vec<String> = tail
                    .lock()
                    .map(|t| t.iter().cloned().collect())
                    .unwrap_or_default();
                let why = match (said.last(), status) {
                    (Some(_), _) => said.join("\n"),
                    (None, Some(status)) => format!("corgi serve ended ({status})"),
                    (None, None) => "corgi serve ended".into(),
                };
                events(number, LinkEvent::Ended(why));
            })?;
        Ok(Self {
            stdin: Mutex::new(stdin),
        })
    }

    /// Writes one request line.
    pub fn send(&self, request: &Request) -> io::Result<()> {
        let mut line = serde_json::to_vec(request).map_err(io::Error::other)?;
        line.push(b'\n');
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| io::Error::other("poisoned"))?;
        let stdin = stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "connection closed"))?;
        stdin.write_all(&line)?;
        stdin.flush()
    }

    /// Says goodbye by closing stdin, so the server exits once its running
    /// commands have replied.
    pub fn close(&self) {
        if let Ok(mut stdin) = self.stdin.lock() {
            stdin.take();
        }
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.close();
    }
}

/// Parses each stdout line. A line that is not a message this frontend
/// knows (a newer server's) is skipped, as the protocol asks.
fn read_messages(stdout: impl Read, mut each: impl FnMut(ServerMessage)) {
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<ServerMessage>(&line) {
            Ok(message) => each(message),
            Err(error) => eprintln!("corgi-gui: skipped a line from corgi serve: {error}"),
        }
    }
}

/// The child's exit status, waited for briefly once its stdout closed,
/// without holding its lock while waiting.
fn exit_status(child: &Mutex<Child>) -> Option<std::process::ExitStatus> {
    for _ in 0..40 {
        match child.lock().ok()?.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(_) => return None,
        }
    }
    None
}

/// Copies stderr through to ours, keeping its last lines.
fn keep_tail(stderr: impl Read, tail: &Mutex<VecDeque<String>>) {
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        eprintln!("corgi serve: {line}");
        if let Ok(mut tail) = tail.lock() {
            if tail.len() == STDERR_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    }
}

/// How long to wait before connecting again: doubling from half a second
/// to fifteen, and back to the start once a connection said hello.
#[derive(Debug, Default)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    const FIRST: Duration = Duration::from_millis(500);
    const LONGEST: Duration = Duration::from_secs(15);

    /// The wait before the next attempt, counting this failure.
    pub fn next_delay(&mut self) -> Duration {
        let delay = Self::FIRST.saturating_mul(1 << self.failures.min(10));
        self.failures = self.failures.saturating_add(1);
        delay.min(Self::LONGEST)
    }

    pub fn reset(&mut self) {
        self.failures = 0;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use corgi_core::protocol::Command as Wire;

    use super::*;

    #[test]
    fn the_backoff_doubles_to_fifteen_seconds_and_starts_over_after_a_hello() {
        let mut backoff = Backoff::default();
        let delays: Vec<u64> = (0..8)
            .map(|_| backoff.next_delay().as_millis() as u64)
            .collect();
        assert_eq!(delays, [500, 1000, 2000, 4000, 8000, 15000, 15000, 15000]);
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_millis(500));
    }

    #[test]
    fn unknown_lines_are_skipped_and_known_ones_read() {
        let input = concat!(
            r#"{"type":"reply","id":1,"ok":true,"result":{"kind":"done","message":"ok"}}"#,
            "\n",
            r#"{"type":"something_newer","x":1}"#,
            "\n\n",
            r#"{"type":"progress","id":1,"step":2}"#,
            "\n"
        );
        let mut seen = Vec::new();
        read_messages(input.as_bytes(), |message| seen.push(message));
        assert_eq!(seen.len(), 2);
        assert!(matches!(&seen[1], ServerMessage::Progress(p) if p.step == Some(2)));
    }

    #[cfg(unix)]
    #[test]
    fn a_child_s_lines_arrive_and_its_end_says_what_it_printed_last() {
        let (sender, receiver) = mpsc::channel();
        let script = concat!(
            r#"read line; echo "$line" >&2; "#,
            r#"echo '{"type":"reply","id":3,"ok":true}'; "#,
            "echo 'corgi: no Herdr here' >&2"
        );
        let link = Link::start(
            7,
            "sh",
            &["-c".into(), script.into()],
            Box::new(move |number, event| {
                let _ = sender.send((number, event));
            }),
        )
        .expect("sh starts");
        link.send(&Request {
            id: 1,
            command: Wire::Refresh,
        })
        .expect("written");
        let timeout = Duration::from_secs(10);
        let (number, first) = receiver.recv_timeout(timeout).expect("a message");
        assert_eq!(number, 7);
        assert!(matches!(first, LinkEvent::Message(m) if matches!(*m, ServerMessage::Reply(_))));
        let (_, last) = receiver.recv_timeout(timeout).expect("the end");
        let LinkEvent::Ended(why) = last else {
            panic!("not the end")
        };
        assert!(why.ends_with("corgi: no Herdr here"), "{why}");
        assert!(why.contains(r#""command":"refresh""#), "{why}");
    }
}
