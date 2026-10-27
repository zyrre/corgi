use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
};

use crate::{
    app::{CloseTarget, CloseWorkspaceForm, ConflictHelp, MergePhase, MergeWorktreeForm},
    paths::dir_name,
};

use super::{
    DialogContent, Drawn, FAILURE_MARK, POPUP_MARK, SUCCESS_MARK, bold, clip, draw_message_dialog,
    indent_left_block, outcome_line, step_lines,
};
use crate::theme::roles;

// Commits the merge dialog lists before summing up the rest, and the columns
// a subject may take before it is cut, so one long message cannot widen the
// box for all the others.
const MERGE_COMMIT_ROWS: usize = 10;
const MERGE_SUBJECT_WIDTH: usize = 64;
// The upstream's new commits are only a sample of what a pull brings in.
const UPSTREAM_COMMIT_ROWS: usize = 5;

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
        Line::styled(question, bold(roles().text)),
        Line::raw(""),
        Line::styled(detail.to_string(), Style::default().fg(roles().muted)),
    ];
    if matches!(form.target, CloseTarget::Worktree { .. }) {
        lines.push(Line::styled(
            "Stops its panes and deletes the checkout from disk; the branch is kept.",
            Style::default().fg(roles().muted),
        ));
    }
    DialogContent {
        mark: POPUP_MARK,
        title,
        color: roles().danger,
        lines,
        legend: vec![
            ("Enter", confirm, roles().danger),
            ("Esc", "cancel", roles().warning),
        ],
    }
}

