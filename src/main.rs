//! Corgi: the `corgi` command-line tools behind the Herdr app's mods and a
//! project's supervisor. They run agents in herdr-core (`hc`) terminals,
//! read the agents' session files, keep the supervisors' memory and inboxes,
//! and wake the supervisors from `corgi daemon`.

use anyhow::Result;

mod activity;
mod app_layout;
mod backlog;
#[cfg(test)]
mod brand_tile;
mod choices;
mod commands;
mod digest;
mod draft;
mod git;
mod harness;
mod hc;
mod herdr_app;
mod inbox;
mod inbox_privacy;
mod job;
mod launch;
mod merge;
mod model;
mod paths;
mod progress;
mod projects;
mod questions;
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
  questions   Print a supervisor's open questions, parsed, as JSON
  backlog     Print a supervisor's backlog as JSON
  answer      Send the user's answer to a question, or a backlog Start
  usage       Print each harness's plan usage (5-hour and weekly) as JSON
  projects    List the projects to offer for Open project, as JSON
  open        Open a project: name its supervisor's terminal, or start it
  daemon      Wake supervisors on herdr-core, as a service
  app-layout  Put Corgi's Projects list into the Herdr app's layout, or take it out

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
        Some("spawn") if help() => usage(commands::SPAWN_USAGE),
        Some("spawn") => commands::spawn_command(&args[2..]),
        Some("supervisor") if help() => usage(commands::SUPERVISOR_USAGE),
        Some("supervisor") => commands::supervisor_command(&args[2..]),
        Some("fleet") if help() => usage(commands::FLEET_USAGE),
        Some("fleet") => commands::fleet_command(&args[2..]),
        Some("digest") if help() => usage(commands::DIGEST_USAGE),
        Some("digest") => commands::digest_command(&args[2..]),
        Some("report") if help() => usage(commands::REPORT_USAGE),
        Some("report") => commands::report_command(&args[2..]),
        Some("notify") if help() => usage(commands::NOTIFY_USAGE),
        Some("notify") => commands::notify_command(&args[2..]),
        Some("inbox") if help() => usage(commands::INBOX_USAGE),
        Some("inbox") => commands::inbox_command(&args[2..]),
        Some("status") if help() => usage(commands::STATUS_USAGE),
        Some("status") => commands::status_command(&args[2..]),
        Some("questions") if help() => usage(commands::QUESTIONS_USAGE),
        Some("questions") => commands::questions_command(&args[2..]),
        Some("backlog") if help() => usage(commands::BACKLOG_USAGE),
        Some("backlog") => commands::backlog_command(&args[2..]),
        Some("answer") if help() => usage(commands::ANSWER_USAGE),
        Some("answer") => commands::answer_command(&args[2..]),
        Some("usage") if help() => usage(commands::USAGE_USAGE),
        Some("usage") => commands::usage_command(&args[2..]),
        Some("merge") if help() => usage(commands::MERGE_USAGE),
        Some("merge") => commands::merge_command(&args[2..]),
        Some("close") if help() => usage(commands::CLOSE_USAGE),
        Some("close") => commands::close_command(&args[2..]),
        Some("pause") if help() => usage(commands::PAUSE_USAGE),
        Some("pause") => commands::pause_command(&args[2..]),
        Some("resume") if help() => usage(commands::RESUME_USAGE),
        Some("resume") => commands::resume_command(&args[2..]),
        Some("tag") if help() => usage(commands::TAG_USAGE),
        Some("tag") => commands::tag_command(&args[2..]),
        Some("role") if help() => usage(commands::ROLE_USAGE),
        Some("role") => commands::role_command(&args[2..]),
        Some("daemon") if help() => usage(commands::DAEMON_USAGE),
        Some("daemon") => commands::daemon_command(&args[2..]),
        Some("app-layout") if help() => usage(commands::APP_LAYOUT_USAGE),
        Some("app-layout") => commands::app_layout_command(&args[2..]),
        Some("projects") if help() => usage(commands::PROJECTS_USAGE),
        Some("projects") => commands::projects_command(&args[2..]),
        Some("open") if help() => usage(commands::OPEN_USAGE),
        Some("open") => commands::open_command(&args[2..]),
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
