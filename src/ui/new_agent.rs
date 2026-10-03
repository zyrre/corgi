use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Padding, Paragraph},
};

use unicode_width::UnicodeWidthStr;

use crate::{
    app::{LaunchState, NewAgentForm, NewAgentLaunch, NewField},
    harness::Harness,
    model::DashboardAgent,
    paths::{dir_name, tilde},
    textfield::{caret_row_column, wrap_rows},
};

use super::choice_list::draw_choice_list;
use super::status::model_color;
use super::{
    ACCENT, DANGER, DIALOG_FRAME_WIDTH, DialogContent, Drawn, FAILURE_MARK, FOCUS_BAR, LegendKey,
    MUTED, POPUP_MARK, SUCCESS, SUCCESS_MARK, TEXT, WARNING, bold, centered, dialog_block,
    dialog_width, draw_message_dialog, indent_left_block, legend_line, legend_width, outcome_line,
    popup_title, rule_line, step_lines,
};

// Columns in front of every new-agent form value: the row marker and the
// label, so the task text and every setting start in the same column.
const NEW_AGENT_GUTTER: u16 = 2;
const NEW_AGENT_LABEL_WIDTH: u16 = 10;
// Rows of the new-agent panel that are neither task nor setting rows: the
// border, the gap after the task, the gap after the settings, and the rule
// and the legend.
const NEW_AGENT_CHROME_HEIGHT: u16 = 6;

/// Why the form is about to start the project's corgi rather than a
/// worker, and on which harness.
fn corgi_notice(project: &str, new_project: bool, harness: &Harness) -> Line<'static> {
    let name = dir_name(project.trim())
        .unwrap_or(crate::corgi::UNNAMED_PROJECT)
        .to_string();
    let reason = if new_project {
        format!("{name} is a new project")
    } else {
        format!("No agent works in {name} yet")
    };
    Line::from(vec![
        Span::styled("  ✦ ", bold(ACCENT)),
        Span::styled(
            if harness.supports_corgi() {
                format!("{reason}, so this starts its corgi on {harness} with your task")
            } else {
                format!(
                    "{reason}, so this starts its corgi, on {}",
                    Harness::corgi_kinds()
                )
            },
            Style::default().fg(ACCENT),
        ),
    ])
}

