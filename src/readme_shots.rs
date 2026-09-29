//! The screenshots in the README, drawn by the dashboard's own UI code into
//! a test terminal and written out as SVG terminal windows. Every agent,
//! project and path in them is made up here, so no Herdr session and nothing
//! of the machine they are drawn on shows in them.
//!
//! Regenerate them with `cargo readme-shots`, the alias for running the
//! ignored test at the bottom of this file; they land in `docs/images/`.
//! The animated one plays the popup motion of `motion.rs` frame by frame on
//! a manual clock, and each frame is rasterized with resvg and the system's
//! fonts into a GIF, since GitHub does not play an SVG's animation.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Modifier},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{
        App, Checkout, MergePhase, MergeWorktreeForm, NewAgentForm, NewField, Overlay, UsageSlot,
    },
    defaults::HarnessDefaults,
    model::{
        Activity, ActivityKind, AgentInfo, AgentState, DashboardAgent, PromptCache, PromptCacheKind,
    },
    motion::{CONTENT, Motion, OPEN},
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

/// The dashboard's size in the screenshots cut from it: wide enough for the
/// header's whole band of cards, and tall enough for three projects.
const WIDTH: u16 = 112;
const HEIGHT: u16 = 44;
/// The hero's height: the header and one project of four agents.
const HERO_HEIGHT: u16 = 28;

/// The title of the terminal window a whole screen is shown in.
const WINDOW_TITLE: &str = "corgi — herdr";
// The labels of the row anatomy: their color, which no part of the dashboard
// uses, so a label never reads as part of the screen, their font, and the
// rows they stack in above the screen.
const CALLOUT: &str = "#ff9e64";
const CALLOUT_FONT: &str = "-apple-system, 'Segoe UI', Helvetica, Arial, sans-serif";
const CALLOUT_FONT_SIZE: u32 = 13;
const CALLOUT_GAP: u32 = 12;
const CALLOUT_TIER: u32 = 18;
/// An animation's frame time in milliseconds: 25 frames a second, which a
/// GIF's hundredths of a second hold exactly.
const GIF_FRAME: u16 = 40;
/// How much larger than the SVG an animation is rasterized, so its text
/// stays sharp on a high-density screen; the README shows it at SVG size.
const GIF_SCALE: f32 = 2.0;

/// A screenshot and the file it is written to, `docs/images/<name>.svg`.
struct Shot {
    name: &'static str,
    svg: String,
}

/// Every screenshot, in the order the README shows them.
fn shots() -> Vec<Shot> {
    let shot = |name, svg| Shot { name, svg };
    vec![
        shot("logo", logo()),
        shot("dashboard", hero()),
        shot("row-anatomy", row_anatomy()),
        shot("expanded-session", expanded_session()),
        shot(
            "new-agent-form",
            popup(&mut new_agent_form(), "◆ New agent"),
        ),
        shot(
            "merge-confirm",
            popup(&mut merge(MergePhase::Confirm), "◆ Merge"),
        ),
    ]
}

/// An animated shot and the file it is written to, `docs/images/<name>.gif`.
struct Animation {
    name: &'static str,
    frames: Vec<Still>,
}

/// One frame of an animation, and how long it shows in hundredths of a
/// second, the unit a GIF counts in.
struct Still {
    svg: String,
    centis: u16,
}

/// Every animated shot.
fn animations() -> Vec<Animation> {
    vec![Animation {
        name: "merge-outcomes",
        frames: merge_outcomes(),
    }]
}

/// How a shot is framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chrome {
    /// A terminal window with a title bar, for a whole screen.
    Window,
    /// A rounded panel of the terminal's background, for part of a screen.
    Panel,
    /// The cells alone on a transparent background, for the logo.
    Bare,
}

/// A label pointing at one cell of a shot, at `x`, `y` in the shot's area.
struct Callout {
    x: u16,
    y: u16,
    label: &'static str,
    place: Place,
}

/// Where a callout's label goes.
enum Place {
    /// Above the shot, in the given row of labels counted up from the
    /// nearest, with a line down to its cell.
    Above(u32),
    /// On the cell's own row, starting at the given column, with a line
    /// back to the cell.
    Right(u16),
}

