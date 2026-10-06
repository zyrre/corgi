use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Padding, Paragraph},
};

use unicode_width::UnicodeWidthStr;

use crate::{
    app::{App, UsageSlot},
    model::AgentState,
    time::unix_now,
    usage::UsageWindow,
};

use super::status::model_color;
use super::{
    ACCENT, DANGER, MERGE, MERGE_MARK, MUTED, SUCCESS, TEXT, WARNING, bold, clip, rounded_block,
};

// Each subscription gets ten content columns: exactly `Weekly 99%`, with one
// blank column between the content and each side of its border.
const USAGE_CARD_WIDTH: u16 = 14;
const USAGE_CARD_PADDING: u16 = 1;
// A note shorter than its label plus this much reason is all label and no
// answer, so the header drops it rather than print a bare provider name.
const USAGE_NOTE_MIN_REASON: usize = 8;
const USAGE_CONTENT_WIDTH: u16 = USAGE_CARD_WIDTH - 2 - 2 * USAGE_CARD_PADDING;
// The herd card leads with the whole pack's total, followed by one row per
// state. It is wide enough for a two-digit count beside its summation mark.
const HERD_WIDTH: u16 = 15;
// Two rules and five counts. A box rule is painted
// across the middle of its cell while the wordmark's bars fill theirs edge to
// edge, so the card can never sit flush with the letters: any row it starts on
// leaves it half a row out. Its fifth content row lets it start one row above
// them instead, which puts its top rule half a row above their top edge and
// its bottom rule half a row below their baseline — out by the same half on
// both sides, so it reads as bracketing the letters rather than as misaligned.
const HERD_HEIGHT: u16 = 7;
// The supervisor sits in the header's top-left corner with the wordmark directly
// beside it, so the mascot and the letters read as one lockup. The art's
// blank first pixel row already holds it one pixel clear of the top edge,
// which is half a terminal row, so it is drawn flush against it.
// Wide enough to separate the mascot from the letters without breaking the
// lockup, matching the wordmark's own letter tracking.
const CORGI_GAP: u16 = 2;
// Keep the compact header cards visually distinct without wasting a column.
const CARD_GAP: u16 = 1;
// Leave an extra blank column after the I before the header cards begin.
const WORDMARK_CARD_GAP: u16 = CARD_GAP + 1;
// The supervisor and wordmark form the brand lockup; the herd and one card per
// detected subscription follow it when the whole row fits.
const HEADER_BRAND_WIDTH: u16 = CORGI_WIDTH + CORGI_GAP + WORDMARK_WIDTH;
// The header is as tall as its cards, which is also room enough for the
// supervisor and the wordmark beside them.
pub(super) const HEADER_HEIGHT: u16 = HERD_HEIGHT;
const _: () = assert!(CORGI_ROWS <= HEADER_HEIGHT && WORDMARK.len() as u16 <= HEADER_HEIGHT);

// One card in the header's band. They are laid out left to right after the
// wordmark, each starting where the last ended, and all share the band's top
// edge and height so the header reads as one row of blocks.
struct HeaderCard<'a> {
    width: u16,
    horizontal_padding: u16,
    title: &'static str,
    color: Color,
    lines: Vec<Line<'a>>,
}

// A card is drawn only when it fits whole. Clipping a bordered box leaves a
// frame with no right edge, which reads as a rendering fault rather than as a
// header that ran out of room, and once one card is dropped every card after it
// would be pulled left out of line with the band.
//
// Returns the first column the cards left free, which is where the header's
// notes begin.
fn draw_header_cards(frame: &mut Frame<'_>, band: Rect, cards: Vec<HeaderCard<'_>>) -> u16 {
    let mut x = band.x;
    for card in cards {
        if (band.x + band.width).saturating_sub(x) < card.width {
            break;
        }
        frame.render_widget(
            Paragraph::new(card.lines).block(
                rounded_block(&format!(" {} ", card.title), Some(card.color))
                    .padding(Padding::horizontal(card.horizontal_padding)),
            ),
            Rect::new(x, band.y, card.width, band.height),
        );
        x += card.width + CARD_GAP;
    }
    x
}

