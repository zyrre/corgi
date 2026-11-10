//! `corgi app-layout`: builds the Herdr app's Corgi layout from Herdr's
//! shipped `main`, or removes it ([`corgi_core::app_layout`]).
//! `herdr-app/install.sh` runs it, and the Corgi mod each time it starts.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde_json::json;

use corgi_core::{
    app_layout::{self, LAYOUT, Refreshed},
    app_space::HerdrApp,
};

pub const APP_LAYOUT_USAGE: &str = "\
Usage: corgi app-layout [--home DIR] [--json]
       corgi app-layout remove [--app PATH] [--home DIR]

Builds the Corgi layout of the Herdr app, layouts/corgi.json in the app's
home, which the app lists as Corgi: Herdr's shipped main layout with Corgi's
Projects mod in place of the sidebar's Spaces, and without its Agents list.
Run again, it writes the layout only when Herdr's main has changed since the
last build. A main whose sidebar is not the shape Corgi knows, or a home
without the Projects mod, builds nothing: the last build is kept, and why is
printed. It never switches the window to the layout; to do that, run

  herdr-app do layout.switch '{\"name\":\"corgi\"}'

and with \"main\" for Herdr's own again.

  remove      deletes the layout Corgi built; a window that shows it is
              switched to main first

  --home DIR  the app's home (default: $HERDR_APP_HOME, else
              ~/.config/herdr-app)
  --app PATH  the herdr-app to ask which layout shows (default: herdr-app on
              PATH)
  --json      print {\"result\", \"message\", \"layout\"}: result is made,
              rebuilt, unchanged, kept (not built, the last build stays) or
              missing (not built, and there is none); it exits 0 either way";

pub fn app_layout_command(args: &[String]) -> Result<()> {
    let (remove, rest) = match args.first().map(String::as_str) {
        Some("remove") => (true, &args[1..]),
        _ => (false, args),
    };
    let mut bin = None;
    let mut home = None;
    let mut json_output = false;
    let mut options = rest.iter();
    while let Some(option) = options.next() {
        let mut value = || {
            options
                .next()
                .map(PathBuf::from)
                .with_context(|| format!("{option} needs a value\n\n{APP_LAYOUT_USAGE}"))
        };
        match option.as_str() {
            "--app" => bin = Some(value()?),
            "--home" => home = Some(value()?),
            "--json" => json_output = true,
            other => bail!("unknown argument {other:?}\n\n{APP_LAYOUT_USAGE}"),
        }
    }
    let home = match home {
        Some(home) => home,
        None => HerdrApp::default_home().context("no home folder to find the Herdr app's in")?,
    };
    if remove {
        let app = HerdrApp::new(bin.unwrap_or_else(|| PathBuf::from("herdr-app")), home);
        if app_layout::remove_layout(&app)? {
            println!("removed the Corgi layout");
        } else {
            println!("no Corgi layout to remove");
        }
        return Ok(());
    }
    let refreshed = app_layout::refresh(&home);
    let message = message(&refreshed);
    if json_output {
        let value = json!({
            "result": refreshed.word(),
            "message": message,
            "layout": app_layout::layout_path(&home),
        });
        println!("{}", serde_json::to_string(&value)?);
    } else {
        println!("{message}");
    }
    Ok(())
}

/// What a refresh did, for a person to read.
fn message(refreshed: &Refreshed) -> String {
    let switch = format!("herdr-app do layout.switch '{{\"name\":\"{LAYOUT}\"}}'");
    match refreshed {
        Refreshed::Made => {
            format!("built the Corgi layout from Herdr's main; to show it, run: {switch}")
        }
        Refreshed::Rebuilt => {
            "rebuilt the Corgi layout: Herdr's main layout has changed".to_string()
        }
        Refreshed::Unchanged => {
            format!("the Corgi layout is up to date; to show it, run: {switch}")
        }
        Refreshed::Kept(why) => {
            format!("kept the Corgi layout as it was built last: {why}")
        }
        Refreshed::Missing(why) => format!("built no Corgi layout: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_outcome_says_what_happened_and_how_to_show_the_layout() {
        let switch = r#"herdr-app do layout.switch '{"name":"corgi"}'"#;
        assert!(message(&Refreshed::Made).ends_with(switch));
        assert!(message(&Refreshed::Unchanged).ends_with(switch));
        assert!(message(&Refreshed::Rebuilt).contains("main layout has changed"));
        assert_eq!(
            message(&Refreshed::Kept("no sidebar".into())),
            "kept the Corgi layout as it was built last: no sidebar"
        );
        assert_eq!(
            message(&Refreshed::Missing("no main".into())),
            "built no Corgi layout: no main"
        );
    }
}
