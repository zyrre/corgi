//! Scrollable Sessions overlays; actions stay visible on short terminals.
use super::Drawn;
use crate::{
    app::recovery::{Sessions, Stage},
    theme::roles,
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Clear, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthChar;

pub(super) fn draw(frame: &mut Frame<'_>, area: Rect, form: &mut Sessions) -> Drawn {
    let menu = form.stage == Stage::Menu;
    let width = area
        .width
        .saturating_sub(4)
        .min(if menu { 88 } else { 110 });
    let height = area
        .height
        .saturating_sub(2)
        .min(if menu { 12 } else { 30 });
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let role = roles();
    let color = if form.stage == Stage::Confirm || form.stage == Stage::Setup {
        role.warning
    } else {
        role.accent
    };
    frame.render_widget(Clear, rect);
    let block = super::dialog_block(&format!(" ◆ {} ", form.title()), color);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    form.can_confirm = area.width >= 50 && area.height >= 12;
    if !form.can_confirm {
        frame.render_widget(Paragraph::new("Enlarge to 50 columns × 12 rows to review/confirm. Esc cancels or stops scheduling.").wrap(Wrap{trim:false}),inner);
        return Drawn {
            area: rect,
            color,
            caret: None,
        };
    }
    let footer_height = if inner.width < 75 { 4 } else { 3 };
    let body = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height.saturating_sub(footer_height),
    );
    let lines = form
        .lines()
        .into_iter()
        .flat_map(|line| {
            let text: String = line.chars().filter(|c| !c.is_control()).collect();
            let style = if text.starts_with('>') {
                Style::default()
                    .fg(role.accent)
                    .add_modifier(Modifier::BOLD)
            } else if text.contains("Skipped")
                || text.contains("unverified")
                || text.contains("unknown")
            {
                Style::default().fg(role.warning)
            } else {
                Style::default().fg(role.text)
            };
            let mut rows = Vec::new();
            let mut row = String::new();
            let mut columns = 0;
            for ch in text.chars() {
                let width = ch.width().unwrap_or(0);
                if columns + width > usize::from(body.width.max(1)) && !row.is_empty() {
                    rows.push(Line::styled(std::mem::take(&mut row), style));
                    columns = 0;
                }
                row.push(ch);
                columns += width;
            }
            rows.push(Line::styled(row, style));
            rows
        })
        .collect::<Vec<_>>();
    form.scroll_limit = lines.len().saturating_sub(usize::from(body.height));
    // Selection pages keep the selected row visible despite variable warning
    // detail lines. Other pages let j/k scroll the body independently of buttons.
    let offset = match form.stage {
        Stage::Menu => 0,
        Stage::Select | Stage::Browser => lines
            .iter()
            .position(|l| l.to_string().starts_with('>'))
            .unwrap_or(0)
            .saturating_sub(body.height.saturating_sub(3) as usize),
        _ => form.cursor,
    };
    frame.render_widget(
        Paragraph::new(lines).scroll((offset.min(u16::MAX as usize) as u16, 0)),
        body,
    );
    let footer = Rect::new(
        body.x,
        body.y + body.height,
        body.width,
        inner.height.saturating_sub(body.height),
    );
    frame.render_widget(
        Paragraph::new(form.footer())
            .style(Style::default().fg(color))
            .wrap(Wrap { trim: false }),
        footer,
    );
    Drawn {
        area: rect,
        color,
        caret: None,
    }
}
