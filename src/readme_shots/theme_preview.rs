//! A preview of the dashboard and its popups in several themes, written as
//! ANSI text to read in a terminal with `less -R`, for checking a change to
//! the theme without a light terminal or an Omarchy machine at hand.
//!
//! Run it with
//! `CORGI_PREVIEW_DIR=<dir> cargo test --lib theme_preview -- --ignored`.
//! Each theme is drawn in its own RGB over its own background, so it reads
//! the same in any truecolor terminal. With `CORGI_PREVIEW_OSC` naming a
//! file of a terminal's answers to the color queries, one more preview is
//! drawn the way Corgi draws in that terminal: its roles left to the
//! terminal's own ANSI colors, and its shades blended from the answers.

use std::{fmt::Write as _, fs, path::PathBuf};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
};
use unicode_width::UnicodeWidthStr;

use super::{
    HEIGHT, HERO_HEIGHT, WIDTH, dashboard, draw_at, merge, new_agent_form, popup_area, screen,
    settle,
};
use corgi_core::palette::resolve;

use crate::{
    app::MergePhase,
    motion::{STAMP_DRAW, STAMP_HOLD},
    test_support::test_terminal,
    theme::{self, Palette, Source, Theme},
};

/// How a preview's cells become escape codes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Paint {
    /// Every color as the theme's RGB, the default ones as its foreground
    /// and background.
    Simulated,
    /// The ANSI colors and defaults left to the terminal showing the preview.
    Native,
}

/// Every view of the dashboard a preview shows: a title and the cells.
fn views() -> Vec<(&'static str, Buffer)> {
    let mut views = Vec::new();

    let mut app = dashboard();
    app.cards.set_expanded("webshop", false);
    app.status = format!("{} agents", app.agents.len());
    let mut terminal = test_terminal(WIDTH, HERO_HEIGHT);
    settle(&mut terminal, &mut app);
    views.push(("dashboard", terminal.backend().buffer().clone()));

    let terminal = screen(&mut new_agent_form());
    let buffer = terminal.backend().buffer();
    views.push((
        "new agent form",
        crop(buffer, popup_area(buffer, "◆ New agent")),
    ));

    let terminal = screen(&mut merge(MergePhase::Confirm));
    let buffer = terminal.backend().buffer();
    let area = popup_area(buffer, "Merge reviewed work");
    views.push(("merge popup, dashboard dimmed behind", crop(buffer, area)));

    let mut app = merge(MergePhase::Running(2));
    let mut terminal = test_terminal(WIDTH, HEIGHT);
    settle(&mut terminal, &mut app);
    if let Some(form) = app.overlay.merge_worktree_form_mut() {
        form.phase = MergePhase::Succeeded;
    }
    draw_at(&mut terminal, &mut app, 3_000);
    let stamped = 3_000 + (STAMP_DRAW + STAMP_HOLD / 2).as_millis() as u64;
    draw_at(&mut terminal, &mut app, stamped);
    let buffer = terminal.backend().buffer();
    views.push((
        "merge succeeded: green frame and check stamp",
        crop(buffer, popup_area(buffer, "Merged and pushed")),
    ));

    let mut app = merge(MergePhase::Running(2));
    let mut terminal = test_terminal(WIDTH, HEIGHT);
    settle(&mut terminal, &mut app);
    if let Some(form) = app.overlay.merge_worktree_form_mut() {
        form.phase = MergePhase::Failed("git push: the remote rejected the branch".into());
    }
    draw_at(&mut terminal, &mut app, 3_000);
    draw_at(&mut terminal, &mut app, 3_120);
    let buffer = terminal.backend().buffer();
    let area = popup_area(buffer, "Merge/push error");
    views.push(("merge failed, mid flash", crop(buffer, area)));
    draw_at(&mut terminal, &mut app, 5_000);
    views.push((
        "merge failed, settled",
        crop(terminal.backend().buffer(), area),
    ));
    views
}

fn crop(buffer: &Buffer, area: Rect) -> Buffer {
    let mut cropped = Buffer::empty(Rect::new(0, 0, area.width, area.height));
    for y in 0..area.height {
        for x in 0..area.width {
            cropped[(x, y)] = buffer[(area.x + x, area.y + y)].clone();
        }
    }
    cropped
}

/// The preview of every view in `theme`, under a heading naming it.
fn preview(name: &str, theme: Theme, paint: Paint) -> String {
    let mut out = String::new();
    let views = theme::scoped(theme, views);
    for (title, buffer) in views {
        let _ = writeln!(out, "\x1b[0m\x1b[1m── {name} · {title} ──\x1b[0m");
        out.push_str(&ansi(&buffer, &theme, paint));
        out.push('\n');
    }
    out
}

