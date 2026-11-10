//! `corgi app-layout`: puts Corgi's Projects list into the Herdr app's
//! layout, or takes it out again ([`corgi_core::app_layout`]).
//! `herdr-app/install.sh` runs it, and `--uninstall` runs `remove`.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde_json::json;

use corgi_core::{
    app_layout::{self, Change, Done, Installed, LAYOUT},
    app_space::HerdrApp,
};

pub const APP_LAYOUT_USAGE: &str = "\
Usage: corgi app-layout [--layout NAME] [--app PATH] [--home DIR] [--json]
       corgi app-layout remove [--layout NAME] [--home DIR]

Puts Corgi's Projects list into the Herdr app's sidebar, in place of Spaces,
and takes out the Agents list, which Projects shows too; the brand row at
the sidebar's top shows Corgi's (\"Herdr + Corgi\") in place of Herdr's.
Nothing else in the layout changes, so other mods' changes to it stay. The layout is the one the
window shows (or showed last), or NAME:

  - one of your own is edited in place;
  - one Herdr ships (main) is cloned to corgi first, as herdr-app do
    layout.clone does, and an open window is switched to the clone; with
    the window closed it says how to switch. A corgi layout that is there
    already is used instead.

Run again, it changes nothing: it only makes sure the Projects list is
there, and does not follow Herdr's later changes to main. A layout with no
Spaces or Projects list in its sidebar is left as it is, and why is printed.

  remove        puts Herdr's Spaces and Agents lists and its brand row back
                in each of your layouts that shows Corgi's (or in NAME); no
                layout is deleted and the window is not switched

  --layout NAME the layout to change, instead of the one the window shows
  --home DIR    the app's home (default: $HERDR_APP_HOME, else
                ~/.config/herdr-app)
  --app PATH    the herdr-app that answers for the window (default:
                herdr-app on PATH)
  --json        print {\"result\", \"layout\", \"path\", \"message\"}: result is
                edited, cloned, unchanged or refused (nothing changed); it
                exits 0 either way";

pub fn app_layout_command(args: &[String]) -> Result<()> {
    let (remove, rest) = match args.first().map(String::as_str) {
        Some("remove") => (true, &args[1..]),
        _ => (false, args),
    };
    let mut bin = None;
    let mut home = None;
    let mut layout = None;
    let mut json_output = false;
    let mut options = rest.iter();
    while let Some(option) = options.next() {
        let mut value = || {
            options
                .next()
                .cloned()
                .with_context(|| format!("{option} needs a value\n\n{APP_LAYOUT_USAGE}"))
        };
        match option.as_str() {
            "--app" if !remove => bin = Some(PathBuf::from(value()?)),
            "--home" => home = Some(PathBuf::from(value()?)),
            "--layout" => layout = Some(value()?),
            "--json" if !remove => json_output = true,
            other => bail!("unknown argument {other:?}\n\n{APP_LAYOUT_USAGE}"),
        }
    }
    let home = match home {
        Some(home) => home,
        None => HerdrApp::default_home().context("no home folder to find the Herdr app's in")?,
    };
    if remove {
        let restored = app_layout::restore(&home, layout.as_deref())?;
        println!("{}", removed_message(&restored));
        return Ok(());
    }
    let app = HerdrApp::new(bin.unwrap_or_else(|| PathBuf::from("herdr-app")), &home);
    let installed = app_layout::install(&app, layout.as_deref())?;
    let message = message(&installed);
    if json_output {
        let value = json!({
            "result": installed.word(),
            "layout": installed.layout(),
            "path": app_layout::layout_path(&home, installed.layout()),
            "message": message,
        });
        println!("{}", serde_json::to_string(&value)?);
    } else {
        println!("{message}");
    }
    Ok(())
}

fn switch_command(name: &str) -> String {
    format!("herdr-app do layout.switch '{{\"name\":\"{name}\"}}'")
}

/// What an install did, for a person to read.
fn message(installed: &Installed) -> String {
    let done = match installed {
        Installed::Refused { layout, why } => {
            return format!("left the layout {layout} as it was: {why}");
        }
        Installed::Done(done) => done,
    };
    let Done {
        layout,
        change,
        instead_of,
        cloned,
        shown,
        window,
    } = done;
    let what = match (instead_of, cloned, change) {
        (Some(from), true, _) => format!(
            "Herdr replaces its own {from} layout on its updates, so Corgi cloned it to \
             {layout} and put its Projects list in the sidebar"
        ),
        (Some(from), false, Change::Edited) => format!(
            "{from} is Herdr's own layout, so Corgi put its Projects list in the sidebar of \
             your layout {layout}"
        ),
        (Some(from), false, Change::Unchanged) => {
            format!("{from} is Herdr's own layout; your layout {layout} has Corgi's Projects list")
        }
        (None, _, Change::Edited) => format!(
            "put Corgi's Projects list in the sidebar of your layout {layout}, in place of \
             Spaces and Agents"
        ),
        (None, _, Change::Unchanged) => {
            format!("your layout {layout} has Corgi's Projects list already")
        }
    };
    let switch = switch_command(layout);
    match (shown, window, instead_of.is_some()) {
        (true, true, true) => format!("{what}; the window shows it"),
        (true, _, _) => what,
        (false, true, _) => format!("{what}; to show it, run: {switch}"),
        (false, false, _) => {
            format!("{what}; the Herdr app is closed: once it is open, show it with: {switch}")
        }
    }
}

/// What a remove did, for a person to read.
fn removed_message(restored: &[String]) -> String {
    if restored.is_empty() {
        return "no layout shows Corgi's Projects list".to_string();
    }
    let names = restored.join(", ");
    let kept = if restored.iter().any(|name| name == LAYOUT) {
        format!(
            "; the {LAYOUT} layout stays, as your own: delete layouts/{LAYOUT}.json if you \
             want it gone"
        )
    } else {
        String::new()
    };
    format!(
        "put Herdr's Spaces and Agents lists back in place of Corgi's Projects list in: {names}{kept}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(layout: &str, change: Change, instead_of: Option<&str>, cloned: bool) -> Done {
        Done {
            layout: layout.into(),
            change,
            instead_of: instead_of.map(str::to_string),
            cloned,
            shown: true,
            window: true,
        }
    }

    #[test]
    fn each_outcome_says_what_happened_and_how_to_show_the_layout() {
        let switch = r#"herdr-app do layout.switch '{"name":"corgi"}'"#;
        let cloned = done("corgi", Change::Edited, Some("main"), true);
        assert_eq!(
            message(&Installed::Done(cloned.clone())),
            "Herdr replaces its own main layout on its updates, so Corgi cloned it to corgi \
             and put its Projects list in the sidebar; the window shows it"
        );
        let closed = Done {
            shown: false,
            window: false,
            ..cloned
        };
        assert!(message(&Installed::Done(closed)).ends_with(&format!(
            "the Herdr app is closed: once it is open, show it with: {switch}"
        )));
        let mine = done("mine", Change::Edited, None, false);
        assert_eq!(
            message(&Installed::Done(mine.clone())),
            "put Corgi's Projects list in the sidebar of your layout mine, in place of Spaces \
             and Agents"
        );
        let other = Done {
            shown: false,
            ..mine
        };
        assert!(
            message(&Installed::Done(other))
                .ends_with(r#"; to show it, run: herdr-app do layout.switch '{"name":"mine"}'"#)
        );
        assert_eq!(
            message(&Installed::Done(done(
                "mine",
                Change::Unchanged,
                None,
                false
            ))),
            "your layout mine has Corgi's Projects list already"
        );
        assert_eq!(
            message(&Installed::Refused {
                layout: "main".into(),
                why: "no sidebar".into()
            }),
            "left the layout main as it was: no sidebar"
        );
    }

    #[test]
    fn a_remove_names_the_layouts_and_says_corgi_stays() {
        assert_eq!(
            removed_message(&[]),
            "no layout shows Corgi's Projects list"
        );
        let said = removed_message(&["corgi".into(), "mine".into()]);
        assert!(
            said.contains("in: corgi, mine; the corgi layout stays"),
            "{said}"
        );
        assert!(!removed_message(&["mine".into()]).contains("stays"));
    }
}
