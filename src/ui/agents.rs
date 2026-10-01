use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{ListItem, ListState, Paragraph},
};

use unicode_width::UnicodeWidthStr;

use crate::{
    app::{App, is_card, project_runs},
    model::{Activity, ActivityKind, AgentState, DashboardAgent},
    textfield::wrap_rows,
    time::unix_now,
};

use super::status::{StatusField, status_fields};
use super::{
    ACCENT, MUTED, SUCCESS, TEXT, TOTAL, WARNING, bold, clip, ellipsized, rounded_block,
    unhighlighted_list,
};

// Every row reserves the same gutter, selected or not, so the columns to the
// right of it stay aligned as the cursor moves. The selected agent row is
// marked in it by a solid bar down its text lines, so the cursor reads as one
// tall block that ends with the text. Both are spelled out rather than built,
// so no row allocates one; a test holds them to GUTTER_WIDTH.
const GUTTER_WIDTH: usize = 4;
const SELECTED_GUTTER: &str = "██  ";
const BLANK_GUTTER: &str = "    ";
// The gutter plus the status badge's leading padding, so a project name sits
// directly above the status word of the rows below it.
const PROJECT_LABEL_COLUMN: usize = GUTTER_WIDTH + 1;
// The same dot sits between every element of an agent's status line: after
// the status badge, and between the task summary and each field after it.
pub(super) const FIELD_SEPARATOR: &str = " · ";
const SEPARATOR_WIDTH: usize = 3;
// Below this the task summary is too clipped to read, so the trailing fields
// are dropped instead of shrinking it further.
const TASK_MIN_WIDTH: usize = 16;
// Rows one turn of the expanded transcript is held to. Two is enough to
// recognize a turn without letting one of them crowd out its neighbours.
const TRANSCRIPT_ENTRY_ROWS: usize = 2;
// The marker column and the space after it, which every continuation row of
// the same turn is indented by so a wrapped turn reads as one block.
const TRANSCRIPT_INDENT: usize = 2;
const TRANSCRIPT_CONTINUATION: &str = "  ";
// Fewer rows than this and the expanded session is not worth reading, so the
// sessions above it give up their place instead of squeezing it.
const MIN_TRANSCRIPT_ROWS: usize = 6;
// A card stands one column in from either side of the Agents box, so its
// border reads apart from the box's, and its own border takes one more on
// either side: its rows are those of a box this much narrower.
const CARD_INSET: u16 = 4;
const CARD_MARGIN: &str = " ";
// Rows a handler's message and its tool each wrap to in its card.
const CARD_ACTIVITY_ROWS: usize = 2;
// The keys that expand and collapse a card, where the card shows them.
const EXPAND_HINT: &str = "→ expand";
const COLLAPSE_HINT: &str = "← collapse";
// The states a card sums its workers up by, the most urgent first.
const SUMMARY_STATES: [AgentState; 5] = [
    AgentState::Blocked,
    AgentState::Working,
    AgentState::Done,
    AgentState::Idle,
    AgentState::Unknown,
];
const SUMMARY_SEPARATOR: &str = "   ";
const BLOCKED_SEPARATOR: &str = " — ";
// What a blocked worker waits on when its screen shows no question.
const WAITING_FOR_INPUT: &str = "waiting for your input";

/// The agent list, and the one session in it that is expanded.
///
/// A project led by its handler is one card: the handler's rows, then a sum
/// of its workers, or, once `→` expanded it, every worker's own rows. Other
/// projects and the scratch sessions are a heading over their rows.
///
/// Expanding a session changes nothing above it: its project heading or card
/// top and every session before it keep their place, and its own identity
/// line stays where it was. Only its message row grows, down to the bottom of
/// the box, into that session's latest turns. The sessions below it have
/// nowhere left to sit, so they are the ones that give way.
pub(super) fn draw_agents(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    if app.agents.is_empty() {
        frame.render_widget(
            Paragraph::new("No coding agents are currently detected in this Herdr session.\n\nPress n to start one.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(MUTED))
                .block(rounded_block(" Agents ", None)),
            area,
        );
        return;
    }

    let mut selected_item = 0;
    let mut items: Vec<ListItem<'_>> = Vec::with_capacity(app.agents.len() * 2);
    let mut zoom: Option<Zoom<'_>> = None;
    let now = unix_now();
    let card = Rim::card(area.width);
    let card_width = card.content_area(area.width);
    'runs: for run in project_runs(&app.agents) {
        let agents = &app.agents[run.clone()];
        if is_card(&app.agents, &run) {
            let open = app.card_expanded(&run);
            let (handler, workers) = (&agents[0], &agents[1..]);
            // A worker folded into a collapsed card is selected as the card.
            let selected = run.contains(&app.selected) && (!open || app.selected == run.start);
            if selected {
                selected_item = items.len();
                if app.expanded {
                    zoom = Some(Zoom {
                        agent: handler,
                        rim: card,
                        lead: vec![card_top(&handler.project_group, area.width)],
                    });
                    break;
                }
            }
            // The gap after a card closes its last item, so a scrolled list
            // never starts on a blank line above a card.
            if !open {
                let mut lines = collapsed_card(handler, workers, selected, area.width, now);
                lines.push(Line::raw(""));
                items.push(ListItem::new(lines));
            } else {
                let mut lines = vec![card_top(&handler.project_group, area.width)];
                lines.extend(
                    handler_lines(handler, selected, card_width, now)
                        .into_iter()
                        .map(|line| card.frame(line)),
                );
                if workers.is_empty() {
                    lines.push(card_bottom(area.width, Some(COLLAPSE_HINT)));
                    lines.push(Line::raw(""));
                } else {
                    lines.push(card.frame(Line::from(gutter(false))));
                }
                items.push(ListItem::new(lines));
                for (offset, worker) in workers.iter().enumerate() {
                    let selected = run.start + 1 + offset == app.selected;
                    if selected {
                        selected_item = items.len();
                        if app.expanded {
                            zoom = Some(Zoom {
                                agent: worker,
                                rim: card,
                                lead: Vec::new(),
                            });
                            break 'runs;
                        }
                    }
                    let mut lines: Vec<Line<'_>> = agent_lines(worker, selected, card_width, now)
                        .into_iter()
                        .map(|line| card.frame(line))
                        .collect();
                    if offset + 1 == workers.len() {
                        lines.push(card_bottom(area.width, Some(COLLAPSE_HINT)));
                        lines.push(Line::raw(""));
                    }
                    items.push(ListItem::new(lines));
                }
            }
        } else {
            for (offset, agent) in agents.iter().enumerate() {
                // The heading leads the item of the project's first session,
                // so no scroll ever shows that session with its heading cut
                // off above it.
                let heading: Vec<Line<'_>> = (offset == 0)
                    .then(|| project_heading(&agent.project_group, area.width))
                    .into_iter()
                    .collect();
                let selected = run.start + offset == app.selected;
                if selected {
                    selected_item = items.len();
                    if app.expanded {
                        zoom = Some(Zoom {
                            agent,
                            rim: Rim::Open,
                            lead: heading,
                        });
                        break 'runs;
                    }
                }
                let mut lines = heading;
                lines.extend(agent_lines(agent, selected, area.width, now));
                // Every agent row already ends in a blank line. Add exactly
                // one more at a project boundary so the next heading is
                // distinct; like a card's, it closes the last item.
                if offset + 1 == agents.len() {
                    lines.push(Line::raw(""));
                }
                items.push(ListItem::new(lines));
            }
        }
    }

    let mut page = 0;
    let mut scroll = 0;
    let zoomed = zoom.is_some();
    if let Some(Zoom { agent, rim, lead }) = zoom {
        let inner_height = usize::from(area.height.saturating_sub(2));
        // Rows everything before the expanded session's identity line takes,
        // so what is left of the box is what its message row can grow into.
        let mut rows_above = lead.len() + items.iter().map(ListItem::height).sum::<usize>();
        // A session in a card closes the card under its turns.
        let closing = usize::from(rim.is_card());
        // A session pushed far down the list would have nothing left to grow
        // into. Let the sessions above it scroll off rather than open into a
        // two-row sliver, which is what the collapsed row already showed.
        while inner_height.saturating_sub(rows_above + 1 + closing) < MIN_TRANSCRIPT_ROWS
            && !items.is_empty()
        {
            rows_above -= items.remove(0).height();
            selected_item -= 1;
        }
        let budget = inner_height.saturating_sub(rows_above + 1 + closing).max(1);
        let content_area = rim.content_area(area.width);
        let mut lines = lead;
        lines.push(rim.frame(agent_status_line(agent, true, content_area, now)));
        if app.transcript.is_empty() {
            lines.extend(
                unread_transcript_rows(agent)
                    .into_iter()
                    .map(|line| rim.frame(line)),
            );
        } else {
            // Only the rows up to the bottom of the scrolled view are built;
            // a scroll past the end is clamped against however many there
            // turn out to be.
            let wanted = usize::from(app.transcript_scroll) + budget;
            let turns =
                transcript_lines(&app.transcript, transcript_text_width(content_area), wanted);
            page = budget;
            scroll = usize::from(app.transcript_scroll).min(turns.len().saturating_sub(budget));
            lines.extend(
                turns
                    .into_iter()
                    .skip(scroll)
                    .take(budget)
                    .map(|line| rim.frame(line)),
            );
        }
        if rim.is_card() {
            lines.push(card_bottom(area.width, None));
        }
        items.push(ListItem::new(lines));
    }

    // The expanded session already arranged its items to fit the box, so it
    // is drawn from the top. The collapsed list starts where it was last
    // drawn from, and the list only scrolls it when the selection passes an
    // edge of the view.
    let offset = if zoomed {
        0
    } else {
        app.agent_list_offset.min(max_offset(
            &items,
            usize::from(area.height.saturating_sub(2)),
        ))
    };
    // Agent rows render their own selection bar so section rules can begin
    // flush with the box edge instead of reserving a list gutter.
    let list = unhighlighted_list(items).block(rounded_block(" Agents ", None));
    let mut state = ListState::default()
        .with_offset(offset)
        .with_selected(Some(selected_item));
    frame.render_stateful_widget(list, area, &mut state);
    if !zoomed {
        app.agent_list_offset = state.offset();
    }

    // The list has been handed over, so the viewport it was drawn in can go
    // back to the app for the next page key.
    if zoomed {
        app.transcript_page = u16::try_from(page).unwrap_or(u16::MAX).max(1);
        app.transcript_scroll = u16::try_from(scroll).unwrap_or(u16::MAX);
    }
}