/// Draws the new-agent form. The width is settled first, so the task knows
/// where it wraps before the panel is sized from the rows that wrapping
/// produces.
pub(super) fn draw_new_agent(
    frame: &mut Frame<'_>,
    area: Rect,
    form: &mut NewAgentForm,
    agents: &[DashboardAgent],
) -> Drawn {
    // Asked once, because it looks at the filesystem and every part of the
    // form below depends on it.
    let new_project = form.is_new_project();
    let starts_corgi = form.starts_corgi_among(new_project, agents);
    let fields = form.fields_for(new_project);
    // The legend is the one row that cannot wrap or scroll, so the panel is
    // never narrower than the widest legend any row shows, whatever share of
    // the screen it would take; measuring every row's keeps the panel one
    // width as the cursor moves and lists open over the legend.
    let legend_width = fields
        .iter()
        .map(|field| legend_width(legend_keys(*field), LEGEND_SEPARATOR))
        .max()
        .unwrap_or(0) as u16
        + DIALOG_FRAME_WIDTH;
    let width = dialog_width(area, 84).max(legend_width).min(area.width);
    // Two columns stay free after the task text so a caret at the end of a
    // full row has somewhere to sit.
    let prompt_width = width
        .saturating_sub(DIALOG_FRAME_WIDTH + 2 + NEW_AGENT_GUTTER + NEW_AGENT_LABEL_WIDTH)
        .max(1) as usize;
    // Hand the width back to the form so its caret can be moved by the rows
    // drawn here.
    form.prompt_width = prompt_width;
    let prompt_rows = wrap_rows(&form.prompt, prompt_width);
    let (caret_row, caret_column) = caret_row_column(&form.prompt, &prompt_rows, form.prompt_caret);
    let settings: Vec<NewField> = fields
        .into_iter()
        .filter(|field| *field != NewField::Task)
        .collect();
    let error_rows = u16::from(form.error.is_some());
    let notice_rows = u16::from(starts_corgi);
    let fixed_rows = NEW_AGENT_CHROME_HEIGHT + settings.len() as u16 + notice_rows + error_rows;
    let visible_task_rows =
        (prompt_rows.len() as u16).clamp(1, area.height.saturating_sub(fixed_rows).max(1));
    let popup = centered(
        area,
        width,
        (fixed_rows + visible_task_rows).min(area.height),
    );
    frame.render_widget(Clear, popup);
    let title = if starts_corgi {
        "New corgi"
    } else if form.is_scratch() {
        "Scratch agent in ~"
    } else {
        "New agent"
    };
    let panel =
        dialog_block(&popup_title(POPUP_MARK, title), ACCENT).padding(Padding::horizontal(1));
    let content = panel.inner(popup);
    frame.render_widget(panel, popup);

    let mut constraints = vec![Constraint::Length(visible_task_rows), Constraint::Length(1)];
    constraints.extend(std::iter::repeat_n(Constraint::Length(1), settings.len()));
    constraints.push(Constraint::Length(1));
    if notice_rows > 0 {
        constraints.push(Constraint::Length(1));
    }
    if error_rows > 0 {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(1));
    let sections = Layout::vertical(constraints).split(content);

    // Scroll to the caret when the task is taller than the panel can be, so
    // its row stays on screen wherever in the text it sits.
    let first_shown_row = (caret_row + 1).saturating_sub(visible_task_rows as usize);
    let last_shown_row = (first_shown_row + visible_task_rows as usize).min(prompt_rows.len());
    let shown_rows: Vec<&str> = prompt_rows[first_shown_row..last_shown_row]
        .iter()
        .map(|row| &form.prompt[row.clone()])
        .collect();
    draw_task_row(frame, sections[0], form, &shown_rows);

    let mut focused_row = None;
    for (index, field) in settings.iter().enumerate() {
        let row = sections[2 + index];
        draw_setting_row(frame, row, form, *field, new_project);
        if form.field == *field {
            focused_row = Some(row);
        }
    }

    let legend_index = sections.len() - 1;
    let rule_index = legend_index - 1;
    if starts_corgi {
        frame.render_widget(
            Paragraph::new(corgi_notice(&form.project, new_project, &form.harness())),
            sections[rule_index - 1 - usize::from(error_rows)],
        );
    }
    if let Some(error) = &form.error {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("  ⚠ ", bold(DANGER)),
                Span::styled(error.as_str(), Style::default().fg(DANGER)),
            ])),
            sections[rule_index - 1],
        );
    }
    frame.render_widget(
        Paragraph::new(rule_line(sections[rule_index].width)),
        sections[rule_index],
    );
    frame.render_widget(
        Paragraph::new(new_agent_legend(form)),
        sections[legend_index],
    );
    let drawn = |caret| Drawn {
        area: popup,
        color: ACCENT,
        caret,
    };

    let value_column = NEW_AGENT_GUTTER + NEW_AGENT_LABEL_WIDTH;
    if let (Some(list), Some(row)) = (&form.list, focused_row) {
        // The list hangs off the row it belongs to, over whatever is below.
        let anchor = Rect {
            x: row.x + value_column - 1,
            y: row.y,
            width: row.width.saturating_sub(value_column - 1),
            height: 1,
        };
        draw_choice_list(frame, area, anchor, list);
        let caret_x =
            row.x + value_column + 2 + UnicodeWidthStr::width(list.filter.as_str()) as u16;
        return drawn(Some(Position::new(
            caret_x.min(row.right().saturating_sub(1)),
            row.y,
        )));
    }
    // The task is typed, so it shows a caret; a selector shows none, which
    // is how the row says it is chosen from, not typed in.
    if form.field == NewField::Task {
        let caret_x = sections[0].x + value_column + caret_column as u16;
        return drawn(Some(Position::new(
            caret_x.min(sections[0].right().saturating_sub(1)),
            sections[0].y + (caret_row - first_shown_row) as u16,
        )));
    }
    drawn(None)
}

/// The marker and label in front of a form row: the labels are dimmed and
/// line up, and the cursor row is marked, its label brightened and a bar laid
/// behind it, so the eye finds it before reading anything.
fn row_prefix(field: NewField, focused: bool) -> Vec<Span<'static>> {
    let (marker, color, modifier) = if focused {
        ("▸ ", TEXT, Modifier::BOLD)
    } else {
        ("  ", MUTED, Modifier::empty())
    };
    vec![
        Span::styled(marker, bold(ACCENT)),
        Span::styled(
            format!(
                "{:<width$}",
                field.label(),
                width = NEW_AGENT_LABEL_WIDTH as usize
            ),
            Style::default().fg(color).add_modifier(modifier),
        ),
    ]
}

