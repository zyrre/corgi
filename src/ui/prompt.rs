//! The dialog that sends a prompt to an existing agent.

use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Clear, Padding, Paragraph},
};

use unicode_width::UnicodeWidthStr;

use crate::{app::PromptForm, textfield::wrap_rows};

use super::{
    ACCENT, DIALOG_FRAME_WIDTH, Drawn, LegendKey, MUTED, POPUP_MARK, SUCCESS, TEXT, WARNING, bold,
    centered, dialog_block, dialog_width, legend_line, popup_title, rule_line,
};

const PROMPT_KEYS: [LegendKey; 2] = [("↵", "send", SUCCESS), ("Esc", "cancel", WARNING)];
// Rows of the prompt that are not its text: the frame, the recipient and the
// gap under it, and the rule and keys at the bottom.
const PROMPT_CHROME_HEIGHT: u16 = 6;

pub(super) fn draw_prompt(frame: &mut Frame<'_>, area: Rect, form: &PromptForm) -> Drawn {
    let width = dialog_width(area, 78);
    // The frame and the `> ` marker in front of the first row.
    let text_width = usize::from(width.saturating_sub(DIALOG_FRAME_WIDTH + 2).max(1));
    let rows = wrap_rows(&form.text, text_width);
    // The box grows a row at a time with the text; when the screen ends
    // first, the earliest rows give way so the caret stays in view.
    let visible_rows = rows.len().min(usize::from(
        area.height.saturating_sub(PROMPT_CHROME_HEIGHT).max(1),
    ));
    let first_shown_row = rows.len() - visible_rows;
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Send to  ", Style::default().fg(MUTED)),
            Span::styled(form.label.clone(), bold(TEXT)),
        ]),
        Line::raw(""),
    ];
    for (index, row) in rows[first_shown_row..].iter().enumerate() {
        let marker = if index == 0 && first_shown_row == 0 {
            "> "
        } else {
            "  "
        };
        lines.push(Line::from(vec![
            Span::styled(marker, bold(ACCENT)),
            Span::styled(
                form.text[row.clone()].to_string(),
                Style::default().fg(TEXT),
            ),
        ]));
    }
    let popup = centered(area, width, visible_rows as u16 + PROMPT_CHROME_HEIGHT);
    let rule_width = popup.width.saturating_sub(DIALOG_FRAME_WIDTH);
    lines.push(rule_line(rule_width));
    lines.push(legend_line(&PROMPT_KEYS, "   "));
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            dialog_block(&popup_title(POPUP_MARK, "Prompt"), ACCENT)
                .padding(Padding::horizontal(1)),
        ),
        popup,
    );
    let last_row = rows.last().cloned().unwrap_or(0..0);
    let column = UnicodeWidthStr::width(&form.text[last_row]) as u16;
    let caret_x =
        (popup.x + DIALOG_FRAME_WIDTH / 2 + 2 + column).min(popup.right().saturating_sub(2));
    let caret_y = (popup.y + 3 + (visible_rows as u16).saturating_sub(1))
        .min(popup.bottom().saturating_sub(4));
    Drawn {
        area: popup,
        color: ACCENT,
        caret: Some(Position::new(caret_x, caret_y)),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        app::Overlay,
        test_support::{buffer_text, test_app, test_terminal},
        ui::draw_overlay,
    };

    #[test]
    fn the_prompt_box_grows_with_its_text_and_keeps_the_caret_on_the_last_row() {
        let mut app = test_app();
        app.overlay = Overlay::Prompt(crate::app::PromptForm {
            target: "p1".into(),
            label: "corgi-worker".into(),
            text: "short".into(),
        });
        let mut terminal = test_terminal(60, 20);
        terminal
            .draw(|frame| draw_overlay(frame, frame.area(), &mut app))
            .expect("draw prompt");
        let short_caret = terminal.get_cursor_position().expect("caret shown");
        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("Send to  corgi-worker"), "{rendered}");
        assert!(rendered.contains("> short"), "{rendered}");
        assert!(rendered.contains("◆ Prompt"), "{rendered}");
        assert!(rendered.contains(" ↵  send"), "{rendered}");

        app.overlay.prompt_form_mut().expect("form").text =
            "a prompt long enough that it has to wrap onto a second row of the box".into();
        terminal
            .draw(|frame| draw_overlay(frame, frame.area(), &mut app))
            .expect("draw prompt");
        let long_caret = terminal.get_cursor_position().expect("caret shown");
        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("of the box"), "{rendered}");
        // The box grew by a row above and the caret moved onto the new row.
        assert!(
            long_caret.y > short_caret.y,
            "{short_caret:?} -> {long_caret:?}"
        );
        assert!(long_caret.x < 60 - 4, "{long_caret:?}");
    }
}