/// A notice another command left for the dashboard, such as `corgi setup`
/// saying which key opens Corgi: a success, stamped once it has opened.
pub(super) fn draw_notice(frame: &mut Frame<'_>, area: Rect, message: &str) -> Drawn {
    draw_message_dialog(
        frame,
        area,
        DialogContent {
            mark: SUCCESS_MARK,
            title: "Corgi set up",
            color: roles().accent,
            lines: vec![outcome_line(true, message.to_string())],
            legend: vec![("Enter / Esc", "close", roles().warning)],
        },
    )
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
        lines.push(Line::styled(
            form.task.trim().to_string(),
            bold(roles().text),
        ));
        lines.push(Line::styled(
            format!("Worktree {worktree}"),
            Style::default().fg(roles().muted),
        ));
    } else {
        lines.push(Line::styled(
            format!("Worktree {worktree}"),
            bold(roles().text),
        ));
    }
    lines.push(Line::raw(""));
    lines.extend([
        // One line, so the two names centre as a pair instead of each
        // finding its own middle a few columns off the other's.
        Line::from(vec![
            Span::styled(&form.source_branch, bold(roles().accent)),
            Span::styled("  →  ", Style::default().fg(roles().muted)),
            Span::styled(&form.target_branch, bold(roles().success)),
        ]),
        Line::raw(""),
    ]);
    let (mark, title, color, legend) = match &form.phase {
        MergePhase::Confirm => {
            lines.extend(merge_commit_lines(
                &form.commits,
                MERGE_COMMIT_ROWS,
                &form.target_branch,
            ));
            lines.push(Line::styled(
                match form.upstream.name() {
                    Some(upstream) => format!(
                        "Fetches {upstream}, then runs git merge and git push in the primary checkout."
                    ),
                    None => "Runs git merge, then git push in the primary checkout.".into(),
                },
                Style::default().fg(roles().muted),
            ));
            lines.push(Line::styled(
                "The agent worktree stays open.",
                Style::default().fg(roles().muted),
            ));
            (
                POPUP_MARK,
                "Merge reviewed work",
                roles().success,
                vec![
                    ("Enter", "merge + push", roles().success),
                    ("Esc", "cancel", roles().warning),
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
                Style::default().fg(roles().muted),
            ));
            (POPUP_MARK, "Merge and push", roles().accent, Vec::new())
        }
        MergePhase::Succeeded => {
            lines.push(outcome_line(true, "Merge and push complete.".into()));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("Close worktree {worktree} too?"),
                bold(roles().text),
            ));
            lines.push(Line::styled(
                "Stops its panes and deletes the checkout from disk; the branch is kept.",
                Style::default().fg(roles().muted),
            ));
            (
                SUCCESS_MARK,
                "Merged and pushed",
                roles().accent,
                vec![
                    ("x", "close worktree", roles().danger),
                    ("Enter / Esc", "keep it open", roles().warning),
                ],
            )
        }
        MergePhase::Failed(error) => {
            lines.push(outcome_line(
                false,
                "Merge and push did not complete.".into(),
            ));
            lines.push(Line::styled(error, Style::default().fg(roles().muted)));
            (
                FAILURE_MARK,
                "Merge/push error",
                roles().danger,
                vec![
                    ("Enter", "retry", roles().success),
                    ("Esc", "close", roles().warning),
                ],
            )
        }
        MergePhase::Behind(commits) => {
            let upstream = form.upstream.name().unwrap_or("The upstream");
            lines.push(Line::styled(
                format!(
                    "{upstream} has commits {} lacks; a push now would be rejected.",
                    form.target_branch
                ),
                bold(roles().warning),
            ));
            lines.push(Line::raw(""));
            lines.extend(merge_commit_lines(
                commits,
                UPSTREAM_COMMIT_ROWS,
                &form.target_branch,
            ));
            lines.push(Line::styled(
                format!(
                    "Enter pulls them into {} (a fast-forward when it can be), then merges and pushes.",
                    form.target_branch
                ),
                Style::default().fg(roles().muted),
            ));
            lines.push(Line::styled(
                "Nothing is merged yet.",
                Style::default().fg(roles().muted),
            ));
            (
                POPUP_MARK,
                "Upstream moved on",
                roles().warning,
                vec![
                    ("Enter", "pull, then merge + push", roles().success),
                    ("Esc", "stop", roles().warning),
                ],
            )
        }
        MergePhase::FetchFailed(error) => {
            let upstream = form.upstream.name().unwrap_or("the upstream");
            lines.push(Line::styled(
                format!("Could not fetch {upstream}."),
                bold(roles().warning),
            ));
            lines.extend(error_lines(error));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("If {upstream} moved on, the push after the merge will be rejected."),
                Style::default().fg(roles().muted),
            ));
            lines.push(Line::styled(
                "Nothing is merged yet.",
                Style::default().fg(roles().muted),
            ));
            (
                POPUP_MARK,
                "Upstream not checked",
                roles().warning,
                vec![
                    ("Enter", "merge + push anyway", roles().success),
                    ("Esc", "stop", roles().warning),
                ],
            )
        }
        MergePhase::PushFailed {
            commit,
            error,
            help,
        } => {
            let at = if commit.is_empty() {
                String::new()
            } else {
                format!(" at {commit}")
            };
            lines.push(outcome_line(
                false,
                format!(
                    "Merged into {}{at}, but the push failed.",
                    form.target_branch
                ),
            ));
            lines.push(Line::raw(""));
            lines.extend(error_lines(error));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                match form.upstream.name() {
                    Some(upstream) => format!(
                        "The branch is merged locally; Enter fetches {upstream} and pushes again."
                    ),
                    None => "The branch is merged locally; Enter pushes again.".into(),
                },
                Style::default().fg(roles().muted),
            ));
            let mut legend = Vec::new();
            match help {
                ConflictHelp::Supervisor(_) => {
                    lines.push(Line::styled(
                        "Its supervisor can tell you what the error means and what to run.",
                        Style::default().fg(roles().text),
                    ));
                    legend.push(("s", "ask the supervisor", roles().success));
                }
                ConflictHelp::NoSupervisor => lines.push(Line::styled(
                    "No supervisor is running for this project.",
                    Style::default().fg(roles().warning),
                )),
                ConflictHelp::Unavailable => {}
            }
            legend.push(("Enter", "retry", roles().success));
            legend.push(("Esc", "close", roles().warning));
            (FAILURE_MARK, "Push failed", roles().danger, legend)
        }
        MergePhase::Conflicted { files, help } => {
            lines.push(outcome_line(false, "Merge stopped on conflicts.".into()));
            lines.push(Line::raw(""));
            lines.extend(conflict_file_lines(files));
            lines.push(Line::raw(""));
            let by_hand = format!(
                "Resolve them in {} and run git merge --continue, or git merge --abort.",
                form.project_root.display()
            );
            let legend = match help {
                ConflictHelp::Supervisor(_) => {
                    lines.push(Line::styled(
                        format!(
                            "Its supervisor can have the agent merge {} into its own branch.",
                            form.target_branch
                        ),
                        Style::default().fg(roles().text),
                    ));
                    lines.push(Line::styled(
                        "The worktree stays open; merge again once it reports.",
                        Style::default().fg(roles().muted),
                    ));
                    lines.push(Line::styled(
                        "Esc leaves the primary checkout mid-merge to resolve by hand.",
                        Style::default().fg(roles().muted),
                    ));
                    vec![
                        (
                            "s",
                            "abort merge and ask the supervisor to fix",
                            roles().success,
                        ),
                        ("Esc", "resolve by hand", roles().warning),
                    ]
                }
                ConflictHelp::NoSupervisor | ConflictHelp::Unavailable => {
                    if *help == ConflictHelp::NoSupervisor {
                        lines.push(Line::styled(
                            "No supervisor is running for this project.",
                            Style::default().fg(roles().warning),
                        ));
                    }
                    lines.push(Line::styled(by_hand, Style::default().fg(roles().muted)));
                    vec![
                        ("Enter", "retry", roles().success),
                        ("Esc", "close", roles().warning),
                    ]
                }
            };
            (FAILURE_MARK, "Merge conflict", roles().danger, legend)
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

/// Git's error, a line of the dialog for each of its lines, left-aligned as
/// Git wrote them.
fn error_lines(error: &str) -> Vec<Line<'static>> {
    error
        .lines()
        .map(|line| {
            Line::styled(line.to_string(), Style::default().fg(roles().muted))
                .alignment(Alignment::Left)
        })
        .collect()
}