/// A value is bright, and bold on the cursor row.
fn value_style(focused: bool) -> Style {
    value_style_in(TEXT, focused)
}

fn value_style_in(color: ratatui::style::Color, focused: bool) -> Style {
    if focused {
        bold(color)
    } else {
        Style::default().fg(color)
    }
}

/// The style of a form row's area: the focus bar behind the cursor row.
fn row_style(focused: bool) -> Style {
    if focused {
        Style::default().bg(FOCUS_BAR)
    } else {
        Style::default()
    }
}

/// The task rows: the first behind the label, the rest indented to the same
/// column, and a hint while nothing has been typed yet.
fn draw_task_row(frame: &mut Frame<'_>, area: Rect, form: &NewAgentForm, rows: &[&str]) {
    let focused = form.field == NewField::Task;
    let indent = " ".repeat((NEW_AGENT_GUTTER + NEW_AGENT_LABEL_WIDTH) as usize);
    let lines: Vec<Line<'_>> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let mut spans = if index == 0 {
                row_prefix(NewField::Task, focused)
            } else {
                vec![Span::raw(indent.clone())]
            };
            if form.prompt.is_empty() {
                spans.push(Span::styled(
                    if form.is_scratch() {
                        "Optional: leave empty to type into the session yourself"
                    } else {
                        "What should the agent do first?"
                    },
                    Style::default().fg(MUTED).add_modifier(Modifier::ITALIC),
                ));
            } else {
                spans.push(Span::styled((*row).to_string(), value_style(focused)));
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).style(row_style(focused)), area);
}

/// One setting row: label, value, a quieter detail, and the ▾ that marks it
/// as a selector, which every setting is. While its list is open the row
/// shows the filter being typed.
fn draw_setting_row(
    frame: &mut Frame<'_>,
    area: Rect,
    form: &NewAgentForm,
    field: NewField,
    new_project: bool,
) {
    let focused = form.field == field;
    let mut spans = row_prefix(field, focused);
    match (&form.list, focused) {
        (Some(list), true) => {
            spans.push(Span::styled("› ", Style::default().fg(ACCENT)));
            if list.filter.is_empty() {
                spans.push(Span::styled(
                    "type to search",
                    Style::default().fg(MUTED).add_modifier(Modifier::ITALIC),
                ));
            } else {
                spans.push(Span::styled(list.filter.clone(), value_style(true)));
            }
        }
        _ => {
            let (value, detail, missing) = setting_value(form, field, new_project);
            // The harness and model are named in their vendor's color, as
            // the dashboard's rows name them.
            let color = match field {
                NewField::Harness | NewField::Model => model_color(&value),
                _ => TEXT,
            };
            spans.push(Span::styled(
                value,
                if missing {
                    bold(WARNING)
                } else {
                    value_style_in(color, focused)
                },
            ));
            spans.push(Span::styled(
                " ▾",
                Style::default().fg(if focused { ACCENT } else { MUTED }),
            ));
            if !detail.is_empty() {
                spans.push(Span::styled(
                    format!("  {detail}"),
                    Style::default().fg(MUTED),
                ));
            }
        }
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(row_style(focused)),
        area,
    );
}

/// What a setting row shows: its value, a detail after it, and whether the
/// value is still missing.
fn setting_value(
    form: &NewAgentForm,
    field: NewField,
    new_project: bool,
) -> (String, String, bool) {
    match field {
        NewField::Harness => {
            let missing = form.kind.trim().is_empty();
            (
                if missing {
                    "Choose a harness".into()
                } else {
                    form.kind.clone()
                },
                String::new(),
                missing,
            )
        }
        NewField::Model => (form.model_label(), String::new(), false),
        NewField::Effort => (form.effort_label(), String::new(), false),
        NewField::Project => {
            let project = form.project.trim();
            if project.is_empty() {
                ("Choose a project".into(), String::new(), true)
            } else {
                let path = tilde(project);
                let detail = if new_project {
                    format!("{path} · new, created with git init")
                } else {
                    path
                };
                (
                    dir_name(project).unwrap_or(project).to_string(),
                    detail,
                    false,
                )
            }
        }
        NewField::Checkout => (
            format!("⎇ {}", form.checkout.label()),
            form.checkout.detail().to_string(),
            false,
        ),
        NewField::Task => unreachable!("the task row is drawn by draw_task_row"),
    }
}

