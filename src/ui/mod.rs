use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{App, Overlay},
    motion::Outcome,
};

mod agents;
mod blend;
mod choice_list;
mod dialogs;
mod effects;
mod header;
mod new_agent;
mod prompt;
mod status;

use agents::draw_agents;
pub(crate) use dialogs::{close_dialog_lines, merge_dialog_lines};
use dialogs::{draw_close_workspace, draw_merge_worktree};
use header::{HEADER_HEIGHT, draw_header};
pub(crate) use header::{show_usage_card, usage_lines_for_slot};
use new_agent::{draw_new_agent, draw_new_agent_launch};
use prompt::draw_prompt;
pub(crate) use status::status_fields;

// ANSI colors come from the terminal, so the dashboard follows the active
// Omarchy theme instead of carrying a separate fixed-RGB palette.
const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const SUCCESS: Color = Color::Green;
const WARNING: Color = Color::Yellow;
const DANGER: Color = Color::Red;
// A state Corgi cannot tell.
const UNKNOWN: Color = Color::Magenta;
// An agent a supervisor tagged ready for the user to merge: magenta, the one
// semantic color no state the user acts on uses, far from working's cyan in
// both Tokyo Night themes, with an arrow up towards the base branch.
const MERGE: Color = Color::Magenta;
const MERGE_MARK: &str = "⇡";
const TEXT: Color = Color::White;
// The quieter greys of a popup's content, from the fixed ramp so they sit
// the same on every theme's black fill: the cap a key is drawn on, the rule
// over the keys, and the bar behind a form's focused row.
const KEYCAP: Color = Color::Indexed(237);
const RULE: Color = Color::Indexed(237);
const FOCUS_BAR: Color = Color::Indexed(236);
// The marks a popup's title and outcome lines lead with: what the popup is
// about, and how it ended.
const POPUP_MARK: &str = "◆";
const SUCCESS_MARK: &str = "✓";
const FAILURE_MARK: &str = "✗";
// A dialog takes its share of the screen up to a comfortable line of text,
// so a wide terminal does not stretch a short question across the whole
// width; a message dialog then shrinks further to what it actually says.
const DIALOG_MAX_WIDTH: u16 = 100;
const DIALOG_MIN_WIDTH: u16 = 40;
// Border plus one column of padding on each side of a dialog's text. A
// message dialog draws no padding of its own, but asks for this much width
// so an unwrapped line still stands a column clear of the border.
const DIALOG_FRAME_WIDTH: u16 = 4;

pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut App) {
    app.motion
        .observe(!matches!(app.overlay, Overlay::None), app.overlay.outcome());
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(HEADER_HEIGHT),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(area);

    draw_header(frame, chunks[0], app);
    draw_agents(frame, chunks[1], app);
    draw_footer(frame, chunks[2], app);
    effects::dim(frame.buffer_mut(), area, app.motion.dim());
    draw_overlay(frame, area, app);
}