/// The session expanded with `space`: the agent, where its rows are drawn,
/// and the lines of its item above its identity line, such as the top of the
/// card it leads.
struct Zoom<'a> {
    agent: &'a DashboardAgent,
    rim: Rim,
    lead: Vec<Line<'static>>,
}

/// Where a row is drawn: straight in the Agents box, or inside a card, whose
/// border it then carries on either side.
#[derive(Debug, Clone, Copy)]
enum Rim {
    Open,
    /// Inside a card `inner` columns wide between its borders.
    Card {
        inner: usize,
    },
}

impl Rim {
    fn card(area_width: u16) -> Self {
        Self::Card {
            inner: card_inner_width(area_width),
        }
    }

    fn is_card(self) -> bool {
        matches!(self, Self::Card { .. })
    }

    /// The width of the box a row inside this rim is laid out for. A card
    /// takes `CARD_INSET` columns of the Agents box, so its rows are those of
    /// a box that much narrower, and keep the same margin to its border.
    fn content_area(self, area_width: u16) -> u16 {
        match self {
            Self::Open => area_width,
            Self::Card { .. } => area_width.saturating_sub(CARD_INSET),
        }
    }

    /// `line` inside this rim: as it is, or between the card's borders,
    /// clipped and padded to the card's width.
    fn frame(self, line: Line<'_>) -> Line<'_> {
        let Self::Card { inner } = self else {
            return line;
        };
        let mut spans = vec![
            Span::raw(CARD_MARGIN),
            Span::styled("│", Style::default().fg(MUTED)),
        ];
        spans.extend(fitted_spans(line, inner));
        spans.push(Span::styled("│", Style::default().fg(MUTED)));
        Line::from(spans)
    }
}

/// Columns between a card's borders in an Agents box `area_width` wide.
fn card_inner_width(area_width: u16) -> usize {
    usize::from(area_width.saturating_sub(CARD_INSET + 2))
}

/// The spans of `line` clipped to `width` columns and padded to exactly that
/// width, so a border drawn after them lines up on every row.
fn fitted_spans(line: Line<'_>, width: usize) -> Vec<Span<'_>> {
    let mut left = width;
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    for span in line.spans {
        if left == 0 {
            break;
        }
        let span_width = span.width();
        if span_width <= left {
            left -= span_width;
            spans.push(span);
            continue;
        }
        let kept = super::leading_columns(&span.content, left).to_string();
        left -= kept.width();
        spans.push(Span::styled(kept, span.style));
        break;
    }
    if left > 0 {
        spans.push(Span::raw(" ".repeat(left)));
    }
    spans
}

/// The top of a card: its corner, the project's name as its title, and the
/// rule on to the other corner.
fn card_top(project: &str, area_width: u16) -> Line<'static> {
    let inner = card_inner_width(area_width);
    let title = clip(&format!(" {project} "), inner);
    let rule = inner.saturating_sub(title.width());
    Line::from(vec![
        Span::raw(CARD_MARGIN),
        Span::styled("╭", Style::default().fg(MUTED)),
        Span::styled(title, bold(SUCCESS)),
        Span::styled(format!("{}╮", "─".repeat(rule)), Style::default().fg(MUTED)),
    ])
}

/// The rule between a card's handler and the sum of its workers, joined to
/// the card's sides.
fn card_divider(area_width: u16) -> Line<'static> {
    let inner = card_inner_width(area_width);
    Line::from(vec![
        Span::raw(CARD_MARGIN),
        Span::styled(
            format!("├{}┤", "─".repeat(inner)),
            Style::default().fg(MUTED),
        ),
    ])
}

