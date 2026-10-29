//! The command line: which `corgi serve` the window talks to, on this
//! machine or over `ssh`.

use std::{env, path::PathBuf};

pub const USAGE: &str = "\
Usage: corgi-gui [--ssh HOST] [--session NAME | --socket PATH] [--corgi PATH]

Opens Corgi's desktop window on `corgi serve`, run on this machine or, with
--ssh, as `ssh HOST corgi serve` on another one.

  --ssh HOST       run `corgi serve` on HOST over ssh
  --session NAME   the Herdr session's socket (~/.config/herdr/sessions/NAME)
  --socket PATH    a Herdr socket by path
  --corgi PATH     the corgi to run: by default the one next to corgi-gui,
                   else `corgi` on PATH; with --ssh, `corgi` on HOST's PATH
  -h, --help       show this help
";

/// Where `corgi serve` runs and which Herdr it reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub ssh: Option<String>,
    pub session: Option<String>,
    pub socket: Option<String>,
    pub corgi: Option<String>,
}

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Run(Options),
    Help,
}

impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Parsed, String> {
        let mut options = Options {
            ssh: None,
            session: None,
            socket: None,
            corgi: None,
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let slot = match arg.as_str() {
                "-h" | "--help" => return Ok(Parsed::Help),
                "--ssh" => &mut options.ssh,
                "--session" => &mut options.session,
                "--socket" => &mut options.socket,
                "--corgi" => &mut options.corgi,
                other => return Err(format!("unknown argument `{other}`")),
            };
            match args.next() {
                Some(value) if !value.is_empty() && !value.starts_with("--") => *slot = Some(value),
                _ => return Err(format!("`{arg}` needs a value")),
            }
        }
        if options.session.is_some() && options.socket.is_some() {
            return Err("give --session or --socket, not both".into());
        }
        Ok(Parsed::Run(options))
    }

    /// The program and arguments that start `corgi serve`.
    pub fn command(&self) -> (String, Vec<String>) {
        let mut serve = vec!["serve".to_string()];
        if let Some(session) = &self.session {
            serve.extend(["--session".into(), session.clone()]);
        }
        if let Some(socket) = &self.socket {
            serve.extend(["--socket".into(), socket.clone()]);
        }
        match &self.ssh {
            None => (local_corgi(self.corgi.as_deref()), serve),
            Some(host) => {
                // ssh hands the remote shell one line, so every word is quoted.
                // A leading `~/` stays outside the quotes, for the remote
                // shell to expand.
                let corgi = self.corgi.as_deref().unwrap_or("corgi");
                let corgi = match corgi.strip_prefix("~/") {
                    Some(rest) => format!("~/{}", shell_quote(rest)),
                    None => shell_quote(corgi),
                };
                let remote = std::iter::once(corgi)
                    .chain(serve.iter().map(|word| shell_quote(word)))
                    .collect::<Vec<_>>()
                    .join(" ");
                let args = [
                    "-T",
                    // Never stop for a password or a host key: there is no
                    // terminal to answer on.
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "ServerAliveInterval=15",
                    "-o",
                    "ServerAliveCountMax=3",
                    "--",
                    host,
                ]
                .into_iter()
                .map(String::from)
                .chain([remote])
                .collect();
                ("ssh".into(), args)
            }
        }
    }

    /// Where the window's data comes from, for its title.
    pub fn place(&self) -> String {
        let herdr = match (&self.session, &self.socket) {
            (Some(session), _) => format!(" · session {session}"),
            (None, Some(socket)) => format!(" · {socket}"),
            (None, None) => String::new(),
        };
        match &self.ssh {
            Some(host) => format!("{host}{herdr}"),
            None => format!("this machine{herdr}"),
        }
    }
}

/// The local `corgi`: the one asked for, else the one installed next to this
/// binary (a release build's `target/release/corgi`, or both in one
/// tarball), else the one on `PATH`.
fn local_corgi(asked: Option<&str>) -> String {
    if let Some(asked) = asked {
        return asked.to_string();
    }
    env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("corgi")))
        .filter(|sibling: &PathBuf| sibling.is_file())
        .map(|sibling| sibling.display().to_string())
        .unwrap_or_else(|| "corgi".into())
}

/// `word` as one word for a POSIX shell.
fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c))
    {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Parsed, String> {
        Options::parse(args.iter().map(|arg| arg.to_string()))
    }

    fn options(args: &[&str]) -> Options {
        match parse(args) {
            Ok(Parsed::Run(options)) => options,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn local_serve_takes_the_session_or_socket_and_the_corgi_asked_for() {
        let (program, args) = options(&["--corgi", "/bin/corgi", "--session", "work"]).command();
        assert_eq!(program, "/bin/corgi");
        assert_eq!(args, ["serve", "--session", "work"]);
        assert_eq!(
            options(&["--session", "work"]).place(),
            "this machine · session work"
        );
        assert_eq!(parse(&["--help"]), Ok(Parsed::Help));
        assert!(parse(&["--session"]).is_err());
        assert!(parse(&["--session", "--ssh", "h"]).is_err());
        assert!(parse(&["--session", "a", "--socket", "/s"]).is_err());
        assert!(parse(&["serve"]).is_err());
    }

    #[test]
    fn ssh_runs_serve_on_the_host_with_every_word_quoted_and_never_prompts() {
        let (program, args) = options(&["--ssh", "box", "--socket", "/tmp/my sock's"]).command();
        assert_eq!(program, "ssh");
        assert!(args.windows(2).any(|pair| pair == ["-o", "BatchMode=yes"]));
        assert_eq!(
            &args[args.len() - 2..],
            ["box", r"corgi serve --socket '/tmp/my sock'\''s'"]
        );
        let (_, args) = options(&["--ssh", "box", "--corgi", "~/my bin/corgi"]).command();
        assert_eq!(args.last().unwrap(), "~/'my bin/corgi' serve");
        assert_eq!(options(&["--ssh", "box"]).place(), "box");
    }
}