pub(super) fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    // An agent tagged ready to merge counts as MERGE rather than done or
    // idle, as its badge reads.
    let count = |state: AgentState| {
        app.agents
            .iter()
            .filter(|agent| agent.info.state == state && !agent.ready_to_merge())
            .count()
    };
    let working = count(AgentState::Working);
    let blocked = count(AgentState::Blocked);
    let merge = app
        .agents
        .iter()
        .filter(|agent| agent.ready_to_merge())
        .count();
    let done = count(AgentState::Done);
    // Unknown is counted as idle: neither is doing anything, and a separate
    // row for a state the user cannot act on would only dilute the card.
    let idle = count(AgentState::Idle) + count(AgentState::Unknown);
    let agent_color = if blocked > 0 {
        WARNING
    } else if working > 0 {
        ACCENT
    } else if merge > 0 {
        MERGE
    } else if done > 0 {
        SUCCESS
    } else {
        MUTED
    };
    // The supervisor hugs the top-left corner and only appears when the header is
    // tall enough to hold it and wide enough to leave room for the band.
    let show_corgi = area.height >= CORGI_ROWS
        && area.width >= HEADER_BRAND_WIDTH + WORDMARK_CARD_GAP + header_cards_width(app);
    if show_corgi {
        draw_corgi(frame, Rect::new(area.x, area.y, CORGI_WIDTH, CORGI_ROWS));
    }
    // The letters start right after the supervisor and the cards follow them.
    let wordmark_x = if show_corgi {
        area.x + CORGI_WIDTH + CORGI_GAP
    } else {
        area.x
    };
    let wordmark_width =
        WORDMARK_WIDTH.min((area.x + area.width).saturating_sub(wordmark_x + WORDMARK_CARD_GAP));
    draw_wordmark(
        frame,
        Rect::new(wordmark_x, area.y, wordmark_width, area.height),
    );
    // Every card is as tall as the tallest card's content, so they line up with
    // each other as well as with the letters.
    let band_height = HERD_HEIGHT.min(area.height);
    // The wordmark is centred against the taller corgi, and the band opens one
    // row above it. See HERD_HEIGHT for why that row matters.
    let wordmark_y = area.y + area.height.saturating_sub(WORDMARK.len() as u16) / 2;
    let band_y = wordmark_y
        .saturating_sub(1)
        .max(area.y)
        .min(area.y + area.height.saturating_sub(band_height));
    let band_x = wordmark_x + wordmark_width + WORDMARK_CARD_GAP;
    let band = Rect::new(
        band_x,
        band_y,
        (area.x + area.width).saturating_sub(band_x),
        band_height,
    );
    let mut cards = vec![HeaderCard {
        width: HERD_WIDTH,
        horizontal_padding: 0,
        title: "HERD",
        color: agent_color,
        // The pack by state, the ones ready to merge right after the
        // blocked, as the next thing the user can act on.
        lines: vec![
            Line::from(agent_summary_spans("●", working, "WORKING", ACCENT)),
            Line::from(agent_summary_spans("!", blocked, "BLOCKED", WARNING)),
            Line::from(agent_summary_spans(MERGE_MARK, merge, "MERGE", MERGE)),
            Line::from(agent_summary_spans("✓", done, "DONE", SUCCESS)),
            Line::from(agent_summary_spans("·", idle, "IDLE", MUTED)),
        ],
    }];
    let now = unix_now();
    cards.extend(
        app.usage
            .iter()
            .filter(|slot| show_usage_card(slot))
            .map(|slot| HeaderCard {
                width: USAGE_CARD_WIDTH,
                horizontal_padding: USAGE_CARD_PADDING,
                title: slot.provider.card_title(),
                color: model_color(slot.provider.label()),
                lines: usage_lines_for_slot(slot, now),
            }),
    );
    let notes_x = draw_header_cards(frame, band, cards);
    draw_usage_notes(frame, band, notes_x, app);
}

fn agent_summary_spans(
    icon: &'static str,
    count: usize,
    label: &'static str,
    status_color: Color,
) -> Vec<Span<'static>> {
    let style = bold(status_color);
    // Reserve three columns before every count: one leading cell, then the
    // icon padded to two cells, so every number in the card sits in the
    // same column whether its icon is one cell wide or two.
    let icon_gap = " ".repeat(2usize.saturating_sub(icon.width()));
    vec![
        Span::styled(format!(" {icon}{icon_gap}"), style),
        Span::styled(count.to_string(), style),
        Span::styled(format!(" {label}"), style),
    ]
}

fn header_cards_width(app: &App) -> u16 {
    app.usage
        .iter()
        .filter(|slot| show_usage_card(slot))
        .fold(HERD_WIDTH, |width, _| {
            width.saturating_add(CARD_GAP + USAGE_CARD_WIDTH)
        })
}

pub(crate) fn show_usage_card(slot: &UsageSlot) -> bool {
    slot.usage.is_some() || slot.error.is_none()
}

/// Why a provider whose CLI is installed shows no usage card, as one line for
/// the header's top right corner. `None` when the slot has a card, or when
/// `width` leaves too little room for the reason to say anything.
fn usage_note_line(slot: &UsageSlot, width: u16) -> Option<Line<'static>> {
    if show_usage_card(slot) {
        return None;
    }
    let reason = slot.error.as_deref()?;
    let label = format!("⚠ {} ", slot.provider.card_title());
    let reason_width = (width as usize).checked_sub(label.width())?;
    if reason_width < USAGE_NOTE_MIN_REASON {
        return None;
    }
    Some(Line::from(vec![
        Span::styled(label, bold(WARNING)),
        Span::styled(clip(reason, reason_width), Style::default().fg(MUTED)),
    ]))
}

// A provider whose CLI is installed but whose usage cannot be read keeps its
// card hidden, because five rows of blanks say less than the reason does. The
// reason goes in the header's top right corner instead, in the room the cards
// left, so an expired login reads as a login to refresh rather than as a panel
// that silently went missing.
fn draw_usage_notes(frame: &mut Frame<'_>, band: Rect, x: u16, app: &App) {
    let width = (band.x + band.width).saturating_sub(x);
    let lines: Vec<Line<'static>> = app
        .usage
        .iter()
        .filter_map(|slot| usage_note_line(slot, width))
        .collect();
    if lines.is_empty() {
        return;
    }
    let height = (lines.len() as u16).min(band.height);
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Right),
        Rect::new(x, band.y, width, height),
    );
}