/// The commits a merge brings in: a heading with their number, then the
/// list itself, left-aligned so the hashes line up in a dialog that centres
/// everything else. The list is cut off after `rows` so a long branch
/// cannot push the keys out of view.
fn merge_commit_lines(commits: &[String], rows: usize, target_branch: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if commits.is_empty() {
        lines.push(Line::styled(
            format!("No commits ahead of {target_branch}."),
            Style::default().fg(roles().warning),
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
        Style::default().fg(roles().muted),
    ));
    for commit in commits.iter().take(rows) {
        let (hash, subject) = split_commit(commit);
        lines.push(
            Line::from(vec![
                Span::styled(format!("{hash}  "), Style::default().fg(roles().accent)),
                Span::styled(
                    clip(subject, MERGE_SUBJECT_WIDTH),
                    Style::default().fg(roles().text),
                ),
            ])
            .alignment(Alignment::Left),
        );
    }
    if count > rows {
        lines.push(
            Line::styled(
                format!("… and {} more", count - rows),
                Style::default().fg(roles().muted),
            )
            .alignment(Alignment::Left),
        );
    }
    lines.push(Line::raw(""));
    lines
}

/// The files a merge left conflicted, left-aligned under their count and cut
/// off like the commit list.
fn conflict_file_lines(files: &[String]) -> Vec<Line<'static>> {
    let count = files.len();
    let mut lines = vec![Line::styled(
        if count == 1 {
            "1 conflicted file".to_string()
        } else {
            format!("{count} conflicted files")
        },
        Style::default().fg(roles().muted),
    )];
    for file in files.iter().take(MERGE_COMMIT_ROWS) {
        lines.push(
            Line::styled(
                clip(file, MERGE_SUBJECT_WIDTH),
                Style::default().fg(roles().warning),
            )
            .alignment(Alignment::Left),
        );
    }
    if count > MERGE_COMMIT_ROWS {
        lines.push(
            Line::styled(
                format!("… and {} more", count - MERGE_COMMIT_ROWS),
                Style::default().fg(roles().muted),
            )
            .alignment(Alignment::Left),
        );
    }
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
        app::{MergeWorktreeForm, Overlay, Upstream},
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
            agent: "w-reviewed-agent".into(),
            project_root: PathBuf::from("/repos/corgi"),
            worktree_checkout: PathBuf::from("/worktrees/corgi/worktree-reviewed-agent"),
            source_branch: "worktree/reviewed-agent".into(),
            target_branch: "main".into(),
            task: String::new(),
            commits: Vec::new(),
            upstream: Default::default(),
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
    fn a_conflicted_merge_lists_its_files_and_offers_the_supervisor_only_if_running() {
        let mut app = test_app();
        app.overlay = Overlay::merge(MergeWorktreeForm {
            label: "corgi/reviewed-agent".into(),
            workspace_id: "w7".into(),
            agent: "w-reviewed-agent".into(),
            project_root: PathBuf::from("/repos/corgi"),
            worktree_checkout: PathBuf::from("/worktrees/corgi/worktree-reviewed-agent"),
            source_branch: "worktree/reviewed-agent".into(),
            target_branch: "main".into(),
            task: "Popup windows styling review".into(),
            commits: Vec::new(),
            upstream: Default::default(),
            phase: MergePhase::Conflicted {
                files: vec!["src/ui/dialogs.rs".into(), "README.md".into()],
                help: ConflictHelp::Supervisor("supervisor-corgi".into()),
            },
        });
        let mut terminal = test_terminal(120, 40);
        terminal
            .draw(|frame| draw_overlay(frame, frame.area(), &mut app))
            .expect("draw conflicted merge dialog");
        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("2 conflicted files"), "{rendered}");
        assert!(rendered.contains("src/ui/dialogs.rs"), "{rendered}");
        assert!(rendered.contains("README.md"), "{rendered}");
        assert!(
            rendered.contains("merge main into its own branch"),
            "{rendered}"
        );
        assert!(
            rendered.contains(" s  abort merge and ask the supervisor to fix"),
            "{rendered}"
        );

        let form = app.overlay.merge_worktree_form_mut().expect("form");
        form.phase = MergePhase::Conflicted {
            files: vec!["README.md".into()],
            help: ConflictHelp::NoSupervisor,
        };
        terminal
            .draw(|frame| draw_overlay(frame, frame.area(), &mut app))
            .expect("draw conflicted merge dialog");
        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("1 conflicted file"), "{rendered}");
        assert!(rendered.contains("No supervisor is running"), "{rendered}");
        assert!(rendered.contains("git merge --continue"), "{rendered}");
        assert!(!rendered.contains("supervisor to fix"), "{rendered}");
    }

    #[test]
    fn the_upstream_popups_list_its_commits_and_say_what_failed() {
        let mut app = test_app();
        let commits: Vec<String> = (0..16)
            .map(|index| format!("{index:07x} Pushed from elsewhere {index}"))
            .collect();
        app.overlay = Overlay::merge(MergeWorktreeForm {
            label: "corgi/reviewed-agent".into(),
            workspace_id: "w7".into(),
            agent: "w-reviewed-agent".into(),
            project_root: PathBuf::from("/repos/corgi"),
            worktree_checkout: PathBuf::from("/worktrees/corgi/worktree-reviewed-agent"),
            source_branch: "worktree/reviewed-agent".into(),
            target_branch: "main".into(),
            task: "Popup windows styling review".into(),
            commits: Vec::new(),
            upstream: Upstream::Check("origin/main".into()),
            phase: MergePhase::Behind(commits),
        });
        let mut terminal = test_terminal(120, 40);
        let mut draw = |app: &mut crate::app::App| {
            terminal
                .draw(|frame| draw_overlay(frame, frame.area(), app))
                .expect("draw merge dialog");
            buffer_text(terminal.backend().buffer())
        };
        let rendered = draw(&mut app);
        assert!(rendered.contains("Upstream moved on"), "{rendered}");
        assert!(
            rendered.contains("origin/main has commits main lacks"),
            "{rendered}"
        );
        assert!(rendered.contains("16 commits"), "{rendered}");
        assert!(rendered.contains("Pushed from elsewhere 4"), "{rendered}");
        assert!(!rendered.contains("Pushed from elsewhere 5"), "{rendered}");
        assert!(rendered.contains("… and 11 more"), "{rendered}");
        assert!(
            rendered.contains(" Enter  pull, then merge + push"),
            "{rendered}"
        );
        assert!(rendered.contains(" Esc  stop"), "{rendered}");

        let form = app.overlay.merge_worktree_form_mut().expect("form");
        form.phase = MergePhase::FetchFailed("git fetch origin: Could not resolve host".into());
        let rendered = draw(&mut app);
        assert!(
            rendered.contains("Could not fetch origin/main."),
            "{rendered}"
        );
        assert!(rendered.contains("Could not resolve host"), "{rendered}");
        assert!(
            rendered.contains(" Enter  merge + push anyway"),
            "{rendered}"
        );

        let form = app.overlay.merge_worktree_form_mut().expect("form");
        form.phase = MergePhase::PushFailed {
            commit: "06d069d".into(),
            error: "! [rejected] main -> main (fetch first)\nerror: failed to push some refs"
                .into(),
            help: ConflictHelp::Supervisor("supervisor-corgi".into()),
        };
        let rendered = draw(&mut app);
        assert!(rendered.contains("Push failed"), "{rendered}");
        assert!(
            rendered.contains("Merged into main at 06d069d, but the push failed."),
            "{rendered}"
        );
        assert!(
            rendered.contains("main -> main (fetch first)"),
            "{rendered}"
        );
        assert!(rendered.contains(" s  ask the supervisor"), "{rendered}");
        assert!(rendered.contains(" Enter  retry"), "{rendered}");

        let form = app.overlay.merge_worktree_form_mut().expect("form");
        form.phase = MergePhase::PushFailed {
            commit: "06d069d".into(),
            error: "fatal: unreachable".into(),
            help: ConflictHelp::NoSupervisor,
        };
        let rendered = draw(&mut app);
        assert!(rendered.contains("No supervisor is running"), "{rendered}");
        assert!(!rendered.contains("ask the supervisor"), "{rendered}");
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
            agent: "w-reviewed-agent".into(),
            project_root: PathBuf::from("/repos/corgi"),
            worktree_checkout: PathBuf::from("/worktrees/corgi/worktree-reviewed-agent"),
            source_branch: "worktree/reviewed-agent".into(),
            target_branch: "main".into(),
            task: "Popup windows styling review".into(),
            commits,
            upstream: Default::default(),
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
