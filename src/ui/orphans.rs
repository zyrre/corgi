//! The "Orphaned sessions" dialog: each candidate with its evidence and its
//! choices, the chosen one marked, as docs/orphaned-sessions.md sketches it.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Clear, Paragraph, Wrap},
};

use super::{
    DIALOG_LEGEND_SEPARATOR, Drawn, POPUP_MARK, SUCCESS_MARK, bold, clip, dialog_block,
    legend_lines, popup_title, rule_line,
};
use crate::{
    app::orphans::{OrphansDialog, Stage},
    theme::roles,
};
use corgi_core::{orphans::Confidence, paths::tilde};

/// The widest the dialog grows.
const WIDTH: u16 = 100;

pub(super) fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    dialog: &OrphansDialog,
    spinner: &str,
) -> Drawn {
    let role = roles();
    let color = role.accent;
    let width = area.width.saturating_sub(4).clamp(20, WIDTH).min(area.width);
    let text_width = usize::from(width.saturating_sub(4));
    let (lines, selected_at) = body(dialog, text_width, spinner);
    let keys: Vec<super::LegendKey> = match &dialog.stage {
        Stage::List if !dialog.candidates.is_empty() => vec![
            ("↵", "apply", role.success),
            ("←→", "choose", role.accent),
            ("j/k", "select", role.accent),
            ("Esc", "done", role.warning),
        ],
        Stage::Applying(_) => vec![],
        _ => vec![("Esc", "close", role.warning)],
    };
    let legend = legend_lines(&keys, DIALOG_LEGEND_SEPARATOR, width.saturating_sub(4));
    let legend_rows = legend.len() as u16;
    let wanted = lines.len() as u16 + 2 + if legend_rows > 0 { legend_rows + 1 } else { 0 };
    let height = wanted.min(area.height.saturating_sub(2)).max(5).min(area.height);
    let rect = Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    let block = dialog_block(&popup_title(POPUP_MARK, "Orphaned sessions"), color);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let [text, rule, keys_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(legend_rows.min(1)),
        Constraint::Length(legend_rows),
    ])
    .areas(inner);
    let text = Rect {
        x: text.x + 1,
        width: text.width.saturating_sub(2),
        ..text
    };
    // The selected candidate's block stays in view.
    let rows = usize::from(text.height);
    let offset = selected_at.saturating_sub(rows.saturating_sub(4));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((offset.min(usize::from(u16::MAX)) as u16, 0)),
        text,
    );
    if legend_rows > 0 {
        let rule = Rect {
            x: rule.x + 1,
            width: rule.width.saturating_sub(2),
            ..rule
        };
        frame.render_widget(Paragraph::new(rule_line(rule.width)), rule);
        frame.render_widget(
            Paragraph::new(legend).alignment(Alignment::Center),
            keys_area,
        );
    }
    Drawn {
        area: rect,
        color,
        caret: None,
    }
}

/// The dialog's text, `width` columns wide, and the line the selected
/// candidate starts on.
fn body(dialog: &OrphansDialog, width: usize, spinner: &str) -> (Vec<Line<'static>>, usize) {
    let role = roles();
    let muted = Style::default().fg(role.muted);
    let mut lines = Vec::new();
    let mut selected_at = 0;
    if dialog.stage == Stage::Looking {
        lines.push(Line::styled(
            format!(
                "{spinner} Looking at Herdr's panes, their processes and the supervisors' files…"
            ),
            muted,
        ));
        return (lines, 0);
    }
    if dialog.candidates.is_empty() {
        lines.push(Line::styled(
            "No orphaned sessions: every agent Herdr lists has its row, role and project.",
            Style::default().fg(role.text),
        ));
    }
    for (index, candidate) in dialog.candidates.iter().enumerate() {
        if index > 0 {
            lines.push(Line::raw(""));
        }
        let selected = index == dialog.selected;
        if selected {
            selected_at = lines.len();
        }
        let (mark, name_style) = if selected {
            ("●", bold(role.accent))
        } else {
            ("○", bold(role.text))
        };
        let place = match &candidate.pane_id {
            Some(pane) => format!("{} · {pane}", candidate.workspace),
            None => candidate.workspace_id.clone(),
        };
        let who = if candidate.harness.is_empty() {
            "(empty)".to_string()
        } else {
            candidate.harness.clone()
        };
        let head = format!("{mark} {who}  {}  ", candidate.name);
        let tail = format!("  {}", candidate.cause.label());
        let room = width.saturating_sub(head.chars().count() + tail.chars().count());
        lines.push(Line::from(vec![
            Span::styled(head, name_style),
            Span::styled(
                clip(&format!("{place}  {}", tilde(&candidate.cwd)), room),
                muted,
            ),
            Span::styled(tail, bold(role.warning)),
        ]));
        for evidence in &candidate.evidence {
            lines.push(Line::styled(format!("    {evidence}"), muted));
        }
        if let Some(fix) = &candidate.fix {
            lines.push(Line::styled(
                format!("    {fix}"),
                Style::default().fg(role.warning),
            ));
        }
        let confidence = match candidate.confidence {
            Confidence::Certain => role.success,
            Confidence::Likely => role.accent,
            Confidence::Guess => role.muted,
        };
        let mut choices = vec![
            Span::raw("    "),
            Span::styled(
                format!("({})", candidate.confidence.label()),
                Style::default().fg(confidence),
            ),
        ];
        let chosen = dialog
            .chosen
            .get(index)
            .copied()
            .unwrap_or(candidate.suggested);
        for (action_index, action) in candidate.actions.iter().enumerate() {
            choices.push(Span::raw("  "));
            let label = format!("[{}] {}", action.key(), action.label());
            if action_index == chosen {
                let style = if selected {
                    bold(role.accent).bg(role.keycap)
                } else {
                    bold(role.text)
                };
                choices.push(Span::styled(format!("{label} ✓"), style));
            } else {
                choices.push(Span::styled(label, muted));
            }
        }
        if matches!(&dialog.stage, Stage::Applying(key) if *key == candidate.key) {
            choices.push(Span::styled(
                format!("  {spinner} applying…"),
                bold(role.accent),
            ));
        }
        lines.push(Line::from(choices));
    }
    if !dialog.done.is_empty() || dialog.error.is_some() {
        lines.push(Line::raw(""));
    }
    for done in &dialog.done {
        lines.push(Line::styled(
            format!("{SUCCESS_MARK} {done}"),
            Style::default().fg(role.success),
        ));
    }
    if let Some(error) = &dialog.error {
        lines.push(Line::styled(format!("✗ {error}"), bold(role.danger)));
    }
    (lines, selected_at)
}