// The mascot is pixel art: every character below is one pixel, and the
// half-block glyphs fit two pixel rows into one terminal cell, so these
// fourteen rows fill seven terminal rows, the same as the wordmark. The
// first and last pixel rows are blank so the art can sit one *pixel* below
// the top of the screen, which is half a terminal row. `o` is the coat, `d`
// the inner ear, `w` the blaze, cheeks and muzzle, `k` the eyes, nose and
// mouth corners, `h` the glint in each eye and `p` the tongue.
const CORGI_PIXELS: [&str; 14] = [
    "................",
    ".o............o.",
    ".oo..........oo.",
    ".odo........odo.",
    ".oddo......oddo.",
    ".oooooooooooooo.",
    ".ookkoowwookkoo.",
    ".ookhoowwoohkoo.",
    ".wwooowwwwoooww.",
    ".wwwwwwkkwwwwww.",
    "..wwwwkwwkwwww..",
    "...wwwwppwwww...",
    ".....wwwwww.....",
    "................",
];
const CORGI_ROWS: u16 = CORGI_PIXELS.len() as u16 / 2;
const CORGI_WIDTH: u16 = CORGI_PIXELS[0].len() as u16;
// Like the vendor colors, the coat is fixed rather than themed: a supervisor in
// the theme's accent shade would no longer read as a corgi.
const CORGI_COAT: Color = Color::Indexed(208);
const CORGI_INNER_EAR: Color = Color::Indexed(166);
const CORGI_WHITE: Color = Color::Indexed(255);
const CORGI_DARK: Color = Color::Indexed(16);
// Brighter than the muzzle so the glint reads as a reflection, not fur.
const CORGI_GLINT: Color = Color::Indexed(231);
const CORGI_TONGUE: Color = Color::Indexed(210);
// Omarchy's own wordmark is Delta Corps Priest 1: solid three-cell stems,
// corners chamfered with half blocks, and crossbars split into a `▄▄` over a
// `▀▀`. This is a CORGI cut of that alphabet, shortened to five rows so the
// mascot beside it stays the taller of the two, and set tight: a single column
// between letters rather than the two Omarchy itself uses.
//
// Every part of a letter is keyed to the two-column counter: the R's `▄▄` and
// `▀▀`, the diagonal shoulder under its leg, and the G's spur. Narrowing the
// counter means redrawing each of them, not shifting them, or the fill overruns
// the stems and the leg's shoulder is left floating.
const WORDMARK: [&str; 5] = [
    "▄██████ ▄██████▄ ▄███████ ▄██████▄ ▄██",
    "███ ▀██ ███  ███ ███  ███ ███      ███",
    "███     ███  ███ ███▄▄██▀ ███ ████ ███",
    "███ ▄██ ███  ███ ███▀▀██▄ ███  ███ ███",
    "██████▀ ▀██████▀ ███  ▀██ ▀██████▀ ██▀",
];
// Every row is padded to the same width, which the tests hold to this value so
// the header can reserve the space before it renders the letters.
const WORDMARK_WIDTH: u16 = 38;

fn draw_wordmark(frame: &mut Frame<'_>, area: Rect) {
    let wordmark_rows = WORDMARK.len() as u16;
    let wordmark_width = WORDMARK[0].width() as u16;
    let logo = if area.height < wordmark_rows || area.width < wordmark_width {
        vec![Line::styled(
            "C O R G I",
            Style::default().add_modifier(Modifier::BOLD),
        )]
    } else {
        WORDMARK
            .iter()
            .map(|letters| Line::from(*letters))
            .collect()
    };
    // Centre the letters vertically against the taller corgi beside them, but
    // keep them flush left so the mascot and the wordmark stay one lockup.
    let rows = logo.len() as u16;
    let logo_area = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(rows) / 2,
        area.width,
        rows.min(area.height),
    );
    frame.render_widget(
        Paragraph::new(logo)
            .alignment(Alignment::Left)
            .style(Style::default().fg(SUCCESS)),
        logo_area,
    );
}

/// Paints the supervisor straight into the frame's cells: the art never changes,
/// so there is nothing to build for it on every frame.
fn draw_corgi(frame: &mut Frame<'_>, area: Rect) {
    let buffer = frame.buffer_mut();
    let area = area.intersection(buffer.area);
    for (row, pixels) in CORGI_PIXELS.chunks(2).enumerate() {
        let pairs = pixels[0].chars().zip(pixels[1].chars());
        for (column, (top, bottom)) in pairs.enumerate() {
            let (x, y) = (area.x + column as u16, area.y + row as u16);
            if x < area.right() && y < area.bottom() {
                let (symbol, style) = half_block(pixel_color(top), pixel_color(bottom));
                buffer[(x, y)].set_symbol(symbol).set_style(style);
            }
        }
    }
}

