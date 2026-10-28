//! Finding out what the terminal looks like, once at startup: the active
//! Omarchy theme, else the terminal's answer to the OSC color queries, else
//! Catppuccin, Mocha or Latte by whether the terminal or the system is
//! known to be light.

use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use super::{Palette, Rgb, Source, Theme, is_light};

/// How long the terminal gets to answer the color queries. Herdr answers
/// them itself, from the colors it read off the terminal it runs in, in well
/// under a millisecond; a terminal that does not answer at all costs this.
const QUERY_TIMEOUT: Duration = Duration::from_millis(100);
/// How long the system's light or dark preference may take to read.
const PREFERENCE_TIMEOUT: Duration = Duration::from_millis(150);

/// The theme to draw with, by `CORGI_THEME` when it names one, else by the
/// first source that answers. Call it with the terminal in raw mode and
/// before anything else reads its input, since the terminal's answer to the
/// queries arrives there.
///
/// It writes its queries to stdout and reads the answers from stdin, so a
/// frontend whose stdio is not a terminal must never call it: in `corgi
/// serve` the queries would corrupt the protocol and the protocol's lines
/// would be read as answers. Such a frontend builds its theme from
/// [`omarchy_palette`] or a fixed palette instead.
pub fn resolve() -> Theme {
    let truecolor =
        env::var("COLORTERM").is_ok_and(|value| matches!(value.as_str(), "truecolor" | "24bit"));
    let forced = env::var("CORGI_THEME").unwrap_or_default();
    match forced.as_str() {
        "catppuccin-mocha" => return Theme::catppuccin(false, truecolor),
        "catppuccin-latte" => return Theme::catppuccin(true, truecolor),
        _ => {}
    }
    if forced != "terminal"
        && let Some(palette) = omarchy_palette()
    {
        return Theme::new(palette, Source::Omarchy, true, truecolor);
    }
    let reply = query_terminal(QUERY_TIMEOUT);
    if let Some(palette) = reply.palette() {
        return Theme::new(palette, Source::Terminal, true, truecolor);
    }
    let light = reply
        .background
        .map(is_light)
        .or_else(system_prefers_light)
        .unwrap_or(false);
    // The terminal paints its own theme, unread, so roles stay its colors;
    // Catppuccin stands in for the shades, with whatever the terminal did
    // say put over it.
    let mut palette = Palette::catppuccin(light);
    if let Some(foreground) = reply.foreground {
        palette.foreground = foreground;
    }
    if let Some(background) = reply.background {
        palette.background = background;
    }
    Theme::new(palette, Source::Catppuccin, true, truecolor)
}

/// The palette of the active Omarchy theme: its Alacritty colors, which
/// Omarchy renders for every theme and which hold the whole terminal
/// palette, already resolved. Omarchy 4 keeps the current theme under
/// `~/.local/state`, earlier versions under `~/.config`.
pub fn omarchy_palette() -> Option<Palette> {
    let home = PathBuf::from(env::var_os("HOME")?);
    [
        ".local/state/omarchy/current/theme/alacritty.toml",
        ".config/omarchy/current/theme/alacritty.toml",
    ]
    .iter()
    .find_map(|relative| read_alacritty(&home.join(relative)))
}

fn read_alacritty(path: &Path) -> Option<Palette> {
    parse_alacritty(&std::fs::read_to_string(path).ok()?)
}

/// The palette in an Alacritty config: `[colors.primary]`, `[colors.normal]`
/// and `[colors.bright]`, each color written `#rrggbb` or `0xrrggbb`. A
/// config without the bright colors repeats the normal ones.
pub fn parse_alacritty(text: &str) -> Option<Palette> {
    let config: toml::Table = toml::from_str(text).ok()?;
    let colors = config.get("colors")?.as_table()?;
    let color = |section: &str, name: &str| -> Option<Rgb> {
        parse_hex(colors.get(section)?.as_table()?.get(name)?.as_str()?)
    };
    const NAMES: [&str; 8] = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    let mut ansi = [(0, 0, 0); 16];
    for (index, name) in NAMES.iter().enumerate() {
        ansi[index] = color("normal", name)?;
        ansi[index + 8] = color("bright", name).unwrap_or(ansi[index]);
    }
    Some(Palette {
        foreground: color("primary", "foreground")?,
        background: color("primary", "background")?,
        ansi,
    })
}