/// The status line and the keys the list answers, under the agent list.
fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let keys = match (app.expanded, app.compact) {
        (true, true) => {
            "j/k select · space collapse · u/d scroll · p prompt · enter focus · q close"
        }
        (true, false) => {
            "j/k select · space collapse · u/d scroll · p prompt · m merge · enter/f focus · x close agent · q close"
        }
        (false, true) => {
            "j/k select · →/← project · space session · p prompt · n new · t scratch · s supervisor · m merge · enter focus · x close · q close"
        }
        (false, false) => {
            "j/k select · →/← expand/collapse project · space session · p prompt · n new agent · t scratch · s supervisor · m merge · enter/f focus · x close agent · r refresh · q close"
        }
    };
    let line = Line::from(vec![
        Span::styled(format!(" {} ", app.status), Style::default().fg(ACCENT)),
        Span::styled(format!("  {keys}"), Style::default().fg(MUTED)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// Where a popup drew itself: its frame, the frame's color, and the caret
/// of its focused text field, if it has one.
struct Drawn {
    area: Rect,
    color: Color,
    caret: Option<Position>,
}

/// Draws the form or dialog open over the agent list, if any, with the
/// effects of its motion: the frame growing before the content shows, the
/// content fading in, the outcome flash and the shake, and a success's
/// check stamp. A popup that has closed leaves its frame shrinking away.
fn draw_overlay(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let placed = draw_popup(frame, area, app);
    // Last, so a success's stamp stands over the popup, its shrinking frame
    // and the dashboard alike.
    if let Some((around, elapsed)) = app.motion.stamp() {
        let open = placed
            .filter(|placed| placed.open)
            .map(|placed| placed.frame);
        let frame_area = placed.map(|placed| placed.frame);
        effects::stamp(frame.buffer_mut(), around, elapsed, open, frame_area);
    }
}

/// Where a popup's frame stands this frame, and whether the popup is open
/// rather than a frame shrinking away.
#[derive(Debug, Clone, Copy)]
struct Placed {
    frame: Rect,
    open: bool,
}

/// Draws the open popup with its effects, or the frame of one shrinking
/// away, and says where it went.
fn draw_popup(frame: &mut Frame<'_>, area: Rect, app: &mut App) -> Option<Placed> {
    let motion = &app.motion;
    // A success colors the frame green for as long as its stamp plays.
    let success = motion
        .stamp_time()
        .map(|elapsed| (Outcome::Success, elapsed));
    if matches!(app.overlay, Overlay::None) {
        let ghost = motion.ghost()?;
        let color = effects::frame_color(ghost.color, success);
        let drawn = effects::draw_frame(frame.buffer_mut(), ghost.area, color, ghost.openness);
        return Some(Placed {
            frame: drawn,
            open: false,
        });
    }
    let openness = motion.openness();
    let shake = motion.shake();
    // What lies under the popup, for a frame still growing over it and the
    // columns a shake uncovers.
    let under = (openness < 1.0 || shake != 0).then(|| frame.buffer_mut().clone());
    let spinner = motion.spinner();
    let drawn = match &mut app.overlay {
        Overlay::None => return None,
        Overlay::Prompt(form) => draw_prompt(frame, area, form),
        Overlay::NewAgent(form) => draw_new_agent(frame, area, form, &app.agents),
        Overlay::Launch(launch) => draw_new_agent_launch(frame, area, launch, spinner),
        Overlay::Close(form) => draw_close_workspace(frame, area, form),
        Overlay::Merge(form) => draw_merge_worktree(frame, area, form, spinner),
    };
    let color = effects::frame_color(drawn.color, motion.outcome_effect().or(success));
    let fade = motion.content_fade();
    let caret_on = motion.caret_on();
    app.motion.record_frame(drawn.area, drawn.color);
    app.motion.anchor_stamp(drawn.area);

    let buffer = frame.buffer_mut();
    if let Some(under) = &under
        && openness < 1.0
    {
        // The frame grows holding only its fill; the content waits for it.
        *buffer = under.clone();
        let grown = effects::draw_frame(buffer, drawn.area, color, openness);
        return Some(Placed {
            frame: grown,
            open: false,
        });
    }
    effects::recolor_frame(buffer, drawn.area, drawn.color, color);
    effects::fade_content(buffer, drawn.area, fade);
    if let Some(under) = &under {
        effects::shift(buffer, under, drawn.area, shake);
    }
    if let Some(caret) = drawn.caret
        && caret_on
    {
        frame.set_cursor_position(caret);
    }
    Some(Placed {
        frame: drawn.area,
        open: true,
    })
}

/// A popup's title as its frame shows it: its mark, then its words.
fn popup_title(mark: &str, title: &str) -> String {
    format!(" {mark} {title} ")
}

/// A key in a legend, what it does there, and the color of that kind of
/// action: the success color for the key that goes ahead, the warning color
/// for the one that backs out, and the accent for everything in between.
pub(crate) type LegendKey = (&'static str, &'static str, Color);

/// `keys` as one legend row: each key on its keycap in its color, what it
/// does muted beside it, and `separator` between neighbours.
fn legend_line(keys: &[LegendKey], separator: &'static str) -> Line<'static> {
    let mut spans = Vec::with_capacity(keys.len() * 3);
    for (index, (key, label, color)) in keys.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(separator));
        }
        spans.push(Span::styled(format!(" {key} "), bold(*color).bg(KEYCAP)));
        spans.push(Span::styled(
            format!(" {label}"),
            Style::default().fg(MUTED),
        ));
    }
    Line::from(spans)
}

/// `keys` as legend rows no wider than `width`, broken between keys so a
/// keycap never parts from what it does.
fn legend_lines(keys: &[LegendKey], separator: &'static str, width: u16) -> Vec<Line<'static>> {
    let mut rows: Vec<&[LegendKey]> = Vec::new();
    let mut start = 0;
    for end in 1..=keys.len() {
        if end > start + 1 && legend_width(&keys[start..end], separator) > usize::from(width) {
            rows.push(&keys[start..end - 1]);
            start = end - 1;
        }
    }
    if start < keys.len() {
        rows.push(&keys[start..]);
    }
    rows.into_iter()
        .map(|row| legend_line(row, separator))
        .collect()
}

/// Columns [`legend_line`] would take, without building it.
fn legend_width(keys: &[LegendKey], separator: &str) -> usize {
    let entries: usize = keys
        .iter()
        .map(|(key, label, _)| key.width() + 2 + 1 + label.width())
        .sum();
    entries + separator.width() * keys.len().saturating_sub(1)
}

/// The keys of a legend as plain words, `Enter merge · Esc cancel`, for a
/// frontend that draws no keycaps.
pub(crate) fn legend_text(keys: &[LegendKey]) -> String {
    keys.iter()
        .map(|(key, label, _)| format!("{key} {label}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The faint rule a popup draws over its keys.
fn rule_line(width: u16) -> Line<'static> {
    Line::styled("─".repeat(usize::from(width)), Style::default().fg(RULE))
}

/// Where one step of a launch or merge is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Done,
    Current,
    Pending,
}

/// One row of a step list: a done step ticked and quiet, the current one
/// bright beside the spinner, and a pending one dimmed behind a dot.
fn step_line(step: Step, text: &str, spinner: &'static str) -> Line<'static> {
    match step {
        Step::Done => Line::from(vec![
            Span::styled(format!("{SUCCESS_MARK} "), bold(SUCCESS)),
            Span::styled(text.to_string(), Style::default().fg(Color::Gray)),
        ]),
        Step::Current => Line::from(vec![
            Span::styled(format!("{spinner} "), bold(ACCENT)),
            Span::styled(format!("{text}…"), bold(TEXT)),
        ]),
        Step::Pending => Line::from(vec![
            Span::styled("· ", Style::default().fg(MUTED)),
            Span::styled(text.to_string(), Style::default().fg(MUTED)),
        ]),
    }
}

/// The step list of work at step `current` of `steps`.
fn step_lines(steps: &[String], current: usize, spinner: &'static str) -> Vec<Line<'static>> {
    steps
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let step = match index.cmp(&current) {
                std::cmp::Ordering::Less => Step::Done,
                std::cmp::Ordering::Equal => Step::Current,
                std::cmp::Ordering::Greater => Step::Pending,
            };
            step_line(step, text, spinner)
        })
        .collect()
}

/// The headline of an outcome: its mark and words, bold in its color.
fn outcome_line(success: bool, text: String) -> Line<'static> {
    let (mark, color) = if success {
        (SUCCESS_MARK, SUCCESS)
    } else {
        (FAILURE_MARK, DANGER)
    };
    Line::styled(format!("{mark} {text}"), bold(color))
}

/// What a message dialog says: the mark and words of its title, the color
/// of its frame, its text, and the keys it answers.
pub(crate) struct DialogContent<'a> {
    pub(crate) mark: &'static str,
    pub(crate) title: &'static str,
    pub(crate) color: Color,
    pub(crate) lines: Vec<Line<'a>>,
    pub(crate) legend: Vec<LegendKey>,
}

/// Clips `text` to `width` columns, marking a clipped tail with an ellipsis.
fn clip(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    format!("{}…", leading_columns(text, width - 1))
}

/// The tail of `text` that fits in `width` columns, marked with a leading
/// ellipsis when anything was dropped.
fn clip_start(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width <= 1 {
        return "…".repeat(width);
    }
    format!("…{}", trailing_columns(text, width - 1))
}

/// A row marked as the point a longer turn was cut at, with room made for the
/// mark when the row already fills the width.
fn ellipsized(row: &str, width: usize) -> String {
    let kept = leading_columns(row.trim_end(), width.saturating_sub(1));
    format!("{}…", kept.trim_end())
}

/// The longest start of `text` that fits in `width` columns.
pub(super) fn leading_columns(text: &str, width: usize) -> &str {
    let mut used = 0;
    for (index, character) in text.char_indices() {
        used += character.width().unwrap_or(0);
        if used > width {
            return &text[..index];
        }
    }
    text
}

/// The longest end of `text` that fits in `width` columns.
fn trailing_columns(text: &str, width: usize) -> &str {
    let mut used = 0;
    for (index, character) in text.char_indices().rev() {
        used += character.width().unwrap_or(0);
        if used > width {
            return &text[index + character.len_utf8()..];
        }
    }
    text
}

/// The frame every dialog shares: rounded corners, the theme's black as a
/// panel tone that lifts it off the dashboard, and its title in the border's
/// color so the color says what kind of dialog it is before the words do.
fn dialog_block(title: &str, color: Color) -> Block<'static> {
    rounded_block(title, Some(color))
        .style(Style::default().bg(Color::Black))
        .border_style(bold(color))
}

