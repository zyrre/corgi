use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
};

use crate::{
    app::{CloseTarget, CloseWorkspaceForm, MergePhase, MergeWorktreeForm},
    paths::dir_name,
};

use super::{
    ACCENT, DANGER, DialogContent, Drawn, FAILURE_MARK, MUTED, POPUP_MARK, SUCCESS, SUCCESS_MARK,
    TEXT, WARNING, bold, clip, draw_message_dialog, indent_left_block, outcome_line, step_lines,
};

// Commits the merge dialog lists before summing up the rest, and the columns
// a subject may take before it is cut, so one long message cannot widen the
// box for all the others.
const MERGE_COMMIT_ROWS: usize = 10;
const MERGE_SUBJECT_WIDTH: usize = 64;

pub(super) fn draw_close_workspace(
    frame: &mut Frame<'_>,
    area: Rect,
    form: &CloseWorkspaceForm,
) -> Drawn {
    draw_message_dialog(frame, area, close_dialog_lines(form))
}

pub(crate) fn close_dialog_lines(form: &CloseWorkspaceForm) -> DialogContent<'_> {
    let (title, question, detail, confirm) = match &form.target {
        CloseTarget::Worktree { force: true, .. } => (
            "Discard changes",
            format!("Discard uncommitted changes in {}?", form.label),
            "The checkout has modified or untracked files. Forcing removal deletes them.",
            "discard and remove",
        ),
        CloseTarget::Worktree {
            checkout,
            force: false,
        } => (
            "Remove worktree",
            format!("Remove worktree {}?", form.label),
            checkout.as_str(),
            "remove",
        ),
        CloseTarget::Tab { .. } => (
            "Close agent tab",
            format!("Close agent tab {}?", form.label),
            "Its pane will stop; the project workspace and other agents stay open.",
            "close tab",
        ),
        CloseTarget::Workspace => (
            "Close workspace",
            format!("Close workspace {}?", form.label),
            "All of its tabs and panes will be stopped.",
            "close",
        ),
    };
    let mut lines = vec![
        Line::styled(question, bold(TEXT)),
        Line::raw(""),
        Line::styled(detail.to_string(), Style::default().fg(MUTED)),
    ];
    if matches!(form.target, CloseTarget::Worktree { .. }) {
        lines.push(Line::styled(
            "Stops its panes and deletes the checkout from disk; the branch is kept.",
            Style::default().fg(MUTED),
        ));
    }
    DialogContent {
        mark: POPUP_MARK,
        title,
        color: DANGER,
        lines,
        legend: vec![("Enter", confirm, DANGER), ("Esc", "cancel", WARNING)],
    }
}

pub(super) fn draw_merge_worktree(
    frame: &mut Frame<'_>,
    area: Rect,
    form: &MergeWorktreeForm,
    spinner: &'static str,
) -> Drawn {
    let content = indent_left_block(area, merge_dialog_lines(form, spinner));
    draw_message_dialog(frame, area, content)
}

