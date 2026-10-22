//! Exercise the real endpoint over stdin and a controlled socket, including
//! untrusted responses that echo the private text. Never contact live Herdr.
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixListener,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Socket {
    path: PathBuf,
    listener: UnixListener,
}

impl Socket {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "corgi-prompt-{}-{}.sock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        Self { path, listener }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corgi"));
        command
            .args([
                "--bar-prompt",
                self.path.to_str().unwrap(),
                "pane-synthetic",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).unwrap();
    }
}

#[test]
fn private_stdin_is_delivered_once_and_errors_never_echo_it() {
    for response in [
        "accepted",
        "blocked",
        "error",
        "disconnect",
        "invalid",
        "wrong_id",
    ] {
        let socket = Socket::new();
        // Generate the unique marker in memory; never pass it in argv/env.
        let marker = format!(
            "PRIVATE_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let text = format!("--{marker}\n\"quotes\" \\backslash café 日本語\nlast line");
        let mut child = socket.command().spawn().unwrap();
        writeln!(child.stdin.as_mut().unwrap(), "{}", json!({"text": text})).unwrap();
        // Keep stdin open, as the widget does. Delivery must not wait for EOF.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match socket.listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "endpoint did not connect"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error}"),
            }
        };
        // On macOS, an accepted stream inherits O_NONBLOCK from the listener
        // (unlike Linux), so read_line below would fail with WouldBlock
        // instead of honoring the read timeout set here.
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        let request: Value = serde_json::from_str(&request).unwrap();
        assert_eq!(request["method"], "agent.prompt");
        assert_eq!(
            request["params"],
            json!({"target": "pane-synthetic", "text": text, "wait": null})
        );
        assert_eq!(request.to_string().matches(&marker).count(), 1);
        // The actual binary is held in flight awaiting this test's reply.
        #[cfg(target_os = "linux")]
        {
            let proc = PathBuf::from(format!("/proc/{}", child.id()));
            for file in ["cmdline", "environ"] {
                let bytes = std::fs::read(proc.join(file)).unwrap();
                assert!(
                    !bytes
                        .windows(marker.len())
                        .any(|part| part == marker.as_bytes())
                );
            }
            assert!(
                std::fs::read_to_string(proc.join(format!("task/{}/children", child.id())))
                    .unwrap()
                    .trim()
                    .is_empty()
            );
        }
        let reply = match response {
            "accepted" => {
                json!({"id": request["id"], "result": {"type": "agent_prompted", "agent": {
                    "pane_id": "pane-synthetic", "workspace_id": "workspace-synthetic", "tab_id": "tab-synthetic"
                }}})
            }
            "blocked" => {
                json!({"id": request["id"], "error": {"code": "agent_blocked", "message": text}})
            }
            "error" => json!({"id": request["id"], "error": {"code": marker, "message": text}}),
            "wrong_id" => json!({"id": marker, "result": {}}),
            _ => Value::Null,
        };
        if response == "invalid" {
            writeln!(stream, "{text}").unwrap();
        } else if response != "disconnect" {
            writeln!(stream, "{reply}").unwrap();
        }
        drop(stream);
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.success(), response == "accepted");
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(!error.contains(&marker));
        if response == "blocked" {
            assert!(error.contains("Blocked agents"));
        } else if response != "accepted" {
            assert!(error.contains("Check the agent before retrying"));
        }
        assert_eq!(
            socket.listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "no automatic retry"
        );
    }
}

#[test]
fn invalid_stdin_never_contacts_herdr_or_echoes_input() {
    for input in [
        "",
        "\n",
        "PRIVATE_MALFORMED\n",
        "{}\n",
        "{\"text\":null}\n",
        "{\"text\":123}\n",
        "{\"text\":\"  \\n\\t\"}\n",
        "{\"text\":\"PRIVATE_UNTERMINATED\"}",
        "{\"text\":\"PRIVATE_EXTRA\",\"extra\":true}\n",
        "{\"text\":\"PRIVATE_TRAILING\"} {}\n",
    ] {
        let socket = Socket::new();
        let mut child = socket.command().spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            "Error: Invalid prompt input: expected one JSON line with nonempty text\n"
        );
        assert_eq!(
            socket.listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    // Invalid UTF-8 is also rejected without reproducing any input bytes.
    let socket = Socket::new();
    let mut child = socket.command().spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"\xffPRIVATE_UTF8\n")
        .unwrap();
    let mut error = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut error)
        .unwrap();
    assert!(!child.wait().unwrap().success());
    assert!(!error.contains("PRIVATE_UTF8"));
    assert!(socket.listener.accept().is_err());
}
