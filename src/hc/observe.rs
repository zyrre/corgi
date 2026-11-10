//! A watch-only viewer of an hc terminal, for its first paint.
//!
//! hc's API opens a viewer (`terminal.attach {observe: true}`) only on a
//! framed connection, whose byte layout hc does not document, so Corgi runs
//! `hc attach --observe` in a pseudo-terminal of its own and reads what it
//! draws. That viewer never types or resizes. Its first paint is the whole
//! screen, each row on a line of its own with its styling (SGR), after a few
//! questions to the viewer's terminal (its colours, its size) that need no
//! answer; then come live updates, so the read stops once the paint has gone
//! quiet (`docs/herdr-app-port.md`, "Detecting a draft through hc").

use std::{
    fs::File,
    io::Read,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

/// How long the paint may take to start, and to finish in all.
const PAINT_LIMIT: Duration = Duration::from_secs(4);
/// How long without a byte ends the paint, once it has started.
const QUIET: Duration = Duration::from_millis(400);
/// How long the viewer has to end after its terminal hung up, before it is
/// killed.
const VIEWER_EXIT: Duration = Duration::from_millis(500);

/// The first paint of `target`, read through `hc` (the binary `bin`) in a
/// pseudo-terminal of `cols` by `rows` (the terminal's own size, so nothing
/// is cut or wrapped).
pub(super) fn paint(bin: &Path, target: &str, cols: u16, rows: u16) -> Result<String> {
    let (master, slave) = open_pty(cols.max(20), rows.max(5))?;
    let mut command = Command::new(bin);
    command
        .args(["attach", "--observe", target])
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    // The pseudo-terminal, by then its standard input, becomes the viewer's
    // controlling terminal, so it hangs up when Corgi closes its end.
    unsafe {
        // The request's type differs between platforms (and libcs); `as _`
        // takes whichever `ioctl` wants.
        #[allow(clippy::unnecessary_cast)]
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("run {} attach --observe", bin.display()))?;
    // The parent's copies of the slave end go with `command`.
    drop(command);
    let read = read_paint(&master);
    // Hanging up ends the viewer; one that lingers is killed.
    drop(master);
    let deadline = Instant::now() + VIEWER_EXIT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    let bytes = read?;
    if bytes.is_empty() {
        bail!("hc attach --observe {target} painted no screen");
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// What the viewer writes to `master` from its paint on: the paint starts
/// with a clear screen, after the viewer's questions to its terminal (its
/// colours, its size), which it waits half a second for answers to. They
/// are left unanswered, so nothing is ever written to the viewer. Reading
/// ends once the paint has gone quiet for [`QUIET`], or [`PAINT_LIMIT`] has
/// passed.
fn read_paint(master: &OwnedFd) -> Result<Vec<u8>> {
    let started = Instant::now();
    let mut bytes = Vec::new();
    let mut paint = None;
    let mut file = File::from(master.try_clone()?);
    loop {
        let elapsed = started.elapsed();
        if elapsed >= PAINT_LIMIT {
            break;
        }
        let wait = match paint {
            None => PAINT_LIMIT - elapsed,
            Some(_) => QUIET.min(PAINT_LIMIT - elapsed),
        };
        if !readable(master, wait)? {
            if paint.is_none() {
                continue;
            }
            break;
        }
        let mut chunk = [0u8; 16 * 1024];
        match file.read(&mut chunk) {
            // EIO: the viewer ended and the terminal hung up.
            Ok(0) | Err(_) => break,
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
        }
        if paint.is_none() {
            paint = paint_start(&bytes);
        }
    }
    Ok(paint.map_or_else(Vec::new, |start| bytes.split_off(start)))
}

/// Where the paint starts in what a viewer wrote: at its clear screen.
pub(super) fn paint_start(bytes: &[u8]) -> Option<usize> {
    const CLEAR: &[u8] = b"\x1b[2J";
    bytes
        .windows(CLEAR.len())
        .position(|window| window == CLEAR)
}

/// Whether `fd` has something to read within `wait`.
fn readable(fd: &OwnedFd, wait: Duration) -> Result<bool> {
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = libc::c_int::try_from(wait.as_millis()).unwrap_or(libc::c_int::MAX);
    let ready = unsafe { libc::poll(&mut poll, 1, millis) };
    if ready < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(error).context("poll the viewer's terminal");
    }
    Ok(ready > 0 && poll.revents & (libc::POLLIN | libc::POLLHUP) != 0)
}

/// A new pseudo-terminal of `cols` by `rows`: its master and slave ends.
fn open_pty(cols: u16, rows: u16) -> Result<(OwnedFd, OwnedFd)> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let mut size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // macOS takes the size by `*mut`, Linux by `*const`: a raw pointer
    // passes as either, where `&mut size` fails clippy on Linux.
    let size: *mut libc::winsize = &mut size;
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            size,
        )
    };
    if opened != 0 {
        return Err(std::io::Error::last_os_error()).context("open a pseudo-terminal");
    }
    // Neither end goes to a program Corgi starts later.
    for fd in [master, slave] {
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) })
}