fn parse_hex(text: &str) -> Option<Rgb> {
    let hex = text
        .trim()
        .strip_prefix('#')
        .or_else(|| text.trim().strip_prefix("0x"))?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    Some((channel(0)?, channel(2)?, channel(4)?))
}

/// What the terminal said of its colors.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
    pub ansi: [Option<Rgb>; 16],
}

impl Reply {
    /// The whole palette, when the terminal told all of it.
    pub fn palette(&self) -> Option<Palette> {
        let mut ansi = [(0, 0, 0); 16];
        for (slot, color) in ansi.iter_mut().zip(self.ansi) {
            *slot = color?;
        }
        Some(Palette {
            foreground: self.foreground?,
            background: self.background?,
            ansi,
        })
    }
}

/// The queries for the foreground (OSC 10), the background (OSC 11) and the
/// sixteen ANSI colors (OSC 4), then for the terminal's attributes (DA1),
/// which every terminal answers, so the end of its answer says that every
/// color it was going to tell has arrived.
pub fn queries() -> String {
    let mut queries = String::from("\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
    for index in 0..16 {
        queries.push_str(&format!("\x1b]4;{index};?\x1b\\"));
    }
    queries.push_str("\x1b[c");
    queries
}

/// Whether `bytes` hold the end of the answer to DA1, `ESC [ ? … c`.
pub fn answered(bytes: &[u8]) -> bool {
    bytes.windows(3).enumerate().any(|(start, window)| {
        window == b"\x1b[?"
            && bytes[start + 3..]
                .iter()
                .find(|byte| !(byte.is_ascii_digit() || **byte == b';'))
                == Some(&b'c')
    })
}

/// The colors in the terminal's answers: `OSC 10 ; spec`, `OSC 11 ; spec`
/// and `OSC 4 ; index ; spec`, each ended by BEL or ST. Anything else, such
/// as the DA1 answer, is passed over.
pub fn parse_reply(bytes: &[u8]) -> Reply {
    let mut reply = Reply::default();
    let text = String::from_utf8_lossy(bytes);
    for answer in text.split("\x1b]").skip(1) {
        let end = answer.find(['\x07', '\x1b']).unwrap_or(answer.len());
        let mut fields = answer[..end].split(';');
        match fields.next() {
            Some("10") => reply.foreground = fields.next().and_then(parse_spec),
            Some("11") => reply.background = fields.next().and_then(parse_spec),
            Some("4") => {
                let index = fields.next().and_then(|index| index.parse::<usize>().ok());
                let color = fields.next().and_then(parse_spec);
                if let (Some(index @ 0..16), Some(color)) = (index, color) {
                    reply.ansi[index] = Some(color);
                }
            }
            _ => {}
        }
    }
    reply
}

/// An X11 color spec as terminals answer with: `rgb:R/G/B` with one to four
/// hex digits a channel, `rgba:` with a fourth, or `#rrggbb`.
fn parse_spec(spec: &str) -> Option<Rgb> {
    if spec.starts_with('#') {
        return parse_hex(spec);
    }
    let channels = spec
        .strip_prefix("rgb:")
        .or_else(|| spec.strip_prefix("rgba:"))?;
    let mut channels = channels.split('/').map(|digits| {
        if digits.is_empty() || digits.len() > 4 {
            return None;
        }
        let value = u32::from_str_radix(digits, 16).ok()?;
        let max = (1u32 << (4 * digits.len())) - 1;
        Some(((value * 255 + max / 2) / max) as u8)
    });
    Some((channels.next()??, channels.next()??, channels.next()??))
}

/// Asks the terminal for its colors and waits up to `timeout` for the
/// answer.
#[cfg(unix)]
fn query_terminal(timeout: Duration) -> Reply {
    use std::{
        fs::OpenOptions,
        io::{Read, Write},
        os::fd::AsRawFd,
    };

    let Ok(mut tty) = OpenOptions::new().read(true).write(true).open("/dev/tty") else {
        return Reply::default();
    };
    if tty
        .write_all(queries().as_bytes())
        .and_then(|()| tty.flush())
        .is_err()
    {
        return Reply::default();
    }
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 1024];
    while !answered(&bytes) {
        let left = deadline.saturating_duration_since(Instant::now());
        // select rather than poll, which macOS does not support on a tty.
        if left.is_zero() || !readable(tty.as_raw_fd(), left) {
            break;
        }
        match tty.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
        }
    }
    parse_reply(&bytes)
}

