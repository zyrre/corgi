use anyhow::Result;

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
        // `steward` is the command's name from before the Steward was
        // renamed Project handler, kept unlisted so existing scripts still work.
        Some("handler" | "steward") if help() => usage(corgi::app::HANDLER_USAGE),
        Some("handler" | "steward") => corgi::app::handler_command(&args[2..]),
        Some("fleet") if help() => usage(corgi::app::FLEET_USAGE),
        Some("fleet") => corgi::app::fleet(&args[2..]),
        Some("report") if help() => usage(corgi::app::REPORT_USAGE),
        Some("report") => corgi::app::report(&args[2..]),
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
        // Anything else, including no argument or `--compact`, starts the TUI.
        _ => corgi::app::run(),
    }
}