/// The bottom of a card, with `hint` set into its rule near the right corner.
fn card_bottom(area_width: u16, hint: Option<&'static str>) -> Line<'static> {
    let inner = card_inner_width(area_width);
    let mut spans = vec![
        Span::raw(CARD_MARGIN),
        Span::styled("╰", Style::default().fg(MUTED)),
    ];
    let hint = hint
        .map(|hint| format!(" {hint} "))
        .filter(|hint| hint.width() + 2 <= inner);
    match hint {
        Some(hint) => {
            let left = inner - hint.width() - 1;
            spans.push(Span::styled("─".repeat(left), Style::default().fg(MUTED)));
            spans.push(Span::styled(hint, bold(MUTED)));
            spans.push(Span::styled("─╯", Style::default().fg(MUTED)));
        }
        None => spans.push(Span::styled(
            format!("{}╯", "─".repeat(inner)),
            Style::default().fg(MUTED),
        )),
    }
    Line::from(spans)
}

/// A collapsed card, which is one item of the list: the handler's rows, a
/// divider, and the sum of its workers with each blocked one named.
fn collapsed_card(
    handler: &DashboardAgent,
    workers: &[DashboardAgent],
    selected: bool,
    area_width: u16,
    now: u64,
) -> Vec<Line<'static>> {
    let rim = Rim::card(area_width);
    let content_area = rim.content_area(area_width);
    let mut lines = vec![card_top(&handler.project_group, area_width)];
    lines.extend(
        handler_lines(handler, selected, content_area, now)
            .into_iter()
            .map(|line| rim.frame(line)),
    );
    lines.push(card_divider(area_width));
    // The same column to spare at the right as every other row.
    let width = card_inner_width(area_width).saturating_sub(1);
    lines.extend(
        worker_summary(workers, width)
            .into_iter()
            .map(|line| rim.frame(line)),
    );
    lines.push(card_bottom(area_width, None));
    lines
}

/// A handler's rows in its card: its identity row, then its message and its
/// tool, each wrapped to at most two rows.
fn handler_lines(
    handler: &DashboardAgent,
    selected: bool,
    area_width: u16,
    now: u64,
) -> Vec<Line<'static>> {
    let text_width = transcript_text_width(area_width);
    let mut lines = vec![agent_status_line(handler, selected, area_width, now)];
    for activity in [&handler.message, &handler.tool] {
        lines.extend(entry_lines(
            activity,
            text_width,
            Some(CARD_ACTIVITY_ROWS),
            selected,
        ));
    }
    lines
}

/// The foot of a collapsed card, `width` columns wide: how many workers the
/// project has and in which states, the key that expands it at the right,
/// and a line naming each blocked worker and what it waits on.
fn worker_summary(workers: &[DashboardAgent], width: usize) -> Vec<Line<'static>> {
    let mut left = vec![Span::raw(BLANK_GUTTER)];
    match workers.len() {
        0 => left.push(Span::styled("no workers", Style::default().fg(MUTED))),
        1 => left.push(Span::styled("1 worker", bold(TEXT))),
        count => left.push(Span::styled(format!("{count} workers"), bold(TEXT))),
    }
    for state in SUMMARY_STATES {
        let count = workers
            .iter()
            .filter(|worker| worker.info.state == state)
            .count();
        if count == 0 {
            continue;
        }
        left.push(Span::raw(SUMMARY_SEPARATOR));
        left.push(Span::styled(state_mark(state), bold(state_color(state))));
        left.push(Span::styled(
            format!(" {count} {}", state_word(state)),
            Style::default().fg(TEXT),
        ));
    }
    let used: usize = left.iter().map(Span::width).sum();
    let hint = format!("{EXPAND_HINT} ");
    if used + SUMMARY_SEPARATOR.width() + hint.width() <= width {
        left.push(Span::raw(" ".repeat(width - used - hint.width())));
        left.push(Span::styled(hint, bold(MUTED)));
    }

    let mut lines = vec![Line::from(left)];
    for worker in workers
        .iter()
        .filter(|worker| worker.info.state == AgentState::Blocked)
    {
        let label = format!("{} blocked: ", state_mark(AgentState::Blocked));
        let waits_on = match worker.message.kind {
            ActivityKind::Question => worker.message.text.as_str(),
            _ => WAITING_FOR_INPUT,
        };
        let room = width.saturating_sub(GUTTER_WIDTH + label.width());
        let task = clip(&worker.task, room);
        let room = room.saturating_sub(task.width() + BLOCKED_SEPARATOR.width());
        let mut spans = vec![
            Span::raw(BLANK_GUTTER),
            Span::styled(label, bold(WARNING)),
            Span::styled(task, bold(TEXT)),
        ];
        if room > 0 {
            spans.push(Span::styled(
                BLOCKED_SEPARATOR,
                Style::default().fg(WARNING),
            ));
            spans.push(Span::styled(
                clip(waits_on, room),
                Style::default().fg(WARNING),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// The mark in front of a state's count in a card's sum, in its color.
fn state_mark(state: AgentState) -> &'static str {
    match state {
        AgentState::Blocked => "▲",
        AgentState::Working => "●",
        AgentState::Done => "✓",
        AgentState::Idle => "○",
        AgentState::Unknown => "?",
    }
}

fn state_word(state: AgentState) -> &'static str {
    match state {
        AgentState::Blocked => "blocked",
        AgentState::Working => "working",
        AgentState::Done => "done",
        AgentState::Idle => "idle",
        AgentState::Unknown => "unknown",
    }
}

/// The furthest down the list can start and still fill `height` rows, so a
/// list that shrank, or a box that grew, shows no blank rows below its end
/// while there are items above the view to fill them with.
fn max_offset(items: &[ListItem<'_>], height: usize) -> usize {
    let mut rows = 0;
    for (index, item) in items.iter().enumerate().rev() {
        rows += item.height();
        if rows > height {
            return index + 1;
        }
    }
    0
}

/// Columns a turn's text is wrapped to: the box, less its borders, the
/// selection gutter, and the marker column every turn starts with.
fn transcript_text_width(area_width: u16) -> usize {
    usize::from(area_width.saturating_sub(3))
        .saturating_sub(GUTTER_WIDTH + TRANSCRIPT_INDENT)
        .max(1)
}

/// What an expanded session shows when Corgi has no transcript for it: the
/// two rows it already had, and why there is nothing more.
fn unread_transcript_rows(agent: &DashboardAgent) -> Vec<Line<'_>> {
    let reason = if agent.info.harness().session_format().is_some() {
        "This session has not written any turns yet"
    } else {
        "Corgi reads transcripts from the session files Claude Code and Codex write"
    };
    vec![
        activity_line(&agent.message, true),
        activity_line(&agent.tool, true),
        Line::from(vec![
            gutter(true),
            Span::styled("○ ", bold(MUTED)),
            Span::styled(reason, Style::default().fg(MUTED)),
        ]),
    ]
}

/// One session's turns as the rows its message row grows into: newest first,
/// each carrying the same selection gutter as the identity line above them, so
/// the block reads as one row that got taller rather than as a new panel.
///
/// Turns stop being added once there are `wanted` rows, which is as far down
/// as anyone is looking; the newest turn is always there in full.
fn transcript_lines(entries: &[Activity], text_width: usize, wanted: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 && lines.len() >= wanted {
            break;
        }
        // The newest turn is normally the agent's closing summary, and a
        // summary cut to two rows is worth nothing, so it is drawn in full.
        let cap = if index == 0 {
            None
        } else {
            Some(TRANSCRIPT_ENTRY_ROWS)
        };
        lines.extend(entry_lines(entry, text_width, cap, true));
        if index == 0 {
            lines.push(Line::from(gutter(true)));
        }
    }
    lines
}

/// The rows one turn draws: its marker on the first row, its text wrapped and
/// indented under it, and an ellipsis on the last row when `max_rows` cut it.
fn entry_lines(
    entry: &Activity,
    text_width: usize,
    max_rows: Option<usize>,
    selected: bool,
) -> Vec<Line<'static>> {
    let mut rows: Vec<String> = Vec::new();
    for paragraph in entry.text.split('\n') {
        let mut wrapped: Vec<String> = wrap_rows(paragraph, text_width)
            .into_iter()
            .map(|range| paragraph[range].to_string())
            .collect();
        // A field that ends exactly on a row boundary gets one more row for
        // the caret to sit on; nothing is being edited here.
        while wrapped.len() > 1 && wrapped.last().is_some_and(|row| row.is_empty()) {
            wrapped.pop();
        }
        rows.extend(wrapped);
        // One row past the cap is enough to know the turn was cut.
        if max_rows.is_some_and(|max_rows| rows.len() > max_rows) {
            break;
        }
    }
    if let Some(max_rows) = max_rows
        && rows.len() > max_rows
    {
        rows.truncate(max_rows);
        if let Some(last) = rows.last_mut() {
            *last = ellipsized(last, text_width);
        }
    }

    let color = activity_color(entry.kind);
    rows.into_iter()
        .enumerate()
        .map(|(index, text)| {
            let prefix = if index == 0 {
                Span::styled(activity_marker(entry.kind), bold(color))
            } else {
                Span::raw(TRANSCRIPT_CONTINUATION)
            };
            Line::from(vec![
                gutter(selected),
                prefix,
                Span::styled(text, Style::default().fg(color)),
            ])
        })
        .collect()
}

