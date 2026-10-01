//! The new-agent form: its rows, the values they hold, and its keys. What the
//! filled-in form launches is a [`LaunchPlan`], which `launch.rs` runs.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    choices::{Choice, ChoiceList, FreeText},
    defaults::HarnessDefaults,
    harness::Harness,
    model::DashboardAgent,
    paths::{expand_home, home, is_home},
    projects::{known_projects, new_project_parent, project_choices},
    textfield::{caret_in_row, caret_left, caret_right, caret_row_column, wrap_rows},
};

use super::{
    App,
    catalog::{
        EFFORT_LEVELS, HARNESS_DEFAULT_EFFORT, HARNESS_DEFAULT_MODEL, cycle_agent_kind,
        cycle_models, default_harness, default_label, effort_choices, harness_choices,
        model_choices, supported_model,
    },
    launch::{
        LaunchPlan, Role, handler_harness_error, handler_plan, starts_handler, unique_agent_name,
    },
    overlay::Overlay,
};

/// The rows of the new-agent form, top to bottom. The task is typed; every
/// other row is a selector whose list opens in place. The session name is
/// always derived from the project, so it has no row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NewField {
    Task,
    Harness,
    Model,
    /// Codex and Claude Code only; the row is left out for harnesses Corgi
    /// cannot hand an effort level to.
    Effort,
    Project,
    Checkout,
}

impl NewField {
    /// The label drawn in front of the row.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Task => "Task",
            Self::Harness => "Harness",
            Self::Model => "Model",
            Self::Effort => "Effort",
            Self::Project => "Project",
            Self::Checkout => "Checkout",
        }
    }

    /// Whether the row chooses from a list rather than taking typed text.
    fn is_selector(self) -> bool {
        self != Self::Task
    }
}

/// Where a new agent's workspace is rooted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Checkout {
    /// A fresh Git worktree of the project's repository, so every agent edits
    /// its own checkout. Falls back to the directory itself outside Git.
    #[default]
    Worktree,
    /// The project directory as it is, shared with whatever already runs there.
    Directory,
    /// The root tab of the project's Corgi workspace itself, in the primary
    /// checkout. That tab is where a coordinating agent such as the Project handler lives,
    /// so only `corgi spawn` offers it; the form's list does not.
    ProjectRoot,
}

impl Checkout {
    fn toggle(self) -> Self {
        match self {
            Self::Worktree => Self::Directory,
            Self::Directory | Self::ProjectRoot => Self::Worktree,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Worktree => "New Git worktree",
            Self::Directory => "Project directory as is",
            Self::ProjectRoot => "Project root tab",
        }
    }

