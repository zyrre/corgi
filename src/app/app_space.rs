//! `corgi app-space`: makes or removes the Corgi space of the Herdr app
//! ([`corgi_core::app_space`]). `herdr-app/install.sh` runs it.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use corgi_core::app_space::{self, Ensured, HerdrApp, Removed};

pub const APP_SPACE_USAGE: &str = "\
Usage: corgi app-space [--app PATH] [--home DIR]
       corgi app-space remove [--app PATH] [--home DIR]

Makes the Corgi space in the Herdr app: a space named Corgi on this
computer, first in the Spaces list, whose only tab shows the Corgi mod. A
space named Corgi that is there already is left as it is, so running it again
makes no second one. When the app's window is not open, it notes the space
as pending, and `corgi daemon` makes it once the window opens.

  remove   closes the Corgi space's Corgi tab (the space goes with its last
           tab; terminals of your own in it stay) and forgets a pending space

  --app PATH  the herdr-app to ask (default: herdr-app on PATH)
  --home DIR  the app's home, which names its window (default:
              $HERDR_APP_HOME, else ~/.config/herdr-app)";

pub fn app_space_command(args: &[String]) -> Result<()> {
    let (remove, rest) = match args.first().map(String::as_str) {
        Some("remove") => (true, &args[1..]),
        _ => (false, args),
    };
    let mut bin = None;
    let mut home = None;
    let mut options = rest.iter();
    while let Some(option) = options.next() {
        let mut value = || {
            options
                .next()
                .map(PathBuf::from)
                .with_context(|| format!("{option} needs a value\n\n{APP_SPACE_USAGE}"))
        };
        match option.as_str() {
            "--app" => bin = Some(value()?),
            "--home" => home = Some(value()?),
            other => bail!("unknown argument {other:?}\n\n{APP_SPACE_USAGE}"),
        }
    }
    let home = match home {
        Some(home) => home,
        None => HerdrApp::default_home().context("no home folder to find the Herdr app's in")?,
    };
    let app = HerdrApp::new(bin.unwrap_or_else(|| PathBuf::from("herdr-app")), home);
    if remove {
        app_space::clear_pending();
        match app_space::remove(&app)? {
            Removed::Closed(0) => println!("no Corgi space to remove"),
            Removed::Closed(_) => println!("removed the Corgi space"),
            Removed::NoWindow => println!(
                "the Herdr app's window is not open: close the Corgi space yourself if it is there"
            ),
        }
        return Ok(());
    }
    match app_space::ensure(&app)? {
        Ensured::Found(id) => println!("the Corgi space is there ({id})"),
        Ensured::Made(id) => println!("made the Corgi space ({id}), first in the Spaces list"),
        Ensured::NoWindow => {
            app_space::set_pending(&app)?;
            println!(
                "the Herdr app's window is not open: corgi daemon makes the Corgi space once it is"
            );
        }
    }
    Ok(())
}