fn pixel_color(pixel: char) -> Option<Color> {
    match pixel {
        'o' => Some(CORGI_COAT),
        'd' => Some(CORGI_INNER_EAR),
        'w' => Some(CORGI_WHITE),
        'k' => Some(CORGI_DARK),
        'h' => Some(CORGI_GLINT),
        'p' => Some(CORGI_TONGUE),
        _ => None,
    }
}

// One terminal cell holding the pixel above and the pixel below. An unpainted
// half is left to the terminal background, so the art has no box around it.
fn half_block(top: Option<Color>, bottom: Option<Color>) -> (&'static str, Style) {
    match (top, bottom) {
        (None, None) => (" ", Style::default()),
        (Some(color), None) => ("▀", Style::default().fg(color)),
        (None, Some(color)) => ("▄", Style::default().fg(color)),
        (Some(top), Some(bottom)) if top == bottom => ("█", Style::default().fg(top)),
        (Some(top), Some(bottom)) => ("▀", Style::default().fg(top).bg(bottom)),
    }
}

pub(crate) fn usage_lines_for_slot(slot: &UsageSlot, now: u64) -> Vec<Line<'static>> {
    let five_hour = slot
        .usage
        .as_ref()
        .and_then(|usage| usage.five_hour.as_ref());
    let week = slot.usage.as_ref().and_then(|usage| usage.week.as_ref());
    vec![
        usage_heading_line(slot, now),
        usage_percent_line("5h", five_hour),
        usage_reset_line(five_hour, now),
        usage_percent_line("Weekly", week),
        usage_reset_line(week, now),
    ]
}

/// The card is titled with the provider, so its first row carries only the
/// subscription, or why there is none to show, and how old the reading is,
/// right-aligned and dim. The row never outgrows the card: the age drops its
/// "ago" first, then the subscription is clipped.
fn usage_heading_line(slot: &UsageSlot, now: u64) -> Line<'static> {
    let muted = Style::default().fg(MUTED);
    let (heading, style) = match &slot.usage {
        Some(usage) if !usage.plan_type.is_empty() => (usage.plan_type.clone(), bold(TEXT)),
        Some(_) => ("—".to_string(), muted),
        None if slot.error.is_some() => ("unavailable".to_string(), muted),
        None => ("loading…".to_string(), muted),
    };
    let age = slot
        .usage
        .as_ref()
        .and(slot.fetched_at)
        .map(|fetched_at| reading_age(now.saturating_sub(fetched_at)));
    let Some(age) = age else {
        return Line::from(Span::styled(heading, style));
    };
    let width = usize::from(USAGE_CONTENT_WIDTH);
    let long = format!("{age} ago");
    let age = if heading.width() + 1 + long.width() <= width {
        long
    } else {
        age
    };
    let heading = clip(&heading, width.saturating_sub(1 + age.width()));
    let padding = width.saturating_sub(heading.width() + age.width());
    Line::from(vec![
        Span::styled(heading, style),
        Span::raw(" ".repeat(padding)),
        Span::styled(age, muted),
    ])
}

/// How long ago a reading was taken, `elapsed` seconds before now, in the
/// largest whole unit: `<1m`, `3m`, `2h`, `4d`.
fn reading_age(elapsed: u64) -> String {
    let minutes = elapsed / 60;
    match minutes {
        0 => "<1m".to_string(),
        1..60 => format!("{minutes}m"),
        60..1_440 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1_440),
    }
}

fn usage_percent_line(label: &'static str, window: Option<&UsageWindow>) -> Line<'static> {
    let (percent, style) = window.map_or_else(
        || ("—".to_string(), Style::default().fg(MUTED)),
        |window| {
            (
                format!("{}%", window.used_percent),
                usage_color(window.used_percent),
            )
        },
    );
    // Keep an exhausted window accurate without clipping the percent sign.
    let label = if label == "Weekly" && percent == "100%" {
        "Week"
    } else {
        label
    };
    let padding = usize::from(USAGE_CONTENT_WIDTH).saturating_sub(label.width() + percent.width());
    Line::from(vec![
        Span::styled(
            format!("{label}{}", " ".repeat(padding)),
            Style::default().fg(MUTED),
        ),
        Span::styled(percent, style),
    ])
}

fn usage_reset_line(window: Option<&UsageWindow>, now: u64) -> Line<'static> {
    let countdown = window.map_or_else(
        || "—".to_string(),
        |window| reset_countdown(window.resets_at, now),
    );
    Line::from(Span::styled(countdown, Style::default().fg(ACCENT))).alignment(Alignment::Right)
}

fn reset_countdown(resets_at: u64, now: u64) -> String {
    if resets_at == 0 {
        return "—".to_string();
    }
    let remaining = resets_at.saturating_sub(now);
    if remaining == 0 {
        return "now".to_string();
    }

    let minutes = remaining / 60;
    if minutes < 60 {
        return if minutes == 0 {
            "<1m".to_string()
        } else {
            format!("{minutes}m")
        };
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h {:02}m", minutes % 60);
    }
    format!("{}d {:02}h", hours / 24, hours % 24)
}