    /// What the choice means, shown after the label in a quieter colour.
    pub(crate) fn detail(self) -> &'static str {
        match self {
            Self::Worktree => "own branch and checkout",
            Self::Directory => "shared checkout, own tab",
            Self::ProjectRoot => "primary checkout, the project's own tab",
        }
    }

    /// The token the checkout selector stores for this choice.
    pub(super) fn value(self) -> &'static str {
        match self {
            Self::Worktree => "worktree",
            Self::Directory => "directory",
            Self::ProjectRoot => "root",
        }
    }

    pub(super) fn from_value(value: &str) -> Option<Self> {
        [Self::Worktree, Self::Directory, Self::ProjectRoot]
            .into_iter()
            .find(|checkout| checkout.value().eq_ignore_ascii_case(value.trim()))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NewAgentForm {
    pub(crate) field: NewField,
    /// The list open under the selected row, while one is.
    pub(crate) list: Option<ChoiceList>,
    /// Why the last Enter did not create the agent, shown in the form until
    /// the next key so the typed task is never lost to a validation slip.
    pub(crate) error: Option<String>,
    /// The harness CLI, as Herdr names agent kinds.
    pub(crate) kind: String,
    /// Model the harness is started on; empty means the harness default.
    pub(crate) model: String,
    /// Codex reasoning level; empty means the selected model's default.
    pub(crate) effort: String,
    /// What the harness's own configuration names for the rows left on
    /// "Harness default", shown in parentheses behind that label.
    pub(crate) defaults: HarnessDefaults,
    pub(crate) project: String,
    /// Set when the project was chosen as a new one from its list, so only a
    /// deliberate choice creates a directory, never a stale preset.
    pub(crate) new_project: bool,
    pub(crate) checkout: Checkout,
    pub(crate) prompt: String,
    /// Where the caret sits in `prompt`, in bytes, so the first task can be
    /// edited anywhere in the text rather than only at its end.
    pub(crate) prompt_caret: usize,
    /// Display width the first task last wrapped at. The renderer records it
    /// because only the drawn popup knows it, and vertical caret moves have
    /// to follow the rows the reader sees.
    pub(crate) prompt_width: usize,
}

impl NewAgentForm {
    /// The rows shown for the selected harness: Effort only for the CLIs
    /// Corgi can hand a reasoning level to, and Checkout only for a project
    /// that exists, since a new one has no commits to branch a worktree from.
    fn fields(&self) -> Vec<NewField> {
        self.fields_for(self.is_new_project())
    }

    /// [`Self::fields`] for a caller that already knows whether the project
    /// is new, so a frame asks the filesystem once rather than per row.
    pub(crate) fn fields_for(&self, new_project: bool) -> Vec<NewField> {
        let mut fields = vec![NewField::Task, NewField::Harness, NewField::Model];
        if self.supports_effort() {
            fields.push(NewField::Effort);
        }
        fields.push(NewField::Project);
        if !new_project {
            fields.push(NewField::Checkout);
        }
        fields
    }

    /// Whether the project is a directory that does not exist yet and is
    /// created, as a fresh Git repository, when the agent starts. This asks
    /// the filesystem, so a renderer calls it once per frame and passes the
    /// answer on.
    pub(crate) fn is_new_project(&self) -> bool {
        let project = Path::new(self.project.trim());
        self.new_project && project.is_absolute() && !project.exists()
    }

    /// Whether the form starts its project's handler rather than a worker,
    /// given whether the project is new, as
    /// [`App::new_agent_starts_handler`] decides it.
    pub(crate) fn starts_handler_among(
        &self,
        new_project: bool,
        agents: &[DashboardAgent],
    ) -> bool {
        starts_handler(new_project, self.project.trim(), agents)
    }

    /// Whether the project is the home directory, which is never a project:
    /// the agent is a scratch session in a plain workspace of its own, and
    /// may start without a task.
    pub(crate) fn is_scratch(&self) -> bool {
        is_home(self.project.trim())
    }

    /// The harness the Harness row names.
    pub(crate) fn harness(&self) -> Harness {
        Harness::from_typed(&self.kind)
    }

    pub(super) fn supports_effort(&self) -> bool {
        self.harness().supports_effort()
    }

    /// Moves the cursor `delta` rows, wrapping at both ends as Tab does.
    fn move_field(&mut self, delta: isize) {
        let fields = self.fields();
        let index = fields
            .iter()
            .position(|field| *field == self.field)
            .unwrap_or(0) as isize;
        self.field = fields[(index + delta).rem_euclid(fields.len() as isize) as usize];
    }

    /// Moves the cursor `delta` rows, stopping at the ends as arrows do.
    fn step_field(&mut self, delta: isize) {
        let fields = self.fields();
        let index = fields
            .iter()
            .position(|field| *field == self.field)
            .unwrap_or(0) as isize;
        let target = (index + delta).clamp(0, fields.len() as isize - 1);
        self.field = fields[target as usize];
    }

    /// Puts the cursor back on a row that exists, after a harness change
    /// removed the row it was on.
    fn settle_field(&mut self) {
        if !self.fields().contains(&self.field) {
            self.field = NewField::Harness;
        }
    }

    /// The row the task caret is on and how many rows the task wraps to.
    fn caret_rows(&self) -> (usize, usize) {
        let rows = wrap_rows(&self.prompt, self.prompt_width);
        let (row, _) = caret_row_column(&self.prompt, &rows, self.prompt_caret);
        (row, rows.len())
    }

    /// Stores a chosen or typed value in a selector row.
    pub(super) fn apply(&mut self, field: NewField, value: &str) {
        match field {
            NewField::Harness => self.kind = value.trim().to_lowercase(),
            NewField::Model => self.model = value.trim().to_string(),
            NewField::Effort => self.effort = value.trim().to_string(),
            NewField::Project => {
                self.project = expand_home(value.trim());
                // Only a new-project row names a directory that is missing.
                self.new_project = !Path::new(&self.project).exists();
            }
            NewField::Checkout => {
                if let Some(checkout) = Checkout::from_value(value) {
                    self.checkout = checkout;
                }
            }
            NewField::Task => {}
        }
    }

    /// Steps the selector under the cursor to its neighbouring value, so a
    /// setting can be changed without opening its list.
    fn cycle(&mut self, delta: isize, model_options: &[String]) {
        match self.field {
            NewField::Harness => self.kind = cycle_agent_kind(&self.kind, delta),
            NewField::Model => self.model = cycle_models(model_options, &self.model, delta),
            NewField::Effort => {
                let efforts: Vec<String> = EFFORT_LEVELS
                    .iter()
                    .map(|effort| (*effort).to_string())
                    .collect();
                self.effort = cycle_models(&efforts, &self.effort, delta);
            }
            NewField::Checkout => self.checkout = self.checkout.toggle(),
            // Projects have no natural order to step through; their list
            // opens instead.
            NewField::Project | NewField::Task => {}
        }
    }

    /// The model as the form shows it: the placeholder, with the configured
    /// model in parentheses when known, while the harness default is in
    /// effect.
    pub(crate) fn model_label(&self) -> String {
        if self.model.is_empty() {
            default_label(HARNESS_DEFAULT_MODEL, self.defaults.model.as_deref())
        } else {
            self.model.clone()
        }
    }

    /// The effort as the form shows it: the placeholder, with the
    /// configured level in parentheses when known, while the default is in
    /// effect.
    pub(crate) fn effort_label(&self) -> String {
        if self.effort.is_empty() {
            default_label(HARNESS_DEFAULT_EFFORT, self.defaults.effort.as_deref())
        } else {
            self.effort.clone()
        }
    }

    /// Inserts a typed character where the caret is and steps over it.
    fn insert_at_caret(&mut self, character: char) {
        self.prompt.insert(self.prompt_caret, character);
        self.prompt_caret += character.len_utf8();
    }

    /// Deletes the character before the caret, as Backspace does.
    fn delete_before_caret(&mut self) {
        let target = caret_left(&self.prompt, self.prompt_caret);
        self.prompt.replace_range(target..self.prompt_caret, "");
        self.prompt_caret = target;
    }

    /// Deletes the character the caret sits on, leaving the caret in place.
    fn delete_at_caret(&mut self) {
        let target = caret_right(&self.prompt, self.prompt_caret);
        self.prompt.replace_range(self.prompt_caret..target, "");
    }

    /// Walks the caret through the first task along the rows it is drawn on:
    /// sideways by a character, up and down by a row keeping the column, and
    /// Home and End to the ends of the caret's own row.
    fn move_caret(&mut self, code: KeyCode) {
        let rows = wrap_rows(&self.prompt, self.prompt_width);
        let (row, column) = caret_row_column(&self.prompt, &rows, self.prompt_caret);
        self.prompt_caret = match code {
            KeyCode::Left => caret_left(&self.prompt, self.prompt_caret),
            KeyCode::Right => caret_right(&self.prompt, self.prompt_caret),
            KeyCode::Up if row > 0 => caret_in_row(&self.prompt, &rows, row - 1, column),
            KeyCode::Down if row + 1 < rows.len() => {
                caret_in_row(&self.prompt, &rows, row + 1, column)
            }
            KeyCode::Home => rows[row].start,
            KeyCode::End => caret_in_row(&self.prompt, &rows, row, usize::MAX),
            // Up on the first row and Down on the last stay put; the fields
            // around the first task are reached with Tab.
            _ => self.prompt_caret,
        };
    }

    /// Drops a model the newly chosen harness does not offer, so switching
    /// harness never launches one CLI on another's model.
    fn forget_foreign_model(&mut self, models: &[String]) {
        self.model = supported_model(models, &self.model);
    }
}

impl App {
    /// Whether the new-agent form starts its project's handler rather than a
    /// worker: for a new project, and for a project with no agent session in
    /// any of its workspaces.
    fn new_agent_starts_handler(&self) -> bool {
        self.overlay.new_agent_form().is_some_and(|form| {
            starts_handler(form.is_new_project(), form.project.trim(), &self.agents)
        })
    }

    /// Opens the new-agent form in its task row with everything else preset:
    /// the selected agent's repository (its primary checkout when it works in
    /// a worktree, so the new agent gets a sibling worktree rather than a
    /// worktree of a worktree), otherwise the project used most recently.
    /// Only when Corgi knows no project at all does the project list open
    /// first.
    pub(super) fn begin_new_agent(&mut self) {
        let project = self.default_project();
        self.begin_new_agent_in(project);
    }

    /// The scratch key: the new-agent form for the home directory, in its
    /// task row with every other value preset as for any new agent.
    pub(super) fn begin_scratch_agent(&mut self) {
        match home() {
            Some(home) => self.begin_new_agent_in(home.to_string_lossy().into_owned()),
            None => self.set_status(
                "HOME is not set, so there is no home directory for a scratch agent",
                Some(Duration::from_secs(5)),
            ),
        }
    }

    fn default_project(&self) -> String {
        // A scratch agent's home directory is no project to add agents to.
        if let Some(root) = self
            .selected_agent()
            .filter(|agent| !agent.scratch)
            .map(|agent| agent.project_root.clone())
            .filter(|path| !path.is_empty())
        {
            return root;
        }
        self.projects
            .roots()
            .iter()
            .find(|root| Path::new(root).is_dir())
            .cloned()
            .or_else(|| {
                known_projects(&self.agents, &self.workspaces, &self.projects)
                    .first()
                    .map(|project| project.root.clone())
            })
            .unwrap_or_default()
    }

    /// Opens the new-agent form with `project` as its repository.
    pub(super) fn begin_new_agent_in(&mut self, project: String) {
        let harness = default_harness();
        let field = if project.trim().is_empty() {
            NewField::Project
        } else {
            NewField::Task
        };
        self.overlay = Overlay::NewAgent(NewAgentForm {
            field,
            list: None,
            error: None,
            kind: harness.kind().to_string(),
            model: String::new(),
            effort: String::new(),
            defaults: HarnessDefaults::default(),
            project,
            new_project: false,
            checkout: Checkout::default(),
            prompt: String::new(),
            prompt_caret: 0,
            prompt_width: 0,
        });
        self.request_model_catalog(&harness);
        self.refresh_form_defaults();
        if field == NewField::Project {
            self.open_list(None);
        }
    }

    /// Brings the form's "Harness default" hints in line with its harness
    /// and model, reading the harness's configuration the first time.
    pub(super) fn refresh_form_defaults(&mut self) {
        let Some((harness, model)) = self
            .overlay
            .new_agent_form()
            .map(|form| (form.harness(), form.model.clone()))
        else {
            return;
        };
        let defaults = self.model_catalogs.defaults(&harness, &model);
        if let Some(form) = self.overlay.new_agent_form_mut() {
            form.defaults = defaults;
        }
    }

    /// Opens the list of the selector under the cursor, optionally with a
    /// first filter character already typed.
    fn open_list(&mut self, seed: Option<char>) {
        let Some(form) = self.overlay.new_agent_form() else {
            return;
        };
        let (choices, current, free_text) = match form.field {
            NewField::Harness => (
                harness_choices(&Harness::installed(), &form.kind),
                form.kind.clone(),
                FreeText::Value,
            ),
            NewField::Model => (
                model_choices(
                    &self.model_options(&form.harness()),
                    &form.model,
                    form.defaults.model.as_deref(),
                ),
                form.model.clone(),
                FreeText::Value,
            ),
            NewField::Effort => (
                effort_choices(form.defaults.effort.as_deref()),
                form.effort.clone(),
                FreeText::None,
            ),
            NewField::Project => {
                let projects = known_projects(&self.agents, &self.workspaces, &self.projects);
                let parent = new_project_parent(&projects);
                let mut choices = project_choices(&projects);
                let current = form.project.trim();
                if !current.is_empty() && choices.iter().all(|choice| choice.value != current) {
                    choices.push(Choice::directory(current).badge(if form.is_new_project() {
                        "new project"
                    } else {
                        "current"
                    }));
                }
                let mut list = ChoiceList::new(choices, &form.project, FreeText::Directory);
                if let Some(parent) = parent {
                    list = list.with_new_project_parent(parent);
                }
                if let Some(character) = seed {
                    list.push_filter(character);
                }
                if let Some(form) = self.overlay.new_agent_form_mut() {
                    form.list = Some(list);
                }
                return;
            }
            NewField::Checkout => (
                checkout_choices(),
                form.checkout.value().to_string(),
                FreeText::None,
            ),
            NewField::Task => return,
        };
        let mut list = ChoiceList::new(choices, &current, free_text);
        if let Some(character) = seed {
            list.push_filter(character);
        }
        if let Some(form) = self.overlay.new_agent_form_mut() {
            form.list = Some(list);
        }
    }

    /// Stores the highlighted row of the open list in its selector, closes
    /// the list, and moves the cursor `step` rows (Tab) or leaves it (Enter).
    /// A filter that leaves no row keeps the list open and says so.
    fn choose_from_list(&mut self, step: isize) {
        let Some(form) = self.overlay.new_agent_form_mut() else {
            return;
        };
        let Some(list) = form.list.take() else {
            return;
        };
        match list.selected_choice() {
            Some(choice) => form.apply(form.field, &choice.value),
            None => {
                form.error = Some(match list.free_text {
                    FreeText::Directory => {
                        "No project matches; type a new project name or a directory path".into()
                    }
                    FreeText::None | FreeText::Value => "Nothing matches that filter".into(),
                });
                form.list = Some(list);
                return;
            }
        }
        if step != 0 {
            form.move_field(step);
        }
    }

    /// Keeps the model and the effort row in step with the harness after it
    /// changed: fetches that harness's catalog and drops a model it cannot
    /// run, so switching harness never launches one CLI on another's model.
    fn harness_changed(&mut self) {
        let Some(harness) = self.overlay.new_agent_form().map(NewAgentForm::harness) else {
            return;
        };
        self.request_model_catalog(&harness);
        let options = self.model_options(&harness);
        if let Some(form) = self.overlay.new_agent_form_mut() {
            form.forget_foreign_model(&options);
            form.settle_field();
        }
    }

    /// Creates the agent, or points at the row that still needs a value. The
    /// form stays open on a validation slip so the typed task is kept.
    pub(super) fn submit_new_agent(&mut self) {
        if let Some(plan) = self.new_agent_plan() {
            self.start_new_agent(plan);
        }
    }

    /// What the filled-in form launches: a worker, or the project's handler.
    /// A value still missing or invalid is pointed out in the form instead.
    fn new_agent_plan(&mut self) -> Option<LaunchPlan> {
        let form = self.overlay.new_agent_form_mut()?;
        let project = PathBuf::from(form.project.trim());
        // A scratch agent may start without a task, as a session the user
        // types into; every other agent is briefed first.
        let problem = if form.prompt.trim().is_empty() && !form.is_scratch() {
            Some((
                NewField::Task,
                "Describe the first task before creating the agent",
            ))
        } else if form.kind.trim().is_empty() {
            Some((NewField::Harness, "Choose a harness"))
        } else if form.project.trim().is_empty() {
            Some((NewField::Project, "Choose a project"))
        } else if !project.is_absolute() || !(project.is_dir() || form.is_new_project()) {
            Some((
                NewField::Project,
                "Project must be an existing absolute directory",
            ))
        } else {
            None
        };
        if let Some((field, message)) = problem {
            form.field = field;
            form.error = Some(message.into());
            form.list = None;
            if field == NewField::Project {
                self.open_list(None);
            }
            return None;
        }
        let new_project = form.is_new_project();
        let harness = form.harness();
        let model = form.model.trim().to_string();
        let effort = form.effort.trim().to_string();
        let prompt = form.prompt.trim().to_string();
        let checkout = form.checkout;
        if !self.new_agent_starts_handler() {
            return Some(LaunchPlan {
                name: unique_agent_name(&self.agents, &project),
                harness,
                model,
                effort,
                prompt,
                project,
                new_project,
                checkout,
                extra_args: Vec::new(),
                role: Role::Worker,
            });
        }
        // The handler takes the harness, model, and effort chosen in the
        // form, like a worker, on a harness it can run on.
        let plan = if harness.supports_handler() {
            let root = project.to_string_lossy().into_owned();
            handler_plan(self, &root, harness, model, effort, prompt)
                .map_err(|error| (None, format!("{error:#}")))
        } else {
            Err((Some(NewField::Harness), handler_harness_error(&harness)))
        };
        match plan {
            Ok(plan) => Some(LaunchPlan {
                new_project,
                ..plan
            }),
            Err((field, message)) => {
                if let Some(form) = self.overlay.new_agent_form_mut() {
                    if let Some(field) = field {
                        form.field = field;
                        form.list = None;
                    }
                    form.error = Some(message);
                }
                None
            }
        }
    }

    pub(super) fn handle_new_agent_key(&mut self, key: KeyEvent) -> bool {
        let Some(form) = self.overlay.new_agent_form() else {
            return false;
        };
        let previous_kind = form.kind.clone();
        if form.list.is_some() {
            self.handle_list_key(key);
        } else {
            self.handle_form_key(key);
        }
        if self
            .overlay
            .new_agent_form()
            .is_some_and(|form| !form.kind.eq_ignore_ascii_case(&previous_kind))
        {
            self.harness_changed();
        }
        // The effort a harness defaults to can depend on the model, so the
        // hints follow every change.
        self.refresh_form_defaults();
        false
    }

    /// Keys while no list is open. One rule set for every row: ↑ ↓ and Tab
    /// move, ← → change a selector in place, Space or typing opens its list,
    /// Enter creates the agent, Esc cancels. The task takes typed text
    /// instead.
    fn handle_form_key(&mut self, key: KeyEvent) {
        let model_options = self
            .overlay
            .new_agent_form()
            .map(|form| self.model_options(&form.harness()))
            .unwrap_or_default();
        let Some(form) = self.overlay.new_agent_form_mut() else {
            return;
        };
        form.error = None;
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let backwards = key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT);
        let mut open_list = None;
        match key.code {
            KeyCode::Esc => {
                self.cancel_editor();
                return;
            }
            KeyCode::Enter => {
                self.submit_new_agent();
                return;
            }
            KeyCode::Tab | KeyCode::BackTab => form.move_field(if backwards { -1 } else { 1 }),
            // The task wraps over several rows, so its arrows move the caret
            // through the text; ↓ on the last row steps on to the settings.
            KeyCode::Up | KeyCode::Down if form.field == NewField::Task => {
                let (row, rows) = form.caret_rows();
                if key.code == KeyCode::Down && row + 1 >= rows {
                    form.step_field(1);
                } else {
                    form.move_caret(key.code);
                }
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End
                if form.field == NewField::Task =>
            {
                form.move_caret(key.code)
            }
            KeyCode::Backspace if form.field == NewField::Task => form.delete_before_caret(),
            KeyCode::Delete if form.field == NewField::Task => form.delete_at_caret(),
            KeyCode::Char(character) if form.field == NewField::Task && !control => {
                form.insert_at_caret(character)
            }
            KeyCode::Up => form.step_field(-1),
            KeyCode::Down => form.step_field(1),
            KeyCode::Left | KeyCode::Right if form.field == NewField::Project => {
                open_list = Some(None)
            }
            KeyCode::Left | KeyCode::Right if form.field.is_selector() => form.cycle(
                if key.code == KeyCode::Left { -1 } else { 1 },
                &model_options,
            ),
            KeyCode::Char(' ') if form.field.is_selector() => open_list = Some(None),
            KeyCode::Char(character) if form.field.is_selector() && !control => {
                open_list = Some(Some(character))
            }
            _ => {}
        }
        if let Some(seed) = open_list {
            self.open_list(seed);
        }
    }

    /// Keys while a selector's list is open: type to filter, ↑ ↓ move, Enter
    /// chooses, Tab chooses and moves on, Esc closes without a change.
    fn handle_list_key(&mut self, key: KeyEvent) {
        let Some(form) = self.overlay.new_agent_form_mut() else {
            return;
        };
        form.error = None;
        let backwards = key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => form.list = None,
            KeyCode::Enter => self.choose_from_list(0),
            KeyCode::Tab | KeyCode::BackTab => {
                self.choose_from_list(if backwards { -1 } else { 1 })
            }
            _ => {
                let Some(list) = &mut form.list else {
                    return;
                };
                match key.code {
                    KeyCode::Up => list.move_selection(-1),
                    KeyCode::Down => list.move_selection(1),
                    KeyCode::Backspace => list.pop_filter(),
                    KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        list.push_filter(character)
                    }
                    _ => {}
                }
            }
        }
    }
}