/// The screen `app` draws, its popups and their effects played out.
fn screen(app: &mut App) -> Terminal<TestBackend> {
    let mut terminal = test_terminal(WIDTH, HEIGHT);
    settle(&mut terminal, app);
    terminal
}

/// The hero: the header over the webshop project, its Steward and a
/// blocked, a working and a finished worker, in a terminal window just tall
/// enough for them.
fn hero() -> String {
    let mut app = dashboard();
    app.agents.retain(|agent| agent.project == "webshop");
    app.status = format!("{} agents", app.agents.len());
    let mut terminal = test_terminal(WIDTH, HERO_HEIGHT);
    settle(&mut terminal, &mut app);
    let area = terminal.backend().buffer().area;
    svg(&mut terminal, area, Chrome::Window, &[])
}

/// The popup titled `title` that `app` has open, with its shadow and a
/// little of the dimmed dashboard around it.
fn popup(app: &mut App, title: &str) -> String {
    let mut terminal = screen(app);
    let area = popup_area(terminal.backend().buffer(), title);
    svg(&mut terminal, area, Chrome::Panel, &[])
}

/// Draws `app` at `millis` on its manual clock.
fn draw_at(terminal: &mut Terminal<TestBackend>, app: &mut App, millis: u64) {
    app.motion
        .set_time(std::time::Duration::from_millis(millis));
    terminal.draw(|frame| draw(frame, app)).expect("draw shot");
}

/// Draws `app` on a manual clock until its popup, if it has one, has grown
/// in and the dashboard behind it has dimmed. A key was just pressed before
/// the last frame, so a text field's caret is in its on phase.
fn settle(terminal: &mut Terminal<TestBackend>, app: &mut App) {
    draw_at(terminal, app, 0);
    draw_at(terminal, app, 1_000);
    app.motion.key_pressed();
    draw_at(terminal, app, 3_000);
}

/// The corgi and the wordmark on their own, cut from the header with no
/// window around them, for the top of the README.
fn logo() -> String {
    let mut terminal = screen(&mut dashboard());
    // The mascot is sixteen cells wide, then two of gap and the 38-cell
    // wordmark; the header is seven rows tall.
    svg(
        &mut terminal,
        Rect::new(0, 0, 16 + 2 + 38, 7),
        Chrome::Bare,
        &[],
    )
}

/// The blocked agent's three rows from the hero dashboard, with a label on
/// every part of them.
fn row_anatomy() -> String {
    let mut terminal = screen(&mut dashboard());
    let buffer = terminal.backend().buffer();
    let task = find(buffer, "Retry failed card payments", buffer.area);
    // Inside the Agents box, so its borders stay out of the crop.
    let area = Rect::new(1, task.y, WIDTH - 2, 3);
    let identity = Rect::new(area.x, task.y, area.width, 1);
    let above = |needle: &str, label: &'static str, tier: u32| {
        let at = find(buffer, needle, identity);
        Callout {
            x: at.x - area.x,
            y: 0,
            label,
            place: Place::Above(tier),
        }
    };
    // The two rows below end where their text does; their labels line up a
    // few columns past the longer of them.
    let ends = [1, 2].map(|row| last_column(buffer, area, task.y + row) - area.x);
    let labels_at = ends.iter().max().copied().unwrap_or_default() + 4;
    let right = |row: u16, label: &'static str| Callout {
        x: ends[usize::from(row) - 1],
        y: row,
        label,
        place: Place::Right(labels_at),
    };
    let callouts = [
        above("██", "selected", 0),
        above("BLOCKED", "state", 1),
        above("Retry", "task", 0),
        above("Sonnet", "model", 1),
        above("medium", "effort", 0),
        above("41%", "context used", 1),
        above("⏱", "prompt cache left", 0),
        above("⑂", "worktree", 1),
        right(
            1,
            "newest thing said: your prompt, its reply or thinking, or a question",
        ),
        right(2, "the command or tool it runs or last ran"),
    ];
    svg(&mut terminal, area, Chrome::Panel, &callouts)
}