fn usage_color(used_percent: u8) -> Style {
    let color = match used_percent {
        0..=59 => SUCCESS,
        60..=84 => WARNING,
        _ => DANGER,
    };
    bold(color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{AgentState, DashboardAgent},
        test_support::{buffer_text, lines_text, test_app, test_terminal},
        ui::draw,
        usage::{PlanUsage, Provider},
    };
    use ratatui::buffer::Buffer;

    #[test]
    fn corgi_pixel_rows_pair_up_into_uniform_lines() {
        let width = CORGI_PIXELS[0].len();
        assert!(CORGI_PIXELS.iter().all(|row| row.len() == width));
        assert!(CORGI_PIXELS.iter().all(|row| {
            row.chars()
                .all(|pixel| pixel == '.' || pixel_color(pixel).is_some())
        }));
        assert_eq!(CORGI_PIXELS.len() % 2, 0, "pixel rows pair up into cells");
        assert!(
            WORDMARK
                .iter()
                .all(|row| row.width() == WORDMARK_WIDTH as usize)
        );
        assert!(
            (WORDMARK.len() as u16) < CORGI_ROWS,
            "the wordmark stays shorter than the supervisor beside it"
        );
    }

    #[test]
    fn wordmark_falls_back_to_text_when_there_is_no_room() {
        let render = |width: u16, height: u16| {
            let mut terminal = test_terminal(width, height);
            terminal
                .draw(|frame| draw_wordmark(frame, frame.area()))
                .expect("draw wordmark");
            let cells = terminal.backend().buffer().content().to_vec();
            let has_wordmark = cells
                .iter()
                .any(|cell| cell.fg == SUCCESS && cell.symbol() == "█");
            let text: String = cells.iter().map(|cell| cell.symbol()).collect();
            (has_wordmark, text)
        };

        let (wordmark, _) = render(WORDMARK_WIDTH, WORDMARK.len() as u16);
        assert!(wordmark, "the letters fit their own measurements exactly");
        let (wordmark, text) = render(WORDMARK_WIDTH - 1, 7);
        assert!(!wordmark && text.contains("C O R G I"));
        let (wordmark, text) = render(80, WORDMARK.len() as u16 - 1);
        assert!(!wordmark && text.contains("C O R G I"));
    }

    /// The dashboard's header, as the cells of a 130-column screen and the
    /// column and row the herd card's top-left corner is drawn at.
    fn rendered_header(app: &mut App) -> (Buffer, u16, u16) {
        // One working, one done, one done a supervisor tagged ready to merge,
        // and one in a state Corgi cannot tell.
        app.agents = vec![DashboardAgent::default(); 4];
        app.agents[0].info.state = AgentState::Working;
        app.agents[1].info.state = AgentState::Done;
        app.agents[2].info.state = AgentState::Done;
        app.agents[2].info.state_change_seq = 9;
        app.agents[2].info.tokens.insert(
            crate::supervisor::MERGE_TOKEN.into(),
            crate::supervisor::merge_tag_value(AgentState::Done, 9),
        );
        let mut terminal = test_terminal(130, 24);
        terminal
            .draw(|frame| draw(frame, app))
            .expect("draw dashboard");
        let buffer = terminal.backend().buffer().clone();
        let herd_x = CORGI_WIDTH + CORGI_GAP + WORDMARK_WIDTH + WORDMARK_CARD_GAP;
        let herd_y = (0..HEADER_HEIGHT)
            .find(|&y| buffer[(herd_x, y)].symbol() == "╭")
            .expect("the herd card is drawn after the wordmark");
        (buffer, herd_x, herd_y)
    }

    #[test]
    fn the_header_is_one_band_of_corgi_wordmark_and_cards() {
        let mut app = test_app();
        app.usage = [Provider::Codex, Provider::Claude]
            .map(|provider| UsageSlot {
                provider,
                usage: None,
                fetched_at: None,
                error: None,
            })
            .into();
        let (buffer, herd_x, herd_y) = rendered_header(&mut app);
        let text = |xs: std::ops::Range<u16>, y: u16| -> String {
            xs.map(|x| buffer[(x, y)].symbol()).collect()
        };

        // The supervisor fills the top-left corner flush to the top edge, and its
        // blank first pixel row leaves one pixel of clearance above the ears.
        let is_supervisor =
            |x: u16, y: u16| [CORGI_COAT, CORGI_WHITE, CORGI_DARK].contains(&buffer[(x, y)].fg);
        let corgi_rows: Vec<u16> = (0..9)
            .filter(|&y| (0..CORGI_WIDTH).any(|x| is_supervisor(x, y)))
            .collect();
        assert_eq!(corgi_rows, (0..CORGI_ROWS).collect::<Vec<_>>());
        assert_eq!(buffer[(1, 0)].symbol(), "▄");
        assert_eq!(buffer[(1, 0)].fg, CORGI_COAT);

        // The letters run their full width from just after the corgi.
        let is_letter = |x: u16, y: u16| {
            let cell = &buffer[(x, y)];
            cell.fg == SUCCESS && ["█", "▄", "▀"].contains(&cell.symbol())
        };
        let letter_columns: Vec<u16> = (0..130)
            .filter(|&x| (0..HEADER_HEIGHT).any(|y| is_letter(x, y)))
            .collect();
        let wordmark_x = CORGI_WIDTH + CORGI_GAP;
        assert_eq!(letter_columns.first(), Some(&wordmark_x));
        assert_eq!(
            letter_columns.last(),
            Some(&(wordmark_x + WORDMARK_WIDTH - 1))
        );

        // The herd card opens one row above the letters and closes one row
        // below them, so it brackets them evenly.
        let letters_y = (0..HEADER_HEIGHT)
            .find(|&y| is_letter(wordmark_x, y))
            .expect("a letter row");
        assert_eq!(herd_y, letters_y - 1);
        assert_eq!(buffer[(herd_x, herd_y + HERD_HEIGHT - 1)].symbol(), "╰");
        assert!(text(herd_x..herd_x + HERD_WIDTH, herd_y).contains("HERD"));
        let rows: Vec<String> = (herd_y + 1..herd_y + HERD_HEIGHT - 1)
            .map(|y| text(herd_x..herd_x + HERD_WIDTH, y))
            .collect();
        // The tagged agent counts as MERGE, not as done; the card keeps its
        // five rows, the total making way for MERGE.
        let expected = [
            "● 1 WORKING",
            "! 0 BLOCKED",
            "⇡ 1 MERGE",
            "✓ 1 DONE",
            "· 1 IDLE",
        ];
        assert_eq!(rows.len(), expected.len(), "{rows:?}");
        for (row, label) in rows.iter().zip(expected) {
            assert!(row.contains(label), "{rows:?}");
        }
        assert!(rows.iter().all(|row| !row.contains("TOTAL")), "{rows:?}");
        // The counts share a column.
        let column = |row: &str| row.chars().position(|character| character.is_ascii_digit());
        assert!(
            rows.iter().all(|row| column(row) == column(&rows[0])),
            "{rows:?}"
        );

        // Each detected subscription gets a card of its own after the herd,
        // sharing the band's height.
        let codex_x = herd_x + HERD_WIDTH + CARD_GAP;
        let claude_x = codex_x + USAGE_CARD_WIDTH + CARD_GAP;
        for (x, provider) in [(codex_x, "CODEX"), (claude_x, "CLAUDE")] {
            let title = text(x..x + USAGE_CARD_WIDTH, herd_y);
            assert!(
                title.starts_with('╭') && title.contains(provider),
                "{title:?}"
            );
            assert_eq!(buffer[(x, herd_y + HERD_HEIGHT - 1)].symbol(), "╰");
        }
    }

    #[test]
    fn every_row_of_the_herd_card_has_a_color_of_its_own() {
        let (buffer, herd_x, herd_y) = rendered_header(&mut test_app());
        let row_colors: Vec<Color> = (herd_y + 1..herd_y + HERD_HEIGHT - 1)
            .map(|y| {
                let colors: Vec<Color> = (herd_x + 1..herd_x + HERD_WIDTH - 1)
                    .filter(|&x| buffer[(x, y)].symbol() != " ")
                    .map(|x| buffer[(x, y)].fg)
                    .collect();
                // The icon, the count and the label of a row share one color.
                assert!(
                    colors.windows(2).all(|pair| pair[0] == pair[1]),
                    "{colors:?}"
                );
                colors[0]
            })
            .collect();
        assert_eq!(row_colors.len(), 5);
        for (index, color) in row_colors.iter().enumerate() {
            assert!(!row_colors[index + 1..].contains(color), "{row_colors:?}");
        }
    }

    #[test]
    fn reset_countdown_is_compact_and_handles_elapsed_or_unknown_times() {
        assert_eq!(reset_countdown(0, 1_000), "—");
        assert_eq!(reset_countdown(1_000, 1_000), "now");
        assert_eq!(reset_countdown(1_059, 1_000), "<1m");
        assert_eq!(reset_countdown(1_060, 1_000), "1m");
        assert_eq!(reset_countdown(10_030, 1_000), "2h 30m");
        assert_eq!(reset_countdown(357_400, 1_000), "4d 03h");
    }

    #[test]
    fn usage_lines_show_reset_countdowns_for_both_windows() {
        let slot = UsageSlot {
            provider: Provider::Codex,
            usage: Some(PlanUsage {
                provider: Provider::Codex,
                plan_type: "Pro".to_string(),
                five_hour: Some(UsageWindow {
                    used_percent: 31,
                    resets_at: 10_000,
                }),
                week: Some(UsageWindow {
                    used_percent: 7,
                    resets_at: 357_400,
                }),
                banked_resets: Some(2),
            }),
            fetched_at: None,
            error: None,
        };
        let lines = usage_lines_for_slot(&slot, 1_000);
        let rendered = lines_text(&lines);

        assert_eq!(rendered.len(), 5);
        assert_eq!(
            rendered[0], "Pro",
            "the heading row is only the subscription"
        );
        assert!(rendered[1].contains("5h") && rendered[1].contains("31%"));
        assert_eq!(rendered[2], "2h 30m");
        assert!(rendered[3].contains("Weekly") && rendered[3].contains("7%"));
        assert_eq!(rendered[4], "4d 03h");
        assert!(rendered.iter().all(|line| !line.contains('↻')));
        assert_eq!(rendered[1].width(), usize::from(USAGE_CONTENT_WIDTH));
        assert_eq!(rendered[3].width(), rendered[1].width());
        assert_eq!(rendered[1], "5h     31%");
        assert_eq!(rendered[3], "Weekly  7%");
        for line in [&lines[2], &lines[4]] {
            assert_eq!(line.alignment, Some(Alignment::Right));
        }
        let exhausted = UsageWindow {
            used_percent: 100,
            resets_at: 10_000,
        };
        let exhausted_line = usage_percent_line("Weekly", Some(&exhausted));
        assert_eq!(exhausted_line.width(), usize::from(USAGE_CONTENT_WIDTH));
        assert_eq!(exhausted_line.to_string(), "Week  100%");
    }

    #[test]
    fn available_claude_usage_card_matches_codex_layout() {
        let slots =
            [(Provider::Codex, "Plus"), (Provider::Claude, "Max")].map(|(provider, plan)| {
                UsageSlot {
                    provider,
                    usage: Some(PlanUsage {
                        provider,
                        plan_type: plan.into(),
                        five_hour: Some(UsageWindow {
                            used_percent: 31,
                            resets_at: 10_000,
                        }),
                        week: Some(UsageWindow {
                            used_percent: 7,
                            resets_at: 357_400,
                        }),
                        banked_resets: None,
                    }),
                    fetched_at: None,
                    error: None,
                }
            });
        assert!(slots.iter().all(show_usage_card));
        let mut terminal = test_terminal(2 * USAGE_CARD_WIDTH + CARD_GAP, HERD_HEIGHT);
        terminal
            .draw(|frame| {
                let cards = slots
                    .iter()
                    .map(|slot| HeaderCard {
                        width: USAGE_CARD_WIDTH,
                        horizontal_padding: USAGE_CARD_PADDING,
                        title: slot.provider.card_title(),
                        color: model_color(slot.provider.label()),
                        lines: usage_lines_for_slot(slot, 1_000),
                    })
                    .collect();
                draw_header_cards(frame, frame.area(), cards);
            })
            .expect("draw both subscriptions");
        let buffer = terminal.backend().buffer();
        for (index, (title, heading)) in [("CODEX", "Plus"), ("CLAUDE", "Max")].iter().enumerate() {
            let x = index as u16 * (USAGE_CARD_WIDTH + CARD_GAP);
            let card_title: String = (x..x + USAGE_CARD_WIDTH)
                .map(|column| buffer[(column, 0)].symbol())
                .collect();
            assert!(
                card_title.contains(title) && !card_title.contains('◈'),
                "the card is titled with its provider alone, got {card_title:?}"
            );
            let rows: Vec<String> = (1..HERD_HEIGHT - 1)
                .map(|y| {
                    (x + 1..x + USAGE_CARD_WIDTH - 1)
                        .map(|column| buffer[(column, y)].symbol())
                        .collect()
                })
                .collect();
            assert_eq!(
                rows,
                [
                    format!(" {heading:<11}"),
                    " 5h     31% ".to_string(),
                    "     2h 30m ".to_string(),
                    " Weekly  7% ".to_string(),
                    "     4d 03h ".to_string()
                ]
            );
            assert_eq!(
                buffer[(x + USAGE_CARD_WIDTH - 1, HERD_HEIGHT - 1)].symbol(),
                "╯"
            );
        }
    }

    fn slot_with_plan(plan: &str, fetched_at: Option<u64>) -> UsageSlot {
        UsageSlot {
            provider: Provider::Claude,
            usage: Some(PlanUsage {
                provider: Provider::Claude,
                plan_type: plan.into(),
                five_hour: Some(UsageWindow {
                    used_percent: 31,
                    resets_at: 10_000,
                }),
                week: None,
                banked_resets: None,
            }),
            fetched_at,
            error: None,
        }
    }

    #[test]
    fn reading_age_is_given_in_its_largest_whole_unit() {
        assert_eq!(reading_age(0), "<1m");
        assert_eq!(reading_age(59), "<1m");
        assert_eq!(reading_age(180), "3m");
        assert_eq!(reading_age(3_599), "59m");
        assert_eq!(reading_age(7_300), "2h");
        assert_eq!(reading_age(3 * 86_400 + 5), "3d");
    }

    #[test]
    fn the_heading_row_shows_the_reading_age_right_aligned_and_dim() {
        let now = 10_000;
        let heading = |plan: &str, age: u64| {
            let line = usage_heading_line(&slot_with_plan(plan, Some(now - age)), now);
            assert_eq!(line.width(), usize::from(USAGE_CONTENT_WIDTH), "{line}");
            assert_eq!(line.alignment, None, "padded, not aligned, for Omarchy");
            let age_span = line.spans.last().expect("an age span");
            assert_eq!(age_span.style.fg, Some(MUTED));
            line.to_string()
        };
        assert_eq!(heading("Max", 180), "Max 3m ago");
        assert_eq!(heading("Pro", 30), "Pro    <1m");
        assert_eq!(heading("Max", 2 * 3_600), "Max 2h ago");
        // Too long for the age in full, so it drops its "ago" first...
        assert_eq!(heading("Max 20x", 180), "Max 20x 3m");
        assert_eq!(heading("Team", 45 * 60), "Team   45m");
        // ...and only then is the subscription clipped.
        assert_eq!(heading("Enterprise", 180), "Enterp… 3m");
        assert_eq!(heading("Enterprise", 45 * 60), "Enter… 45m");

        // Without a reading's time the row is only the subscription.
        let line = usage_heading_line(&slot_with_plan("Max", None), now);
        assert_eq!(line.to_string(), "Max");
    }

    #[test]
    fn a_long_plan_name_and_its_age_keep_the_card_its_size() {
        let now = 10_000;
        for plan in ["Max 20x", "Enterprise", "Enterprise Ultra Max"] {
            let slot = slot_with_plan(plan, Some(now - 45 * 60));
            let mut terminal = test_terminal(USAGE_CARD_WIDTH + 2, HERD_HEIGHT);
            terminal
                .draw(|frame| {
                    let card = HeaderCard {
                        width: USAGE_CARD_WIDTH,
                        horizontal_padding: USAGE_CARD_PADDING,
                        title: slot.provider.card_title(),
                        color: model_color(slot.provider.label()),
                        lines: usage_lines_for_slot(&slot, now),
                    };
                    draw_header_cards(frame, frame.area(), vec![card]);
                })
                .expect("draw the card");
            let buffer = terminal.backend().buffer();
            let row = |y: u16| -> String {
                (0..USAGE_CARD_WIDTH + 2)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            };
            // The right border sits where it always does and nothing spills
            // past it.
            assert_eq!(buffer[(USAGE_CARD_WIDTH - 1, 0)].symbol(), "╮", "{plan}");
            assert_eq!(
                buffer[(USAGE_CARD_WIDTH - 1, HERD_HEIGHT - 1)].symbol(),
                "╯",
                "{plan}"
            );
            let heading = row(1);
            assert!(heading.ends_with("45m │  "), "{plan}: {heading:?}");
            assert!(heading.starts_with("│ "), "{plan}: {heading:?}");
            assert_eq!(
                row(2),
                "│ 5h     31% │  ",
                "{plan}: the next row is unmoved"
            );
        }
    }

    #[test]
    fn usage_cards_hide_providers_without_a_detected_subscription() {
        let unavailable = UsageSlot {
            provider: Provider::Claude,
            usage: None,
            fetched_at: None,
            error: Some("no Claude Code login found".into()),
        };
        let loading = UsageSlot {
            provider: Provider::Codex,
            usage: None,
            fetched_at: None,
            error: None,
        };

        assert!(!show_usage_card(&unavailable));
        assert!(show_usage_card(&loading));
    }

    #[test]
    fn a_provider_without_a_card_states_its_reason_in_the_top_right_corner() {
        let mut app = test_app();
        // One working, one done, one done a supervisor tagged ready to merge,
        // and one in a state Corgi cannot tell.
        app.agents = vec![DashboardAgent::default(); 4];
        app.agents[0].info.state = AgentState::Working;
        app.agents[1].info.state = AgentState::Done;
        app.agents[2].info.state = AgentState::Done;
        app.agents[2].info.state_change_seq = 9;
        app.agents[2].info.tokens.insert(
            crate::supervisor::MERGE_TOKEN.into(),
            crate::supervisor::merge_tag_value(AgentState::Done, 9),
        );
        app.usage = vec![
            UsageSlot {
                provider: Provider::Codex,
                usage: None,
                fetched_at: None,
                error: None,
            },
            UsageSlot {
                provider: Provider::Claude,
                usage: None,
                fetched_at: None,
                error: Some("Claude login has expired; run Claude Code once to refresh it".into()),
            },
        ];
        let width = 130;
        let mut terminal = test_terminal(width, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw dashboard");
        let buffer = terminal.backend().buffer();
        let top: String = (0..width).map(|x| buffer[(x, 0)].symbol()).collect();
        assert!(
            top.contains("⚠ CLAUDE Claude login has expired"),
            "the reason replaces the card it lost, got {top:?}"
        );
        assert_eq!(
            top.chars().last(),
            Some('…'),
            "and runs flush to the right edge, clipped where it ran out, got {top:?}"
        );
        // The provider that is merely still loading keeps its card, so a note
        // never doubles up with the panel it stands in for.
        assert!(top.contains("CODEX"), "got {top:?}");

        // Too narrow for the reason, and the header says nothing rather than
        // printing a bare provider name.
        let mut narrow = test_terminal(
            HEADER_BRAND_WIDTH + WORDMARK_CARD_GAP + HERD_WIDTH + CARD_GAP + USAGE_CARD_WIDTH + 4,
            24,
        );
        narrow
            .draw(|frame| draw(frame, &mut app))
            .expect("draw dashboard");
        let rendered = buffer_text(narrow.backend().buffer());
        assert!(!rendered.contains('⚠'), "got {rendered:?}");
    }
}
