//! `corgi setup`: binds a key to the plugin's open action in Herdr's
//! configuration, then opens the dashboard.
//!
//! A Herdr plugin cannot declare key bindings, and `herdr plugin install`
//! only runs the build step, so a fresh install opens nothing until the user
//! binds the action themselves. Setup does that once: it leaves a config
//! that already binds the action alone, asks for a key otherwise, and only
//! ever appends to the file, so nothing the user wrote is reformatted.

use std::{
    env, fs,
    io::{self, BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use toml::{Table, Value as Toml};

use crate::paths::home;

pub const SETUP_USAGE: &str = "\
Usage: corgi setup

Binds a key to open Corgi in Herdr's config.toml (HERDR_CONFIG_PATH, else
$XDG_CONFIG_HOME/herdr/config.toml, else ~/.config/herdr/config.toml), then
opens the dashboard, or focuses it when it is already open.

When the config already binds Corgi's open action, with any key, setup says
which key and changes nothing. Otherwise it asks for a key, prefix+d unless
you type another, checks it with herdr config check and against the keys
the config binds already, appends the binding to the end of the file
(creating it if needed), and runs herdr server reload-config. Without a
terminal to ask on it writes nothing and prints the binding to add instead.
Running it again is safe.";

/// The plugin's id, as `herdr-plugin.toml` names it.
const PLUGIN_ID: &str = "io.github.zyrre.corgi";
/// The plugin's action that opens the dashboard or focuses it.
const OPEN_ACTION: &str = "open";
/// The key setup suggests, the one the README has always used.
const DEFAULT_KEY: &str = "prefix+d";

/// What setup needs from Herdr, so tests can stand in for it.
trait HerdrCli {
    /// What Herdr finds wrong with `snippet` as a config file, if anything.
    /// `None` also when Herdr cannot be asked: the key is then taken on trust.
    fn check(&self, snippet: &str) -> Option<String>;
    /// Makes the running server read its config again.
    fn reload_config(&self) -> Result<(), HerdrError>;
    /// Runs the plugin's open action.
    fn open_dashboard(&self) -> Result<(), HerdrError>;
}

/// Why a `herdr` command failed: its error code, when it printed one, and
/// its message.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HerdrError {
    code: Option<String>,
    message: String,
}

pub fn setup(args: &[String]) -> Result<()> {
    if let Some(arg) = args.first() {
        bail!("unexpected argument {arg:?}; see corgi setup --help");
    }
    let config =
        config_path().context("cannot tell where Herdr's config.toml is: HOME is not set")?;
    let stdin = io::stdin();
    let mut terminal = stdin.is_terminal().then(|| stdin.lock());
    let mut out = io::stdout().lock();
    run(
        &config,
        terminal.as_mut().map(|lock| lock as &mut dyn BufRead),
        &mut out,
        &Herdr::from_env(),
    )
}