/// The rounded box every panel is drawn in, its border and bold title in
/// `color` when there is one. An empty title draws none.
fn rounded_block(title: &str, color: Option<Color>) -> Block<'static> {
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);
    if let Some(color) = color {
        block = block.border_style(Style::default().fg(color));
    }
    if title.is_empty() {
        return block;
    }
    let title = title.to_string();
    match color {
        Some(color) => block.title(Span::styled(title, bold(color))),
        None => block.title(title),
    }
}

/// A list that marks no row itself, because its rows draw their own marker.
fn unhighlighted_list(items: Vec<ListItem<'_>>) -> List<'_> {
    List::new(items)
        .highlight_symbol("")
        .highlight_style(Style::default())
}

/// Bold text in `color`, the style of every key, marker and heading.
fn bold(color: Color) -> Style {
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

/// A dialog that only has something to say: centered text in a frame no
/// wider than its longest line, and no taller than the rows that text wraps
/// to, so a short question is a small box and a long error a taller one.
/// Its keys stand under a rule at the bottom, as keycaps.
fn draw_message_dialog(frame: &mut Frame<'_>, area: Rect, content: DialogContent<'_>) -> Drawn {
    let title = popup_title(content.mark, content.title);
    let popup = centered_for_lines(area, 72, &title, &content.lines, &content.legend);
    frame.render_widget(Clear, popup);
    let block = dialog_block(&title, content.color);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let legend_rows = legend_rows(&content.legend, inner.width);
    let [text, rule, keys] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(legend_rows.min(1)),
        Constraint::Length(legend_rows.saturating_sub(1)),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(content.lines)
            .alignment(Alignment::Center)
            // Indents are kept, so a left-aligned list can sit in from the
            // border; no dialog line wraps onto a row that begins with air.
            .wrap(Wrap { trim: false }),
        text,
    );
    if legend_rows > 0 {
        let padded = |area: Rect| Rect {
            x: area.x + 1,
            width: area.width.saturating_sub(2),
            ..area
        };
        frame.render_widget(Paragraph::new(rule_line(padded(rule).width)), padded(rule));
        frame.render_widget(
            Paragraph::new(legend_lines(
                &content.legend,
                DIALOG_LEGEND_SEPARATOR,
                keys.width,
            ))
            .alignment(Alignment::Center),
            keys,
        );
    }
    Drawn {
        area: popup,
        color: content.color,
        caret: None,
    }
}

/// Moves the left-aligned rows of a centred dialog, a commit or step list,
/// in as one block: the dialog is sized once without an indent, then the
/// block is moved in by half the room it leaves, so its rows line up and it
/// sits under the heading.
fn indent_left_block<'a>(area: Rect, mut content: DialogContent<'a>) -> DialogContent<'a> {
    let is_left = |line: &Line<'_>| line.alignment == Some(Alignment::Left);
    let Some(widest) = content
        .lines
        .iter()
        .filter(|line| is_left(line))
        .map(Line::width)
        .max()
    else {
        return content;
    };
    let title = popup_title(content.mark, content.title);
    let popup = centered_for_lines(area, 72, &title, &content.lines, &content.legend);
    let inner = usize::from(popup.width.saturating_sub(2));
    let pad = " ".repeat(inner.saturating_sub(widest) / 2);
    for line in content.lines.iter_mut().filter(|line| is_left(line)) {
        line.spans.insert(0, Span::raw(pad.clone()));
    }
    content
}