/// The key legend for the row under the cursor. An open list explains its
/// own keys in its border, because it is usually drawn over this line.
fn new_agent_legend(form: &NewAgentForm) -> Line<'static> {
    if form.list.is_some() {
        return Line::raw("");
    }
    legend_line(legend_keys(form.field), LEGEND_SEPARATOR)
}

// The form's legend has its row to itself, so its entries get some air; an
// open list fits its keys into its bottom border and keeps them tighter.
const LEGEND_SEPARATOR: &str = "  ";
pub(super) const CHOICE_LEGEND_SEPARATOR: &str = " ";

/// The keys that work on a form row, each with what it does.
fn legend_keys(field: NewField) -> &'static [LegendKey] {
    match field {
        NewField::Task => &[
            ("↵", "create agent", SUCCESS),
            ("↓ ⇥", "settings", ACCENT),
            ("Esc", "cancel", WARNING),
        ],
        _ => &[
            ("↑ ↓ ⇥", "move", ACCENT),
            ("← →", "change", ACCENT),
            ("␣ / type", "search", ACCENT),
            ("↵", "create agent", SUCCESS),
            ("Esc", "cancel", WARNING),
        ],
    }
}

/// The panel of a submitted agent: its steps while it starts, then that it
/// runs, or why it did not.
pub(super) fn draw_new_agent_launch(
    frame: &mut Frame<'_>,
    area: Rect,
    launch: &NewAgentLaunch,
    spinner: &'static str,
) -> Drawn {
    let left = |lines: Vec<Line<'static>>| {
        lines
            .into_iter()
            .map(|line| line.alignment(Alignment::Left))
    };
    let mut lines = Vec::new();
    let content = match &launch.state {
        LaunchState::Running { status, step } => {
            lines.push(Line::styled(
                format!("Launching {}", launch.name),
                bold(TEXT),
            ));
            lines.push(Line::raw(""));
            if launch.steps.is_empty() {
                lines.extend(left(step_lines(std::slice::from_ref(status), 0, spinner)));
            } else {
                lines.extend(left(step_lines(&launch.steps, *step, spinner)));
                lines.push(Line::raw(""));
                lines.push(Line::styled(status.clone(), Style::default().fg(MUTED)));
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "Please wait; this window closes when the agent is ready.",
                Style::default().fg(MUTED),
            ));
            DialogContent {
                mark: POPUP_MARK,
                title: "Starting new agent",
                color: ACCENT,
                lines,
                legend: Vec::new(),
            }
        }
        LaunchState::Started { message, .. } => {
            lines.push(outcome_line(true, format!("{} is running", launch.name)));
            lines.push(Line::raw(""));
            if !launch.steps.is_empty() {
                lines.extend(left(step_lines(&launch.steps, launch.steps.len(), spinner)));
                lines.push(Line::raw(""));
            }
            lines.push(Line::styled(message.clone(), Style::default().fg(MUTED)));
            DialogContent {
                mark: SUCCESS_MARK,
                title: "New agent started",
                color: ACCENT,
                lines,
                legend: vec![("Esc", "close", WARNING)],
            }
        }
        LaunchState::Failed(error) => {
            lines.push(outcome_line(
                false,
                format!("{} did not start", launch.name),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(error.clone(), Style::default().fg(MUTED)));
            DialogContent {
                mark: FAILURE_MARK,
                title: "New agent failed",
                color: DANGER,
                lines,
                legend: vec![("Esc", "close", WARNING)],
            }
        }
    };
    let content = indent_left_block(area, content);
    draw_message_dialog(frame, area, content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{App, Checkout, HARNESS_DEFAULT_MODEL, NewAgentForm, NewAgentLaunch, Overlay},
        choices::{Choice, ChoiceList, FreeText},
        defaults::HarnessDefaults,
        model::DashboardAgent,
        projects::{Project, ProjectSource, project_choices},
        test_support::{buffer_text, test_app, test_terminal},
        ui::{choice_list::CHOICE_LIST_KEYS, clip_start, draw_overlay},
    };

    #[test]
    fn a_legend_is_measured_as_wide_as_it_is_drawn() {
        for keys in [
            legend_keys(NewField::Task),
            legend_keys(NewField::Model),
            &CHOICE_LIST_KEYS,
        ] {
            for separator in [LEGEND_SEPARATOR, CHOICE_LEGEND_SEPARATOR] {
                assert_eq!(
                    legend_width(keys, separator),
                    legend_line(keys, separator).width()
                );
            }
        }
        // Each key is in the color of what it does.
        let line = legend_line(legend_keys(NewField::Task), LEGEND_SEPARATOR);
        let color = |key: &str| {
            line.spans
                .iter()
                .find(|span| span.content.trim() == key)
                .and_then(|span| span.style.fg)
        };
        assert_eq!(color("↵"), Some(SUCCESS));
        assert_eq!(color("Esc"), Some(WARNING));
        assert_eq!(color("↓ ⇥"), Some(ACCENT));
        // And drawn on a keycap.
        assert!(
            line.spans
                .iter()
                .filter(|span| span.content.trim() == "Esc")
                .all(|span| span.style.bg == Some(super::super::KEYCAP))
        );
    }

    fn form(field: NewField, prompt: &str) -> NewAgentForm {
        NewAgentForm {
            field,
            list: None,
            error: None,
            kind: "claude".into(),
            model: String::new(),
            effort: String::new(),
            defaults: HarnessDefaults {
                model: Some("opus".into()),
                effort: Some("xhigh".into()),
            },
            project: "/repos/corgi".into(),
            new_project: false,
            checkout: Checkout::Worktree,
            prompt: prompt.into(),
            prompt_caret: 0,
            prompt_width: 0,
        }
    }

    fn render_new_agent(app: &mut App, width: u16, height: u16) -> (String, Option<(u16, u16)>) {
        let mut terminal = test_terminal(width, height);
        terminal
            .draw(|frame| draw_overlay(frame, frame.area(), app))
            .expect("draw new agent form");
        let rendered = buffer_text(terminal.backend().buffer());
        let caret = terminal
            .get_cursor_position()
            .ok()
            .map(|position| (position.x, position.y));
        (rendered, caret)
    }

    #[test]
    fn the_new_agent_form_lists_every_setting_with_its_preset_value() {
        let mut app = test_app();
        app.overlay = Overlay::NewAgent(form(NewField::Model, "Wire in model selection"));

        // Every row stays legible on a narrow pane too.
        for width in [120, 80] {
            let (rendered, caret) = render_new_agent(&mut app, width, 30);
            for text in [
                "Task",
                "Wire in model selection",
                "Harness",
                "claude",
                "Model",
                "Harness default (opus)",
                "Effort",
                "Harness default (xhigh)",
                "Project",
                "corgi",
                "/repos/corgi",
                "Checkout",
                "New Git worktree",
                "▸ Model",
                "← →  change",
                "create agent",
            ] {
                assert!(
                    rendered.contains(text),
                    "{width}: missing {text}\n{rendered}"
                );
            }
            // Claude Code takes an effort level, so its row is there.
            assert!(rendered.contains("Effort"), "{rendered}");
            // A selector has no caret: it is chosen from, not typed into, so
            // the fresh terminal's cursor is never moved off the origin.
            assert_eq!(caret, Some((0, 0)));
        }

        // Gemini has no effort control, so the row goes away.
        app.overlay.new_agent_form_mut().expect("form").kind = "gemini".into();
        let (rendered, _) = render_new_agent(&mut app, 100, 30);
        assert!(!rendered.contains("Effort"), "{rendered}");

        // An empty task shows its hint; a validation slip is shown in place.
        let mut empty = form(NewField::Task, "");
        empty.error = Some("Describe the first task before creating the agent".into());
        app.overlay = Overlay::NewAgent(empty);
        let (rendered, caret) = render_new_agent(&mut app, 100, 30);
        assert!(
            rendered.contains("What should the agent do first?"),
            "{rendered}"
        );
        assert!(rendered.contains("⚠ Describe the first task"), "{rendered}");
        assert!(caret.is_some());
    }

    #[test]
    fn an_open_project_list_hangs_below_its_row_with_paths_and_origins() {
        let mut app = test_app();
        let mut form = form(NewField::Project, "Fix the tests");
        form.list = Some(ChoiceList::new(
            project_choices(&[
                Project {
                    root: "/repos/corgi".into(),
                    name: "corgi".into(),
                    source: ProjectSource::Agents(2),
                },
                Project {
                    root: "/repos/webshop-backend".into(),
                    name: "webshop-backend".into(),
                    source: ProjectSource::Remembered,
                },
            ]),
            "/repos/webshop-backend",
            FreeText::Directory,
        ));
        app.overlay = Overlay::NewAgent(form);

        let (rendered, caret) = render_new_agent(&mut app, 100, 30);
        for text in [
            "type to search",
            "corgi",
            "/repos/corgi",
            "2 agents",
            "▸ webshop-backend",
            "recent",
            "↵  choose",
            "⇥  choose, next",
            "Esc  back",
        ] {
            assert!(rendered.contains(text), "missing {text}\n{rendered}");
        }
        assert!(caret.is_some(), "the filter shows a caret");
        assert_eq!(clip_start("/a/very/long/path", 8), "…ng/path");
        assert_eq!(clip_start("short", 8), "short");

        // A filter is shown in the row, and a filter without a match says so.
        let list = app
            .overlay
            .new_agent_form_mut()
            .and_then(|form| form.list.as_mut())
            .expect("list");
        list.filter = "nothing-here".into();
        let (rendered, _) = render_new_agent(&mut app, 100, 30);
        assert!(rendered.contains("› nothing-here"), "{rendered}");
        assert!(
            rendered.contains("No match. Type a new project name or a directory path."),
            "{rendered}"
        );
    }

    #[test]
    fn the_model_list_marks_the_highlighted_choice_and_the_default() {
        let mut app = test_app();
        let mut form = form(NewField::Model, "Fix the tests");
        form.list = Some(ChoiceList::new(
            vec![
                Choice::labelled("", HARNESS_DEFAULT_MODEL).badge("default"),
                Choice::new("gpt-5.6-sol"),
            ],
            "gpt-5.6-sol",
            FreeText::Value,
        ));
        app.overlay = Overlay::NewAgent(form);

        let (rendered, _) = render_new_agent(&mut app, 100, 20);
        for text in [
            HARNESS_DEFAULT_MODEL,
            "· default",
            "▸ gpt-5.6-sol",
            "⇥  choose, next",
        ] {
            assert!(rendered.contains(text), "missing {text}: {rendered}");
        }
    }

    #[test]
    fn the_new_agent_form_fits_a_short_terminal_by_scrolling_the_task() {
        let mut app = test_app();
        let prompt = "one two three four five six seven eight nine ten eleven twelve thirteen \
             fourteen fifteen sixteen seventeen eighteen nineteen twenty twenty-one twenty-two";
        let mut form = form(NewField::Task, prompt);
        form.prompt_caret = prompt.len();
        app.overlay = Overlay::NewAgent(form);

        let (rendered, caret) = render_new_agent(&mut app, 40, 14);
        // The settings and the legend survive; the task keeps its caret row.
        assert!(rendered.contains("Checkout"), "{rendered}");
        assert!(rendered.contains("create agent"), "{rendered}");
        assert!(rendered.contains("twenty-two"), "{rendered}");
        assert!(caret.is_some());
    }

    #[test]
    fn the_first_task_caret_is_drawn_on_the_row_it_was_moved_to() {
        let mut app = test_app();
        let prompt = "Grow the first task field so a long prompt wraps onto \
             a second row instead of being clipped at the right edge";
        let mut task = form(NewField::Task, prompt);
        task.prompt_caret = 4;
        app.overlay = Overlay::NewAgent(task);

        let mut terminal = test_terminal(120, 40);
        terminal
            .draw(|frame| draw_overlay(frame, frame.area(), &mut app))
            .expect("draw new agent form");
        let first_row = terminal.get_cursor_position().expect("caret shown");

        // The caret keeps its column while it moves down a row of its own.
        let form = app.overlay.new_agent_form_mut().expect("form");
        let rows = wrap_rows(&form.prompt, form.prompt_width);
        assert!(
            rows.len() > 1 && rows[1].end > rows[1].start + 4,
            "the task should wrap onto a second row: {rows:?}"
        );
        form.prompt_caret = rows[1].start + 4;
        terminal
            .draw(|frame| draw_overlay(frame, frame.area(), &mut app))
            .expect("draw new agent form");
        let second_row = terminal.get_cursor_position().expect("caret shown");

        assert_eq!(second_row.x, first_row.x);
        assert_eq!(second_row.y, first_row.y + 1);
    }

    #[test]
    fn the_form_says_when_it_starts_the_projects_corgi() {
        let mut app = test_app();
        app.overlay = Overlay::NewAgent(form(NewField::Task, "Plan the release"));
        let (screen, _) = render_new_agent(&mut app, 120, 30);
        assert!(screen.contains("New corgi"), "{screen}");
        assert!(
            screen.contains("No agent works in corgi yet, so this starts its corgi"),
            "{screen}"
        );

        // With an agent at work in the project, the form starts a worker.
        app.agents = vec![DashboardAgent {
            project_root: "/repos/corgi".into(),
            ..DashboardAgent::default()
        }];
        let (screen, _) = render_new_agent(&mut app, 120, 30);
        assert!(screen.contains("New agent"), "{screen}");
        assert!(!screen.contains("New corgi"), "{screen}");
        assert!(!screen.contains("starts its corgi"), "{screen}");
    }

    #[test]
    fn the_new_agent_launch_panel_lists_its_steps_with_a_spinner_on_the_current_one() {
        let mut app = test_app();
        // The spinner runs on the motion clock, three frames in.
        app.motion.set_time(std::time::Duration::from_millis(160));
        app.overlay = Overlay::Launch(NewAgentLaunch {
            name: "corgi-worker".into(),
            steps: vec![
                "Create a worktree".into(),
                "Start claude on opus".into(),
                "Send the first prompt".into(),
            ],
            state: LaunchState::Running {
                status: "Waiting for the new shell before starting corgi-worker… (2/40)".into(),
                step: 1,
            },
        });

        let mut terminal = test_terminal(100, 24);
        terminal
            .draw(|frame| draw_overlay(frame, frame.area(), &mut app))
            .expect("draw launch panel");
        let rows = crate::test_support::buffer_rows(terminal.backend().buffer());
        let rendered = rows.join("\n");

        assert!(rendered.contains("◆ Starting new agent"), "{rendered}");
        assert!(rendered.contains("Launching corgi-worker"), "{rendered}");
        assert!(rendered.contains("✓ Create a worktree"), "{rendered}");
        assert!(rendered.contains("⠹ Start claude on opus…"), "{rendered}");
        assert!(rendered.contains("· Send the first prompt"), "{rendered}");
        assert!(rendered.contains("Waiting for the new shell"), "{rendered}");
        // The steps line up as one block.
        let columns: Vec<usize> = ["✓ Create", "⠹ Start", "· Send"]
            .iter()
            .map(|step| {
                let row = rows
                    .iter()
                    .find(|row| row.contains(step))
                    .expect("step row");
                row[..row.find(step).unwrap()].chars().count()
            })
            .collect();
        assert!(
            columns.iter().all(|column| *column == columns[0]),
            "{columns:?}"
        );
    }

    #[test]
    fn a_launch_outcome_leads_with_its_mark_and_keeps_the_details_quiet() {
        let mut app = test_app();
        app.overlay = Overlay::Launch(NewAgentLaunch {
            name: "corgi-worker".into(),
            steps: vec!["Create a worktree".into(), "Start claude".into()],
            state: LaunchState::Started {
                message: "corgi-worker started in worktree calm-river-1a2b".into(),
                at: std::time::Duration::ZERO,
            },
        });
        let mut terminal = test_terminal(100, 24);
        terminal
            .draw(|frame| draw_overlay(frame, frame.area(), &mut app))
            .expect("draw started panel");
        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("✓ New agent started"), "{rendered}");
        assert!(rendered.contains("✓ corgi-worker is running"), "{rendered}");
        assert!(rendered.contains("✓ Start claude"), "{rendered}");
        assert!(
            rendered.contains("in worktree calm-river-1a2b"),
            "{rendered}"
        );

        app.overlay = Overlay::Launch(NewAgentLaunch {
            name: "corgi-worker".into(),
            steps: Vec::new(),
            state: LaunchState::Failed("git worktree add: already exists".into()),
        });
        terminal
            .draw(|frame| draw_overlay(frame, frame.area(), &mut app))
            .expect("draw failed panel");
        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("✗ New agent failed"), "{rendered}");
        assert!(
            rendered.contains("✗ corgi-worker did not start"),
            "{rendered}"
        );
        assert!(
            rendered.contains("git worktree add: already exists"),
            "{rendered}"
        );
        assert!(rendered.contains(" Esc  close"), "{rendered}");
    }
}
