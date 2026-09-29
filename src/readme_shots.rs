//! The screenshots in the README, drawn by the dashboard's own UI code into
//! a test terminal and written out as SVG terminal windows. Every agent,
//! project and path in them is made up here, so no Herdr session and nothing
//! of the machine they are drawn on shows in them.
//!
//! Regenerate them with `cargo readme-shots`, the alias for running the
//! ignored test at the bottom of this file; they land in `docs/images/`.

use std::{fmt::Write as _, fs, path::Path, sync::Arc};

use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Modifier},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{App, Checkout, NewAgentForm, NewField, Overlay, UsageSlot},
    defaults::HarnessDefaults,
    model::PromptCacheKind,
    model::{Activity, ActivityKind, AgentInfo, AgentState, DashboardAgent, PromptCache},
    motion::Motion,
    test_support::{test_app, test_terminal},
    time::unix_now,
    ui::draw,
    usage::{PlanUsage, Provider, UsageWindow},
};

// One terminal cell in SVG pixels. The font size is chosen so a common
// monospace face advances by about the cell width, and every glyph is placed
// at its own cell anyway, so a face that runs a little wider or narrower
// still keeps the columns in line. A cell is twice as tall as it is wide, so
// the half-block pixels of the corgi and the wordmark come out square.
const CELL_WIDTH: u32 = 9;
const CELL_HEIGHT: u32 = 18;
const FONT_SIZE: u32 = 15;
// Where the baseline sits in a cell, from its top.
const BASELINE: u32 = 13;
const FONT_FAMILY: &str = "ui-monospace, SFMono-Regular, Menlo, Consolas, 'DejaVu Sans Mono', 'Liberation Mono', monospace";
// The window drawn around a screenshot: its padding, title bar and corners.
const PADDING: u32 = 14;
const TITLE_BAR: u32 = 30;
const WINDOW_RADIUS: u32 = 10;
const WINDOW_EDGE: &str = "#2a2e42";
const TITLE_TEXT: &str = "#787c99";

// Corgi draws in the terminal's own ANSI colors, so a screenshot has to pick
// a theme. This is Tokyo Night, Omarchy's default; the default foreground and
// background are the theme's, and the fixed 256-color cube and grey ramp are
// xterm's, as every terminal has them.
const BACKGROUND: &str = "#1a1b26";
const FOREGROUND: &str = "#a9b1d6";
const ANSI: [&str; 16] = [
    "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
    "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5",
];
const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The dashboard's size in the hero screenshot and the ones like it: wide
/// enough for the header's whole band of cards, and tall enough for three
/// projects.
const WIDTH: u16 = 112;
const HEIGHT: u16 = 44;

/// A screenshot: the file it is written to, the window title, and the cells
/// of the rendered screen it shows.
struct Shot {
    name: &'static str,
    title: Option<&'static str>,
    svg: String,
}

/// Every screenshot, in the order the README shows them.
fn shots() -> Vec<Shot> {
    vec![
        logo_shot(),
        Shot {
            name: "dashboard",
            title: Some("corgi — herdr"),
            svg: String::new(),
        }
        .drawn(&mut dashboard()),
        Shot {
            name: "expanded-session",
            title: Some("corgi — herdr"),
            svg: String::new(),
        }
        .drawn(&mut expanded_session()),
        Shot {
            name: "new-agent-form",
            title: Some("corgi — herdr"),
            svg: String::new(),
        }
        .drawn(&mut new_agent_form()),
    ]
}

impl Shot {
    /// The shot with `app` drawn into it as a whole screen, popups and their
    /// effects played out to where they rest.
    fn drawn(mut self, app: &mut App) -> Self {
        let mut terminal = test_terminal(WIDTH, HEIGHT);
        settle(&mut terminal, app);
        let area = terminal.backend().buffer().area;
        self.svg = svg(&mut terminal, area, self.title);
        self
    }
}

/// Draws `app` on a manual clock until its popup, if it has one, has grown
/// in and the dashboard behind it has dimmed. A key was just pressed before
/// the last frame, so a text field's caret is in its on phase.
fn settle(terminal: &mut Terminal<TestBackend>, app: &mut App) {
    for millis in [0, 1_000, 3_000] {
        app.motion
            .set_time(std::time::Duration::from_millis(millis));
        if millis == 3_000 {
            app.motion.key_pressed();
        }
        terminal.draw(|frame| draw(frame, app)).expect("draw shot");
    }
}