#[cfg(test)]
mod tests {
    use corgi_core::orphans::{Action, Candidate, Cause};

    use super::*;
    use crate::{app::Overlay, test_support::test_app};

    fn candidates() -> Vec<Candidate> {
        vec![
            Candidate {
                key: "pane:wQ:p1".into(),
                cause: Cause::Worker,
                confidence: Confidence::Certain,
                pane_id: Some("wQ:p1".into()),
                workspace_id: "wQ".into(),
                workspace: "pay".into(),
                harness: "claude".into(),
                name: "restored-w1".into(),
                cwd: "/repos/webshop-pay".into(),
                summary: String::new(),
                evidence: vec![
                    "Ledger entry r-pay (w-pay, dispatched) names this checkout and branch pay"
                        .into(),
                ],
                fix: None,
                actions: vec![
                    Action::Worker {
                        project: "webshop".into(),
                        project_root: "/repos/webshop".into(),
                        request_id: "r-pay".into(),
                        agent: "w-pay".into(),
                    },
                    Action::Leave,
                    Action::Close {
                        target: "pane".into(),
                    },
                ],
                suggested: 0,
                fingerprint: "f".into(),
            },
            Candidate {
                key: "pane:wR:p2".into(),
                cause: Cause::Hidden,
                confidence: Confidence::Likely,
                pane_id: Some("wR:p2".into()),
                workspace_id: "wR".into(),
                workspace: "scratchpad".into(),
                harness: "codex".into(),
                name: "codex".into(),
                cwd: "/repos/scratchpad".into(),
                summary: String::new(),
                evidence: vec!["codex runs in the foreground at /repos/scratchpad (pid 41)".into()],
                fix: Some(
                    "Run `herdr integration install codex` so Herdr learns Codex sessions".into(),
                ),
                actions: vec![
                    Action::Leave,
                    Action::Close {
                        target: "pane".into(),
                    },
                ],
                suggested: 0,
                fingerprint: "g".into(),
            },
        ]
    }

    fn drawn(app: &mut App) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).unwrap();
        terminal
            .draw(|frame| super::super::draw(frame, app))
            .unwrap();
        crate::test_support::buffer_text(terminal.backend().buffer())
    }

    use crate::app::App;

    #[test]
    fn the_dialog_lists_each_candidate_its_evidence_fix_and_chosen_action() {
        let mut app = test_app();
        app.overlay = Overlay::Orphans(OrphansDialog::with(candidates()));
        let text = drawn(&mut app);
        assert!(text.contains("◆ Orphaned sessions"), "{text}");
        assert!(text.contains("● claude  restored-w1"), "{text}");
        assert!(text.contains("worker, untracked"), "{text}");
        assert!(text.contains("[w] worker w-pay of webshop ✓"), "{text}");
        assert!(text.contains("○ codex  codex"), "{text}");
        assert!(text.contains("herdr integration install codex"), "{text}");
        assert!(text.contains("[l] leave ✓"), "{text}");
        assert!(text.contains("apply"), "{text}");
    }

    #[test]
    fn with_nothing_found_the_dialog_says_so() {
        let mut app = test_app();
        app.overlay = Overlay::Orphans(OrphansDialog::with(Vec::new()));
        assert!(drawn(&mut app).contains("No orphaned sessions"));
    }
}