/// Where Herdr reads its config from: `HERDR_CONFIG_PATH`, else
/// `$XDG_CONFIG_HOME/herdr/config.toml`, else `~/.config/herdr/config.toml`.
fn config_path() -> Option<PathBuf> {
    let set = |name| env::var_os(name).filter(|value| !value.is_empty());
    if let Some(path) = set("HERDR_CONFIG_PATH") {
        return Some(PathBuf::from(path));
    }
    let base = set("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|home| home.join(".config")))?;
    Some(base.join("herdr").join("config.toml"))
}

/// Setup on `config`, asking on `terminal`, or not at all when there is
/// none, and telling the user on `out`.
fn run(
    config: &Path,
    terminal: Option<&mut dyn BufRead>,
    out: &mut dyn Write,
    herdr: &dyn HerdrCli,
) -> Result<()> {
    let text = match fs::read_to_string(config) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("read {}", config.display())),
    };
    let table: Table = text.parse().with_context(|| {
        format!(
            "{} is not valid TOML; fix it, then run corgi setup again",
            config.display()
        )
    })?;
    let shown = config.display();
    if let Some(key) = bound_key(&table) {
        writeln!(
            out,
            "Corgi already opens with {key} ({shown}); nothing changed."
        )?;
    } else if let Some(terminal) = terminal {
        let Some(key) = ask_key(&table, terminal, out, herdr)? else {
            writeln!(out, "No key chosen; nothing changed.")?;
            return Ok(());
        };
        let appended = appended(&text, &key);
        if let Err(error) = appended.parse::<Table>() {
            writeln!(
                out,
                "Adding the binding would make {shown} invalid, so nothing changed:\n{error}\n\
                 Add this to it by hand instead:\n\n{}",
                snippet(&key)
            )?;
            return Ok(());
        }
        if let Some(dir) = config.parent() {
            fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        fs::write(config, appended).with_context(|| format!("write {}", config.display()))?;
        writeln!(out, "Added {key}, which opens Corgi, to {shown}.")?;
        match herdr.reload_config() {
            Ok(()) => writeln!(out, "Herdr reloaded its config.")?,
            Err(error) if error.code.as_deref() == Some("server_not_running") => writeln!(
                out,
                "Herdr is not running; {key} works from its next start."
            )?,
            Err(error) => writeln!(
                out,
                "Herdr could not reload its config ({}); run herdr server reload-config.",
                error.message
            )?,
        }
    } else {
        writeln!(
            out,
            "No terminal to ask for a key on, so nothing changed. To open Corgi with \
             {DEFAULT_KEY}, add this to {shown} and run herdr server reload-config:\n\n{}",
            snippet(DEFAULT_KEY)
        )?;
    }
    open(out, herdr)
}

/// Opens the dashboard through the plugin's own action, and says why when
/// it cannot.
fn open(out: &mut dyn Write, herdr: &dyn HerdrCli) -> Result<()> {
    match herdr.open_dashboard() {
        Ok(()) => writeln!(out, "Opened Corgi.")?,
        Err(error) => match error.code.as_deref() {
            Some("server_not_running") => writeln!(
                out,
                "Herdr is not running, so Corgi did not open; start herdr, then press the key."
            )?,
            Some("plugin_not_found") => writeln!(
                out,
                "The Corgi plugin is not installed in Herdr, so it did not open; run herdr plugin install zyrre/corgi."
            )?,
            _ => writeln!(out, "Corgi did not open: {}", error.message)?,
        },
    }
    Ok(())
}

/// Asks for a key until one is free and Herdr accepts it; `None` when the
/// terminal ends first.
fn ask_key(
    table: &Table,
    terminal: &mut dyn BufRead,
    out: &mut dyn Write,
    herdr: &dyn HerdrCli,
) -> Result<Option<String>> {
    loop {
        write!(out, "Key to open Corgi [{DEFAULT_KEY}]: ")?;
        out.flush()?;
        let mut line = String::new();
        if terminal.read_line(&mut line)? == 0 {
            writeln!(out)?;
            return Ok(None);
        }
        let key = match line.trim() {
            "" => DEFAULT_KEY,
            typed => typed,
        };
        if let Some(owner) = key_owner(table, key) {
            writeln!(
                out,
                "{key} is already bound to {owner}; choose another key."
            )?;
        } else if let Some(problem) = herdr.check(&snippet(key)) {
            writeln!(out, "Herdr does not accept {key}: {problem}")?;
        } else {
            return Ok(Some(key.to_string()));
        }
    }
}

/// The `[[keys.command]]` entries of a config.
fn commands(table: &Table) -> impl Iterator<Item = &Table> {
    table
        .get("keys")
        .and_then(Toml::as_table)
        .and_then(|keys| keys.get("command"))
        .and_then(Toml::as_array)
        .into_iter()
        .flatten()
        .filter_map(Toml::as_table)
}

/// The key a config binds to Corgi's open action, if it binds one.
fn bound_key(table: &Table) -> Option<String> {
    let open = format!("{PLUGIN_ID}.{OPEN_ACTION}");
    commands(table)
        .find(|command| {
            command.get("type").and_then(Toml::as_str) == Some("plugin_action")
                && command.get("command").and_then(Toml::as_str) == Some(&open)
        })
        .map(|command| {
            command
                .get("key")
                .and_then(Toml::as_str)
                .unwrap_or("no key")
                .to_string()
        })
}

