//! Mockups of the ways a project card could set its corgi apart from its
//! workers, drawn by the dashboard's own UI code from the README's made-up
//! herd, each in a dark and a light theme. Not part of the dashboard: run
//! `cargo test --lib mockups -- --ignored` and open `mockups/index.html`.

use std::{fmt::Write as _, fs, path::Path};

use crate::{
    model::{Activity, ActivityKind, AgentState, DashboardAgent},
    readme_shots::{Chrome, Palette, TOKYO_NIGHT, color, dashboard, set_palette, settle, svg},
    test_support::test_terminal,
    ui::{CardStyle, set_card_style},
};

const WIDTH: u16 = 112;
const HEIGHT: u16 = 52;
/// The width of the terminal-viewable copies, for a pager over SSH.
const ANSI_WIDTH: u16 = 100;

/// Tokyo Night Day, the light Tokyo Night.
const TOKYO_DAY: Palette = Palette {
    background: "#e1e2e7",
    foreground: "#3760bf",
    ansi: [
        "#e9e9ed", "#f52a65", "#587539", "#8c6c3e", "#2e7de9", "#9854f1", "#007197", "#6172b0",
        "#a1a6c5", "#f52a65", "#587539", "#8c6c3e", "#2e7de9", "#9854f1", "#007197", "#3760bf",
    ],
    edge: "#c4c8da",
    title: "#6172b0",
};

/// Every variant: its style, file name, title and one-line caption.
const VARIANTS: [(CardStyle, &str, &str, &str); 8] = [
    (
        CardStyle::Current,
        "0-current",
        "Current",
        "Today's card, for comparison: the corgi differs from its workers only by being on top.",
    ),
    (
        CardStyle::Band,
        "1-band",
        "Band",
        "A faint coat-orange band behind the corgi's rows that fades out to the right. Fixed RGB, so it breaks in the light theme.",
    ),
    (
        CardStyle::Rail,
        "2-rail",
        "Rail",
        "A coat-orange rail right before the corgi's badge on each of its rows; title in coat orange.",
    ),
    (
        CardStyle::Ember,
        "3-ember",
        "Ember frame",
        "The whole card border in coat orange; worker text muted, only badges and anything blocked keep colour.",
    ),
    (
        CardStyle::Tree,
        "4-tree",
        "Tree",
        "Workers hang under the corgi as a tree (├─ └─), the corgi flush left; the collapsed sum is a branch too.",
    ),
    (
        CardStyle::CoatBadge,
        "5-coat-badge",
        "Coat badge",
        "The corgi's state badge becomes a ▐CORGI▌ pill in coat colours with an icon; its state follows as a mark.",
    ),
    (
        CardStyle::Heavy,
        "6-heavy",
        "Heavy section",
        "A heavy coat-orange border round the corgi's section, a light muted one round the workers.",
    ),
    (
        CardStyle::Lead,
        "7-lead",
        "Lead panel",
        "Combination: corgi on the theme's panel tone with a coat tab, icon in the title, workers as a dimmed tree. Adapts to both themes.",
    ),
];

/// The README herd with a third corgi project and a scratch session: the
/// webshop card collapsed with its blocked worker, the weather card expanded
/// with a worker selected, the notes card collapsed, the scratch rows below.
fn herd_app() -> crate::app::App {
    let mut app = dashboard();
    let weather_corgi = app
        .agents
        .iter()
        .find(|agent| agent.corgi && agent.project_group == "weather")
        .cloned()
        .expect("weather corgi");
    let notes_corgi = DashboardAgent {
        project_group: "notes-app".into(),
        project: "notes-app".into(),
        info: crate::model::AgentInfo {
            state: AgentState::Idle,
            ..weather_corgi.info.clone()
        },
        message: Activity {
            kind: ActivityKind::Message,
            text: "md-export is done; I asked whether the tags should go into front matter too."
                .into(),
        },
        tool: Activity {
            kind: ActivityKind::Command,
            text: "corgi fleet notes-app".into(),
        },
        context_percent: Some(18),
        ..weather_corgi
    };
    let at = app
        .agents
        .iter()
        .position(|agent| agent.project_group == "notes-app")
        .expect("notes-app");
    app.agents.insert(at, notes_corgi);
    let mut scratch = app.agents[at + 1].clone();
    scratch.project_group = "Scratch".into();
    scratch.scratch = true;
    scratch.task = "Look up the ratatui 0.30 changelog".into();
    scratch.worktree_label = None;
    scratch.info.state = AgentState::Done;
    scratch.message = Activity {
        kind: ActivityKind::Message,
        text: "0.30 splits the crate into ratatui-core and widgets; nothing we use moved.".into(),
    };
    scratch.tool = Activity {
        kind: ActivityKind::Tool,
        text: "WebFetch github.com/ratatui/ratatui/releases".into(),
    };
    app.agents.push(scratch);
    app.cards.set_expanded("webshop", false);
    app.cards.set_expanded("weather", true);
    app.selected = app
        .agents
        .iter()
        .position(|agent| agent.task == "Hourly forecast chart")
        .expect("hourly chart");
    app.status = format!("{} agents", app.agents.len());
    app
}