/// Where `needle` first starts within `area` of the screen.
fn find(buffer: &Buffer, needle: &str, area: Rect) -> Position {
    for y in area.top()..area.bottom() {
        let mut row = String::new();
        let mut starts = Vec::new();
        for x in area.left()..area.right() {
            starts.push((row.len(), x));
            row.push_str(buffer[(x, y)].symbol());
        }
        if let Some(offset) = row.find(needle) {
            let x = starts
                .iter()
                .rev()
                .find(|(start, _)| *start <= offset)
                .map_or(area.x, |(_, x)| *x);
            return Position { x, y };
        }
    }
    panic!("{needle:?} is not on the screen");
}

/// The first cell from `from` on, stepping by `step`, that holds `symbol`.
fn scan(buffer: &Buffer, from: Position, step: (i32, i32), symbol: &str) -> Position {
    let mut at = from;
    while buffer[(at.x, at.y)].symbol() != symbol {
        at = Position {
            x: u16::try_from(i32::from(at.x) + step.0).expect("on the screen"),
            y: u16::try_from(i32::from(at.y) + step.1).expect("on the screen"),
        };
        assert!(buffer.area.contains(at), "no {symbol:?} from {from:?}");
    }
    at
}

/// The column after the last drawn character of row `y` within `area`.
fn last_column(buffer: &Buffer, area: Rect, y: u16) -> u16 {
    (area.left()..area.right())
        .rev()
        .find(|&x| !buffer[(x, y)].symbol().trim().is_empty())
        .map_or(area.x, |x| x + 1)
}