/// What a config already binds `key` to: one of Herdr's own actions under
/// `[keys]`, or a `[[keys.command]]` entry.
fn key_owner(table: &Table, key: &str) -> Option<String> {
    let keys = table.get("keys").and_then(Toml::as_table)?;
    let binds = |value: &Toml| match value {
        Toml::String(bound) => bound == key,
        Toml::Array(bound) => bound.iter().any(|bound| bound.as_str() == Some(key)),
        _ => false,
    };
    if let Some((action, _)) = keys
        .iter()
        .find(|(action, value)| *action != "command" && binds(value))
    {
        return Some(format!("Herdr's {action} action"));
    }
    commands(table)
        .find(|command| command.get("key").is_some_and(binds))
        .map(|command| {
            let field = |name| command.get(name).and_then(Toml::as_str);
            let what = match (field("type"), field("command")) {
                (Some(kind), Some(command)) => format!("the {kind} {command:?}"),
                (None, Some(command)) => format!("the command {command:?}"),
                _ => "a command".to_string(),
            };
            match field("description") {
                Some(description) => format!("{what} ({description})"),
                None => what,
            }
        })
}

/// The binding of `key` to Corgi's open action, as it is appended.
fn snippet(key: &str) -> String {
    let key = key.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "[[keys.command]]\n\
         key = \"{key}\"\n\
         type = \"plugin_action\"\n\
         command = \"{PLUGIN_ID}.{OPEN_ACTION}\"\n\
         description = \"Corgi (open or focus)\"\n"
    )
}

/// `text` with the binding of `key` after it, set off by a blank line, and
/// the text itself unchanged.
fn appended(text: &str, key: &str) -> String {
    let mut appended = text.to_string();
    if !appended.is_empty() {
        if !appended.ends_with('\n') {
            appended.push('\n');
        }
        appended.push('\n');
    }
    appended.push_str(&snippet(key));
    appended
}

/// The `herdr` command: `HERDR_BIN_PATH`, which Herdr sets for plugins, else
/// the one on `PATH`.
struct Herdr {
    bin: PathBuf,
}

impl Herdr {
    fn from_env() -> Self {
        let bin = env::var_os("HERDR_BIN_PATH")
            .filter(|path| !path.is_empty())
            .map_or_else(|| PathBuf::from("herdr"), PathBuf::from);
        Self { bin }
    }

    /// Runs `herdr args`, its JSON error read into a [`HerdrError`].
    fn run(&self, args: &[&str]) -> Result<(), HerdrError> {
        let output = Command::new(&self.bin)
            .args(args)
            .output()
            .map_err(|error| HerdrError {
                code: None,
                message: format!("cannot run {}: {error}", self.bin.display()),
            })?;
        if output.status.success() {
            return Ok(());
        }
        let printed = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Err(herdr_error(&printed))
    }
}

/// The error a failed `herdr` command printed: the `error` of its JSON
/// reply, else its first line.
fn herdr_error(printed: &str) -> HerdrError {
    let error = printed
        .lines()
        .find_map(|line| serde_json::from_str::<Value>(line).ok())
        .and_then(|reply| reply.get("error").cloned());
    let text = |field: &str| {
        error
            .as_ref()
            .and_then(|error| error.get(field))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    HerdrError {
        code: text("code"),
        message: text("message").unwrap_or_else(|| {
            printed
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or("it failed without saying why")
                .to_string()
        }),
    }
}

/// What `herdr config check` found wrong, without its summary line and the
/// binding's index.
fn check_problem(printed: &str) -> Option<String> {
    let problems: Vec<&str> = printed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("config:"))
        .map(|line| line.strip_suffix("; disabling binding").unwrap_or(line))
        .collect();
    (!problems.is_empty()).then(|| problems.join(" "))
}