#[cfg(not(unix))]
fn query_terminal(_timeout: Duration) -> Reply {
    Reply::default()
}

/// Whether `fd` has something to read within `timeout`.
#[cfg(unix)]
fn readable(fd: std::os::fd::RawFd, timeout: Duration) -> bool {
    // SAFETY: the set is initialised by FD_ZERO before use, `fd` is an open
    // descriptor below FD_SETSIZE (a freshly opened tty), and the timeval
    // lives across the call.
    unsafe {
        if usize::try_from(fd).is_ok_and(|fd| fd >= libc::FD_SETSIZE) {
            return false;
        }
        let mut set: libc::fd_set = std::mem::zeroed();
        libc::FD_ZERO(&mut set);
        libc::FD_SET(fd, &mut set);
        let mut wait = libc::timeval {
            tv_sec: timeout.as_secs() as libc::time_t,
            tv_usec: timeout.subsec_micros() as libc::suseconds_t,
        };
        libc::select(
            fd + 1,
            &mut set,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut wait,
        ) > 0
    }
}

/// Whether the system prefers a light appearance, when it says: macOS's
/// appearance, else the freedesktop portal's or GNOME's color scheme.
pub fn system_prefers_light() -> Option<bool> {
    if cfg!(target_os = "macos") {
        // The key is only there in dark mode.
        let output = run(
            "defaults",
            &["read", "-g", "AppleInterfaceStyle"],
            PREFERENCE_TIMEOUT,
        )?;
        return Some(!output.contains("Dark"));
    }
    if cfg!(unix) {
        let portal = run(
            "busctl",
            &[
                "--user",
                "call",
                "org.freedesktop.portal.Desktop",
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.portal.Settings",
                "ReadOne",
                "ss",
                "org.freedesktop.appearance",
                "color-scheme",
            ],
            PREFERENCE_TIMEOUT,
        );
        // `v u 1` prefers dark, `v u 2` light, and `v u 0` has no preference.
        match portal.as_deref().map(str::trim) {
            Some("v u 1") => return Some(false),
            Some("v u 2") => return Some(true),
            _ => {}
        }
        let gnome = run(
            "gsettings",
            &["get", "org.gnome.desktop.interface", "color-scheme"],
            PREFERENCE_TIMEOUT,
        )?;
        if gnome.contains("prefer-dark") {
            return Some(false);
        }
        if gnome.contains("prefer-light") {
            return Some(true);
        }
    }
    None
}