fn render(style: CardStyle, palette: &'static Palette) -> String {
    set_card_style(style);
    set_palette(palette);
    let mut app = herd_app();
    let mut terminal = test_terminal(WIDTH, HEIGHT);
    settle(&mut terminal, &mut app);
    let area = terminal.backend().buffer().area;
    let out = svg(&mut terminal, area, Chrome::Window, &[]);
    set_card_style(CardStyle::Current);
    set_palette(&TOKYO_NIGHT);
    out
}

fn index() -> String {
    let mut html = String::from(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Corgi card mockups</title>
<style>
body { font-family: -apple-system, 'Segoe UI', Helvetica, Arial, sans-serif; background: #f4f4f6; color: #222; margin: 24px; }
h1 { font-size: 20px; } h2 { font-size: 16px; margin: 32px 0 4px; }
p.caption { margin: 0 0 10px; color: #555; }
.pair { display: flex; gap: 16px; overflow-x: auto; }
.pair img { width: 49%; min-width: 560px; height: auto; }
</style></head><body>
<h1>Making each project's corgi stand out: card mockups</h1>
<p>Each variant is drawn by the dashboard's own code from made-up agents, Tokyo Night (dark) on the left and Tokyo Night Day (light) on the right. webshop is collapsed with a blocked worker, weather is expanded with a worker selected, notes-app is collapsed, Scratch is below. See NOTES.md for the ranking.</p>
"#,
    );
    for (_, file, title, caption) in VARIANTS {
        let _ = write!(
            html,
            "<h2>{title}</h2>\n<p class=\"caption\">{caption}</p>\n<div class=\"pair\"><img src=\"{file}-dark.svg\" alt=\"{title}, dark\"><img src=\"{file}-light.svg\" alt=\"{title}, light\"></div>\n"
        );
    }
    html.push_str("</body></html>\n");
    html
}

#[test]
#[ignore = "writes the card mockups to mockups/"]
fn card_mockups() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("mockups");
    fs::create_dir_all(&directory).expect("create mockups");
    for (style, file, _, _) in VARIANTS {
        for (theme, palette) in [("dark", &TOKYO_NIGHT), ("light", &TOKYO_DAY)] {
            let path = directory.join(format!("{file}-{theme}.svg"));
            let svg = render(style, palette);
            if let Some(png_directory) = std::env::var_os("MOCKUP_PNG_DIR") {
                let png = Path::new(&png_directory).join(format!("{file}-{theme}.png"));
                fs::write(png, rasterize(&svg)).expect("write png");
            }
            fs::write(&path, svg).expect("write mockup");
            println!("wrote {}", path.display());
        }
    }
    fs::write(directory.join("index.html"), index()).expect("write index");
    let ansi_directory = directory.join("ansi");
    fs::create_dir_all(&ansi_directory).expect("create mockups/ansi");
    for (style, file, title, caption) in VARIANTS {
        for (theme, palette) in [("dark", &TOKYO_NIGHT), ("light", &TOKYO_DAY)] {
            let suffix = if theme == "dark" { "" } else { "-light" };
            let path = ansi_directory.join(format!("{file}{suffix}.ans"));
            let mut out = format!("\x1b[1m{title} ({theme})\x1b[0m: {caption}\n");
            out.push_str(&render_ansi(style, palette));
            fs::write(&path, out).expect("write ansi mockup");
        }
    }
}

/// `svg` as a PNG, for looking at a mockup without a browser.
fn rasterize(svg: &str) -> Vec<u8> {
    use resvg::{tiny_skia, usvg};
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_str(svg, &options).expect("parse mockup");
    let size = tree.size().to_int_size();
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height()).expect("pixmap");
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    pixmap.encode_png().expect("encode png")
}

/// The dashboard in `style` and `palette`, `ANSI_WIDTH` columns wide, as
/// lines of true-colour SGR escapes a pager such as `less -R` shows.
fn render_ansi(style: CardStyle, palette: &'static Palette) -> String {
    use ratatui::style::Modifier;
    set_card_style(style);
    set_palette(palette);
    let mut app = herd_app();
    let mut terminal = test_terminal(ANSI_WIDTH, HEIGHT);
    settle(&mut terminal, &mut app);
    let buffer = terminal.backend().buffer().clone();
    set_card_style(CardStyle::Current);
    set_palette(&TOKYO_NIGHT);
    let rgb = |hex: &str| {
        let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or(0);
        format!("{};{};{}", channel(1), channel(3), channel(5))
    };
    let mut out = String::new();
    for y in 0..buffer.area.height {
        let mut last = String::new();
        // The cells a wide glyph covers after its own.
        let mut covered = 0;
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            if covered > 0 {
                covered -= 1;
                continue;
            }
            covered = unicode_width::UnicodeWidthStr::width(cell.symbol()).saturating_sub(1);
            let reversed = cell.modifier.contains(Modifier::REVERSED);
            let (fg, bg) = if reversed {
                (cell.bg, cell.fg)
            } else {
                (cell.fg, cell.bg)
            };
            let mut sgr = format!(
                "\x1b[0;38;2;{};48;2;{}",
                rgb(&color(fg, true)),
                rgb(&color(bg, false))
            );
            if cell.modifier.contains(Modifier::BOLD) {
                sgr.push_str(";1");
            }
            if cell.modifier.contains(Modifier::DIM) {
                sgr.push_str(";2");
            }
            sgr.push('m');
            if sgr != last {
                out.push_str(&sgr);
                last = sgr;
            }
            out.push_str(cell.symbol());
        }
        out.push_str("\x1b[0m\n");
    }
    out
}
