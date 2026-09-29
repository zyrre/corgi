use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Clear, ListItem, ListState},
};

use unicode_width::UnicodeWidthStr;

use crate::{
    choices::{Choice, ChoiceList, FreeText},
    paths::tilde,
};

use super::new_agent::CHOICE_LEGEND_SEPARATOR;
use super::{
    ACCENT, LegendKey, MUTED, SUCCESS, TEXT, WARNING, bold, clip_start, dialog_block, legend_line,
    unhighlighted_list,
};

// Two blank columns separate a choice's label from its detail.
const FIELD_GAP: usize = 2;
// Most rows a selector's list shows before it scrolls.
const CHOICE_LIST_ROWS: u16 = 8;
/// The keys of an open list, which explains them in its own border.
pub(super) const CHOICE_LIST_KEYS: [LegendKey; 4] = [
    ("↑ ↓", "move", ACCENT),
    ("↵", "choose", SUCCESS),
    ("⇥", "choose, next", ACCENT),
    ("Esc", "back", WARNING),
];

/// A selector's open list, hung below its row (or above it when the screen
/// ends first) and as wide as the value column, so the choices line up with
/// the value they replace.
pub(super) fn draw_choice_list(frame: &mut Frame<'_>, area: Rect, anchor: Rect, list: &ChoiceList) {
    let rows = list.rows();
    let visible_rows = (rows.len() as u16).clamp(1, CHOICE_LIST_ROWS);
    let height = (visible_rows + 2).min(area.height);
    let width = anchor
        .width
        .max(24)
        .min(area.width.saturating_sub(anchor.x));
    let y = if anchor.y + 1 + height <= area.bottom() {
        anchor.y + 1
    } else {
        anchor.y.saturating_sub(height)
    };
    let popup = Rect {
        x: anchor.x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    // The list can cover the form's legend, so it carries its own keys, a
    // column in from either corner.
    let mut keys = legend_line(&CHOICE_LIST_KEYS, CHOICE_LEGEND_SEPARATOR);
    keys.spans.insert(0, Span::raw(" "));
    keys.spans
        .push(Span::styled(" ", Style::default().fg(MUTED)));
    let block = dialog_block("", ACCENT).title_bottom(keys);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let selected = list.selected.min(rows.len().saturating_sub(1));
    let items: Vec<ListItem<'_>> = if rows.is_empty() {
        let hint = match list.free_text {
            FreeText::Directory => "No match. Type a new project name or a directory path.",
            FreeText::Value | FreeText::None => "No match.",
        };
        vec![ListItem::new(Line::styled(
            format!(" {hint}"),
            Style::default().fg(WARNING),
        ))]
    } else {
        rows.iter()
            .enumerate()
            .map(|(index, choice)| choice_item(choice, index == selected, inner.width))
            .collect()
    };
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(unhighlighted_list(items), inner, &mut state);
}

/// One list row: the label, its detail clipped from the left so the end of a
/// path stays legible, and the badge saying where the choice comes from.
fn choice_item(choice: &Choice, selected: bool, width: u16) -> ListItem<'static> {
    let marker = if selected { "▸ " } else { "  " };
    let label_style = if selected {
        bold(ACCENT)
    } else {
        Style::default().fg(TEXT)
    };
    let badge = if choice.badge.is_empty() {
        String::new()
    } else {
        format!("· {}", choice.badge)
    };
    let detail = tilde(&choice.detail);
    // Marker, label, the gaps, the badge, and a column of margin at the end.
    let fixed = marker.width()
        + choice.label.width()
        + FIELD_GAP
        + if badge.is_empty() {
            0
        } else {
            badge.width() + 1
        }
        + 1;
    let detail = clip_start(&detail, usize::from(width).saturating_sub(fixed));
    let mut spans = vec![
        Span::styled(marker, Style::default().fg(ACCENT)),
        Span::styled(choice.label.clone(), label_style),
    ];
    if !detail.is_empty() {
        spans.push(Span::raw(" ".repeat(FIELD_GAP)));
        spans.push(Span::styled(detail, Style::default().fg(MUTED)));
    }
    if !badge.is_empty() {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(badge, Style::default().fg(SUCCESS)));
    }
    ListItem::new(Line::from(spans))
}