/// The popup titled `title`: its frame, a list hanging out of it, the shadow
/// it casts to the right and below, and a margin of the dashboard around
/// them.
fn popup_area(buffer: &Buffer, title: &str) -> Rect {
    let title = find(buffer, title, buffer.area);
    let top_left = scan(buffer, title, (-1, 0), "╭");
    let top_right = scan(buffer, title, (1, 0), "╮");
    // The lowest bottom corner between the frame's sides, which is a list's
    // when one hangs below the frame.
    let bottom = (title.y..buffer.area.bottom())
        .filter(|&y| (top_left.x..=top_right.x).any(|x| buffer[(x, y)].symbol() == "╰"))
        .max()
        .unwrap_or(title.y);
    let (margin_x, margin_y) = (3, 1);
    let left = top_left.x.saturating_sub(margin_x);
    let top = top_left.y.saturating_sub(margin_y);
    let right = (top_right.x + 1 + margin_x).min(buffer.area.right());
    let bottom = (bottom + 1 + margin_y).min(buffer.area.bottom());
    Rect::new(left, top, right - left, bottom - top)
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

/// The finished order-history worker opened with `space`, from its own row
/// down to the oldest turn shown.
fn expanded_session() -> String {
    let mut terminal = screen(&mut expanded());
    let buffer = terminal.backend().buffer();
    let top = find(buffer, "Paginate the order history", buffer.area).y;
    let bottom = find(buffer, "Keep the URL shareable", buffer.area).y;
    // Inside the Agents box, so its borders stay out of the crop.
    let area = Rect::new(1, top, WIDTH - 2, bottom + 1 - top);
    svg(&mut terminal, area, Chrome::Panel, &[])
}

/// A merge that fails and then succeeds, as the popup plays it: the
/// confirmation grows in, `Enter` finds the worker's checkout dirty and the
/// popup pulses red and shakes; once that is committed, `Enter` retries,
/// the steps run, and the success is stamped with its check before the
/// popup closes, and the dashboard shows until it opens again.
fn merge_outcomes() -> Vec<Still> {
    enum Beat {
        Phase(MergePhase),
        Close,
    }
    let dirty = MergePhase::Failed("agent worktree has uncommitted or untracked changes".into());
    // Every phase's popup and the columns its shake reaches, so no frame
    // is cut off.
    let area = [
        (MergePhase::Confirm, "Merge reviewed work"),
        (MergePhase::Running(0), "Merge and push"),
        (dirty.clone(), "Merge/push error"),
        (MergePhase::Succeeded, "Merged and pushed"),
    ]
    .into_iter()
    .map(|(phase, title)| popup_area(screen(&mut merge(phase)).backend().buffer(), title))
    .reduce(Rect::union)
    .expect("a popup");
    // When each beat starts, in milliseconds on the motion clock.
    let beats: [(u64, Beat); 8] = [
        (0, Beat::Phase(MergePhase::Confirm)),
        (1_800, Beat::Phase(MergePhase::Running(0))),
        (2_400, Beat::Phase(dirty)),
        (4_800, Beat::Phase(MergePhase::Running(0))),
        (5_200, Beat::Phase(MergePhase::Running(1))),
        (5_800, Beat::Phase(MergePhase::Running(2))),
        (6_400, Beat::Phase(MergePhase::Succeeded)),
        (8_600, Beat::Close),
    ];
    let end: u64 = 9_400;

    let mut app = merge(MergePhase::Confirm);
    let mut terminal = test_terminal(WIDTH, HEIGHT);
    let mut drawn = Vec::new();
    let mut next = 0;
    for millis in (0..end).step_by(usize::from(GIF_FRAME)) {
        while let Some((_, beat)) = beats.get(next).filter(|(at, _)| *at <= millis) {
            match beat {
                Beat::Phase(phase) => {
                    if let Some(form) = app.overlay.merge_worktree_form_mut() {
                        form.phase = phase.clone();
                    }
                }
                Beat::Close => app.overlay = Overlay::None,
            }
            next += 1;
        }
        draw_at(&mut terminal, &mut app, millis);
        drawn.push(svg(&mut terminal, area, Chrome::Panel, &[]));
    }
    // The GIF starts on the settled confirmation, which is also what shows
    // where it does not play; the popup growing in ends the loop instead.
    let settled = OPEN + CONTENT;
    drawn.rotate_left(settled.as_millis() as usize / usize::from(GIF_FRAME) + 1);
    let mut frames: Vec<Still> = Vec::new();
    for svg in drawn {
        let centis = GIF_FRAME / 10;
        match frames.last_mut() {
            Some(last) if last.svg == svg => last.centis += centis,
            _ => frames.push(Still { svg, centis }),
        }
    }
    frames
}

/// The finished order-history worker opened with `space`: its latest turns,
/// newest first, grow down from its row.
fn expanded() -> App {
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

/// The merge popup for the finished order-history worker, at `phase`.
fn merge(phase: MergePhase) -> App {
    let mut app = dashboard();
    app.selected = 3;
    app.overlay = Overlay::merge(MergeWorktreeForm {
        label: "webshop/order-pages-2d5a".into(),
        workspace_id: "w3".into(),
        project_root: PathBuf::from(format!("{HOME}/repos/webshop")),
        worktree_checkout: PathBuf::from(format!(
            "{HOME}/.herdr/worktrees/webshop/worktree-order-pages-2d5a"
        )),
        source_branch: "worktree/order-pages-2d5a".into(),
        target_branch: "main".into(),
        task: "Paginate the order history".into(),
        commits: vec![
            "3f2a9c1 Test the first and last page of the order history".into(),
            "b71e04d Paginate the order history with a stable cursor".into(),
        ],
        phase,
    });
    app
}

// ---------------------------------------------------------------------------
// Cells to SVG.

/// The `area` of the terminal's screen as an SVG image, framed by `chrome`,
/// with `callouts` labelling its parts.
fn svg(
    terminal: &mut Terminal<TestBackend>,
    area: Rect,
    chrome: Chrome,
    callouts: &[Callout],
) -> String {
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
    // Room above the cells for the rows of labels placed there.
    let label_rows = callouts
        .iter()
        .filter_map(|callout| match callout.place {
            Place::Above(tier) => Some(tier + 1),
            Place::Right(_) => None,
        })
        .max()
        .unwrap_or(0);
    let labels = if label_rows == 0 {
        0
    } else {
        CALLOUT_GAP + (label_rows - 1) * CALLOUT_TIER + CALLOUT_FONT_SIZE + PADDING / 2
    };
    let (left, top, right, bottom) = match chrome {
        Chrome::Window => (PADDING, TITLE_BAR + PADDING / 2, PADDING, PADDING),
        Chrome::Panel => (PADDING, PADDING, PADDING, PADDING),
        Chrome::Bare => (0, 0, 0, 0),
    };
    let top = top + labels;
    let (outer_width, outer_height) = (left + width + right, top + height + bottom);
    let mut out = String::new();
    let _ = writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{outer_width}" height="{outer_height}" viewBox="0 0 {outer_width} {outer_height}">"#
    );
    if chrome != Chrome::Bare {
        let _ = writeln!(
            out,
            r#"<rect x="0.5" y="0.5" width="{}" height="{}" rx="{WINDOW_RADIUS}" fill="{BACKGROUND}" stroke="{WINDOW_EDGE}"/>"#,
            outer_width - 1,
            outer_height - 1
        );
    }
    if chrome == Chrome::Window {
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
            escape(WINDOW_TITLE)
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
    for callout in callouts {
        callout_svg(&mut out, callout);
    }
    out.push_str("</g>\n</svg>\n");
    out
}

/// `frames` as a GIF that loops, each rasterized at [`GIF_SCALE`] with the
/// system's fonts. Each frame after the first holds only the pixels that
/// changed, the rest transparent over the one before, which keeps the file
/// small.
fn gif(frames: &[Still]) -> Vec<u8> {
    use resvg::{tiny_skia, usvg};
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let raster = |svg: &str| {
        let tree = usvg::Tree::from_str(svg, &options).expect("parse frame");
        let size = tree
            .size()
            .to_int_size()
            .scale_by(GIF_SCALE)
            .expect("frame size");
        let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height()).expect("pixmap");
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(GIF_SCALE, GIF_SCALE),
            &mut pixmap.as_mut(),
        );
        let pixels: Vec<[u8; 4]> = pixmap
            .pixels()
            .iter()
            .map(|pixel| {
                let color = pixel.demultiply();
                // A GIF pixel is opaque or not at all: the window's rounded
                // corners are cut at half their coverage.
                let alpha = if color.alpha() >= 128 { 255 } else { 0 };
                [color.red(), color.green(), color.blue(), alpha]
            })
            .collect();
        (size.width() as u16, size.height() as u16, pixels)
    };

    let mut out = Vec::new();
    let (width, height, _) = raster(&frames.first().expect("a frame").svg);
    let mut encoder = gif::Encoder::new(&mut out, width, height, &[]).expect("gif");
    encoder.set_repeat(gif::Repeat::Infinite).expect("loop");
    let mut previous: Option<Vec<[u8; 4]>> = None;
    let mut pending: Option<gif::Frame<'static>> = None;
    for still in frames {
        let (_, _, pixels) = raster(&still.svg);
        let changed = |x: u16, y: u16| {
            let at = usize::from(y) * usize::from(width) + usize::from(x);
            previous
                .as_ref()
                .is_none_or(|previous| previous[at] != pixels[at])
        };
        // The smallest box around every changed pixel.
        let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
        for y in 0..height {
            for x in 0..width {
                if changed(x, y) {
                    (left, top) = (left.min(x), top.min(y));
                    (right, bottom) = (right.max(x + 1), bottom.max(y + 1));
                }
            }
        }
        if left >= right {
            // Nothing moved: the frame before just shows longer.
            if let Some(frame) = &mut pending {
                frame.delay += still.centis;
            }
            continue;
        }
        let mut patch = Vec::new();
        for y in top..bottom {
            for x in left..right {
                let pixel = pixels[usize::from(y) * usize::from(width) + usize::from(x)];
                patch.push((changed(x, y) && pixel[3] != 0).then_some(pixel));
            }
        }
        let frame = gif::Frame {
            left,
            top,
            width: right - left,
            height: bottom - top,
            delay: still.centis,
            dispose: gif::DisposalMethod::Keep,
            ..indexed(&patch)
        };
        if let Some(frame) = pending.replace(frame) {
            encoder.write_frame(&frame).expect("write frame");
        }
        previous = Some(pixels);
    }
    if let Some(frame) = pending {
        encoder.write_frame(&frame).expect("write frame");
    }
    drop(encoder);
    out
}