/// The merge dialog's title, frame color, text and keys for the phase it is
/// in, its steps beside `spinner` while it runs.
pub(crate) fn merge_dialog_lines<'a>(
    form: &'a MergeWorktreeForm,
    spinner: &'static str,
) -> DialogContent<'a> {
    let worktree = dir_name(&form.worktree_checkout).unwrap_or(&form.label);
    // The work is named first, the way the agent's own row names it, and
    // the worktree it lives in comes second; a merge is of the task, and
    // the branch is only where it happens to be.
    let mut lines = Vec::new();
    if !form.task.trim().is_empty() {
        lines.push(Line::styled(form.task.trim().to_string(), bold(TEXT)));
        lines.push(Line::styled(
            format!("Worktree {worktree}"),
            Style::default().fg(MUTED),
        ));
    } else {
        lines.push(Line::styled(format!("Worktree {worktree}"), bold(TEXT)));
    }
    lines.push(Line::raw(""));
    lines.extend([
        // One line, so the two names centre as a pair instead of each
        // finding its own middle a few columns off the other's.
        Line::from(vec![
            Span::styled(&form.source_branch, bold(ACCENT)),
            Span::styled("  →  ", Style::default().fg(MUTED)),
            Span::styled(&form.target_branch, bold(SUCCESS)),
        ]),
        Line::raw(""),
    ]);
    let (mark, title, color, legend) = match &form.phase {
        MergePhase::Confirm => {
            lines.extend(merge_commit_lines(&form.commits, &form.target_branch));
            lines.push(Line::styled(
                "Runs git merge, then git push in the primary checkout.",
                Style::default().fg(MUTED),
            ));
            lines.push(Line::styled(
                "The agent worktree stays open.",
                Style::default().fg(MUTED),
            ));
            (
                POPUP_MARK,
                "Merge reviewed work",
                SUCCESS,
                vec![
                    ("Enter", "merge + push", SUCCESS),
                    ("Esc", "cancel", WARNING),
                ],
            )
        }
        MergePhase::Running(step) => {
            lines.extend(
                step_lines(&form.steps(), *step, spinner)
                    .into_iter()
                    .map(|line| line.alignment(Alignment::Left)),
            );
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "Please wait; this operation cannot be cancelled safely.",
                Style::default().fg(MUTED),
            ));
            (POPUP_MARK, "Merge and push", ACCENT, Vec::new())
        }
        MergePhase::Succeeded => {
            lines.push(outcome_line(true, "Merge and push complete.".into()));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("Close worktree {worktree} too?"),
                bold(TEXT),
            ));
            lines.push(Line::styled(
                "Stops its panes and deletes the checkout from disk; the branch is kept.",
                Style::default().fg(MUTED),
            ));
            (
                SUCCESS_MARK,
                "Merged and pushed",
                ACCENT,
                vec![
                    ("x", "close worktree", DANGER),
                    ("Enter / Esc", "keep it open", WARNING),
                ],
            )
        }
        MergePhase::Failed(error) => {
            lines.push(outcome_line(
                false,
                "Merge and push did not complete.".into(),
            ));
            lines.push(Line::styled(error, Style::default().fg(MUTED)));
            (
                FAILURE_MARK,
                "Merge/push error",
                DANGER,
                vec![("Enter", "retry", SUCCESS), ("Esc", "close", WARNING)],
            )
        }
    };
    DialogContent {
        mark,
        title,
        color,
        lines,
        legend,
    }
}

/// The commits a merge brings in: a heading with their number, then the
/// list itself, left-aligned so the hashes line up in a dialog that centres
/// everything else. The list is cut off after a screenful so a long branch
/// cannot push the keys out of view.
fn merge_commit_lines(commits: &[String], target_branch: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if commits.is_empty() {
        lines.push(Line::styled(
            format!("No commits ahead of {target_branch}."),
            Style::default().fg(WARNING),
        ));
        lines.push(Line::raw(""));
        return lines;
    }
    let count = commits.len();
    lines.push(Line::styled(
        if count == 1 {
            "1 commit".to_string()
        } else {
            format!("{count} commits")
        },
        Style::default().fg(MUTED),
    ));
    for commit in commits.iter().take(MERGE_COMMIT_ROWS) {
        let (hash, subject) = split_commit(commit);
        lines.push(
            Line::from(vec![
                Span::styled(format!("{hash}  "), Style::default().fg(ACCENT)),
                Span::styled(
                    clip(subject, MERGE_SUBJECT_WIDTH),
                    Style::default().fg(TEXT),
                ),
            ])
            .alignment(Alignment::Left),
        );
    }
    if count > MERGE_COMMIT_ROWS {
        lines.push(
            Line::styled(
                format!("… and {} more", count - MERGE_COMMIT_ROWS),
                Style::default().fg(MUTED),
            )
            .alignment(Alignment::Left),
        );
    }
    lines.push(Line::raw(""));
    lines
}