fn project_heading(project: &str, area_width: u16) -> Line<'static> {
    // The block consumes one column on either side. Start the label above the
    // agent-state word, then extend the trailing rule to the far edge.
    let label = format!(" {project} ");
    let inner_width = usize::from(area_width.saturating_sub(2));
    let left_rule_width = PROJECT_LABEL_COLUMN
        .saturating_sub(1)
        .min(inner_width.saturating_sub(label.width()));
    let right_rule_width = inner_width.saturating_sub(left_rule_width + label.width());
    Line::from(vec![
        Span::styled("─".repeat(left_rule_width), Style::default().fg(MUTED)),
        Span::styled(label, bold(SUCCESS)),
        Span::styled("─".repeat(right_rule_width), Style::default().fg(MUTED)),
    ])
}

/// One agent: its identity line, the newest thing said in the session, the
/// tool call it is running or last ran, and a blank line before the next. The
/// blank line carries no selection bar, so the cursor ends with the text.
fn agent_lines(agent: &DashboardAgent, selected: bool, area_width: u16, now: u64) -> Vec<Line<'_>> {
    vec![
        agent_status_line(agent, selected, area_width, now),
        activity_line(&agent.message, selected),
        activity_line(&agent.tool, selected),
        Line::from(gutter(false)),
    ]
}

/// A message or tool row: a marker saying who or what it is, then the text.
/// The list clips the text at the right edge, so a long command shows as much
/// of itself as the terminal is wide.
fn activity_line(activity: &Activity, selected: bool) -> Line<'_> {
    let color = activity_color(activity.kind);
    Line::from(vec![
        gutter(selected),
        Span::styled(activity_marker(activity.kind), bold(color)),
        Span::styled(&activity.text, Style::default().fg(color)),
    ])
}

/// The left gutter of an agent row: the selection bar when the row is the one
/// an action would apply to, and blanks of the same width when it is not.
fn gutter(selected: bool) -> Span<'static> {
    if selected {
        Span::styled(SELECTED_GUTTER, bold(ACCENT))
    } else {
        Span::raw(BLANK_GUTTER)
    }
}

/// The identity row of an agent: its state, what it is working on, the model
/// answering, how full its context window is, how long its prompt cache stays
/// warm, and the checkout it edits.
///
/// The fields follow the task summary directly so a row reads as one phrase.
/// Only the summary is clipped, and a row too narrow for everything drops its
/// trailing fields from the right.
pub(super) fn agent_status_line(
    agent: &DashboardAgent,
    selected: bool,
    area_width: u16,
    now: u64,
) -> Line<'static> {
    let marker = gutter(selected);
    let status = format!(" {} ", agent.info.state.label());
    let mut trailing = status_fields(agent, now);
    // The block borders take one column on either side, and one more column
    // keeps the longest row off the right border.
    let inner_width = usize::from(area_width.saturating_sub(3));
    let leading_width = GUTTER_WIDTH + status.width() + SEPARATOR_WIDTH;
    let mut trailing_width: usize = trailing.iter().map(StatusField::width).sum();
    // Narrow terminals lose the trailing fields, least important one first,
    // rather than clipping the summary down to a few letters.
    while inner_width.saturating_sub(leading_width + trailing_width) < TASK_MIN_WIDTH
        && let Some(dropped) = trailing.pop()
    {
        trailing_width -= dropped.width();
    }
    let task = clip(
        &agent.task,
        inner_width.saturating_sub(leading_width + trailing_width),
    );

    let task_style = Style::default().fg(TEXT);
    let mut spans = vec![
        marker,
        Span::styled(
            status,
            Style::default()
                .fg(Color::Black)
                .bg(state_color(agent.info.state))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(FIELD_SEPARATOR, Style::default().fg(MUTED)),
        // The selected summary is bold as well, so the row still stands out
        // in a terminal whose theme renders the accent color faintly.
        Span::styled(
            task,
            if selected {
                task_style.add_modifier(Modifier::BOLD)
            } else {
                task_style
            },
        ),
    ];
    for field in trailing {
        spans.push(Span::styled(field.separator, Style::default().fg(MUTED)));
        spans.extend(field.spans);
    }
    Line::from(spans)
}

fn state_color(state: AgentState) -> Color {
    match state {
        AgentState::Working => ACCENT,
        AgentState::Blocked => WARNING,
        AgentState::Done => SUCCESS,
        AgentState::Idle => MUTED,
        AgentState::Unknown => TOTAL,
    }
}

/// The one character in front of a message, tool, or command row that says
/// who or what it is, with the space that follows it.
fn activity_marker(kind: ActivityKind) -> &'static str {
    match kind {
        ActivityKind::Command => "$ ",
        ActivityKind::Tool => "● ",
        ActivityKind::Prompt => "» ",
        ActivityKind::Message => "› ",
        ActivityKind::Thinking => "… ",
        ActivityKind::Question => "? ",
        ActivityKind::Ready => "○ ",
    }
}