impl HerdrCli for Herdr {
    fn check(&self, snippet: &str) -> Option<String> {
        let path = env::temp_dir().join(format!("corgi-setup-{}.toml", std::process::id()));
        fs::write(&path, snippet).ok()?;
        let output = Command::new(&self.bin)
            .args(["config", "check"])
            .env("HERDR_CONFIG_PATH", &path)
            .output();
        fs::remove_file(&path).ok();
        let output = output.ok()?;
        if output.status.success() {
            return None;
        }
        check_problem(&format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }

    fn reload_config(&self) -> Result<(), HerdrError> {
        self.run(&["server", "reload-config"])
    }

    fn open_dashboard(&self) -> Result<(), HerdrError> {
        self.run(&[
            "plugin",
            "action",
            "invoke",
            OPEN_ACTION,
            "--plugin",
            PLUGIN_ID,
        ])
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, io::Cursor};

    use super::*;
    use crate::test_support::ScratchDir;

    /// A Herdr that refuses the keys in `invalid`, answers reloads and opens
    /// with `reply`, and remembers what it was asked.
    struct FakeHerdr {
        invalid: Vec<&'static str>,
        reply: Result<(), HerdrError>,
        calls: RefCell<Vec<String>>,
    }

    impl FakeHerdr {
        fn new() -> Self {
            Self {
                invalid: Vec::new(),
                reply: Ok(()),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl HerdrCli for FakeHerdr {
        fn check(&self, snippet: &str) -> Option<String> {
            self.calls.borrow_mut().push("check".into());
            self.invalid
                .iter()
                .find(|key| snippet.contains(&format!("key = \"{key}\"")))
                .map(|key| format!("invalid keybinding: {key}"))
        }

        fn reload_config(&self) -> Result<(), HerdrError> {
            self.calls.borrow_mut().push("reload".into());
            self.reply.clone()
        }

        fn open_dashboard(&self) -> Result<(), HerdrError> {
            self.calls.borrow_mut().push("open".into());
            self.reply.clone()
        }
    }

    /// Runs setup on `config` with `typed` on a terminal, or with no
    /// terminal when it is `None`; what it printed.
    fn setup_with(config: &Path, typed: Option<&str>, herdr: &FakeHerdr) -> String {
        let mut out = Vec::new();
        let mut terminal = typed.map(|typed| Cursor::new(typed.as_bytes().to_vec()));
        run(
            config,
            terminal.as_mut().map(|cursor| cursor as &mut dyn BufRead),
            &mut out,
            herdr,
        )
        .expect("setup runs");
        String::from_utf8(out).expect("utf-8 output")
    }

    const OTHER_CONFIG: &str = "\
# My Herdr
[theme]
name = \"kanagawa\"   # keep

[keys]
new_tab = \"prefix+c\"

[[keys.command]]
key = \"prefix+g\"
type = \"plugin_action\"
command = \"other.plugin.open\"
description = \"Other\"";

    #[test]
    fn an_existing_binding_with_any_key_is_reported_and_left_alone() {
        let dir = ScratchDir::new("setup-bound");
        let config = dir.join("config.toml");
        let text = "[[keys.command]]\nkey = \"ctrl+g\"\ntype = \"plugin_action\"\n\
                    command = \"io.github.zyrre.corgi.open\"\n";
        fs::write(&config, text).unwrap();
        let herdr = FakeHerdr::new();

        let printed = setup_with(&config, Some("prefix+x\n"), &herdr);

        assert!(printed.contains("already opens with ctrl+g"), "{printed}");
        assert_eq!(fs::read_to_string(&config).unwrap(), text);
        assert_eq!(*herdr.calls.borrow(), ["open"]);
    }

    #[test]
    fn the_default_key_is_appended_and_everything_before_it_kept_byte_for_byte() {
        let dir = ScratchDir::new("setup-append");
        let config = dir.join("config.toml");
        fs::write(&config, OTHER_CONFIG).unwrap();
        let herdr = FakeHerdr::new();

        let printed = setup_with(&config, Some("\n"), &herdr);

        let written = fs::read_to_string(&config).unwrap();
        assert_eq!(
            written,
            format!("{OTHER_CONFIG}\n\n{}", snippet("prefix+d"))
        );
        assert!(printed.contains("Added prefix+d"), "{printed}");
        assert_eq!(*herdr.calls.borrow(), ["check", "reload", "open"]);
        let table: Table = written.parse().unwrap();
        assert_eq!(bound_key(&table).as_deref(), Some("prefix+d"));
    }

    #[test]
    fn a_key_bound_to_something_else_or_refused_by_herdr_is_asked_again() {
        let dir = ScratchDir::new("setup-taken");
        let config = dir.join("config.toml");
        fs::write(&config, OTHER_CONFIG).unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.invalid = vec!["prefix+zz"];

        let printed = setup_with(
            &config,
            Some("prefix+c\nprefix+g\nprefix+zz\nprefix+o\n"),
            &herdr,
        );

        assert!(
            printed.contains("prefix+c is already bound to Herdr's new_tab action"),
            "{printed}"
        );
        assert!(
            printed.contains(
                "prefix+g is already bound to the plugin_action \"other.plugin.open\" (Other)"
            ),
            "{printed}"
        );
        assert!(
            printed.contains("Herdr does not accept prefix+zz"),
            "{printed}"
        );
        let written = fs::read_to_string(&config).unwrap();
        assert!(written.ends_with(&snippet("prefix+o")), "{written}");
    }

    #[test]
    fn a_missing_config_and_its_directory_are_created() {
        let dir = ScratchDir::new("setup-missing");
        let config = dir.join("herdr").join("config.toml");
        let mut herdr = FakeHerdr::new();
        herdr.reply = Err(HerdrError {
            code: Some("server_not_running".into()),
            message: "no herdr server is running".into(),
        });

        let printed = setup_with(&config, Some("ctrl+g\n"), &herdr);

        assert_eq!(fs::read_to_string(&config).unwrap(), snippet("ctrl+g"));
        assert!(printed.contains("works from its next start"), "{printed}");
        assert!(
            printed.contains("Herdr is not running, so Corgi did not open"),
            "{printed}"
        );
    }

    #[test]
    fn without_a_terminal_nothing_is_written_but_the_binding_is_shown_and_corgi_opened() {
        let dir = ScratchDir::new("setup-no-tty");
        let config = dir.join("config.toml");
        fs::write(&config, OTHER_CONFIG).unwrap();
        let herdr = FakeHerdr::new();

        let printed = setup_with(&config, None, &herdr);

        assert_eq!(fs::read_to_string(&config).unwrap(), OTHER_CONFIG);
        assert!(printed.contains(&snippet("prefix+d")), "{printed}");
        assert_eq!(*herdr.calls.borrow(), ["open"]);
        assert!(!dir.join("herdr").exists());
    }

    #[test]
    fn the_terminal_ending_before_a_key_writes_nothing() {
        let dir = ScratchDir::new("setup-eof");
        let config = dir.join("config.toml");
        let herdr = FakeHerdr::new();

        let printed = setup_with(&config, Some(""), &herdr);

        assert!(printed.contains("nothing changed"), "{printed}");
        assert!(!config.exists());
        assert!(herdr.calls.borrow().is_empty());
    }

    #[test]
    fn a_binding_that_would_break_the_config_is_shown_instead_of_written() {
        let dir = ScratchDir::new("setup-inline");
        let config = dir.join("config.toml");
        let text = "[keys]\ncommand = []\n";
        fs::write(&config, text).unwrap();

        let printed = setup_with(&config, Some("\n"), &FakeHerdr::new());

        assert_eq!(fs::read_to_string(&config).unwrap(), text);
        assert!(printed.contains("Add this to it by hand"), "{printed}");
    }

    #[test]
    fn herdr_replies_are_read_for_their_code_and_message() {
        let error = herdr_error(
            "{\"id\":\"cli:plugin\",\"error\":{\"code\":\"plugin_not_found\",\"message\":\"plugin not found\"}}\n",
        );
        assert_eq!(error.code.as_deref(), Some("plugin_not_found"));
        assert_eq!(error.message, "plugin not found");
        assert_eq!(
            check_problem(
                "config: issues found\ninvalid keybinding: keys.command[0].key = \"foo+d\"; disabling binding\n"
            )
            .as_deref(),
            Some("invalid keybinding: keys.command[0].key = \"foo+d\"")
        );
        assert_eq!(check_problem("config: ok\n"), None);
    }
}
