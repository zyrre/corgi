//! hc's event stream, read straight from the core's socket.
//!
//! `events.subscribe` on a plain (unframed) connection answers with a
//! snapshot of every terminal, then turns the connection into a stream of
//! notifications, one JSON line each: `event` (a fact changed: `created`,
//! `status`, `turn`, `labels`, `foreground`, `finished`, `lost`, `restarted`,
//! `keep`, `removed`, `title`, `size`, `doc`), `heartbeat` every 15 seconds,
//! and `missed` with a fresh snapshot for a subscriber that fell behind. The
//! stream ends when the core stops or restarts, and a new core counts `seq`
//! from the start again, so the daemon never compares it across connections:
//! it reconnects and takes the new snapshot as the truth.
//!
//! The socket is used rather than `hc events` because any `hc` command starts
//! a stopped core, with the caller's environment; a connection to a socket
//! nobody listens on starts nothing.

use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;

use super::{ROLE_LABEL, Terminal};

/// One line of the stream.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// Every terminal, as they are now: the first line, and after a
    /// subscriber fell behind.
    Snapshot(Vec<Terminal>),
    /// A fact of one terminal changed.
    Event {
        /// `created`, `status`, `turn`, `labels`, `foreground`, `finished`,
        /// `lost`, `restarted`, `keep`, `removed`, `title`, `size` or `doc`.
        kind: String,
        terminal: Option<String>,
        data: Value,
    },
    Heartbeat,
    /// The subscriber fell behind and missed events.
    Missed,
    /// A line this Corgi does not know.
    Other,
}

/// A subscription to one core's events.
pub struct EventStream {
    reader: BufReader<UnixStream>,
}

impl EventStream {
    /// Subscribes to the events of the core listening on `socket`, and
    /// returns the stream with the snapshot it starts from.
    pub fn connect(socket: &Path) -> Result<(Self, Vec<Terminal>)> {
        let mut stream = UnixStream::connect(socket)
            .with_context(|| format!("connect to hc at {}", socket.display()))?;
        stream
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"events.subscribe\",\"params\":{}}\n",
            )
            .context("subscribe to hc's events")?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .context("read hc's answer to the subscription")?;
        match parse(&line) {
            Some(Message::Snapshot(terminals)) => Ok((Self { reader }, terminals)),
            _ => bail!("hc did not answer the subscription: {}", line.trim()),
        }
    }

    /// The next line of the stream; `None` once the core closed it.
    pub fn next_message(&mut self) -> Result<Option<Message>> {
        loop {
            let mut line = String::new();
            if self.reader.read_line(&mut line)? == 0 {
                return Ok(None);
            }
            if line.trim().is_empty() {
                continue;
            }
            return Ok(Some(parse(&line).unwrap_or(Message::Other)));
        }
    }
}

#[derive(Deserialize)]
struct Line {
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Deserialize)]
struct Snapshot {
    #[serde(default)]
    terminals: Vec<Terminal>,
}

/// `line` as a message of the stream, or `None` when it is not JSON, or an
/// error.
pub fn parse(line: &str) -> Option<Message> {
    let line: Line = serde_json::from_str(line.trim()).ok()?;
    if line.error.is_some() {
        return None;
    }
    if let Some(result) = line.result {
        let snapshot: Snapshot = serde_json::from_value(result).ok()?;
        return Some(Message::Snapshot(snapshot.terminals));
    }
    let params = line.params.unwrap_or(Value::Null);
    Some(match line.method.as_deref() {
        Some("event") => Message::Event {
            kind: params
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            terminal: params
                .get("terminal")
                .and_then(Value::as_str)
                .map(str::to_string),
            data: params.get("data").cloned().unwrap_or(Value::Null),
        },
        Some("heartbeat") => Message::Heartbeat,
        Some("missed") => match params.get("terminals") {
            Some(terminals) => {
                Message::Snapshot(serde_json::from_value(terminals.clone()).unwrap_or_default())
            }
            None => Message::Missed,
        },
        _ => Message::Other,
    })
}

/// Whether `message` may change what the daemon does: a snapshot, a gap in
/// the stream, any event of a terminal it follows (`known`, by address), or
/// one that gave a terminal Corgi's role label. Heartbeats and the events of
/// the user's other terminals do not.
pub fn concerns_corgi(message: &Message, known: &HashSet<String>) -> bool {
    match message {
        Message::Snapshot(_) | Message::Missed => true,
        Message::Heartbeat | Message::Other => false,
        Message::Event {
            kind,
            terminal,
            data,
        } => {
            if terminal.as_ref().is_some_and(|t| known.contains(t)) {
                return true;
            }
            // A label change carries the full map, alone or under `labels`
            // as a created terminal does.
            let role = |labels: &Value| {
                labels
                    .get(ROLE_LABEL)
                    .and_then(Value::as_str)
                    .is_some_and(|role| !role.is_empty())
            };
            match kind.as_str() {
                "created" | "labels" => {
                    data.get("labels").is_some_and(role) || (kind == "labels" && role(data))
                }
                _ => false,
            }
        }
    }
}