fn activity_color(kind: ActivityKind) -> Color {
    match kind {
        ActivityKind::Command | ActivityKind::Tool => ACCENT,
        ActivityKind::Question => WARNING,
        // The user's own words stand apart from the assistant's in the
        // success color, which no other row text uses.
        ActivityKind::Prompt => SUCCESS,
        ActivityKind::Thinking => Color::LightBlue,
        ActivityKind::Message => TEXT,
        ActivityKind::Ready => MUTED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::AgentInfo,
        test_support::{buffer_rows, line_text, lines_text, test_app, test_terminal},
        ui::{
            draw,
            tests::{NOW, dashboard_agent},
        },
    };

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// The bar a selected row's gutter starts with.
    const SELECTION_BAR: &str = "██";
    /// Rows an agent takes in the list: its identity, its message, its tool,
    /// and the blank line that ends it.
    const AGENT_ROWS: usize = 4;

    /// A row with nothing on it but the box edge and the selection gutter.
    fn is_blank_row(row: &str) -> bool {
        row.trim_matches(|character| matches!(character, '│' | '█' | ' '))
            .is_empty()
    }

    fn press_key(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn rendered_screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = test_terminal(width, height);
        terminal
            .draw(|frame| draw(frame, app))
            .expect("draw dashboard");
        buffer_rows(terminal.backend().buffer())
    }

    #[test]
    fn the_expanded_transcript_leads_with_the_newest_turn_in_full() {
        let entries = vec![
            Activity {
                kind: ActivityKind::Message,
                text: "The uploader now retries three times before it gives up, and a new test covers the backoff.".into(),
            },
            Activity {
                kind: ActivityKind::Command,
                text: "cargo test --workspace --all-features -- --nocapture".into(),
            },
            Activity {
                kind: ActivityKind::Prompt,
                text: "Add a retry to the uploader and a test that covers the exponential backoff path".into(),
            },
        ];
        let text_width = 28;

        let rows = lines_text(&transcript_lines(&entries, text_width, usize::MAX));

        // Every turn carries the selected row's gutter, so the block reads as
        // that row having grown rather than as a panel of its own.
        assert!(rows.iter().all(|row| row.starts_with(SELECTION_BAR)));
        // The newest turn is on top and keeps every row it needs, because it
        // is normally the summary the reader expanded the session for.
        assert!(rows[0].starts_with("██  › The uploader now retries"));
        let newest = rows.iter().take_while(|row| !is_blank_row(row)).count();
        assert!(newest > TRANSCRIPT_ENTRY_ROWS, "{rows:?}");
        // A blank row, gutter and all, sets it apart from the history.
        assert!(is_blank_row(&rows[newest]));

        // Every older turn is held to two rows, with the cut marked.
        let history = &rows[newest + 1..];
        assert_eq!(history.len(), 2 * TRANSCRIPT_ENTRY_ROWS, "{history:?}");
        assert!(history[0].starts_with("██  $ cargo test"));
        assert!(history[1].ends_with('…'));
        assert!(history[2].starts_with("██  » Add a retry"));
        assert!(history[3].ends_with('…'));
        // Continuation rows sit under the text, not under the marker.
        assert!(history[1].starts_with("██    ") && history[3].starts_with("██    "));
        // Nothing overflows the width it was wrapped to.
        let width = GUTTER_WIDTH + TRANSCRIPT_INDENT + text_width;
        assert!(rows.iter().all(|row| row.width() <= width), "{rows:?}");
    }

    #[test]
    fn the_transcript_stops_building_rows_below_the_view_but_keeps_the_newest_turn_whole() {
        let newest = Activity {
            kind: ActivityKind::Message,
            text: "word ".repeat(40),
        };
        let older = |index: usize| Activity {
            kind: ActivityKind::Command,
            text: format!("cargo test turn-{index}"),
        };
        let mut entries = vec![newest];
        entries.extend((0..60).map(older));
        let all = lines_text(&transcript_lines(&entries, 20, usize::MAX));

        // Asked for fewer rows than the newest turn has, it is still whole.
        let newest_rows = all.iter().take_while(|row| !is_blank_row(row)).count();
        let short = lines_text(&transcript_lines(&entries, 20, 2));
        assert_eq!(short.len(), newest_rows + 1, "{short:?}");

        // Beyond that, turns are added until the rows asked for are there,
        // and every row built is the one the full list has in its place.
        let some = lines_text(&transcript_lines(&entries, 20, newest_rows + 5));
        assert!(some.len() >= newest_rows + 5 && some.len() < all.len());
        assert_eq!(some[..], all[..some.len()]);
    }

    #[test]
    fn an_uncapped_turn_keeps_the_paragraphs_it_was_written_with() {
        let entry = Activity {
            kind: ActivityKind::Message,
            text: "Done.\n\n- Added the retry\n- Added the test".into(),
        };

        let rows = lines_text(&entry_lines(&entry, 40, None, true));

        assert_eq!(rows[0], "██  › Done.");
        assert!(is_blank_row(&rows[1]));
        assert_eq!(rows[2], "██    - Added the retry");
        assert_eq!(rows[3], "██    - Added the test");
    }

    #[test]
    fn expanding_grows_the_selected_session_where_it_already_sits() {
        let session = |task: &str| DashboardAgent {
            project_group: "corgi".into(),
            task: task.into(),
            ..DashboardAgent::default()
        };
        let mut app = test_app();
        app.agents = vec![
            session("First task"),
            session("Second task"),
            session("Third task"),
        ];
        app.selected = 1;

        let collapsed = rendered_screen(&mut app, 100, 30);

        app.expanded = true;
        app.transcript = vec![
            Activity {
                kind: ActivityKind::Message,
                text: "The uploader now retries.".into(),
            },
            Activity {
                kind: ActivityKind::Command,
                text: "cargo test --workspace".into(),
            },
            Activity {
                kind: ActivityKind::Prompt,
                text: "Add a retry to the uploader".into(),
            },
        ]
        .into();
        let expanded = rendered_screen(&mut app, 100, 30);

        // Everything down to the selected session's identity line is drawn
        // exactly where it was, including the session above it.
        let identity = expanded
            .iter()
            .position(|row| row.contains("Second task"))
            .expect("identity row");
        assert_eq!(collapsed[..=identity], expanded[..=identity]);
        assert!(collapsed[identity - AGENT_ROWS].contains("First task"));

        // Its message row is where the turns begin, newest first, and they
        // run on down the same box rather than into a panel of their own.
        assert!(expanded[identity + 1].contains("› The uploader now retries."));
        assert!(is_blank_row(&expanded[identity + 2]));
        assert!(expanded[identity + 3].contains("$ cargo test --workspace"));
        assert!(expanded[identity + 4].contains("» Add a retry to the uploader"));
        assert_eq!(
            expanded
                .iter()
                .filter(|row| row.contains(" Agents "))
                .count(),
            1
        );

        // The sessions below it are what gives way.
        assert!(collapsed.iter().any(|row| row.contains("Third task")));
        assert!(!expanded.iter().any(|row| row.contains("Third task")));
    }

    #[test]
    fn a_session_low_in_a_short_list_pulls_the_ones_above_it_off_the_top() {
        let session = |task: &str| DashboardAgent {
            project_group: "corgi".into(),
            task: task.into(),
            ..DashboardAgent::default()
        };
        let mut app = test_app();
        app.agents = vec![
            session("First task"),
            session("Second task"),
            session("Third task"),
        ];
        app.selected = 2;
        app.expanded = true;
        app.transcript = (0..12)
            .map(|index| Activity {
                kind: ActivityKind::Command,
                text: format!("cargo test turn-{index}"),
            })
            .collect();

        let screen = rendered_screen(&mut app, 100, 20);

        // There was no room to keep them and still say anything useful, so
        // the sessions above gave way rather than the transcript.
        assert!(screen.iter().any(|row| row.contains("Third task")));
        assert!(!screen.iter().any(|row| row.contains("First task")));
        assert!(!screen.iter().any(|row| row.contains("Second task")));
        assert!(usize::from(app.transcript_page) >= MIN_TRANSCRIPT_ROWS);
        assert!(screen.iter().any(|row| row.contains("$ cargo test turn-0")));
    }

    fn numbered_sessions(count: usize) -> Vec<DashboardAgent> {
        (0..count)
            .map(|index| DashboardAgent {
                project_group: "corgi".into(),
                task: format!("Task number {index}"),
                ..DashboardAgent::default()
            })
            .collect()
    }

    /// The session drawn highest in the list, and the row the selection bar
    /// starts on.
    fn top_task_and_bar(screen: &[String]) -> (String, usize) {
        let top = screen
            .iter()
            .find_map(|row| {
                let start = row.find("Task number ")?;
                Some(
                    row[start..]
                        .split_whitespace()
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            })
            .expect("a session row");
        let bar = screen
            .iter()
            .position(|row| row.contains(SELECTION_BAR) && row.contains("Task number "))
            .expect("selection bar");
        (top, bar)
    }

    #[test]
    fn moving_up_inside_a_scrolled_list_moves_the_bar_and_not_the_view() {
        let (width, height) = (100, 24);
        let mut app = test_app();
        app.agents = numbered_sessions(10);

        // Move down until the view has to scroll to follow the selection.
        let (first_top, _) = top_task_and_bar(&rendered_screen(&mut app, width, height));
        let mut screen;
        loop {
            app.selected += 1;
            screen = rendered_screen(&mut app, width, height);
            if top_task_and_bar(&screen).0 != first_top {
                break;
            }
            assert!(app.selected < 9, "the view never scrolled");
        }
        let (top, bar) = top_task_and_bar(&screen);
        assert!(app.agent_list_offset > 0);

        // One up and the same session is still on top; the bar moved instead.
        app.selected -= 1;
        let screen = rendered_screen(&mut app, width, height);
        assert_eq!(top_task_and_bar(&screen), (top.clone(), bar - AGENT_ROWS));

        // The view only follows once the selection passes its top edge.
        let top_index: usize = top["Task number ".len()..].parse().expect("index");
        while app.selected > top_index {
            app.selected -= 1;
            let screen = rendered_screen(&mut app, width, height);
            assert_eq!(top_task_and_bar(&screen).0, top);
        }
        app.selected -= 1;
        let screen = rendered_screen(&mut app, width, height);
        let (new_top, _) = top_task_and_bar(&screen);
        assert_eq!(new_top, format!("Task number {}", app.selected));
    }

    #[test]
    fn a_shrinking_list_or_growing_box_pulls_the_view_back_over_blank_rows() {
        let mut app = test_app();
        app.agents = numbered_sessions(10);
        app.selected = 9;
        rendered_screen(&mut app, 100, 24);
        let scrolled = app.agent_list_offset;
        assert!(scrolled > 0);

        // A taller box has room for more of the sessions above the view.
        rendered_screen(&mut app, 100, 40);
        assert!(app.agent_list_offset < scrolled);
        let screen = rendered_screen(&mut app, 100, 40);
        assert!(screen.iter().any(|row| row.contains("Task number 9")));

        // Sessions closing below the view leave nothing to scroll past.
        app.agents.truncate(2);
        app.selected = 1;
        let screen = rendered_screen(&mut app, 100, 24);
        assert_eq!(app.agent_list_offset, 0);
        assert!(screen.iter().any(|row| row.contains(" corgi ")));
        assert_eq!(top_task_and_bar(&screen).0, "Task number 0");

        // A shorter box still shows the selection, whatever the saved offset.
        app.agents = numbered_sessions(10);
        app.selected = 0;
        app.agent_list_offset = usize::MAX;
        let screen = rendered_screen(&mut app, 100, 16);
        let (top, _) = top_task_and_bar(&screen);
        assert_eq!(top, "Task number 0");
    }

    #[test]
    fn collapsing_an_expanded_session_keeps_the_list_where_it_was() {
        let mut app = test_app();
        app.agents = numbered_sessions(10);
        app.selected = 7;
        let before = rendered_screen(&mut app, 100, 24);
        let offset = app.agent_list_offset;

        app.expanded = true;
        rendered_screen(&mut app, 100, 24);
        assert_eq!(app.agent_list_offset, offset);

        app.expanded = false;
        assert_eq!(rendered_screen(&mut app, 100, 24), before);
    }

    fn member(group: &str, task: &str, state: AgentState, handler: bool) -> DashboardAgent {
        DashboardAgent {
            info: AgentInfo {
                state,
                ..AgentInfo::default()
            },
            project_group: group.into(),
            task: task.into(),
            handler,
            ..DashboardAgent::default()
        }
    }

    /// A handler project whose handler talks at length, with a blocked, a
    /// working and a finished worker; a project without a handler; and a
    /// scratch session.
    fn carded_herd() -> Vec<DashboardAgent> {
        let mut handler = member("webshop", "Project handler", AgentState::Idle, true);
        handler.message = Activity {
            kind: ActivityKind::Message,
            text: "word ".repeat(60),
        };
        handler.tool = Activity {
            kind: ActivityKind::Command,
            text: "git log --oneline main..HEAD".into(),
        };
        let mut blocked = member("webshop", "Retry card payments", AgentState::Blocked, false);
        blocked.message = Activity {
            kind: ActivityKind::Question,
            text: "Allow Bash: npm test?".into(),
        };
        vec![
            handler,
            blocked,
            member("webshop", "Cart badge count", AgentState::Working, false),
            member("webshop", "Paginate orders", AgentState::Done, false),
            member("notes", "Export as Markdown", AgentState::Idle, false),
            DashboardAgent {
                scratch: true,
                ..member("Scratch", "Look something up", AgentState::Idle, false)
            },
        ]
    }

    fn row_with<'a>(screen: &'a [String], needle: &str) -> &'a str {
        screen
            .iter()
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("no row with {needle:?} in {screen:#?}"))
    }

    #[test]
    fn a_handler_project_opens_as_a_collapsed_card_of_its_handler_and_its_workers_sum() {
        let mut app = test_app();
        app.agents = carded_herd();
        let screen = rendered_screen(&mut app, 100, 40);

        // The card's title is the project, inside the Agents box.
        let top = screen
            .iter()
            .position(|row| row.starts_with("│ ╭ webshop ─"))
            .expect("card top");
        assert!(screen[top].ends_with("╮ │"), "{}", screen[top]);
        // The handler's identity row, then its message wrapped to two rows
        // and cut, then its tool.
        assert!(
            screen[top + 1].contains("IDLE  · Project handler"),
            "{screen:#?}"
        );
        assert!(screen[top + 1].contains(SELECTION_BAR));
        assert!(screen[top + 2].contains("› word word"));
        assert!(screen[top + 3].trim_end_matches([' ', '│']).ends_with('…'));
        assert!(screen[top + 4].contains("$ git log --oneline main..HEAD"));
        // A divider joined to the card's sides, then the sum, in state order,
        // with the key that expands it at the right.
        assert!(screen[top + 5].starts_with("│ ├─") && screen[top + 5].contains("┤"));
        let sum = &screen[top + 6];
        assert!(
            sum.contains("3 workers   ▲ 1 blocked   ● 1 working   ✓ 1 done"),
            "{sum}"
        );
        assert!(!sum.contains("idle"), "{sum}");
        assert!(
            sum.trim_end_matches([' ', '│']).ends_with(EXPAND_HINT),
            "{sum}"
        );
        // The blocked worker is named, with what it waits on.
        assert!(
            screen[top + 7].contains("▲ blocked: Retry card payments — Allow Bash: npm test?"),
            "{}",
            screen[top + 7]
        );
        assert!(screen[top + 8].starts_with("│ ╰─"));
        // The other workers have no rows of their own.
        assert!(!screen.iter().any(|row| row.contains("Cart badge count")));
        assert!(!screen.iter().any(|row| row.contains("Paginate orders")));

        // A project without a handler and the scratch sessions are a heading
        // over their rows, as before.
        assert!(row_with(&screen, " notes ").starts_with("│────"));
        assert!(row_with(&screen, "Export as Markdown").contains("IDLE"));
        assert!(row_with(&screen, " Scratch ").starts_with("│────"));
        assert!(row_with(&screen, "Look something up").contains("IDLE"));
        assert_eq!(
            screen.iter().filter(|row| row.starts_with("│ ╭")).count(),
            1,
            "only the one card: {screen:#?}"
        );
    }

    #[test]
    fn a_blocked_worker_without_a_question_on_screen_waits_for_input() {
        let workers = vec![member("webshop", "Retry", AgentState::Blocked, false)];
        let lines = lines_text(&worker_summary(&workers, 80));
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("1 worker   ▲ 1 blocked"), "{lines:?}");
        assert!(lines[1].ends_with("▲ blocked: Retry — waiting for your input"));
        // A narrow card clips what it waits on before the task.
        let narrow = lines_text(&worker_summary(&workers, 30));
        assert!(narrow[1].contains("Retry"), "{narrow:?}");
        assert!(narrow.iter().all(|row| row.width() <= 30), "{narrow:?}");
        // Nothing to sum up is said so.
        assert!(lines_text(&worker_summary(&[], 80))[0].contains("no workers"));
    }

    #[test]
    fn an_expanded_card_lists_its_workers_rows_and_drops_the_sum() {
        let mut app = test_app();
        app.agents = carded_herd();
        app.cards.set_expanded("webshop", true);
        app.selected = 2;
        let screen = rendered_screen(&mut app, 100, 40);

        let handler = screen
            .iter()
            .position(|row| row.contains("IDLE  · Project handler"))
            .expect("handler");
        // Each worker's three rows, in the list's order, inside the card.
        // The handler's message wraps to two rows, then its tool and a gap.
        let blocked = handler + 5;
        assert!(screen[blocked].contains("BLOCKED  · Retry card payments"));
        assert!(screen[blocked + 1].contains("? Allow Bash: npm test?"));
        let working = blocked + AGENT_ROWS;
        assert!(screen[working].contains("WORKING  · Cart badge count"));
        assert!(screen[working].contains(SELECTION_BAR));
        assert!(screen[working + AGENT_ROWS].contains("DONE  · Paginate orders"));
        for row in &screen[handler..working + 2 * AGENT_ROWS] {
            assert!(row.starts_with("│ │") && row.ends_with("│ │"), "{row}");
        }
        // No sum and no divider, and the card closes with its collapse key.
        assert!(!screen.iter().any(|row| row.contains("workers")));
        assert!(!screen.iter().any(|row| row.contains("├")));
        assert!(row_with(&screen, COLLAPSE_HINT).starts_with("│ ╰─"));
    }

    #[test]
    fn a_session_opened_in_a_card_grows_inside_it() {
        let mut app = test_app();
        app.agents = carded_herd();
        let collapsed = rendered_screen(&mut app, 100, 30);
        app.expanded = true;
        app.transcript = vec![
            Activity {
                kind: ActivityKind::Message,
                text: "Both workers are reviewed.".into(),
            },
            Activity {
                kind: ActivityKind::Command,
                text: "corgi fleet webshop".into(),
            },
        ]
        .into();
        let expanded = rendered_screen(&mut app, 100, 30);

        // The card's top and the handler's identity row stay where they were.
        let identity = expanded
            .iter()
            .position(|row| row.contains("Project handler"))
            .expect("identity");
        assert_eq!(collapsed[..=identity], expanded[..=identity]);
        // Its turns follow inside the card, and the card closes under them.
        assert!(expanded[identity + 1].contains("› Both workers are reviewed."));
        assert!(expanded[identity + 1].starts_with("│ │██"));
        assert!(expanded[identity + 3].contains("$ corgi fleet webshop"));
        assert!(expanded[identity + 4].starts_with("│ ╰─"), "{expanded:#?}");
        assert!(!expanded.iter().any(|row| row.contains("3 workers")));
    }

    fn numbered_cards(count: usize) -> Vec<DashboardAgent> {
        (0..count)
            .flat_map(|index| {
                let project = format!("project-{index:02}");
                [
                    member(&project, "Project handler", AgentState::Idle, true),
                    member(&project, "A worker", AgentState::Working, false),
                ]
            })
            .collect()
    }

    /// The card drawn highest in the list, whole or cut off at the top, and
    /// the row the selection bar starts on.
    fn top_card_and_bar(screen: &[String]) -> (String, usize) {
        let box_top = screen
            .iter()
            .position(|row| row.contains(" Agents "))
            .expect("Agents box");
        let top = screen[box_top + 1]
            .split_whitespace()
            .find(|word| word.starts_with("project-"))
            .map(str::to_string)
            .unwrap_or_default();
        let bar = screen
            .iter()
            .position(|row| row.starts_with("│ │██"))
            .expect("selection bar");
        (top, bar)
    }

    #[test]
    fn a_list_of_cards_scrolls_only_when_the_selection_passes_an_edge() {
        let (width, height) = (100, 40);
        let mut app = test_app();
        app.agents = numbered_cards(8);

        // Down card by card until the view has to scroll to follow.
        let (first_top, _) = top_card_and_bar(&rendered_screen(&mut app, width, height));
        assert_eq!(first_top, "project-00");
        let mut screen;
        loop {
            press_key(&mut app, KeyCode::Char('j'));
            screen = rendered_screen(&mut app, width, height);
            if top_card_and_bar(&screen).0 != first_top {
                break;
            }
            assert!(app.selected < 14, "the view never scrolled");
        }
        let (top, bar) = top_card_and_bar(&screen);
        assert!(app.agent_list_offset > 0);

        // One card up and the same card is still on top; the bar moved by a
        // card and its gap instead.
        press_key(&mut app, KeyCode::Char('k'));
        let screen = rendered_screen(&mut app, width, height);
        let (same_top, higher_bar) = top_card_and_bar(&screen);
        assert_eq!(same_top, top);
        assert!(higher_bar < bar, "{higher_bar} {bar} {screen:#?}");

        // Expanding a card in view, and collapsing it again, keeps the view.
        let offset = app.agent_list_offset;
        press_key(&mut app, KeyCode::Right);
        let screen = rendered_screen(&mut app, width, height);
        assert_eq!(top_card_and_bar(&screen).0, top);
        press_key(&mut app, KeyCode::Char('j'));
        press_key(&mut app, KeyCode::Left);
        let screen = rendered_screen(&mut app, width, height);
        assert_eq!(top_card_and_bar(&screen), (same_top, higher_bar));
        assert_eq!(app.agent_list_offset, offset);

        // Back up past the top card, and the view follows.
        let top_card = app.agents[app.selected].project_group.clone();
        while app.agents[app.selected].project_group != top {
            press_key(&mut app, KeyCode::Char('k'));
            rendered_screen(&mut app, width, height);
        }
        assert_ne!(top_card, "");
        press_key(&mut app, KeyCode::Char('k'));
        let screen = rendered_screen(&mut app, width, height);
        assert_eq!(
            top_card_and_bar(&screen).0,
            app.agents[app.selected].project_group
        );
    }

    #[test]
    fn a_projects_first_session_never_shows_without_its_heading() {
        let (width, height) = (100, 30);
        let mut app = test_app();
        let mut agents = Vec::new();
        for project in ["alpha", "bravo", "charlie"] {
            for index in 0..3 {
                agents.push(member(
                    project,
                    &format!("{project} task {index}"),
                    AgentState::Idle,
                    false,
                ));
            }
        }
        agents.push(member("delta", "Project handler", AgentState::Idle, true));
        agents.push(member("delta", "delta task 1", AgentState::Idle, false));
        agents.push(member("delta", "delta task 2", AgentState::Idle, false));
        for index in 0..3 {
            agents.push(DashboardAgent {
                scratch: true,
                ..member(
                    "Scratch",
                    &format!("Scratch task {index}"),
                    AgentState::Idle,
                    false,
                )
            });
        }
        app.agents = agents;
        app.cards.set_expanded("delta", true);

        // Every first session on screen sits right under its heading, or
        // its card's top, whichever way the list scrolled to it.
        let check = |screen: &[String]| {
            for (project, first) in [
                ("alpha", "alpha task 0"),
                ("bravo", "bravo task 0"),
                ("charlie", "charlie task 0"),
                ("delta", "Project handler"),
                ("Scratch", "Scratch task 0"),
            ] {
                if let Some(row) = screen.iter().position(|row| row.contains(first)) {
                    let heading = &screen[row - 1];
                    assert!(
                        heading.contains(&format!(" {project} "))
                            && (heading.starts_with("│─") || heading.starts_with("│ ╭")),
                        "{first} without its heading: {screen:#?}"
                    );
                }
            }
        };
        let last = app.agents.len() - 1;
        while app.selected < last {
            press_key(&mut app, KeyCode::Char('j'));
            check(&rendered_screen(&mut app, width, height));
        }
        assert!(app.agent_list_offset > 0);
        let mut seen_at_top = Vec::new();
        while app.selected > 0 {
            press_key(&mut app, KeyCode::Char('k'));
            let screen = rendered_screen(&mut app, width, height);
            check(&screen);
            seen_at_top.push(screen[usize::from(crate::ui::header::HEADER_HEIGHT) + 1].clone());
        }
        // Scrolling up did stop on projects' first sessions, with their
        // headings as the view's top row.
        for project in ["alpha", "bravo", "charlie"] {
            assert!(
                seen_at_top
                    .iter()
                    .any(|row| row.starts_with("│─") && row.contains(&format!(" {project} "))),
                "{project}: {seen_at_top:#?}"
            );
        }
    }

    #[test]
    fn a_status_line_shows_state_task_model_context_and_worktree_in_that_order() {
        let agent = dashboard_agent();
        let line = line_text(&agent_status_line(&agent, true, 100, NOW));

        let status = line.find("WORKING").expect("status");
        let task = line.find("Agent session status line").expect("task");
        let model = line.find("Opus 5").expect("model");
        let effort = line.find("high").expect("effort");
        let context = line.find("13% ctx").expect("context");
        let worktree = line.find("silver-cloud-028f").expect("worktree");
        assert!(
            status < task
                && task < model
                && model < effort
                && effort < context
                && context < worktree,
            "{line}"
        );
        // The fields follow the summary instead of being pushed to the far
        // edge of the list.
        assert!(line.width() < 97, "{line}");
        assert!(line.contains("Opus 5 high"), "{line}");
        assert!(!line.contains("Opus 5 · high"), "{line}");
        // The badge is followed by the same dot as every field after the task.
        assert!(line.contains("WORKING  · Agent session"), "{line}");
        assert!(
            line.contains("Agent session status line · Opus 5"),
            "{line}"
        );
    }

    #[test]
    fn the_selection_bar_marks_every_line_of_the_selected_row_without_shifting_the_columns() {
        let agent = dashboard_agent();
        let selected = line_text(&agent_status_line(&agent, true, 100, NOW));
        let unselected = line_text(&agent_status_line(&agent, false, 100, NOW));

        assert!(selected.starts_with(SELECTION_BAR), "{selected}");
        assert!(!unselected.contains(SELECTION_BAR), "{unselected}");
        // The gutter is the same width either way, so the status word does not
        // move as the cursor passes over a row. Compared in columns rather
        // than byte offsets, because the bar is multi-byte.
        let status_column = |line: &str| {
            line.split("WORKING")
                .next()
                .map(|before| before.width())
                .expect("status")
        };
        assert_eq!(
            status_column(&selected),
            status_column(&unselected),
            "{selected} / {unselected}"
        );
        // The bar continues down the message and tool lines of the row.
        assert_eq!(gutter(true).width(), GUTTER_WIDTH);
        assert!(gutter(true).content.starts_with(SELECTION_BAR));
        assert_eq!(gutter(false).width(), GUTTER_WIDTH);
        assert!(gutter(false).content.trim().is_empty());
    }

    #[test]
    fn a_row_without_a_reported_model_falls_back_to_the_agent_kind() {
        let agent = DashboardAgent {
            model: None,
            ..dashboard_agent()
        };
        assert!(line_text(&agent_status_line(&agent, false, 80, NOW)).contains("claude"));
    }

    #[test]
    fn a_narrow_row_drops_trailing_fields_before_clipping_the_task() {
        let agent = dashboard_agent();
        // Wide enough for the model but not for the worktree.
        let line = line_text(&agent_status_line(&agent, false, 60, NOW));
        assert!(line.contains("Opus 5"), "{line}");
        assert!(!line.contains("silver-cloud-028f"), "{line}");

        // Too narrow for any field, and the summary is clipped rather than
        // the status hidden.
        let line = line_text(&agent_status_line(&agent, false, 30, NOW));
        assert!(line.contains("WORKING"), "{line}");
        assert!(!line.contains("Opus 5"), "{line}");
        assert!(line.ends_with('…'), "{line}");
        assert!(line.width() <= 27, "{line}");
    }
}
