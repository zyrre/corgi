//! The new-agent form, as the dashboard's: every value preset by `corgi
//! serve`, one set of keys on every row (`↑`/`↓` and `Tab` move, `←`/`→`
//! change in place, `Space` or typing opens the row's list inline, filtered
//! by what is typed, `Enter` creates, `Esc` cancels). The task's text lives
//! in the window's text field; everything else is here, without a window.

use corgi_core::protocol::{Choice, NewAgentForm, SpawnRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Task,
    Harness,
    Model,
    Effort,
    Project,
    Checkout,
}

impl Row {
    pub fn label(self) -> &'static str {
        match self {
            Row::Task => "Task",
            Row::Harness => "Harness",
            Row::Model => "Model",
            Row::Effort => "Effort",
            Row::Project => "Project",
            Row::Checkout => "Checkout",
        }
    }

    /// The row a refused `spawn`'s `field` points at.
    pub fn from_field(field: &str) -> Option<Row> {
        match field {
            "task" => Some(Row::Task),
            "harness" => Some(Row::Harness),
            "project" => Some(Row::Project),
            _ => None,
        }
    }
}

/// A key, as the form reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Tab,
    BackTab,
    Left,
    Right,
    Space,
    Char(char),
    Backspace,
    Enter,
    Escape,
}

/// What the window does after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Nothing beyond drawing again.
    None,
    /// The key was not the form's: the text field takes it.
    Unhandled,
    /// Put the keyboard in the task's text field, or take it out.
    FocusTask,
    BlurTask,
    Submit,
    Cancel,
    /// Ask `corgi serve` for the form's lists on this harness and model.
    Reload {
        harness: String,
        model: String,
    },
}

/// An open list: the filter typed over it and the row under the cursor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Picker {
    pub filter: String,
    pub cursor: usize,
}

/// The form while it is open.
#[derive(Debug, Clone, PartialEq)]
pub struct NewAgent {
    /// The lists and presets `corgi serve` sent.
    pub form: NewAgentForm,
    pub harness: String,
    pub model: String,
    pub effort: String,
    pub project: String,
    pub new_project: bool,
    pub checkout: String,
    pub row: Row,
    pub picker: Option<Picker>,
    /// Why the last `Enter` did not create the agent.
    pub error: Option<String>,
    /// The launch's steps while it runs.
    pub running: Option<crate::dialogs::Steps>,
}

impl NewAgent {
    pub fn new(form: NewAgentForm) -> Self {
        Self {
            harness: form.harness.clone(),
            model: form.model.clone(),
            effort: form.effort.clone(),
            project: form.project.clone(),
            new_project: false,
            checkout: form.checkout.clone(),
            form,
            row: Row::Task,
            picker: None,
            error: None,
            running: None,
        }
    }

    /// New lists for a harness or model the user chose; their own choices
    /// of project, checkout and effort stay where the lists still have them.
    pub fn reloaded(&mut self, form: NewAgentForm) {
        self.harness = form.harness.clone();
        self.model = form.model.clone();
        if !form
            .efforts
            .iter()
            .any(|choice| choice.value == self.effort)
        {
            self.effort = form.effort.clone();
        }
        self.form = form;
    }