/// `pixels`, `None` where transparent, as a frame's palette and indices: an
/// exact palette when there are few enough colors, else NeuQuant's 255, and
/// the last index for transparency.
fn indexed(pixels: &[Option<[u8; 4]>]) -> gif::Frame<'static> {
    const TRANSPARENT: u8 = 255;
    let mut exact: Vec<[u8; 4]> = pixels.iter().flatten().copied().collect();
    exact.sort_unstable();
    exact.dedup();
    let (mut palette, buffer): (Vec<u8>, Vec<u8>) = if exact.len() < usize::from(TRANSPARENT) {
        let palette = exact
            .iter()
            .flat_map(|color| [color[0], color[1], color[2]]);
        let index = |color: &[u8; 4]| exact.binary_search(color).expect("in the palette") as u8;
        (
            palette.collect(),
            pixels
                .iter()
                .map(|pixel| pixel.as_ref().map_or(TRANSPARENT, index))
                .collect(),
        )
    } else {
        let samples: Vec<u8> = pixels.iter().flatten().flatten().copied().collect();
        let quant = color_quant::NeuQuant::new(10, usize::from(TRANSPARENT), &samples);
        let index = |color: &[u8; 4]| quant.index_of(color) as u8;
        (
            quant.color_map_rgb(),
            pixels
                .iter()
                .map(|pixel| pixel.as_ref().map_or(TRANSPARENT, index))
                .collect(),
        )
    };
    palette.resize(256 * 3, 0);
    gif::Frame {
        buffer: buffer.into(),
        palette: Some(palette),
        transparent: Some(TRANSPARENT),
        ..gif::Frame::default()
    }
}

