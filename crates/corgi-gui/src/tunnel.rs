//! A machine's Herdr, brought here over `ssh` for the focus view's terminal:
//! one `ssh -L … -L …` per machine forwards its `herdr.sock` and the
//! `herdr-client.sock` beside it to two sockets of the same names in a
//! private directory here, so the terminal reads them as it reads a local
//! session's. The remote side first checks that both sockets exist (a Herdr
//! too old for the endpoint has no `herdr-client.sock`), says `ready`, and
//! then waits on its stdin: when this process ends, however it ends, ssh
//! sees the pipe close and exits by itself.
//!
//! Dropping a [`Tunnel`] ends its ssh and removes its directory. The
//! directories of a process that ended without that are removed by the
//! next window's first tunnel.

use std::{
    collections::VecDeque,
    fs,
    io::{self, BufRead, BufReader, Read},
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex, Once,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{endpoint, options::shell_quote};

/// The JSON socket's name here; the endpoint's is beside it, under the
/// name [`endpoint::client_socket`] gives.
const JSON_SOCKET: &str = "herdr.sock";
/// How long ssh may take, after the remote side said `ready`, to show both
/// local sockets.
const BIND_WAIT: Duration = Duration::from_secs(5);
/// macOS's limit on a socket's path, less a margin.
const MAX_SOCKET_PATH: usize = 100;
/// Lines of ssh's stderr kept to explain why it ended.
const STDERR_LINES: usize = 4;

/// What the remote side prints first.
const READY: &str = "ready";
const NO_HERDR: &str = "no-herdr";
const NO_ENDPOINT: &str = "no-endpoint";

/// What a tunnel's reader saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelEvent {
    /// Both sockets are forwarded; the local JSON socket is
    /// [`Tunnel::json_socket`].
    Ready,
    /// ssh ended, or never got as far as `ready`.
    Ended(Ended),
}

/// Why a tunnel ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The host's Herdr has no `herdr-client.sock` beside its `herdr.sock`:
    /// older than the endpoint, or not serving it.
    NoEndpoint,
    /// No Herdr socket where the host's `corgi serve` said it reads.
    NoHerdr,
    /// ssh's own last words, or how it exited.
    Failed(String),
}

/// One running `ssh` forwarding a machine's two Herdr sockets.
pub struct Tunnel {
    dir: PathBuf,
    child: Arc<Mutex<Child>>,
    /// Held open: ssh's remote side exits when it closes.
    stdin: Option<ChildStdin>,
}

