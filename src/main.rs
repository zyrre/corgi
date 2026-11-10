use anyhow::Result;

/// What `corgi --help` prints, and an unknown command prints to stderr.
const USAGE: &str = "\
Usage: corgi [--compact]
       corgi COMMAND [ARGS...]

Without a command, opens the dashboard; --compact opens its popup layout.
It must run inside a Herdr pane.

Commands:
  supervisor  Start a project's supervisor
  spawn       Start an agent in its own worktree
  fleet       List a project's agents, their context and prompt cache
  orphans     List agent sessions Corgi gets wrong, and leftover workspaces
  digest      Print a summary of a supervisor's memory
  report      Print the report an agent left
  notify      Add an item to a supervisor's inbox
  inbox       Print what a supervisor's inbox holds
  recovery    List recovered sessions held from wakes, and release one
  tag         Tag an agent ready to merge, or clear its tag
  merge       Merge an agent's worktree branch, as the dashboard's m does
  close       Close a worker on herdr-core and remove its worktree
  pause       Pause the agents working on herdr-core, before the network goes
  resume      Resume the agents corgi pause stopped
  status      Print every project's supervisor, agents and questions as JSON
  usage       Print each harness's plan usage (5-hour and weekly) as JSON
  role        Print a part of the supervisor's role it reads on demand
  edit        Open the focused or selected agent's checkout in your editor
  daemon      Wake supervisors on herdr-core (the Herdr app), as a service
  app-space   Make the Herdr app's Corgi space by hand, or remove it
  app-layout  Put Corgi's Projects list into the Herdr app's layout, or take it out
  projects    List the projects to offer for Open project, as JSON
  open        Open a project: name its supervisor's terminal, or start it

Run corgi COMMAND --help for a command's options, and corgi --version for the
version and the commit it was built from (--version --json for both as JSON).";

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
        Some("--version" | "-V") => corgi::version::version_command(&args[2..]),
        Some("spawn") if help() => usage(corgi::app::SPAWN_USAGE),
        Some("spawn") => corgi::app::spawn(&args[2..]),
        Some("--help" | "-h" | "help") => usage(USAGE),
        Some("supervisor") if help() => usage(corgi::app::SUPERVISOR_USAGE),
        Some("supervisor") => corgi::app::supervisor_command(&args[2..]),
        Some("fleet") if help() => usage(corgi::app::FLEET_USAGE),
        Some("fleet") => corgi::app::fleet(&args[2..]),
        Some("orphans") if help() => usage(corgi::app::ORPHANS_USAGE),
        Some("orphans") => corgi::app::orphans_command(&args[2..]),
        Some("digest") if help() => usage(corgi::app::DIGEST_USAGE),
        Some("digest") => corgi::app::digest(&args[2..]),
        Some("report") if help() => usage(corgi::app::REPORT_USAGE),
        Some("report") => corgi::app::report(&args[2..]),
        Some("notify") if help() => usage(corgi::app::NOTIFY_USAGE),
        Some("notify") => corgi::app::notify(&args[2..]),
        Some("inbox") if help() => usage(corgi::app::INBOX_USAGE),
        Some("inbox") => corgi::app::inbox_command(&args[2..]),
        Some("recovery") if help() => usage(corgi::app::RECOVERY_USAGE),
        Some("recovery") => corgi::app::recovery_command(&args[2..]),
        Some("status") if help() => usage(corgi::app::STATUS_USAGE),
        Some("status") => corgi::app::status_command(&args[2..]),
        Some("usage") if help() => usage(corgi::app::USAGE_USAGE),
        Some("usage") => corgi::app::usage_command(&args[2..]),
        Some("merge") if help() => usage(corgi::app::MERGE_USAGE),
        Some("merge") => corgi::app::merge_command(&args[2..]),
        Some("close") if help() => usage(corgi::app::CLOSE_USAGE),
        Some("close") => corgi::app::close_command(&args[2..]),
        Some("pause") if help() => usage(corgi::app::PAUSE_USAGE),
        Some("pause") => corgi::app::pause_command(&args[2..]),
        Some("resume") if help() => usage(corgi::app::RESUME_USAGE),
        Some("resume") => corgi::app::resume_command(&args[2..]),
        Some("tag") if help() => usage(corgi::app::TAG_USAGE),
        Some("tag") => corgi::app::tag_command(&args[2..]),
        Some("role") if help() => usage(corgi::app::ROLE_USAGE),
        Some("role") => corgi::app::role_command(&args[2..]),
        Some("edit") if help() => usage(corgi::app::EDIT_USAGE),
        Some("edit") => corgi::app::edit_command(&args[2..]),
        Some("daemon") if help() => usage(corgi::app::DAEMON_USAGE),
        Some("daemon") => corgi::app::daemon_command(&args[2..]),
        Some("app-space") if help() => usage(corgi::app::APP_SPACE_USAGE),
        Some("app-space") => corgi::app::app_space_command(&args[2..]),
        Some("app-layout") if help() => usage(corgi::app::APP_LAYOUT_USAGE),
        Some("app-layout") => corgi::app::app_layout_command(&args[2..]),
        Some("projects") if help() => usage(corgi::app::PROJECTS_USAGE),
        Some("projects") => corgi::app::projects_command(&args[2..]),
        Some("open") if help() => usage(corgi::app::OPEN_USAGE),
        Some("open") => corgi::app::open_command(&args[2..]),
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
