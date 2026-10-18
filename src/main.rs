use anyhow::Result;

/// What `corgi --help` prints, and an unknown command prints to stderr.
const USAGE: &str = "\
Usage: corgi [--compact]
       corgi COMMAND [ARGS...]

Without a command, opens the dashboard; --compact opens its popup layout.
It must run inside a Herdr pane.

Commands:
  setup       Bind a key that opens the dashboard
  supervisor  Start a project's supervisor
  spawn       Start an agent in its own worktree
  fleet       List a project's agents, their context and prompt cache
  digest      Print a summary of a supervisor's memory
  report      Print the report an agent left
  notify      Add an item to a supervisor's inbox
  inbox       Print what a supervisor's inbox holds
  tag         Tag an agent ready to merge, or clear its tag

Run corgi COMMAND --help for a command's options, and corgi --version for the
version.";

/// Whether the arguments after the program name start the dashboard: none,
/// or `--compact` as the plugin's `quick` pane passes it.
fn starts_dashboard(args: &[String]) -> bool {
    args.is_empty() || args == ["--compact"]
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let help = || args[2..].iter().any(|arg| arg == "--help" || arg == "-h");
    let usage = |text: &str| {
        println!("{text}");
        Ok(())
    };
    match args.get(1).map(String::as_str) {
        // scripts/install.sh runs this to check a downloaded release binary.
        Some("--version" | "-V") => usage(concat!("corgi ", env!("CARGO_PKG_VERSION"))),
        Some("spawn") if help() => usage(corgi::app::SPAWN_USAGE),
        Some("spawn") => corgi::app::spawn(&args[2..]),
        Some("--help" | "-h" | "help") => usage(USAGE),
        Some("supervisor") if help() => usage(corgi::app::SUPERVISOR_USAGE),
        Some("supervisor") => corgi::app::supervisor_command(&args[2..]),
        Some("fleet") if help() => usage(corgi::app::FLEET_USAGE),
        Some("fleet") => corgi::app::fleet(&args[2..]),
        Some("digest") if help() => usage(corgi::app::DIGEST_USAGE),
        Some("digest") => corgi::app::digest(&args[2..]),
        Some("report") if help() => usage(corgi::app::REPORT_USAGE),
        Some("report") => corgi::app::report(&args[2..]),
        Some("notify") if help() => usage(corgi::app::NOTIFY_USAGE),
        Some("notify") => corgi::app::notify(&args[2..]),
        Some("inbox") if help() => usage(corgi::app::INBOX_USAGE),
        Some("inbox") => corgi::app::inbox_command(&args[2..]),
        Some("tag") if help() => usage(corgi::app::TAG_USAGE),
        Some("tag") => corgi::app::tag_command(&args[2..]),
        Some("setup") if help() => usage(corgi::app::SETUP_USAGE),
        Some("setup") => corgi::app::setup(&args[2..]),
        Some("--bar-stream") => {
            let socket = args
                .get(2)
                .ok_or_else(|| anyhow::anyhow!("--bar-stream requires a Herdr socket path"))?;
            corgi::app::bar_stream(socket)
        }
        Some("--bar-transcript") => {
            let socket = args
                .get(2)
                .ok_or_else(|| anyhow::anyhow!("--bar-transcript requires a Herdr socket path"))?;
            let pane_id = args
                .get(3)
                .ok_or_else(|| anyhow::anyhow!("--bar-transcript requires a pane ID"))?;
            corgi::app::bar_transcript(socket, pane_id)
        }
        Some("--bar-prompt") => {
            anyhow::ensure!(args.len() == 4, "--bar-prompt requires socket and pane ID");
            corgi::app::bar_prompt(&args[2], &args[3])
        }
        Some("--bar-action") => {
            anyhow::ensure!(
                args.len() == 5,
                "--bar-action requires socket, pane ID, and x or m"
            );
            corgi::app::bar_action(&args[2], &args[3], &args[4])
        }
        Some("--bar-new-options") => {
            anyhow::ensure!(
                args.len() == 6,
                "--bar-new-options requires socket, pane ID, harness and model"
            );
            corgi::app::bar_new_options(&args[2], &args[3], &args[4], &args[5])
        }
        Some("--bar-new-agent") => corgi::app::bar_new_agent(
            args.get(2)
                .ok_or_else(|| anyhow::anyhow!("missing socket"))?,
        ),
        Some("--bar-navigate") => std::process::exit(corgi::app::bar_navigate(&args[2..])),
        _ if starts_dashboard(&args[1..]) => corgi::app::run(),
        // Not the dashboard, so that a mistyped command, or one this version
        // does not have yet, says so instead of opening it.
        _ => {
            eprintln!(
                "corgi: unknown command or option: {}\n\n{USAGE}",
                args[1..].join(" ")
            );
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::starts_dashboard;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    #[test]
    fn only_no_argument_or_compact_starts_the_dashboard() {
        assert!(starts_dashboard(&args(&[])));
        assert!(starts_dashboard(&args(&["--compact"])));
        for other in [
            &["bogus"][..],
            &["steward"],
            &["start"],
            &["handler"],
            &["--compact", "--compact"],
            &["--compact", "setup"],
            &["--verbose"],
        ] {
            assert!(!starts_dashboard(&args(other)), "{other:?}");
        }
    }
}