impl Tunnel {
    /// Starts forwarding `host`'s Herdr, whose JSON socket there is
    /// `remote_json`, and a thread that says when it is ready and when it
    /// ends.
    pub fn start(
        host: &str,
        remote_json: &str,
        events: impl Fn(TunnelEvent) + Send + 'static,
    ) -> io::Result<Self> {
        let dir = new_dir()?;
        let local_json = dir.join(JSON_SOCKET);
        let args = ssh_args(host, &local_json, Path::new(remote_json));
        let spawned = Command::new("ssh")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                let _ = fs::remove_dir_all(&dir);
                return Err(io::Error::new(
                    error.kind(),
                    format!("could not start ssh: {error}"),
                ));
            }
        };
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let stdin = child.stdin.take();
        let child = Arc::new(Mutex::new(child));
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let stderr_thread = {
            let tail = tail.clone();
            thread::Builder::new()
                .name("corgi-tunnel-stderr".into())
                .spawn(move || keep_tail(stderr, &tail))?
        };
        let reader_child = child.clone();
        let sockets = [local_json.clone(), endpoint::client_socket(&local_json)];
        thread::Builder::new()
            .name("corgi-tunnel-reader".into())
            .spawn(move || {
                let mut said = None;
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let line = line.trim().to_string();
                    if line == READY && said.is_none() && bound(&sockets, BIND_WAIT) {
                        events(TunnelEvent::Ready);
                    }
                    said.get_or_insert(line);
                }
                let status = exit_status(&reader_child);
                let _ = stderr_thread.join();
                let last: Vec<String> = tail
                    .lock()
                    .map(|t| t.iter().cloned().collect())
                    .unwrap_or_default();
                events(TunnelEvent::Ended(ended(said.as_deref(), &last, status)));
            })?;
        Ok(Self { dir, child, stdin })
    }

    /// The forwarded JSON socket; the endpoint's is beside it.
    pub fn json_socket(&self) -> PathBuf {
        self.dir.join(JSON_SOCKET)
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.stdin.take();
        // Our own child, by its handle: ended at once, and reaped.
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// ssh's arguments: never prompting, forwarding both sockets (failing if it
/// cannot), replacing a leftover local socket, and running the check that
/// then waits on stdin.
fn ssh_args(host: &str, local_json: &Path, remote_json: &Path) -> Vec<String> {
    let local_client = endpoint::client_socket(local_json);
    let remote_client = endpoint::client_socket(remote_json);
    let forward = |local: &Path, remote: &Path| format!("{}:{}", local.display(), remote.display());
    // Run by `sh`, whatever the user's login shell is.
    let script = format!(
        "test -S \"$1\" || {{ echo {NO_HERDR}; exit 0; }}; \
         test -S \"$2\" || {{ echo {NO_ENDPOINT}; exit 0; }}; \
         echo {READY}; exec cat >/dev/null"
    );
    let remote = [
        "sh".to_string(),
        "-c".into(),
        script,
        "corgi-tunnel".into(),
        remote_json.display().to_string(),
        remote_client.display().to_string(),
    ]
    .iter()
    .map(|word| shell_quote(word))
    .collect::<Vec<_>>()
    .join(" ");
    [
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "StreamLocalBindUnlink=yes",
        "-o",
        "StreamLocalBindMask=0177",
        "-L",
        &forward(local_json, remote_json),
        "-L",
        &forward(&local_client, &remote_client),
        "--",
        host,
        &remote,
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Why a tunnel ended, from the first line its remote side printed, ssh's
/// last stderr line and its exit status.
fn ended(said: Option<&str>, stderr: &[String], status: Option<std::process::ExitStatus>) -> Ended {
    match said {
        Some(NO_ENDPOINT) => Ended::NoEndpoint,
        Some(NO_HERDR) => Ended::NoHerdr,
        _ => Ended::Failed(match (stderr.last(), status) {
            (Some(said), _) => said.clone(),
            (None, Some(status)) => format!("ssh ended ({status})"),
            (None, None) => "ssh ended".into(),
        }),
    }
}

/// Waits until every socket exists, up to `wait`.
fn bound(sockets: &[PathBuf], wait: Duration) -> bool {
    let started = Instant::now();
    loop {
        if sockets.iter().all(|socket| socket.exists()) {
            return true;
        }
        if started.elapsed() > wait {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

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

fn keep_tail(stderr: impl Read, tail: &Mutex<VecDeque<String>>) {
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        eprintln!("corgi-gui: ssh: {line}");
        if let Ok(mut tail) = tail.lock() {
            if tail.len() == STDERR_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    }
}

// ---- The directories ----

/// Each tunnel's number in this process.
static NEXT: AtomicU64 = AtomicU64::new(1);
static SWEEP: Once = Once::new();

/// A new directory of this process's, `<root>/<pid>.<n>`, readable by the
/// user alone.
fn new_dir() -> io::Result<PathBuf> {
    let root = root()?;
    SWEEP.call_once(|| sweep(&root));
    let dir = root.join(format!(
        "{}.{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::DirBuilder::new().mode(0o700).create(&dir)?;
    Ok(dir)
}

/// `corgi-gui-<uid>` in the user's runtime or temporary directory, or in
/// `/tmp` when that is too long for a socket's path; made private, and
/// refused when someone else owns it or can read it.
fn root() -> io::Result<PathBuf> {
    let uid = unsafe { libc::getuid() };
    let name = format!("corgi-gui-{uid}");
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .or_else(|| std::env::var_os("TMPDIR"))
        .map(PathBuf::from)
        .filter(|base| base.is_absolute())
        .filter(|base| fits(&base.join(&name)))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let root = base.join(name);
    match fs::DirBuilder::new().mode(0o700).create(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let meta = fs::symlink_metadata(&root)?;
    if !meta.is_dir() || meta.uid() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a directory of yours", root.display()),
        ));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    }
    Ok(root)
}

/// Whether the longest socket path under `root` fits a socket address.
fn fits(root: &Path) -> bool {
    let longest = root
        .join("4294967295.9999")
        .join(endpoint::client_socket(Path::new(JSON_SOCKET)));
    longest.as_os_str().len() <= MAX_SOCKET_PATH
}

/// Removes the directories of processes that have ended.
fn sweep(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = name
            .to_str()
            .and_then(|name| name.split_once('.'))
            .and_then(|(pid, _)| pid.parse::<i32>().ok());
        if let Some(pid) = pid
            && !alive(pid)
        {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Whether process `pid` exists: signal 0 checks without sending anything.
fn alive(pid: i32) -> bool {
    if pid <= 0 {
        return true;
    }
    let sent = unsafe { libc::kill(pid, 0) };
    sent == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn ssh_forwards_both_sockets_under_their_own_names_and_never_prompts() {
        let args = ssh_args(
            "box",
            Path::new("/run/me/corgi-gui-501/7.1/herdr.sock"),
            Path::new("/home/me/.config/herdr/herdr.sock"),
        );
        let pair = |flag: &str| {
            args.windows(2)
                .filter(|w| w[0] == flag)
                .map(|w| w[1].clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            pair("-L"),
            [
                "/run/me/corgi-gui-501/7.1/herdr.sock:/home/me/.config/herdr/herdr.sock",
                "/run/me/corgi-gui-501/7.1/herdr-client.sock:/home/me/.config/herdr/herdr-client.sock",
            ]
        );
        for option in [
            "BatchMode=yes",
            "ExitOnForwardFailure=yes",
            "StreamLocalBindUnlink=yes",
        ] {
            assert!(pair("-o").iter().any(|o| o == option), "{option}");
        }
        let at = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(args[at + 1], "box");
        assert_eq!(args.len(), at + 3, "one remote line");
        assert!(
            args[at + 2].starts_with("sh -c 'test -S"),
            "{}",
            args[at + 2]
        );
        assert!(
            args[at + 2].ends_with(
                "corgi-tunnel /home/me/.config/herdr/herdr.sock \
                 /home/me/.config/herdr/herdr-client.sock"
            ),
            "{}",
            args[at + 2]
        );
    }

    /// The remote line, run by a local shell as ssh's remote side would:
    /// it tells a missing Herdr from a missing endpoint, and otherwise says
    /// `ready` and waits for its stdin to close.
    #[test]
    fn the_remote_check_tells_what_is_missing_and_otherwise_waits_on_stdin() {
        let scratch =
            std::env::temp_dir().join(format!("corgi-tunnel-check-{}", std::process::id()));
        fs::create_dir_all(&scratch).unwrap();
        let json = scratch.join("herdr.sock");
        let run = |json: &Path| {
            let args = ssh_args("box", Path::new("/x/herdr.sock"), json);
            let mut child = Command::new("sh")
                .args(["-c", args.last().unwrap()])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            drop(child.stdin.take());
            let output = child.wait_with_output().unwrap();
            String::from_utf8(output.stdout).unwrap()
        };
        assert_eq!(run(&json).trim(), NO_HERDR);
        let _json = std::os::unix::net::UnixListener::bind(&json).unwrap();
        assert_eq!(run(&json).trim(), NO_ENDPOINT);
        let _client = std::os::unix::net::UnixListener::bind(endpoint::client_socket(&json));
        assert_eq!(run(&json).trim(), READY);
        let _ = fs::remove_dir_all(&scratch);
    }

    #[test]
    fn an_ending_says_what_the_remote_side_found_else_what_ssh_said() {
        assert_eq!(ended(Some(NO_ENDPOINT), &[], None), Ended::NoEndpoint);
        assert_eq!(ended(Some(NO_HERDR), &[], None), Ended::NoHerdr);
        let said = vec![
            "Warning: something".to_string(),
            "ssh: connect to host box port 22: Connection refused".to_string(),
        ];
        assert_eq!(ended(None, &said, None), Ended::Failed(said[1].clone()));
        assert_eq!(
            ended(Some(READY), &[], None),
            Ended::Failed("ssh ended".into())
        );
    }

    #[test]
    fn a_dead_process_s_directory_is_swept_and_a_live_one_s_kept() {
        let root = std::env::temp_dir().join(format!("corgi-tunnel-sweep-{}", std::process::id()));
        let ours = root.join(format!("{}.1", std::process::id()));
        // Process IDs wrap well below this on macOS and Linux.
        let dead = root.join("2147483646.1");
        let other = root.join("notes");
        for dir in [&ours, &dead, &other] {
            fs::create_dir_all(dir).unwrap();
        }
        sweep(&root);
        assert!(ours.exists());
        assert!(!dead.exists());
        assert!(other.exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// A real tunnel whose "ssh" fails at once: it says why, and dropping it
    /// removes its directory.
    #[test]
    fn a_tunnel_that_cannot_connect_says_why_and_leaves_nothing() {
        let (sender, receiver) = mpsc::channel();
        let started = Tunnel::start(
            "-oProxyCommand=false",
            "/nowhere/herdr.sock",
            move |event| {
                let _ = sender.send(event);
            },
        );
        let tunnel = match started {
            Ok(tunnel) => tunnel,
            // No ssh on this machine: nothing to check.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
            Err(error) => panic!("{error}"),
        };
        let dir = tunnel.dir.clone();
        assert!(dir.is_dir());
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let event = receiver
            .recv_timeout(Duration::from_secs(20))
            .expect("an end");
        assert!(
            matches!(event, TunnelEvent::Ended(Ended::Failed(_))),
            "{event:?}"
        );
        drop(tunnel);
        assert!(!dir.exists());
    }
}