    /// The rows shown: no effort for a harness without one, no checkout for
    /// a project that does not exist yet.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Task, Row::Harness, Row::Model];
        if self.form.supports_effort {
            rows.push(Row::Effort);
        }
        rows.push(Row::Project);
        if !self.new_project {
            rows.push(Row::Checkout);
        }
        rows
    }

    pub fn title(&self) -> &'static str {
        if self.form.starts_supervisor {
            "New project: its supervisor"
        } else {
            "New agent"
        }
    }

    /// The row's value as the form shows it.
    pub fn shown(&self, row: Row) -> String {
        let (value, choices) = match row {
            Row::Task => return String::new(),
            Row::Harness => (&self.harness, &self.form.harnesses),
            Row::Model => (&self.model, &self.form.models),
            Row::Effort => (&self.effort, &self.form.efforts),
            Row::Project => (&self.project, &self.form.projects),
            Row::Checkout => (&self.checkout, &self.form.checkouts),
        };
        choices
            .iter()
            .find(|choice| &choice.value == value)
            .map(|choice| choice.label.clone())
            .unwrap_or_else(|| value.clone())
    }

    /// The detail shown after the row's value: the project's path, what a
    /// checkout means.
    pub fn detail(&self, row: Row) -> String {
        match row {
            Row::Project if self.new_project => format!("new project at {}", self.project),
            Row::Project => self.project.clone(),
            Row::Checkout => self
                .form
                .checkouts
                .iter()
                .find(|choice| choice.value == self.checkout)
                .map(|choice| choice.detail.clone())
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn choices(&self, row: Row) -> &[Choice] {
        match row {
            Row::Task => &[],
            Row::Harness => &self.form.harnesses,
            Row::Model => &self.form.models,
            Row::Effort => &self.form.efforts,
            Row::Project => &self.form.projects,
            Row::Checkout => &self.form.checkouts,
        }
    }

    /// The open list's rows: every choice whose label, value or detail holds
    /// each word typed, then, for a harness or model, the typed text as a
    /// value of its own, and for a project a new one by that name.
    pub fn picker_rows(&self) -> Vec<Choice> {
        let Some(picker) = &self.picker else {
            return Vec::new();
        };
        let words: Vec<String> = picker
            .filter
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let choices = self.choices(self.row);
        let mut rows: Vec<Choice> = choices
            .iter()
            .filter(|choice| {
                let haystack =
                    format!("{} {} {}", choice.label, choice.value, choice.detail).to_lowercase();
                words.iter().all(|word| haystack.contains(word))
            })
            .cloned()
            .collect();
        let typed = picker.filter.trim();
        let listed = |text: &str| {
            choices.iter().any(|choice| {
                choice.value.eq_ignore_ascii_case(text) || choice.label.eq_ignore_ascii_case(text)
            })
        };
        if typed.is_empty() || typed.contains(char::is_whitespace) || listed(typed) {
            return rows;
        }
        match self.row {
            Row::Harness | Row::Model => rows.push(Choice {
                value: typed.into(),
                label: typed.into(),
                detail: String::new(),
                badge: "as typed".into(),
            }),
            Row::Project => rows.extend(self.typed_project(typed)),
            _ => {}
        }
        rows
    }

    /// A project typed by hand: an absolute path, used as it is (the server
    /// checks it exists), or a bare name, a new project under the parent.
    fn typed_project(&self, typed: &str) -> Option<Choice> {
        if typed.starts_with('/') || typed.starts_with("~/") {
            return Some(Choice {
                value: typed.trim_end_matches('/').into(),
                label: typed.into(),
                detail: String::new(),
                badge: "typed path".into(),
            });
        }
        let parent = self.form.new_project_parent.as_deref()?;
        if typed.contains('/') || typed == "." || typed == ".." {
            return None;
        }
        let path = format!("{}/{typed}", parent.trim_end_matches('/'));
        Some(Choice {
            value: path.clone(),
            label: typed.into(),
            detail: path,
            badge: "new project".into(),
        })
    }

    /// Handles `key`.
    pub fn key(&mut self, key: Key) -> Effect {
        if self.running.is_some() {
            return Effect::None;
        }
        if self.picker.is_some() {
            return self.picker_key(key);
        }
        match key {
            Key::Enter => Effect::Submit,
            Key::Escape => Effect::Cancel,
            Key::Down | Key::Tab => self.step(1, key == Key::Tab),
            Key::Up | Key::BackTab => self.step(-1, key == Key::BackTab),
            _ if self.row == Row::Task => Effect::Unhandled,
            Key::Left => self.cycle(-1),
            Key::Right => self.cycle(1),
            Key::Space => {
                self.open_picker(String::new());
                Effect::None
            }
            Key::Char(c) => {
                self.open_picker(c.to_string());
                Effect::None
            }
            Key::Backspace => Effect::None,
        }
    }

    /// Moves `delta` rows; `Tab` wraps, the arrows stop at the ends.
    fn step(&mut self, delta: isize, wrap: bool) -> Effect {
        let rows = self.rows();
        let at = rows.iter().position(|row| *row == self.row).unwrap_or(0) as isize;
        let count = rows.len() as isize;
        let next = if wrap {
            (at + delta).rem_euclid(count)
        } else {
            (at + delta).clamp(0, count - 1)
        };
        let was_task = self.row == Row::Task;
        self.row = rows[next as usize];
        match (was_task, self.row == Row::Task) {
            (true, false) => Effect::BlurTask,
            (false, true) => Effect::FocusTask,
            _ => Effect::None,
        }
    }

    /// The next or previous value of the row's list, in place.
    fn cycle(&mut self, delta: isize) -> Effect {
        let choices = self.choices(self.row).to_vec();
        if choices.is_empty() {
            return Effect::None;
        }
        let current = self.value(self.row);
        let at = choices
            .iter()
            .position(|choice| choice.value == current)
            .map_or(if delta > 0 { -1 } else { 0 }, |at| at as isize);
        let next = (at + delta).rem_euclid(choices.len() as isize) as usize;
        self.choose(choices[next].clone())
    }

    fn value(&self, row: Row) -> String {
        match row {
            Row::Task => String::new(),
            Row::Harness => self.harness.clone(),
            Row::Model => self.model.clone(),
            Row::Effort => self.effort.clone(),
            Row::Project => self.project.clone(),
            Row::Checkout => self.checkout.clone(),
        }
    }

    fn open_picker(&mut self, filter: String) {
        let current = self.value(self.row);
        let cursor = if filter.is_empty() {
            self.choices(self.row)
                .iter()
                .position(|choice| choice.value == current)
                .unwrap_or(0)
        } else {
            0
        };
        self.picker = Some(Picker { filter, cursor });
    }

    fn picker_key(&mut self, key: Key) -> Effect {
        let count = self.picker_rows().len();
        let picker = self.picker.as_mut().expect("an open list");
        match key {
            Key::Escape => {
                self.picker = None;
                Effect::None
            }
            Key::Char(c) => {
                picker.filter.push(c);
                picker.cursor = 0;
                Effect::None
            }
            Key::Space => {
                picker.filter.push(' ');
                picker.cursor = 0;
                Effect::None
            }
            Key::Backspace => {
                picker.filter.pop();
                picker.cursor = 0;
                Effect::None
            }
            Key::Up | Key::Down => {
                if count > 0 {
                    let delta = if key == Key::Up { -1 } else { 1 };
                    let cursor = picker.cursor.min(count - 1) as isize;
                    picker.cursor = (cursor + delta).rem_euclid(count as isize) as usize;
                }
                Effect::None
            }
            Key::Enter | Key::Tab => {
                let cursor = picker.cursor;
                let chosen = self.picker_rows().into_iter().nth(cursor);
                self.picker = None;
                let effect = match chosen {
                    Some(choice) => self.choose(choice),
                    None => Effect::None,
                };
                if key == Key::Tab && effect == Effect::None {
                    return self.step(1, true);
                }
                effect
            }
            Key::Left | Key::Right | Key::BackTab => Effect::None,
        }
    }

    /// Sets the row to `choice`; a new harness or model asks for new lists.
    fn choose(&mut self, choice: Choice) -> Effect {
        match self.row {
            Row::Task => Effect::None,
            Row::Harness => {
                if choice.value == self.harness {
                    return Effect::None;
                }
                self.harness = choice.value;
                // A model of another harness would not run: the new
                // harness's lists start from its default.
                self.model.clear();
                Effect::Reload {
                    harness: self.harness.clone(),
                    model: String::new(),
                }
            }
            Row::Model => {
                if choice.value == self.model {
                    return Effect::None;
                }
                self.model = choice.value;
                Effect::Reload {
                    harness: self.harness.clone(),
                    model: self.model.clone(),
                }
            }
            Row::Effort => {
                self.effort = choice.value;
                Effect::None
            }
            Row::Project => {
                self.new_project = choice.badge == "new project";
                self.project = choice.value;
                Effect::None
            }
            Row::Checkout => {
                self.checkout = choice.value;
                Effect::None
            }
        }
    }

    /// The `spawn` request for the form as filled in, with `task`.
    pub fn spawn(&self, task: &str) -> SpawnRequest {
        SpawnRequest {
            project: self.project.clone(),
            new_project: self.new_project,
            harness: self.harness.clone(),
            model: self.model.clone(),
            effort: if self.form.supports_effort {
                self.effort.clone()
            } else {
                String::new()
            },
            task: task.trim().to_string(),
            checkout: if self.new_project {
                String::new()
            } else {
                self.checkout.clone()
            },
        }
    }

    /// A refused `spawn`: say why, and go to the row it points at.
    pub fn refused(&mut self, error: String, field: Option<&str>) -> Effect {
        self.running = None;
        self.error = Some(error);
        let Some(row) = field.and_then(Row::from_field) else {
            return Effect::None;
        };
        let was_task = self.row == Row::Task;
        self.row = row;
        match (was_task, row == Row::Task) {
            (false, true) => Effect::FocusTask,
            (true, false) => Effect::BlurTask,
            _ => Effect::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(value: &str, label: &str) -> Choice {
        Choice {
            value: value.into(),
            label: label.into(),
            detail: String::new(),
            badge: String::new(),
        }
    }

    fn form() -> NewAgentForm {
        NewAgentForm {
            project: "/repos/weather".into(),
            projects: vec![
                choice("/repos/weather", "weather"),
                choice("/repos/webshop", "webshop"),
            ],
            new_project_parent: Some("/repos".into()),
            harness: "claude".into(),
            harnesses: vec![choice("codex", "codex"), choice("claude", "claude")],
            model: String::new(),
            models: vec![
                choice("", "Harness default (opus)"),
                choice("sonnet", "sonnet"),
            ],
            effort: String::new(),
            efforts: vec![choice("", "Harness default (high)"), choice("low", "low")],
            supports_effort: true,
            checkout: "worktree".into(),
            checkouts: vec![
                choice("worktree", "New Git worktree"),
                choice("directory", "Project directory as is"),
            ],
            default_model: Some("opus".into()),
            default_effort: Some("high".into()),
            starts_supervisor: false,
        }
    }

    #[test]
    fn the_common_path_is_type_the_task_and_press_enter() {
        let mut new = NewAgent::new(form());
        assert_eq!(new.row, Row::Task);
        // Letters belong to the task's text field.
        assert_eq!(new.key(Key::Char('f')), Effect::Unhandled);
        assert_eq!(new.key(Key::Enter), Effect::Submit);
        let spawn = new.spawn("  Fix leap days ");
        assert_eq!(spawn.task, "Fix leap days");
        assert_eq!(spawn.project, "/repos/weather");
        assert_eq!(
            (spawn.harness.as_str(), spawn.checkout.as_str()),
            ("claude", "worktree")
        );
        assert_eq!(new.key(Key::Escape), Effect::Cancel);
    }

    #[test]
    fn arrows_move_between_rows_and_change_values_in_place() {
        let mut new = NewAgent::new(form());
        assert_eq!(new.key(Key::Down), Effect::BlurTask);
        assert_eq!(new.row, Row::Harness);
        assert_eq!(
            new.key(Key::Right),
            Effect::Reload {
                harness: "codex".into(),
                model: String::new()
            }
        );
        assert_eq!(new.harness, "codex");
        new.key(Key::Down);
        new.key(Key::Down);
        assert_eq!(new.row, Row::Effort);
        assert_eq!(new.key(Key::Right), Effect::None);
        assert_eq!(new.effort, "low");
        assert_eq!(new.shown(Row::Effort), "low");
        // The arrows stop at the last row, Tab wraps to the task.
        new.key(Key::Down);
        new.key(Key::Down);
        assert_eq!(new.row, Row::Checkout);
        new.key(Key::Down);
        assert_eq!(new.row, Row::Checkout);
        assert_eq!(new.key(Key::Tab), Effect::FocusTask);
        assert_eq!(new.row, Row::Task);
        assert_eq!(new.key(Key::BackTab), Effect::BlurTask);
        assert_eq!(new.row, Row::Checkout);
    }

    #[test]
    fn typing_opens_the_list_filtered_and_offers_what_was_typed() {
        let mut new = NewAgent::new(form());
        new.key(Key::Down);
        new.key(Key::Down);
        assert_eq!(new.row, Row::Model);
        new.key(Key::Char('s'));
        new.key(Key::Char('o'));
        let rows: Vec<String> = new.picker_rows().into_iter().map(|c| c.value).collect();
        assert_eq!(rows, ["sonnet", "so"]);
        assert_eq!(
            new.key(Key::Enter),
            Effect::Reload {
                harness: "claude".into(),
                model: "sonnet".into()
            }
        );
        assert!(new.picker.is_none());
        // Esc closes a list unchanged.
        new.key(Key::Space);
        new.key(Key::Down);
        new.key(Key::Escape);
        assert_eq!(new.model, "sonnet");
        assert!(new.picker.is_none());
    }

    #[test]
    fn a_new_project_is_typed_by_name_and_drops_the_checkout_row() {
        let mut new = NewAgent::new(form());
        while new.row != Row::Project {
            new.key(Key::Down);
        }
        for c in "forecast".chars() {
            new.key(Key::Char(c));
        }
        let rows = new.picker_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].value, "/repos/forecast");
        new.key(Key::Enter);
        assert!(new.new_project);
        assert!(!new.rows().contains(&Row::Checkout));
        let spawn = new.spawn("Start it");
        assert!(spawn.new_project);
        assert_eq!(spawn.checkout, "");
        assert_eq!(new.detail(Row::Project), "new project at /repos/forecast");
        // Choosing a listed project again brings the checkout back.
        new.key(Key::Right);
        assert!(!new.new_project);
        assert!(new.rows().contains(&Row::Checkout));
    }

    #[test]
    fn a_refusal_says_why_and_goes_to_its_row_and_a_reload_keeps_choices() {
        let mut new = NewAgent::new(form());
        new.key(Key::Down);
        assert_eq!(
            new.refused("Describe the first task".into(), Some("task")),
            Effect::FocusTask
        );
        assert_eq!(new.row, Row::Task);
        assert_eq!(new.error.as_deref(), Some("Describe the first task"));
        new.project = "/repos/webshop".into();
        new.effort = "low".into();
        let mut codex = form();
        codex.harness = "codex".into();
        codex.supports_effort = false;
        codex.efforts.clear();
        new.reloaded(codex);
        assert_eq!(new.project, "/repos/webshop");
        assert_eq!(new.effort, "");
        assert!(!new.rows().contains(&Row::Effort));
        assert_eq!(new.spawn("t").effort, "");
    }
}