/// One callout: its label, and a dashed line from the label to its cell
/// that ends in a dot on the cell.
fn callout_svg(out: &mut String, callout: &Callout) {
    let line = |out: &mut String, (x1, y1): (u32, u32), (x2, y2): (u32, u32)| {
        let _ = writeln!(
            out,
            r#"<path d="M{x1} {y1}L{x2} {y2}" stroke="{CALLOUT}" stroke-width="1" stroke-dasharray="3 2"/><circle cx="{x2}" cy="{y2}" r="2" fill="{CALLOUT}"/>"#
        );
    };
    let label = |out: &mut String, x: u32, y: u32| {
        let _ = writeln!(
            out,
            r#"<text x="{x}" y="{y}" fill="{CALLOUT}" font-family="{CALLOUT_FONT}" font-size="{CALLOUT_FONT_SIZE}">{}</text>"#,
            escape(callout.label)
        );
    };
    let cell_x = u32::from(callout.x) * CELL_WIDTH;
    let cell_top = u32::from(callout.y) * CELL_HEIGHT;
    match callout.place {
        Place::Above(tier) => {
            let baseline = CALLOUT_GAP + tier * CALLOUT_TIER;
            // Labels sit above the cells, at negative y in their group.
            let _ = writeln!(out, r#"<g transform="translate(0 -{baseline})">"#);
            label(out, cell_x, 0);
            out.push_str("</g>\n");
            let x = cell_x + CELL_WIDTH / 2;
            let _ = writeln!(
                out,
                r#"<path d="M{x} -{}V{}" stroke="{CALLOUT}" stroke-width="1" stroke-dasharray="3 2"/><circle cx="{x}" cy="{}" r="2" fill="{CALLOUT}"/>"#,
                baseline - 4,
                cell_top + 2,
                cell_top + 2
            );
        }
        Place::Right(column) => {
            let y = cell_top + CELL_HEIGHT / 2;
            let x = u32::from(column) * CELL_WIDTH;
            line(out, (x - 6, y), (cell_x + CELL_WIDTH / 2, y));
            label(out, x, cell_top + BASELINE);
        }
    }
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
                r#"<rect x="{}" y="{row_top}" width="{}" height="{CELL_HEIGHT}" fill="{bg}" shape-rendering="crispEdges"/>"#,
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
    // Filled cells have crisp edges, so neighbours still meet without a
    // hairline seam when GitHub scales the image down to its column.
    let rect = |x: u32, y: u32, width: u32, height: u32| {
        format!(
            r#"<rect x="{x}" y="{y}" width="{width}" height="{height}" fill="{fill}" shape-rendering="crispEdges"/>"#
        )
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
        let svgs = || {
            let stills = animations()
                .into_iter()
                .flat_map(|animation| animation.frames);
            shots()
                .into_iter()
                .map(|shot| shot.svg)
                .chain(stills.map(|still| still.svg))
                .collect::<Vec<String>>()
        };
        let (first, second) = (svgs(), svgs());
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
        for animation in animations() {
            let path = directory.join(format!("{}.gif", animation.name));
            fs::write(&path, gif(&animation.frames)).expect("write animation");
            println!("wrote {}", path.display());
        }
    }
}