pub(super) fn checkout_choices() -> Vec<Choice> {
    [Checkout::Worktree, Checkout::Directory]
        .into_iter()
        .map(|checkout| {
            Choice::labelled(checkout.value(), checkout.label()).detail(checkout.detail())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::{
        app::test_helpers::press,
        model::{DashboardAgent, WorkspaceInfo, WorkspaceWorktreeInfo},
        projects::ProjectMemory,
        test_support::{ScratchDir, test_app},
    };

    use super::*;

    #[test]
    fn new_agent_composer_starts_in_the_task_row_with_everything_preset() {
        let mut app = test_app();
        app.agents = vec![DashboardAgent {
            project_root: "/repos/corgi".into(),
            ..DashboardAgent::default()
        }];

        app.begin_new_agent();

        assert!(matches!(app.overlay, Overlay::NewAgent(_)));
        let form = app.overlay.new_agent_form().expect("form open");
        assert_eq!(form.field, NewField::Task);
        assert_eq!(form.project, "/repos/corgi");
        assert_eq!(form.checkout, Checkout::Worktree);
        assert_eq!(form.model, "");
        assert_eq!(form.effort, "");
        assert!(form.list.is_none());
        assert!(form.error.is_none());
    }

    #[test]
    fn n_with_no_agent_selected_presets_the_most_recent_project() {
        let mut app = test_app();
        app.workspaces = vec![WorkspaceInfo {
            workspace_id: "w2".into(),
            label: "corgi".into(),
            worktree: Some(WorkspaceWorktreeInfo {
                repo_name: "corgi".into(),
                repo_root: "/repos/corgi".into(),
                checkout_path: "/repos/corgi".into(),
                is_linked_worktree: false,
            }),
            ..WorkspaceInfo::default()
        }];

        // The one repository open in Herdr is preset; no picker in between.
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
        assert!(matches!(app.overlay, Overlay::NewAgent(_)));
        let form = app.overlay.new_agent_form().expect("form open");
        assert_eq!(form.project, "/repos/corgi");
        assert_eq!(form.field, NewField::Task);
        assert!(form.list.is_none());

        // A remembered project that still exists wins over an open one.
        app.cancel_editor();
        let recent = std::env::temp_dir().to_string_lossy().into_owned();
        app.projects.remember(&recent);
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
        let form = app.overlay.new_agent_form().expect("form open");
        assert_eq!(form.project, app.projects.roots()[0]);
        assert_eq!(form.field, NewField::Task);

        // With nothing known at all, the project list opens first; Esc
        // closes the list, and a second Esc the form.
        app.cancel_editor();
        app.workspaces.clear();
        app.projects = ProjectMemory::default();
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
        let form = app.overlay.new_agent_form().expect("form open");
        assert_eq!(form.project, "");
        assert_eq!(form.field, NewField::Project);
        assert!(form.list.is_some());
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.overlay, Overlay::NewAgent(_)));
        assert!(
            app.overlay
                .new_agent_form()
                .is_some_and(|form| form.list.is_none())
        );
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.overlay.new_agent_form().is_none());
    }

    #[test]
    fn arrows_and_tab_move_through_the_rows_and_typing_opens_a_filtered_list() {
        let mut app = test_app();
        app.agents = vec![DashboardAgent {
            project_root: "/repos/corgi".into(),
            ..DashboardAgent::default()
        }];
        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
        app.overlay.new_agent_form_mut().expect("form").prompt_width = 40;
        for character in "Fix the".chars() {
            press(&mut app, KeyCode::Char(character), KeyModifiers::NONE);
        }
        let field = |app: &App| app.overlay.new_agent_form().expect("form").field;

        // ↓ on the task's last row steps on to the first setting.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(field(&app), NewField::Harness);

        // ← → change a selector in place; a model the new harness cannot
        // run goes with the old harness.
        {
            let form = app.overlay.new_agent_form_mut().expect("form");
            form.kind = "claude".into();
            form.model = "opus".into();
        }
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        let form = app.overlay.new_agent_form().expect("form");
        assert_eq!(form.kind, "gemini");
        assert_eq!(form.model, "");
        assert!(!form.fields().contains(&NewField::Effort));

        // Typing on a selector opens its list filtered to what was typed;
        // Enter takes the highlighted row and stays on the row.
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(field(&app), NewField::Model);
        for character in "fl".chars() {
            press(&mut app, KeyCode::Char(character), KeyModifiers::NONE);
        }
        let list = app
            .overlay
            .new_agent_form()
            .and_then(|form| form.list.as_ref())
            .expect("model list open");
        assert_eq!(list.filter, "fl");
        assert_eq!(
            list.selected_choice().map(|choice| choice.value),
            Some("gemini-2.5-flash".into())
        );
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let form = app.overlay.new_agent_form().expect("form");
        assert_eq!(form.model, "gemini-2.5-flash");
        assert!(form.list.is_none());
        assert_eq!(form.field, NewField::Model);

        // Gemini has no effort row, so Tab goes straight to the project.
        // Space opens its list on the current project.
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(field(&app), NewField::Project);
        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
        let list = app
            .overlay
            .new_agent_form()
            .and_then(|form| form.list.as_ref())
            .expect("project list open");
        assert_eq!(
            list.selected_choice().map(|choice| choice.value),
            Some("/repos/corgi".into())
        );

        // An absolute directory typed into the list is offered as a row;
        // Tab chooses it and moves on to the checkout.
        // Unique enough that path completion offers it alone.
        let scratch = ScratchDir::new("form-project");
        let directory = scratch.to_string_lossy().into_owned();
        for character in directory.chars() {
            press(&mut app, KeyCode::Char(character), KeyModifiers::NONE);
        }
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        let form = app.overlay.new_agent_form().expect("form");
        assert_eq!(form.project, directory);
        assert!(form.list.is_none());
        assert_eq!(form.field, NewField::Checkout);

        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(
            app.overlay.new_agent_form().map(|form| form.checkout),
            Some(Checkout::Directory)
        );

        // Tab from the last row wraps back to the task; ↑ and ↓ stop at the
        // ends instead.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(field(&app), NewField::Checkout);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(field(&app), NewField::Task);
        press(&mut app, KeyCode::BackTab, KeyModifiers::NONE);
        assert_eq!(field(&app), NewField::Checkout);
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(field(&app), NewField::Project);

        let form = app.overlay.new_agent_form().expect("form");
        assert_eq!(form.prompt, "Fix the");
        assert_eq!(form.model, "gemini-2.5-flash");

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.overlay.new_agent_form().is_none());
    }

    #[test]
    fn the_effort_row_exists_only_for_codex_and_claude_and_the_cursor_survives_its_removal() {
        let mut app = test_app();
        app.begin_new_agent_in("/repos/corgi".into());
        {
            let form = app.overlay.new_agent_form_mut().expect("form");
            form.kind = "codex".into();
            form.field = NewField::Effort;
        }
        let form = app.overlay.new_agent_form().expect("form");
        assert_eq!(
            form.fields(),
            [
                NewField::Task,
                NewField::Harness,
                NewField::Model,
                NewField::Effort,
                NewField::Project,
                NewField::Checkout,
            ]
        );

        // → on the effort row steps through the levels with the default in
        // the ring.
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(
            app.overlay
                .new_agent_form()
                .map(|form| form.effort.as_str()),
            Some("low")
        );
        press(&mut app, KeyCode::Left, KeyModifiers::NONE);
        press(&mut app, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(
            app.overlay
                .new_agent_form()
                .map(|form| form.effort.as_str()),
            Some("max")
        );

        // Choosing a harness without an effort row from the list removes the
        // row; the cursor lands on a row that still exists.
        {
            let form = app.overlay.new_agent_form_mut().expect("form");
            form.field = NewField::Harness;
        }
        for character in "gem".chars() {
            press(&mut app, KeyCode::Char(character), KeyModifiers::NONE);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let form = app.overlay.new_agent_form().expect("form");
        assert_eq!(form.kind, "gemini");
        assert!(!form.fields().contains(&NewField::Effort));
        assert_eq!(form.field, NewField::Harness);
        assert_eq!(
            form.harness().launch_args(&form.model, &form.effort),
            Vec::<String>::new()
        );
    }

    #[test]
    fn t_opens_the_form_for_a_scratch_agent_in_home_that_needs_no_task() {
        let Some(home) = home().filter(|home| home.is_dir()) else {
            return;
        };
        let home = home.to_string_lossy().into_owned();
        let mut app = test_app();
        // A scratch agent is selected: its home directory is still no
        // project for n to preset.
        app.agents = vec![DashboardAgent {
            project_root: home.clone(),
            scratch: true,
            ..DashboardAgent::default()
        }];
        assert_ne!(app.default_project(), home);

        press(&mut app, KeyCode::Char('t'), KeyModifiers::NONE);
        let form = app.overlay.new_agent_form().expect("form open");
        assert_eq!(form.field, NewField::Task);
        assert_eq!(form.project, home);
        assert_eq!(form.checkout, Checkout::Worktree);
        assert_eq!(form.kind, default_harness().kind());
        assert_eq!((form.model.as_str(), form.effort.as_str()), ("", ""));
        assert!(form.list.is_none());
        assert!(form.is_scratch());
        // Even with only a scratch agent there, it starts no handler.
        assert!(!app.new_agent_starts_handler());

        // An empty task is no slip: the agent starts as a session to type in.
        let plan = app.new_agent_plan().expect("an empty task is allowed");
        assert_eq!(plan.prompt, "");
        assert_eq!(plan.role, Role::Worker);
        assert_eq!(plan.project, Path::new(&home));
        assert!(!plan.new_project);
        assert!(
            app.overlay
                .new_agent_form()
                .is_some_and(|form| form.error.is_none())
        );
    }

    #[test]
    fn a_missing_task_or_project_keeps_the_form_open_and_points_at_the_row() {
        let mut app = test_app();
        app.begin_new_agent_in("/repos/corgi".into());

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.overlay, Overlay::NewAgent(_)));
        let form = app.overlay.new_agent_form().expect("form kept");
        assert_eq!(form.field, NewField::Task);
        assert!(
            form.error
                .as_deref()
                .is_some_and(|error| error.contains("first task"))
        );

        for character in "Do it".chars() {
            press(&mut app, KeyCode::Char(character), KeyModifiers::NONE);
        }
        assert!(
            app.overlay
                .new_agent_form()
                .is_some_and(|form| form.error.is_none())
        );

        // The preset repository does not exist on this machine, so Enter
        // opens the project list instead of launching or closing.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.overlay, Overlay::NewAgent(_)));
        let form = app.overlay.new_agent_form().expect("form kept");
        assert_eq!(form.field, NewField::Project);
        assert!(form.list.is_some());
        assert!(
            form.error
                .as_deref()
                .is_some_and(|error| error.contains("existing"))
        );
        assert_eq!(form.prompt, "Do it");

        // A filter that leaves no row says so and keeps the list open. A
        // search of several words is not a new project name.
        for character in "no such project".chars() {
            press(&mut app, KeyCode::Char(character), KeyModifiers::NONE);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let form = app.overlay.new_agent_form().expect("form kept");
        assert!(form.list.is_some());
        assert!(
            form.error
                .as_deref()
                .is_some_and(|error| error.contains("No project"))
        );
    }

    #[test]
    fn a_typed_name_starts_a_new_project_in_a_shared_checkout() {
        let mut app = test_app();
        app.begin_new_agent_in("/repos/corgi".into());
        app.overlay.new_agent_form_mut().expect("form").field = NewField::Project;
        let name = "corgi-test-fresh-project-7f3a";
        for character in name.chars() {
            press(&mut app, KeyCode::Char(character), KeyModifiers::NONE);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        let form = app.overlay.new_agent_form().expect("form kept");
        assert!(form.list.is_none(), "{:?}", form.error);
        assert!(
            form.project.ends_with(&format!("/{name}")),
            "{}",
            form.project
        );
        assert!(form.is_new_project());
        // A new repository has no commits to branch a worktree from.
        assert!(!form.fields().contains(&NewField::Checkout));
    }

    #[test]
    fn the_form_starts_a_handler_on_the_chosen_harness_model_and_effort() {
        let mut app = test_app();
        let project = std::env::temp_dir().to_string_lossy().into_owned();
        let form = |kind: &str, model: &str, effort: &str| NewAgentForm {
            field: NewField::Task,
            list: None,
            error: None,
            kind: kind.into(),
            model: model.into(),
            effort: effort.into(),
            defaults: HarnessDefaults::default(),
            project: project.clone(),
            new_project: false,
            checkout: Checkout::Worktree,
            prompt: "Plan the release".into(),
            prompt_caret: 0,
            prompt_width: 0,
        };

        app.overlay = Overlay::NewAgent(form("codex", "gpt-5-codex", "high"));
        let plan = app.new_agent_plan().expect("a Codex Project handler");
        assert!(matches!(plan.role, Role::Handler { .. }));
        assert_eq!(plan.checkout, Checkout::ProjectRoot);
        assert_eq!(
            (
                plan.harness.kind(),
                plan.model.as_str(),
                plan.effort.as_str()
            ),
            ("codex", "gpt-5-codex", "high")
        );

        app.overlay = Overlay::NewAgent(form("claude", "", ""));
        let plan = app.new_agent_plan().expect("a Claude Project handler");
        assert!(matches!(plan.role, Role::Handler { .. }));
        assert_eq!(
            (
                plan.harness.kind(),
                plan.model.as_str(),
                plan.effort.as_str()
            ),
            ("claude", "", "")
        );

        // A harness no handler runs on is pointed out, and the form stays.
        app.overlay = Overlay::NewAgent(form("gemini", "gemini-2.5-pro", ""));
        assert!(app.new_agent_plan().is_none());
        let form = app.overlay.new_agent_form().expect("the form stays open");
        assert_eq!(form.field, NewField::Harness);
        assert_eq!(
            form.error.as_deref(),
            Some("A Project handler runs on claude or codex, not gemini")
        );
    }

    #[test]
    fn the_first_task_is_edited_where_the_arrows_left_the_caret() {
        let mut app = test_app();
        app.begin_new_agent_in("/repos/corgi".into());
        let form = app.overlay.new_agent_form_mut().expect("form");
        form.field = NewField::Task;
        // The width the renderer reports for the field it drew, which is what
        // the vertical moves below follow. The task wraps as
        // "fix the / failing / test".
        form.prompt_width = 10;
        for character in "fix the failing test".chars() {
            app.handle_new_agent_key(KeyEvent::from(KeyCode::Char(character)));
        }
        let typed_at_the_end = |app: &App| {
            let form = app.overlay.new_agent_form().expect("form");
            form.prompt[..form.prompt_caret].to_string()
        };
        assert_eq!(typed_at_the_end(&app), "fix the failing test");

        // Up keeps the column, so the caret lands on the row above under
        // where it stood.
        app.handle_new_agent_key(KeyEvent::from(KeyCode::Up));
        assert_eq!(typed_at_the_end(&app), "fix the fail");
        app.handle_new_agent_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(typed_at_the_end(&app), "fix the fai");
        app.handle_new_agent_key(KeyEvent::from(KeyCode::Right));
        app.handle_new_agent_key(KeyEvent::from(KeyCode::Home));
        assert_eq!(typed_at_the_end(&app), "fix the ");

        // Typing goes in at the caret rather than at the end of the task.
        for character in "not ".chars() {
            app.handle_new_agent_key(KeyEvent::from(KeyCode::Char(character)));
        }
        let prompt = |app: &App| app.overlay.new_agent_form().expect("form").prompt.clone();
        assert_eq!(prompt(&app), "fix the not failing test");
        assert_eq!(typed_at_the_end(&app), "fix the not ");

        // Backspace takes the character before the caret and Delete the one
        // under it, both leaving the rest of the task alone.
        app.handle_new_agent_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(prompt(&app), "fix the notfailing test");
        app.handle_new_agent_key(KeyEvent::from(KeyCode::Delete));
        assert_eq!(prompt(&app), "fix the notailing test");
        assert_eq!(typed_at_the_end(&app), "fix the not");

        // End stops at the end of the caret's own row, past the last
        // character that row draws rather than on the row below.
        app.handle_new_agent_key(KeyEvent::from(KeyCode::End));
        assert_eq!(typed_at_the_end(&app), "fix the notailing");
        // Up from a longer row clamps to the end of the shorter one above.
        app.handle_new_agent_key(KeyEvent::from(KeyCode::Up));
        assert_eq!(typed_at_the_end(&app), "fix the");
        // The first row has none above it, so Up holds the caret there.
        app.handle_new_agent_key(KeyEvent::from(KeyCode::Up));
        assert_eq!(typed_at_the_end(&app), "fix the");
        app.handle_new_agent_key(KeyEvent::from(KeyCode::Home));
        assert_eq!(typed_at_the_end(&app), "");
        // Left at the start of the task has nowhere to go either.
        app.handle_new_agent_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(app.overlay.new_agent_form().expect("form").prompt_caret, 0);
    }

    #[test]
    fn switching_harness_drops_a_model_the_new_one_cannot_run() {
        let mut app = test_app();
        app.begin_new_agent_in("/repos/corgi".into());
        let form = app.overlay.new_agent_form_mut().expect("form");
        form.field = NewField::Harness;
        form.kind = "claude".into();
        form.model = "opus".into();

        app.handle_new_agent_key(KeyEvent::from(KeyCode::Right));

        let form = app.overlay.new_agent_form().expect("form");
        assert_ne!(form.kind, "claude");
        assert_eq!(form.model, "");
        assert_eq!(form.model_label(), HARNESS_DEFAULT_MODEL);
    }
}