/// The output of `program` with `args`, or `None` when it cannot run, fails
/// or takes longer than `timeout`. A command that fails because a setting
/// is absent still has its say: `defaults` fails when there is no dark mode.
fn run(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKYO_NIGHT: &str = include_str!("fixtures/omarchy-tokyo-night-alacritty.toml");

    #[test]
    fn an_omarchy_theme_is_read_from_its_alacritty_colors() {
        let palette = parse_alacritty(TOKYO_NIGHT).expect("Tokyo Night");
        assert_eq!(palette.background, (0x1a, 0x1b, 0x26));
        assert_eq!(palette.foreground, (0xa9, 0xb1, 0xd6));
        assert_eq!(palette.ansi[0], (0x1a, 0x1b, 0x26));
        assert_eq!(palette.ansi[6], (0x44, 0x9d, 0xab));
        assert_eq!(palette.ansi[8], (0x41, 0x48, 0x68));
        assert_eq!(palette.ansi[15], (0xc0, 0xca, 0xf5));
        assert!(!palette.is_light());
    }

    #[test]
    fn an_older_alacritty_theme_with_hex_numbers_and_no_brights_is_read_too() {
        let palette = parse_alacritty(
            "[colors.primary]\nbackground = '0xeff1f5'\nforeground = '0x4c4f69'\n\
             [colors.normal]\nblack = '0x5c5f77'\nred = '0xd20f39'\ngreen = '0x40a02b'\n\
             yellow = '0xdf8e1d'\nblue = '0x1e66f5'\nmagenta = '0xea76cb'\n\
             cyan = '0x179299'\nwhite = '0xacb0be'\n",
        )
        .expect("Latte");
        assert!(palette.is_light());
        assert_eq!(palette.ansi[9], palette.ansi[1]);
        // A config without a palette is no theme.
        assert_eq!(parse_alacritty("[window]\nopacity = 0.9\n"), None);
        assert_eq!(parse_alacritty("not toml ["), None);
        assert_eq!(parse_hex("#12345"), None);
    }

    #[test]
    fn a_terminals_color_answers_are_read_in_every_form_they_come_in() {
        // Herdr's answer, as recorded from a pane: four hex digits a
        // channel, ended by ST, then the DA1 answer.
        let mut bytes =
            b"\x1b]10;rgb:ebeb/dbdb/b2b2\x1b\\\x1b]11;rgb:2828/2828/2828\x1b\\".to_vec();
        for index in 0..16 {
            bytes.extend(
                format!("\x1b]4;{index};rgb:{index:02x}{index:02x}/0000/ffff\x1b\\").bytes(),
            );
        }
        bytes.extend(b"\x1b[?62;22c");
        assert!(answered(&bytes));
        let reply = parse_reply(&bytes);
        let palette = reply.palette().expect("the whole palette");
        assert_eq!(palette.foreground, (0xeb, 0xdb, 0xb2));
        assert_eq!(palette.background, (0x28, 0x28, 0x28));
        assert_eq!(palette.ansi[0], (0, 0, 255));
        assert_eq!(palette.ansi[15], (15, 0, 255));

        // Two digits and BEL, one digit, and `#rrggbb`.
        let reply =
            parse_reply(b"\x1b]11;rgb:ef/f1/f5\x07\x1b]10;rgb:f/0/8\x07\x1b]4;3;#df8e1d\x07");
        assert_eq!(reply.background, Some((0xef, 0xf1, 0xf5)));
        assert_eq!(reply.foreground, Some((255, 0, 136)));
        assert_eq!(reply.ansi[3], Some((0xdf, 0x8e, 0x1d)));
        // Only part of the palette is no palette.
        assert_eq!(reply.palette(), None);

        // Nothing, or nothing but DA1.
        assert_eq!(parse_reply(b""), Reply::default());
        assert_eq!(parse_reply(b"\x1b[?1;2c"), Reply::default());
        assert!(!answered(b"\x1b]11;rgb:0/0/0\x07"));
        assert!(!answered(b"\x1b[?62;2"));
        // Garbage in a spec is passed over.
        let reply = parse_reply(b"\x1b]11;rgb:zz/00/00\x07\x1b]4;99;rgb:0/0/0\x07");
        assert_eq!(reply, Reply::default());
    }

    #[test]
    fn the_queries_ask_for_every_color_and_end_with_da1() {
        let queries = queries();
        assert!(queries.starts_with("\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b]4;0;?\x1b\\"));
        assert!(queries.contains("\x1b]4;15;?\x1b\\"));
        assert!(queries.ends_with("\x1b[c"));
    }
}