// The keys of a message dialog get some air between them.
const DIALOG_LEGEND_SEPARATOR: &str = "   ";

/// Rows a dialog's legend takes under its text, `width` columns wide: the
/// rule and the rows the keys wrap to, or nothing when it answers no keys.
fn legend_rows(legend: &[LegendKey], width: u16) -> u16 {
    if legend.is_empty() {
        return 0;
    }
    1 + legend_lines(legend, DIALOG_LEGEND_SEPARATOR, width).len() as u16
}

/// Sizes a message dialog to its text: as wide as its longest line (or its
/// title, or its keys) plus a column of air on each side, up to the dialog
/// width, and never narrower than a small box. The height counts rendered
/// rows rather than logical lines, because `Wrap` folds long lines and a
/// wrapped explanation must never push the keys out of view.
fn centered_for_lines(
    area: Rect,
    width_percent: u16,
    title: &str,
    lines: &[Line<'_>],
    legend: &[LegendKey],
) -> Rect {
    let widest = lines
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .max(title.width())
        .max(legend_width(legend, DIALOG_LEGEND_SEPARATOR)) as u16;
    let widest_allowed = dialog_width(area, width_percent).max(DIALOG_MIN_WIDTH);
    let width = widest
        .saturating_add(DIALOG_FRAME_WIDTH)
        .clamp(DIALOG_MIN_WIDTH, widest_allowed)
        .min(area.width);
    let inner = usize::from(width.saturating_sub(2).max(1));
    let rows: u16 = lines
        .iter()
        .map(|line| wrapped_rows(line, inner) as u16)
        .sum();
    centered(
        area,
        width,
        rows.saturating_add(2)
            .saturating_add(legend_rows(legend, inner as u16)),
    )
}

/// Rows a word-wrapped line occupies at `width` columns.
fn wrapped_rows(line: &Line<'_>, width: usize) -> usize {
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let mut rows = 1;
    // A leading indent is drawn, so it takes room on the first row.
    let mut used = text.len() - text.trim_start().len();
    for word in text.split_whitespace() {
        let word_width = word.width();
        if used == 0 {
            used = word_width;
        } else if used + 1 + word_width <= width {
            used += 1 + word_width;
        } else {
            rows += 1;
            used = word_width;
        }
    }
    rows
}

/// The columns a dialog may take: its share of the screen, capped so a wide
/// terminal does not stretch it into a band.
fn dialog_width(area: Rect, width_percent: u16) -> u16 {
    let share = (u32::from(area.width) * u32::from(width_percent) / 100) as u16;
    share.min(DIALOG_MAX_WIDTH).min(area.width)
}

/// A `width` by `height` box in the middle of `area`, shrunk to fit it.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{MergePhase, MergeWorktreeForm},
        model::{AgentInfo, AgentState, DashboardAgent},
        paths::tilde,
        test_support::{buffer_text, test_app, test_terminal},
    };
    use std::path::PathBuf;

    /// A fixed wall clock for the row tests.
    pub(super) const NOW: u64 = 1_000_000;

    pub(super) fn dashboard_agent() -> DashboardAgent {
        DashboardAgent {
            info: AgentInfo {
                agent: Some("claude".into()),
                state: AgentState::Working,
                ..AgentInfo::default()
            },
            task: "Agent session status line".into(),
            model: Some("Opus 5".into()),
            effort: Some("high".into()),
            context_percent: Some(13),
            worktree_label: Some("silver-cloud-028f".into()),
            ..DashboardAgent::default()
        }
    }

    #[test]
    fn clipping_keeps_what_fits_and_marks_what_it_cut() {
        assert_eq!(clip("Agent session", 20), "Agent session");
        assert_eq!(clip("Agent session", 6), "Agent…");
        assert_eq!(clip("Agent session", 0), "");
        assert_eq!(clip("日本語のテキスト", 7), "日本語…");
        assert_eq!(clip_start("/a/very/long/path", 8), "…ng/path");
        assert_eq!(clip_start("/a/日本語", 5), "…本語");
        assert_eq!(clip_start("/a/very/long/path", 1), "…");
        // A cut row is always marked, even when it fits, and loses the space
        // it was cut after.
        assert_eq!(ellipsized("wraps here ", 20), "wraps here…");
        assert_eq!(ellipsized("wraps here", 10), "wraps her…");
        assert_eq!(ellipsized("wraps here", 7), "wraps…");
        assert_eq!(ellipsized("wraps", 0), "…");
    }

    #[test]
    fn home_is_shortened_to_a_tilde_only_at_a_path_boundary() {
        let home = std::env::var("HOME").expect("HOME set");
        assert_eq!(tilde(&format!("{home}/repos/corgi")), "~/repos/corgi");
        assert_eq!(tilde(&home), "~");
        assert_eq!(tilde(&format!("{home}2/repos")), format!("{home}2/repos"));
        assert_eq!(tilde("/repos/corgi"), "/repos/corgi");
    }

    #[test]
    fn a_short_dialog_on_a_wide_screen_is_no_wider_than_its_text() {
        let lines = vec![
            Line::raw("Close workspace corgi/worker?"),
            Line::raw(""),
            Line::raw("All of its tabs and panes will be stopped."),
        ];
        let area = Rect::new(0, 0, 240, 60);
        let popup = centered_for_lines(area, 72, " Close workspace ", &lines, &[]);
        // The longest line plus the frame, not the screen's share.
        assert_eq!(popup.width, 42 + DIALOG_FRAME_WIDTH);
        assert_eq!(popup.height, 5);
        assert_eq!(popup.x, (240 - popup.width) / 2);

        // A long explanation is held to the cap and wraps instead.
        let long = vec![Line::raw("word ".repeat(40))];
        let popup = centered_for_lines(area, 72, "", &long, &[]);
        assert_eq!(popup.width, DIALOG_MAX_WIDTH);
        assert_eq!(popup.height, 3 + 2);

        // A tiny message still gets a box a title fits in.
        let popup = centered_for_lines(area, 72, " Merge and push ", &[Line::raw("ok")], &[]);
        assert_eq!(popup.width, DIALOG_MIN_WIDTH);

        // Keys take a rule and a row of their own under the text.
        let keys: [LegendKey; 1] = [("Enter", "close", SUCCESS)];
        let popup = centered_for_lines(area, 72, "", &[Line::raw("ok")], &keys);
        assert_eq!(popup.height, 1 + 2 + 2);
    }

    #[test]
    fn every_dialog_draws_the_same_rounded_frame() {
        let mut app = test_app();
        let merge = Overlay::merge(MergeWorktreeForm {
            label: "corgi/reviewed-agent".into(),
            workspace_id: "w7".into(),
            agent: "w-reviewed-agent".into(),
            project_root: PathBuf::from("/repos/corgi"),
            worktree_checkout: PathBuf::from("/worktrees/corgi/worktree-reviewed-agent"),
            source_branch: "worktree/reviewed-agent".into(),
            target_branch: "main".into(),
            task: "Popup windows styling review".into(),
            commits: vec![
                "cf87db5 Merge branch 'worktree/clear-field-df92'".into(),
                "ec5e620 Lay the header out as one band and shorten the wordmark".into(),
            ],
            phase: MergePhase::Confirm,
        });
        let prompt = Overlay::Prompt(crate::app::PromptForm {
            target: "p1".into(),
            label: "corgi-worker".into(),
            text: "hello".into(),
        });
        let mut terminal = test_terminal(120, 30);
        for overlay in [merge, prompt] {
            app.overlay = overlay;
            terminal.clear().expect("clear");
            terminal
                .draw(|frame| draw_overlay(frame, frame.area(), &mut app))
                .expect("draw dialog");
            let rendered = buffer_text(terminal.backend().buffer());
            for corner in ["╭", "╮", "╰", "╯"] {
                assert!(rendered.contains(corner), "missing {corner}: {rendered}");
            }
        }
    }

    fn prompt_overlay() -> Overlay {
        Overlay::Prompt(crate::app::PromptForm {
            target: "p1".into(),
            label: "corgi-worker".into(),
            text: "hello".into(),
        })
    }

    fn ms(millis: u64) -> std::time::Duration {
        std::time::Duration::from_millis(millis)
    }

    /// Draws the whole dashboard at `now` on the app's manual clock.
    fn draw_at(
        terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
        app: &mut App,
        now: u64,
    ) -> String {
        app.motion.set_time(ms(now));
        terminal.draw(|frame| draw(frame, app)).expect("draw");
        buffer_text(terminal.backend().buffer())
    }

    #[test]
    fn a_popup_grows_empty_then_shows_its_content_and_shrinks_away_on_close() {
        let mut app = test_app();
        app.motion = crate::motion::Motion::manual();
        let mut terminal = test_terminal(80, 24);
        let corners = |screen: &str| screen.matches('╭').count();
        let dashboard = corners(&draw_at(&mut terminal, &mut app, 1_000));

        app.overlay = prompt_overlay();
        let growing = draw_at(&mut terminal, &mut app, 1_000);
        // The frame starts as a small box in the middle, holding only fill.
        assert_eq!(corners(&growing), dashboard + 1, "{growing}");
        assert!(!growing.contains("corgi-worker"), "{growing}");
        assert!(terminal.get_cursor_position().is_ok());
        let half = draw_at(&mut terminal, &mut app, 1_090);
        assert!(!half.contains("Prompt"), "{half}");

        // Grown, the content is there, fading in and then at its colors.
        let open = draw_at(&mut terminal, &mut app, 1_200);
        assert!(open.contains("◆ Prompt"), "{open}");
        assert!(open.contains("corgi-worker"), "{open}");
        let settled = draw_at(&mut terminal, &mut app, 1_400);
        assert_eq!(settled, open);
        let name = {
            let rows = crate::test_support::buffer_rows(terminal.backend().buffer());
            let y = rows
                .iter()
                .position(|row| row.contains("corgi-worker"))
                .unwrap();
            let x = rows[y][..rows[y].find("corgi-worker").unwrap()]
                .chars()
                .count();
            terminal.backend().buffer()[(x as u16, y as u16)].fg
        };
        assert_eq!(name, TEXT, "the content ends on its own color");

        // Closed, the frame shrinks and is gone once it has.
        app.overlay = Overlay::None;
        let closing = draw_at(&mut terminal, &mut app, 1_450);
        assert_eq!(corners(&closing), dashboard + 1, "{closing}");
        assert!(!closing.contains("corgi-worker"), "{closing}");
        let closed = draw_at(&mut terminal, &mut app, 1_700);
        assert_eq!(corners(&closed), dashboard, "{closed}");
    }

    #[test]
    fn the_dashboard_dims_behind_a_popup_and_comes_back_exactly() {
        let mut app = test_app();
        app.motion = crate::motion::Motion::manual();
        app.status = "3 agents".into();
        let mut terminal = test_terminal(100, 30);
        draw_at(&mut terminal, &mut app, 0);
        let footer = terminal.backend().buffer().clone();
        let status_cell = |buffer: &ratatui::buffer::Buffer| buffer[(1, 28)].fg;
        assert_eq!(status_cell(&footer), ACCENT);

        app.overlay = prompt_overlay();
        draw_at(&mut terminal, &mut app, 1_000);
        draw_at(&mut terminal, &mut app, 1_500);
        assert!(
            matches!(
                status_cell(terminal.backend().buffer()),
                Color::Indexed(232..=255)
            ),
            "{:?}",
            status_cell(terminal.backend().buffer())
        );

        app.overlay = Overlay::None;
        draw_at(&mut terminal, &mut app, 2_000);
        draw_at(&mut terminal, &mut app, 2_500);
        assert_eq!(status_cell(terminal.backend().buffer()), ACCENT);
    }

    #[test]
    fn a_failure_shakes_the_popup_and_flashes_its_frame_once() {
        let mut app = test_app();
        app.motion = crate::motion::Motion::manual();
        let form = MergeWorktreeForm {
            label: "corgi/reviewed-agent".into(),
            workspace_id: "w7".into(),
            agent: "w-reviewed-agent".into(),
            project_root: PathBuf::from("/repos/corgi"),
            worktree_checkout: PathBuf::from("/worktrees/corgi/worktree-reviewed-agent"),
            source_branch: "worktree/reviewed-agent".into(),
            target_branch: "main".into(),
            task: String::new(),
            commits: Vec::new(),
            phase: MergePhase::Running(1),
        };
        app.overlay = Overlay::merge(form);
        let mut terminal = test_terminal(100, 30);
        draw_at(&mut terminal, &mut app, 0);
        draw_at(&mut terminal, &mut app, 1_000);
        // The popup's top-left corner, found by the title beside it.
        let top_left = |terminal: &ratatui::Terminal<ratatui::backend::TestBackend>| {
            crate::test_support::buffer_rows(terminal.backend().buffer())
                .iter()
                .enumerate()
                .find_map(|(y, row)| {
                    let title = row.find("Merge")?;
                    let corner = row[..title].rfind('╭')?;
                    Some((row[..corner].chars().count() as u16, y as u16))
                })
                .expect("the popup's frame")
        };
        let left_edge =
            |terminal: &ratatui::Terminal<ratatui::backend::TestBackend>| top_left(terminal).0;
        app.overlay.merge_worktree_form_mut().unwrap().phase =
            MergePhase::Failed("git push failed".into());
        draw_at(&mut terminal, &mut app, 1_000);
        let corner = |terminal: &ratatui::Terminal<ratatui::backend::TestBackend>| {
            terminal.backend().buffer()[top_left(terminal)].fg
        };
        assert_eq!(corner(&terminal), Color::Indexed(blend::FLASH_WHITE));
        let edges: Vec<u16> = (1..40)
            .map(|step| {
                draw_at(&mut terminal, &mut app, 1_000 + step * 10);
                left_edge(&terminal)
            })
            .collect();
        draw_at(&mut terminal, &mut app, 1_700);
        let resting = left_edge(&terminal);
        let offsets: Vec<i64> = edges
            .iter()
            .map(|edge| i64::from(*edge) - i64::from(resting))
            .collect();
        assert!(offsets.iter().any(|offset| *offset > 0), "{offsets:?}");
        assert!(offsets.iter().any(|offset| *offset < 0), "{offsets:?}");
        assert!(
            offsets.iter().all(|offset| offset.abs() <= 2),
            "{offsets:?}"
        );

        // Settled, it stands still in the failure's own red.
        draw_at(&mut terminal, &mut app, 1_900);
        assert_eq!(left_edge(&terminal), resting);
        assert_eq!(corner(&terminal), DANGER);
    }

    #[test]
    fn a_success_stamps_a_big_check_over_the_popup_and_leaves_it_as_it_was() {
        let mut app = test_app();
        app.motion = crate::motion::Motion::manual();
        app.overlay = Overlay::merge(MergeWorktreeForm {
            label: "corgi/reviewed-agent".into(),
            workspace_id: "w7".into(),
            agent: "w-reviewed-agent".into(),
            project_root: PathBuf::from("/repos/corgi"),
            worktree_checkout: PathBuf::from("/worktrees/corgi/worktree-reviewed-agent"),
            source_branch: "worktree/reviewed-agent".into(),
            target_branch: "main".into(),
            task: String::new(),
            commits: Vec::new(),
            phase: MergePhase::Running(2),
        });
        let mut terminal = test_terminal(100, 30);
        draw_at(&mut terminal, &mut app, 0);
        draw_at(&mut terminal, &mut app, 1_000);
        // The corner of the popup's frame, found by the title beside it, and
        // the color of the first cell of `text`.
        let corner = |terminal: &ratatui::Terminal<ratatui::backend::TestBackend>| {
            let rows = crate::test_support::buffer_rows(terminal.backend().buffer());
            let (y, row) = rows
                .iter()
                .enumerate()
                .find(|(_, row)| row.contains("Merged and pushed"))
                .expect("the popup's title");
            let title = row.find("✓ Merged").unwrap();
            let x = row[..row[..title].rfind('╭').unwrap()].chars().count();
            terminal.backend().buffer()[(x as u16, y as u16)].fg
        };
        let color_of = |terminal: &ratatui::Terminal<ratatui::backend::TestBackend>, text: &str| {
            let rows = crate::test_support::buffer_rows(terminal.backend().buffer());
            let (y, row) = rows
                .iter()
                .enumerate()
                .find(|(_, row)| row.contains(text))?;
            let x = row[..row.find(text).unwrap()].chars().count();
            Some(terminal.backend().buffer()[(x as u16, y as u16)].fg)
        };

        app.overlay.merge_worktree_form_mut().unwrap().phase = MergePhase::Succeeded;
        draw_at(&mut terminal, &mut app, 1_000);
        // The frame is green at once; the stroke has not started yet.
        assert_eq!(corner(&terminal), Color::Indexed(blend::SUCCESS_FRAME));
        assert_eq!(color_of(&terminal, "▀█▄█▀"), None);

        // Drawn and holding: the whole ✓ over content faded almost out.
        let screen = draw_at(&mut terminal, &mut app, 1_400);
        for line in effects::CHECK {
            assert!(screen.contains(line.trim()), "missing {line}\n{screen}");
        }
        assert_eq!(
            color_of(&terminal, "▀█▄█▀"),
            Some(Color::Indexed(blend::STAMP_GREEN))
        );
        assert_eq!(
            color_of(&terminal, "Worktree worktree-reviewed-agent"),
            Some(Color::Indexed(blend::FADE_FROM))
        );
        assert_eq!(corner(&terminal), Color::Indexed(blend::SUCCESS_FRAME));

        // Played out, the panel is as it was, in the theme's own colors.
        let screen = draw_at(&mut terminal, &mut app, 1_900);
        assert!(!screen.contains("▀█▄█▀"), "{screen}");
        assert_eq!(corner(&terminal), ACCENT);
        assert_eq!(
            color_of(&terminal, "✓ Merge and push complete."),
            Some(SUCCESS)
        );
    }

    #[test]
    fn a_confirm_that_closes_its_popup_stamps_on_over_the_dashboard_and_leaves_nothing() {
        let mut app = test_app();
        app.motion = crate::motion::Motion::manual();
        let mut terminal = test_terminal(100, 30);
        let bare = draw_at(&mut terminal, &mut app, 0);
        let bare_buffer = terminal.backend().buffer().clone();

        app.overlay = prompt_overlay();
        draw_at(&mut terminal, &mut app, 1_000);
        draw_at(&mut terminal, &mut app, 1_500);
        // Sent: the prompt closes at once and its stamp plays where it was.
        app.motion.succeeded();
        app.overlay = Overlay::None;
        let closing = draw_at(&mut terminal, &mut app, 1_600);
        assert!(!closing.contains("Send to"), "{closing}");
        let screen = draw_at(&mut terminal, &mut app, 2_000);
        for line in effects::CHECK {
            assert!(screen.contains(line.trim()), "missing {line}\n{screen}");
        }
        assert!(app.motion.is_moving(), "the stamp keeps the loop fast");

        // Played out, not a cell of it is left on the dashboard.
        draw_at(&mut terminal, &mut app, 2_400);
        draw_at(&mut terminal, &mut app, 3_000);
        assert!(!app.motion.is_moving());
        assert_eq!(terminal.backend().buffer(), &bare_buffer);
        assert_eq!(buffer_text(terminal.backend().buffer()), bare);

        // A popup opening while a stamp plays cuts it off.
        app.overlay = prompt_overlay();
        draw_at(&mut terminal, &mut app, 4_000);
        draw_at(&mut terminal, &mut app, 4_500);
        app.motion.succeeded();
        app.overlay = Overlay::None;
        draw_at(&mut terminal, &mut app, 4_600);
        app.overlay = prompt_overlay();
        let reopened = draw_at(&mut terminal, &mut app, 4_700);
        assert!(!reopened.contains("▀█▄█▀"), "{reopened}");
        assert_eq!(app.motion.stamp(), None);
    }

    #[test]
    fn the_caret_blinks_on_its_own_clock_and_comes_back_under_typing() {
        let mut app = test_app();
        app.overlay = prompt_overlay();
        let mut terminal = test_terminal(80, 24);
        let caret_at = |terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
                        app: &mut App,
                        now: u64| {
            app.motion.set_time(ms(now));
            terminal
                .draw(|frame| draw_overlay(frame, frame.area(), app))
                .expect("draw");
            terminal.backend().cursor_visible()
        };
        assert!(caret_at(&mut terminal, &mut app, 0));
        assert!(!caret_at(&mut terminal, &mut app, 600));
        app.motion.key_pressed();
        assert!(caret_at(&mut terminal, &mut app, 600));
        assert!(!caret_at(&mut terminal, &mut app, 1_200));
    }
}