/// A `<hash> <subject>` log line split in two.
fn split_commit(commit: &str) -> (&str, &str) {
    commit.split_once(' ').unwrap_or((commit, ""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{MergeWorktreeForm, Overlay},
        test_support::{buffer_rows, buffer_text, test_app, test_terminal},
        ui::draw_overlay,
    };

    use std::path::PathBuf;

    #[test]
    fn a_finished_merge_asks_whether_to_close_the_worktree() {
        let mut app = test_app();
        app.overlay = Overlay::merge(MergeWorktreeForm {
            label: "corgi/reviewed-agent".into(),
            workspace_id: "w7".into(),
            project_root: PathBuf::from("/repos/corgi"),
            worktree_checkout: PathBuf::from("/worktrees/corgi/worktree-reviewed-agent"),
            source_branch: "worktree/reviewed-agent".into(),
            target_branch: "main".into(),
            task: String::new(),
            commits: Vec::new(),
            phase: MergePhase::Succeeded,
        });
        // The narrow terminal wraps the explanation, so the hints only survive
        // if the popup is sized from rendered rows rather than logical lines.
        for width in [100, 64] {
            let mut terminal = test_terminal(width, 30);
            terminal
                .draw(|frame| draw_overlay(frame, frame.area(), &mut app))
                .expect("draw finished merge dialog");
            let rendered = buffer_text(terminal.backend().buffer());

            assert!(rendered.contains("Merge and push complete"), "{rendered}");
            assert!(rendered.contains("Close worktree"), "{rendered}");
            assert!(rendered.contains(" x  close worktree"), "{rendered}");
            assert!(rendered.contains("keep it open"), "{rendered}");
        }
    }

    #[test]
    fn the_merge_confirmation_names_the_task_and_lists_its_commits() {
        let mut app = test_app();
        let commits: Vec<String> = (0..12)
            .map(|index| format!("{index:07x} Commit number {index} on the branch"))
            .collect();
        app.overlay = Overlay::merge(MergeWorktreeForm {
            label: "corgi/reviewed-agent".into(),
            workspace_id: "w7".into(),
            project_root: PathBuf::from("/repos/corgi"),
            worktree_checkout: PathBuf::from("/worktrees/corgi/worktree-reviewed-agent"),
            source_branch: "worktree/reviewed-agent".into(),
            target_branch: "main".into(),
            task: "Popup windows styling review".into(),
            commits,
            phase: MergePhase::Confirm,
        });
        let mut terminal = test_terminal(120, 40);
        terminal
            .draw(|frame| draw_overlay(frame, frame.area(), &mut app))
            .expect("draw merge dialog");
        let rows = buffer_rows(terminal.backend().buffer());
        let rendered = rows.join("\n");
        assert!(
            rendered.contains("Popup windows styling review"),
            "{rendered}"
        );
        assert!(
            rendered.contains("Worktree worktree-reviewed-agent"),
            "{rendered}"
        );
        assert!(rendered.contains("12 commits"), "{rendered}");
        assert!(rendered.contains("0000000  Commit number 0"), "{rendered}");
        assert!(rendered.contains("0000009  Commit number 9"), "{rendered}");
        assert!(!rendered.contains("Commit number 10"), "{rendered}");
        assert!(rendered.contains("… and 2 more"), "{rendered}");
        assert!(rendered.contains(" Enter  merge + push"), "{rendered}");
        // The hashes start in one column: the list is left-aligned inside a
        // dialog that centres everything else.
        let columns: Vec<usize> = rows
            .iter()
            .filter_map(|row| {
                row.find("Commit number")
                    .map(|_| row.find("000000").unwrap())
            })
            .collect();
        assert_eq!(columns.len(), 10, "{rendered}");
        assert!(
            columns.iter().all(|column| *column == columns[0]),
            "{columns:?}\n{rendered}"
        );

        // Without a task the worktree name is the headline, and an empty list
        // says so instead of leaving a gap.
        let form = app.overlay.merge_worktree_form_mut().expect("form");
        form.task.clear();
        form.commits.clear();
        terminal
            .draw(|frame| draw_overlay(frame, frame.area(), &mut app))
            .expect("draw merge dialog");
        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("No commits ahead of main"), "{rendered}");
    }
}