/// The corgi and the wordmark on their own, cut from the header with no
/// window around them, for the top of the README.
fn logo_shot() -> Shot {
    let mut app = dashboard();
    let mut terminal = test_terminal(WIDTH, HEIGHT);
    settle(&mut terminal, &mut app);
    // The mascot is sixteen cells wide, then two of gap and the 38-cell
    // wordmark; the header is seven rows tall.
    Shot {
        name: "logo",
        title: None,
        svg: svg(&mut terminal, Rect::new(0, 0, 16 + 2 + 38, 7), None),
    }
}

// ---------------------------------------------------------------------------
// The made-up herd.

/// A made-up home directory, for the paths the screenshots show.
const HOME: &str = "/home/jane.doe";

/// One made-up agent: where it works, what state it is in, and what it said
/// and ran last.
struct Agent {
    project: &'static str,
    kind: &'static str,
    state: AgentState,
    task: &'static str,
    model: &'static str,
    effort: Option<&'static str>,
    context: u8,
    /// Seconds the prompt cache stays warm, negative once it went cold.
    cache: Option<(i64, PromptCacheKind, u64)>,
    worktree: Option<&'static str>,
    message: (ActivityKind, &'static str),
    tool: (ActivityKind, &'static str),
    steward: bool,
}

impl Agent {
    fn dashboard_agent(&self, now: u64) -> DashboardAgent {
        let project_root = format!("{HOME}/repos/{}", self.project);
        DashboardAgent {
            info: AgentInfo {
                agent: Some(self.kind.into()),
                state: self.state,
                cwd: Some(project_root.clone()),
                ..AgentInfo::default()
            },
            project_group: self.project.into(),
            project: self.project.into(),
            project_root,
            worktree_label: self.worktree.map(str::to_string),
            task: if self.steward { "Steward" } else { self.task }.into(),
            model: Some(self.model.into()),
            effort: self.effort.map(str::to_string),
            context_percent: Some(self.context),
            cache: self.cache.map(|(seconds, kind, tokens)| PromptCache {
                expires_at: now.saturating_add_signed(seconds),
                tokens,
                kind,
            }),
            message: activity(self.message),
            tool: activity(self.tool),
            steward: self.steward,
            ..DashboardAgent::default()
        }
    }
}

fn activity((kind, text): (ActivityKind, &str)) -> Activity {
    Activity {
        kind,
        text: text.into(),
    }
}

/// The herd every dashboard screenshot shows, already in the dashboard's
/// order: each project's Steward first, then blocked, working, done and idle.
/// Every countdown sits in the middle of its minute, so a second passing
/// while the shot is drawn never changes what it reads.
fn herd() -> Vec<Agent> {
    use ActivityKind::*;
    use PromptCacheKind::*;
    vec![
        Agent {
            project: "webshop",
            kind: "claude",
            state: AgentState::Idle,
            task: "",
            model: "Opus 5",
            effort: Some("high"),
            context: 34,
            cache: Some((52 * 60 + 30, Exact, 68_000)),
            worktree: None,
            message: (
                Message,
                "Two workers are on it: checkout-retry and cart-badge. I'll review each as it reports.",
            ),
            tool: (Command, "corgi fleet webshop"),
            steward: true,
        },
        Agent {
            project: "webshop",
            kind: "claude",
            state: AgentState::Blocked,
            task: "Retry failed card payments",
            model: "Sonnet 5",
            effort: Some("medium"),
            context: 41,
            cache: Some((3 * 60 + 30, Exact, 82_000)),
            worktree: Some("checkout-retry-4b1e"),
            message: (Question, "Allow Bash: npm run test:e2e -- checkout?"),
            tool: (Command, "npm run test:e2e -- checkout"),
            steward: false,
        },
        Agent {
            project: "webshop",
            kind: "codex",
            state: AgentState::Working,
            task: "Cart badge shows the item count",
            model: "gpt-5.6",
            effort: Some("high"),
            context: 22,
            cache: Some((18 * 60 + 30, Estimated, 44_000)),
            worktree: Some("cart-badge-9c07"),
            message: (
                Thinking,
                "The badge re-renders on every cart event; debouncing the store subscription should stop the flicker.",
            ),
            tool: (Tool, "Edit src/components/CartBadge.tsx"),
            steward: false,
        },
        Agent {
            project: "webshop",
            kind: "claude",
            state: AgentState::Done,
            task: "Paginate the order history",
            model: "Opus 5",
            effort: Some("high"),
            context: 67,
            cache: Some((-9 * 60 - 30, Exact, 182_000)),
            worktree: Some("order-pages-2d5a"),
            message: (
                Message,
                "Pagination is in: 20 orders a page with a stable cursor, and tests for the first and last page.",
            ),
            tool: (Command, "pnpm test -- orders"),
            steward: false,
        },
        Agent {
            project: "weather",
            kind: "codex",
            state: AgentState::Working,
            task: "Hourly forecast chart",
            model: "gpt-5.6",
            effort: Some("medium"),
            context: 58,
            cache: Some((24 * 60 + 30, Estimated, 96_000)),
            worktree: Some("hourly-chart-71fa"),
            message: (
                Prompt,
                "Use the same colours as the daily view, and keep it readable at 320px.",
            ),
            tool: (Command, "npm run storybook -- --smoke-test"),
            steward: false,
        },
        Agent {
            project: "weather",
            kind: "claude",
            state: AgentState::Done,
            task: "Cache API responses for ten minutes",
            model: "Sonnet 5",
            effort: None,
            context: 29,
            cache: Some((41 * 60 + 30, Exact, 51_000)),
            worktree: Some("api-cache-e3b8"),
            message: (
                Message,
                "Responses are cached for ten minutes, and a stale entry is refreshed in the background.",
            ),
            tool: (Tool, "Read src/api/client.ts"),
            steward: false,
        },
        Agent {
            project: "notes-app",
            kind: "claude",
            state: AgentState::Idle,
            task: "Export notes as Markdown",
            model: "Opus 5",
            effort: Some("low"),
            context: 12,
            cache: None,
            worktree: Some("md-export-5a60"),
            message: (
                Message,
                "Export is done. Want front matter with the tags as well?",
            ),
            tool: (Command, "git log --oneline -3"),
            steward: false,
        },
    ]
}

/// A plan-usage card read half a minute ago.
fn usage_slot(provider: Provider, plan: &str, windows: [(u8, u64); 2], now: u64) -> UsageSlot {
    let window = |(used_percent, resets_in): (u8, u64)| UsageWindow {
        used_percent,
        resets_at: now + resets_in,
    };
    UsageSlot {
        provider,
        usage: Some(PlanUsage {
            provider,
            plan_type: plan.into(),
            five_hour: Some(window(windows[0])),
            week: Some(window(windows[1])),
            banked_resets: None,
        }),
        fetched_at: Some(now - 30),
        error: None,
    }
}

/// The dashboard over the whole herd, the blocked agent selected.
fn dashboard() -> App {
    let now = unix_now();
    let mut app = test_app();
    app.motion = Motion::manual();
    app.agents = herd()
        .iter()
        .map(|agent| agent.dashboard_agent(now))
        .collect();
    app.selected = 1;
    app.status = format!("{} agents", app.agents.len());
    let minutes = |minutes: u64| minutes * 60 + 30;
    app.usage = vec![
        usage_slot(
            Provider::Codex,
            "Pro",
            [
                (38, minutes(2 * 60 + 14)),
                (61, minutes(3 * 24 * 60 + 7 * 60)),
            ],
            now,
        ),
        usage_slot(
            Provider::Claude,
            "Max",
            [(72, minutes(65)), (44, minutes(4 * 24 * 60 + 12 * 60))],
            now,
        ),
    ];
    app
}

/// The finished order-history worker opened with `space`: its latest turns,
/// newest first, grow down from its row.
fn expanded_session() -> App {
    use ActivityKind::*;
    let mut app = dashboard();
    app.selected = 3;
    app.expanded = true;
    app.transcript = Arc::from(vec![
        activity((
            Message,
            "Pagination is in. The order history loads 20 orders at a time from a cursor on \
             (created_at, id), so a new order never shifts a page under the reader.\n\
             \n\
             - GET /orders?after=<cursor> returns the next page and its own cursor\n\
             - The page shows Load more until the last order is on screen\n\
             - Tests cover the first page, the last page and an empty history",
        )),
        activity((Command, "pnpm test -- orders")),
        activity((Tool, "Edit src/pages/OrderHistory.tsx")),
        activity((Tool, "Edit src/api/orders.ts")),
        activity((
            Thinking,
            "The API already sorts by created_at, so a cursor on (created_at, id) keeps the \
             pages stable while new orders arrive, which an offset would not.",
        )),
        activity((Command, "rg -n \"orders\" src/api")),
        activity((
            Prompt,
            "Paginate the order history page, 20 orders at a time. Keep the URL shareable.",
        )),
    ]);
    app
}

/// The new-agent form opened with `n` on the webshop Steward, the task typed
/// and every other row on its preset.
fn new_agent_form() -> App {
    let mut app = dashboard();
    app.selected = 0;
    let task = "Show a toast when an item is added to the cart, with an undo button.";
    app.overlay = Overlay::NewAgent(NewAgentForm {
        field: NewField::Task,
        list: None,
        error: None,
        kind: "claude".into(),
        model: String::new(),
        effort: String::new(),
        defaults: HarnessDefaults {
            model: Some("opus".into()),
            effort: Some("high".into()),
        },
        project: format!("{HOME}/repos/webshop"),
        new_project: false,
        checkout: Checkout::Worktree,
        prompt: task.into(),
        prompt_caret: task.len(),
        prompt_width: 0,
    });
    app
}

// ---------------------------------------------------------------------------
// Cells to SVG.

/// The `area` of the terminal's screen as an SVG image: a window with a
/// title bar around it when `title` is given, else the cells alone on a
/// transparent background.
fn svg(terminal: &mut Terminal<TestBackend>, area: Rect, title: Option<&str>) -> String {
    let cursor = terminal
        .backend()
        .cursor_visible()
        .then(|| terminal.get_cursor_position().ok())
        .flatten();
    let buffer = terminal.backend().buffer();
    let (width, height) = (
        u32::from(area.width) * CELL_WIDTH,
        u32::from(area.height) * CELL_HEIGHT,
    );
    let (left, top, outer_width, outer_height) = if title.is_some() {
        (
            PADDING,
            TITLE_BAR + PADDING / 2,
            width + 2 * PADDING,
            height + TITLE_BAR + PADDING / 2 + PADDING,
        )
    } else {
        (0, 0, width, height)
    };
    let mut out = String::new();
    let _ = writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{outer_width}" height="{outer_height}" viewBox="0 0 {outer_width} {outer_height}">"#
    );
    if let Some(title) = title {
        let _ = writeln!(
            out,
            r#"<rect x="0.5" y="0.5" width="{}" height="{}" rx="{WINDOW_RADIUS}" fill="{BACKGROUND}" stroke="{WINDOW_EDGE}"/>"#,
            outer_width - 1,
            outer_height - 1
        );
        for (index, color) in ["#ff5f57", "#febc2e", "#28c840"].iter().enumerate() {
            let _ = writeln!(
                out,
                r#"<circle cx="{}" cy="{}" r="6" fill="{color}"/>"#,
                PADDING + 6 + index as u32 * 20,
                TITLE_BAR / 2 + 1
            );
        }
        let _ = writeln!(
            out,
            r#"<text x="{}" y="{}" fill="{TITLE_TEXT}" font-family="{FONT_FAMILY}" font-size="13" text-anchor="middle">{}</text>"#,
            outer_width / 2,
            TITLE_BAR / 2 + 5,
            escape(title)
        );
    }
    let _ = writeln!(
        out,
        r#"<g transform="translate({left} {top})" font-family="{FONT_FAMILY}" font-size="{FONT_SIZE}">"#
    );
    for y in area.top()..area.bottom() {
        row_svg(&mut out, buffer, area, y);
    }
    if let Some(Position { x, y }) = cursor
        && area.contains(Position { x, y })
    {
        let _ = writeln!(
            out,
            r#"<rect x="{}" y="{}" width="2" height="{CELL_HEIGHT}" fill="{FOREGROUND}"/>"#,
            u32::from(x - area.x) * CELL_WIDTH,
            u32::from(y - area.y) * CELL_HEIGHT
        );
    }
    out.push_str("</g>\n</svg>\n");
    out
}

/// How one cell is drawn, once its colors are resolved.
#[derive(Clone, PartialEq)]
struct Look {
    fg: String,
    bold: bool,
    italic: bool,
    underline: bool,
    dim: bool,
}

/// One row of cells: the backgrounds that differ from the window's as
/// rectangles, the block and box-drawing characters as shapes so they meet
/// their neighbours without the seams a font leaves, and the rest as text,
/// each glyph at its own cell.
fn row_svg(out: &mut String, buffer: &Buffer, area: Rect, y: u16) {
    let row_top = u32::from(y - area.y) * CELL_HEIGHT;
    let column = |x: u16| u32::from(x - area.x) * CELL_WIDTH;
    let looks: Vec<(String, Look, String)> = (area.left()..area.right())
        .map(|x| {
            let cell = &buffer[(x, y)];
            let reversed = cell.modifier.contains(Modifier::REVERSED);
            let (fg, bg) = if reversed {
                (cell.bg, cell.fg)
            } else {
                (cell.fg, cell.bg)
            };
            let look = Look {
                fg: color(fg, true),
                bold: cell.modifier.contains(Modifier::BOLD),
                italic: cell.modifier.contains(Modifier::ITALIC),
                underline: cell.modifier.contains(Modifier::UNDERLINED),
                dim: cell.modifier.contains(Modifier::DIM),
            };
            (cell.symbol().to_string(), look, color(bg, false))
        })
        .collect();

    // Backgrounds, a run of cells of one color at a time.
    let mut x = 0;
    while x < looks.len() {
        let bg = &looks[x].2;
        let run = looks[x..].iter().take_while(|cell| &cell.2 == bg).count();
        if bg != BACKGROUND {
            let _ = writeln!(
                out,
                r#"<rect x="{}" y="{row_top}" width="{}" height="{CELL_HEIGHT}" fill="{bg}"/>"#,
                x as u32 * CELL_WIDTH,
                run as u32 * CELL_WIDTH
            );
        }
        x += run;
    }

    // Glyphs: shapes for the characters that fill or cross their cell, and
    // runs of text for the rest.
    let mut x = 0;
    while x < looks.len() {
        let (symbol, look, _) = &looks[x];
        let left = column(area.x + x as u16);
        if symbol.trim().is_empty() {
            x += 1;
            continue;
        }
        if let Some(shape) = shape(symbol, left, row_top, &look.fg) {
            out.push_str(&shape);
            x += 1;
            continue;
        }
        let mut xs = Vec::new();
        let mut text = String::new();
        while x < looks.len() {
            let (symbol, next, _) = &looks[x];
            if next != look || symbol.trim().is_empty() || shape(symbol, 0, 0, "").is_some() {
                break;
            }
            xs.push(column(area.x + x as u16).to_string());
            text.push_str(symbol);
            // A wide glyph covers the cell after it too.
            x += symbol.width().max(1);
            if symbol.chars().count() != 1 {
                break;
            }
        }
        let mut attributes = format!(r#"fill="{}""#, look.fg);
        if look.bold {
            attributes.push_str(r#" font-weight="bold""#);
        }
        if look.italic {
            attributes.push_str(r#" font-style="italic""#);
        }
        if look.underline {
            attributes.push_str(r#" text-decoration="underline""#);
        }
        if look.dim {
            attributes.push_str(r#" fill-opacity="0.6""#);
        }
        let _ = writeln!(
            out,
            r#"<text x="{}" y="{}" {attributes}>{}</text>"#,
            xs.join(" "),
            row_top + BASELINE,
            escape(&text)
        );
    }
}

/// The block and box-drawing characters Corgi draws with, as shapes filling
/// the cell at `left`, `top`. `None` for any other character, which is text.
fn shape(symbol: &str, left: u32, top: u32, fill: &str) -> Option<String> {
    let (w, h) = (CELL_WIDTH, CELL_HEIGHT);
    let rect = |x: u32, y: u32, width: u32, height: u32| {
        format!(r#"<rect x="{x}" y="{y}" width="{width}" height="{height}" fill="{fill}"/>"#)
    };
    // Box lines run through the middle of the cell, one pixel wide.
    let (mid_x, mid_y) = (left as f32 + w as f32 / 2.0, top as f32 + h as f32 / 2.0);
    let (right, bottom) = (left + w, top + h);
    let radius = w as f32 / 2.0;
    let path =
        |d: String| format!(r#"<path d="{d}" fill="none" stroke="{fill}" stroke-width="1.2"/>"#);
    Some(match symbol {
        "█" => rect(left, top, w, h),
        "▀" => rect(left, top, w, h / 2),
        "▄" => rect(left, top + h / 2, w, h - h / 2),
        "▌" => rect(left, top, w / 2, h),
        "▐" => rect(left + w / 2, top, w - w / 2, h),
        "─" => path(format!("M{left} {mid_y}H{right}")),
        "│" => path(format!("M{mid_x} {top}V{bottom}")),
        "╭" => path(format!(
            "M{right} {mid_y}H{}Q{mid_x} {mid_y} {mid_x} {}V{bottom}",
            mid_x + radius,
            mid_y + radius
        )),
        "╮" => path(format!(
            "M{left} {mid_y}H{}Q{mid_x} {mid_y} {mid_x} {}V{bottom}",
            mid_x - radius,
            mid_y + radius
        )),
        "╰" => path(format!(
            "M{right} {mid_y}H{}Q{mid_x} {mid_y} {mid_x} {}V{top}",
            mid_x + radius,
            mid_y - radius
        )),
        "╯" => path(format!(
            "M{left} {mid_y}H{}Q{mid_x} {mid_y} {mid_x} {}V{top}",
            mid_x - radius,
            mid_y - radius
        )),
        "┌" => path(format!("M{right} {mid_y}H{mid_x}V{bottom}")),
        "┐" => path(format!("M{left} {mid_y}H{mid_x}V{bottom}")),
        "└" => path(format!("M{right} {mid_y}H{mid_x}V{top}")),
        "┘" => path(format!("M{left} {mid_y}H{mid_x}V{top}")),
        "├" => path(format!("M{mid_x} {top}V{bottom}M{mid_x} {mid_y}H{right}")),
        "┤" => path(format!("M{mid_x} {top}V{bottom}M{mid_x} {mid_y}H{left}")),
        _ => return None,
    })
}

/// A cell color in the screenshot's theme. The terminal's default is the
/// theme's foreground or background, depending on which the cell asks for.
fn color(color: Color, foreground: bool) -> String {
    let default = if foreground { FOREGROUND } else { BACKGROUND };
    let ansi = |index: usize| ANSI[index].to_string();
    match color {
        Color::Reset => default.to_string(),
        Color::Black => ansi(0),
        Color::Red => ansi(1),
        Color::Green => ansi(2),
        Color::Yellow => ansi(3),
        Color::Blue => ansi(4),
        Color::Magenta => ansi(5),
        Color::Cyan => ansi(6),
        Color::Gray => ansi(7),
        Color::DarkGray => ansi(8),
        Color::LightRed => ansi(9),
        Color::LightGreen => ansi(10),
        Color::LightYellow => ansi(11),
        Color::LightBlue => ansi(12),
        Color::LightMagenta => ansi(13),
        Color::LightCyan => ansi(14),
        Color::White => ansi(15),
        Color::Indexed(index @ 0..=15) => ansi(usize::from(index)),
        Color::Indexed(index @ 16..=231) => {
            let cube = index - 16;
            hex(
                CUBE_LEVELS[usize::from(cube / 36)],
                CUBE_LEVELS[usize::from(cube / 6 % 6)],
                CUBE_LEVELS[usize::from(cube % 6)],
            )
        }
        Color::Indexed(index) => {
            let grey = 8 + 10 * (index - 232);
            hex(grey, grey, grey)
        }
        Color::Rgb(r, g, b) => hex(r, g, b),
    }
}

fn hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_screenshot_is_the_same_every_time_it_is_drawn() {
        let first: Vec<String> = shots().into_iter().map(|shot| shot.svg).collect();
        let second: Vec<String> = shots().into_iter().map(|shot| shot.svg).collect();
        assert_eq!(first, second);
        for svg in &first {
            assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
            // Nothing of the machine the shots are drawn on gets into them.
            if let Ok(home) = std::env::var("HOME") {
                assert!(!svg.contains(&home), "{svg}");
            }
        }
    }

    #[test]
    fn text_is_escaped_and_boxes_and_blocks_are_drawn_as_shapes() {
        assert_eq!(escape("a <b> & c"), "a &lt;b&gt; &amp; c");
        assert!(shape("╭", 0, 0, "#fff").is_some_and(|shape| shape.starts_with("<path")));
        assert!(shape("▀", 0, 0, "#fff").is_some_and(|shape| shape.starts_with("<rect")));
        assert_eq!(shape("a", 0, 0, "#fff"), None);
        assert_eq!(color(Color::Indexed(16), true), "#000000");
        assert_eq!(color(Color::Indexed(255), true), "#eeeeee");
        assert_eq!(color(Color::Reset, false), BACKGROUND);
    }

    /// Writes every screenshot to `docs/images/`. Run it with
    /// `cargo readme-shots`.
    #[test]
    #[ignore = "writes the README screenshots; run it with `cargo readme-shots`"]
    fn readme_shots() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/images");
        fs::create_dir_all(&directory).expect("create docs/images");
        for shot in shots() {
            let path = directory.join(format!("{}.svg", shot.name));
            fs::write(&path, shot.svg).expect("write screenshot");
            println!("wrote {}", path.display());
        }
    }
}