/// The cells of `buffer` as lines of text with SGR escape codes.
fn ansi(buffer: &Buffer, theme: &Theme, paint: Paint) -> String {
    let mut out = String::new();
    let area = buffer.area;
    for y in area.top()..area.bottom() {
        let mut skip = 0;
        let mut last = String::new();
        for x in area.left()..area.right() {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let cell = &buffer[(x, y)];
            let mut sgr = vec!["0".to_string()];
            for (modifier, code) in [
                (Modifier::BOLD, "1"),
                (Modifier::DIM, "2"),
                (Modifier::ITALIC, "3"),
                (Modifier::UNDERLINED, "4"),
                (Modifier::REVERSED, "7"),
            ] {
                if cell.modifier.contains(modifier) {
                    sgr.push(code.into());
                }
            }
            sgr.push(color(cell.fg, true, theme, paint));
            sgr.push(color(cell.bg, false, theme, paint));
            let sgr = format!("\x1b[{}m", sgr.join(";"));
            if sgr != last {
                out.push_str(&sgr);
                last = sgr;
            }
            let symbol = cell.symbol();
            out.push_str(if symbol.is_empty() { " " } else { symbol });
            skip = symbol.width().saturating_sub(1);
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

fn color(color: Color, foreground: bool, theme: &Theme, paint: Paint) -> String {
    let base = if foreground { 30 } else { 40 };
    if paint == Paint::Native {
        let ansi_named = |index: u8| {
            if index < 8 {
                format!("{}", base + index)
            } else {
                format!("{}", base + 60 + index - 8)
            }
        };
        match color {
            Color::Reset => return format!("{}", base + 9),
            Color::Indexed(index @ 0..=15) => return ansi_named(index),
            Color::Indexed(index) => return format!("{};5;{index}", base + 8),
            Color::Rgb(r, g, b) => return format!("{};2;{r};{g};{b}", base + 8),
            named => {
                // The sixteen by name, numbered in ANSI's order.
                let order = [
                    Color::Black,
                    Color::Red,
                    Color::Green,
                    Color::Yellow,
                    Color::Blue,
                    Color::Magenta,
                    Color::Cyan,
                    Color::Gray,
                    Color::DarkGray,
                    Color::LightRed,
                    Color::LightGreen,
                    Color::LightYellow,
                    Color::LightBlue,
                    Color::LightMagenta,
                    Color::LightCyan,
                    Color::White,
                ];
                let index = order.iter().position(|name| *name == named).unwrap_or(0);
                return ansi_named(index as u8);
            }
        }
    }
    let palette = theme.palette;
    let default = if foreground {
        palette.foreground
    } else {
        palette.background
    };
    let (r, g, b) = theme.rgb(color).unwrap_or(default);
    format!("{};2;{r};{g};{b}", base + 8)
}

#[test]
#[ignore = "writes the theme preview; run it with CORGI_PREVIEW_DIR set"]
fn theme_preview() {
    let directory = PathBuf::from(std::env::var("CORGI_PREVIEW_DIR").expect("CORGI_PREVIEW_DIR"));
    fs::create_dir_all(&directory).expect("create the preview directory");
    let tokyo = resolve::parse_alacritty(include_str!(
        "../../crates/corgi-core/src/palette/fixtures/omarchy-tokyo-night-alacritty.toml"
    ))
    .expect("Tokyo Night");
    let mut previews = Vec::new();
    if let Ok(path) = std::env::var("CORGI_PREVIEW_OSC") {
        let reply = resolve::parse_reply(&fs::read(&path).expect("the terminal's answers"));
        let palette = reply.palette().expect("a whole palette");
        previews.push((
            "1-terminal",
            "This terminal's own palette, read by OSC through Herdr",
            Theme::new(palette, Source::Terminal, true, true),
            Paint::Native,
        ));
    }
    previews.extend([
        (
            "2-catppuccin-mocha",
            "Catppuccin Mocha (the dark fallback)",
            Theme::catppuccin(false, true),
            Paint::Simulated,
        ),
        (
            "3-catppuccin-latte",
            "Catppuccin Latte (the light fallback)",
            Theme::catppuccin(true, true),
            Paint::Simulated,
        ),
        (
            "4-omarchy-tokyo-night",
            "Omarchy Tokyo Night",
            Theme::new(tokyo, Source::Omarchy, true, true),
            Paint::Simulated,
        ),
        (
            "5-xterm",
            "xterm's defaults, snapped to 256 colors (as before this change)",
            Theme::new(Palette::XTERM, Source::Assumed, true, false),
            Paint::Simulated,
        ),
    ]);
    for (file, name, theme, paint) in previews {
        let path = directory.join(format!("{file}.ans"));
        fs::write(&path, preview(name, theme, paint)).expect("write the preview");
        println!("wrote {}", path.display());
    }
}
