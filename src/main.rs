//! Corgi: the `corgi` command-line tools behind the Herdr app's mods and a
//! project's supervisor. They run agents in herdr-core (`hc`) terminals,
//! read the agents' session files, keep the supervisors' memory and inboxes,
//! and wake the supervisors from `corgi daemon`.

use anyhow::Result;

mod activity;
mod app;
mod app_layout;
mod app_space;
mod backlog;
#[cfg(test)]
mod brand_tile;
mod choices;
mod digest;
mod draft;
mod git;
mod harness;
mod hc;
mod inbox;
mod inbox_privacy;
mod job;
mod launch;
mod merge;
mod model;
mod paths;
mod progress;
mod projects;
mod rows;
mod session;
mod supervisor;
mod tag;
#[cfg(test)]
mod test_support;
mod time;
mod usage;
mod usage_cache;
mod version;
mod waker;

/// What `corgi --help` prints, and an unknown command prints to stderr.
const USAGE: &str = "\
Usage: corgi COMMAND [ARGS...]

Corgi runs a project's supervisor and its worker agents in herdr-core (hc)
terminals, for the Herdr app.

Commands:
  supervisor  Start a project's supervisor
  spawn       Start an agent in its own worktree
  fleet       List a project's agents, their context and prompt cache
  digest      Print a summary of a supervisor's memory
  report      Print the report an agent left
  notify      Add an item to a supervisor's inbox
  inbox       Print what a supervisor's inbox holds
  tag         Tag an agent ready to merge, or clear its tag
  role        Print a part of the supervisor's role it reads on demand
  merge       Merge a worker's worktree branch and push it
  close       Close a worker and remove its worktree
  pause       Pause the working agents, before the network goes
  resume      Resume the agents corgi pause stopped
  status      Print every project's supervisor, agents and questions as JSON
  usage       Print each harness's plan usage (5-hour and weekly) as JSON
  projects    List the projects to offer for Open project, as JSON
  open        Open a project: name its supervisor's terminal, or start it
  daemon      Wake supervisors on herdr-core, as a service
  app-layout  Put Corgi's Projects list into the Herdr app's layout, or take it out
  app-space   Make the Herdr app's Corgi space by hand, or remove it

Run corgi COMMAND --help for a command's options, and corgi --version for the
version and the commit it was built from (--version --json for both as JSON).";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let help = || args[2..].iter().any(|arg| arg == "--help" || arg == "-h");
    let usage = |text: &str| {
        println!("{text}");
        Ok(())
    };
    match args.get(1).map(String::as_str) {
        // scripts/install.sh runs this to check a downloaded release binary.
        Some("--version" | "-V") => version::version_command(&args[2..]),
        None | Some("--help" | "-h" | "help") => usage(USAGE),
        Some("spawn") if help() => usage(app::SPAWN_USAGE),
        Some("spawn") => app::spawn(&args[2..]),
        Some("supervisor") if help() => usage(app::SUPERVISOR_USAGE),
        Some("supervisor") => app::supervisor_command(&args[2..]),
        Some("fleet") if help() => usage(app::FLEET_USAGE),
        Some("fleet") => app::fleet(&args[2..]),
        Some("digest") if help() => usage(app::DIGEST_USAGE),
        Some("digest") => app::digest(&args[2..]),
        Some("report") if help() => usage(app::REPORT_USAGE),
        Some("report") => app::report(&args[2..]),
        Some("notify") if help() => usage(app::NOTIFY_USAGE),
        Some("notify") => app::notify(&args[2..]),
        Some("inbox") if help() => usage(app::INBOX_USAGE),
        Some("inbox") => app::inbox_command(&args[2..]),
        Some("status") if help() => usage(app::STATUS_USAGE),
        Some("status") => app::status_command(&args[2..]),
        Some("usage") if help() => usage(app::USAGE_USAGE),
        Some("usage") => app::usage_command(&args[2..]),
        Some("merge") if help() => usage(app::MERGE_USAGE),
        Some("merge") => app::merge_command(&args[2..]),
        Some("close") if help() => usage(app::CLOSE_USAGE),
        Some("close") => app::close_command(&args[2..]),
        Some("pause") if help() => usage(app::PAUSE_USAGE),
        Some("pause") => app::pause_command(&args[2..]),
        Some("resume") if help() => usage(app::RESUME_USAGE),
        Some("resume") => app::resume_command(&args[2..]),
        Some("tag") if help() => usage(app::TAG_USAGE),
        Some("tag") => app::tag_command(&args[2..]),
        Some("role") if help() => usage(app::ROLE_USAGE),
        Some("role") => app::role_command(&args[2..]),
        Some("daemon") if help() => usage(app::DAEMON_USAGE),
        Some("daemon") => app::daemon_command(&args[2..]),
        Some("app-space") if help() => usage(app::APP_SPACE_USAGE),
        Some("app-space") => app::app_space_command(&args[2..]),
        Some("app-layout") if help() => usage(app::APP_LAYOUT_USAGE),
        Some("app-layout") => app::app_layout_command(&args[2..]),
        Some("projects") if help() => usage(app::PROJECTS_USAGE),
        Some("projects") => app::projects_command(&args[2..]),
        Some("open") if help() => usage(app::OPEN_USAGE),
        Some("open") => app::open_command(&args[2..]),
        // A mistyped command, or one this version does not have, says so.
        _ => {
            eprintln!(
                "corgi: unknown command or option: {}\n\n{USAGE}",
                args[1..].join(" ")
            );
            std::process::exit(2);
        }
    }
}
