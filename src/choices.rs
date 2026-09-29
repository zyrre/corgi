//! The filterable list behind every selector in the new-agent form.
//!
//! A selector shows its current value in the form and, on request, opens a
//! list of choices directly beneath it. Typing filters the list, and where a
//! value outside the list makes sense (a model Corgi does not know, a
//! directory Corgi has never seen) the typed text is offered as a row too.
//! Everything here is pure so the behaviour can be tested without a terminal.

use std::path::Path;

use crate::{
    paths::{dir_name, expand_home},
    projects::path_completions,
};

/// One row of a selector: the value the form stores, the label the row shows,
/// and an optional detail and badge drawn after it in a quieter colour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
    pub detail: String,
    pub badge: String,
}

impl Choice {
    /// A choice whose label is its value.
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            label: value.clone(),
            value,
            detail: String::new(),
            badge: String::new(),
        }
    }

    /// A choice shown under a different label than the value it stores.
    pub fn labelled(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            detail: String::new(),
            badge: String::new(),
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = badge.into();
        self
    }

    /// A directory offered as a project: named after its last component.
    pub fn directory(path: impl Into<String>) -> Self {
        let path = path.into();
        Self {
            label: dir_name(&path).unwrap_or(&path).to_string(),
            detail: path.clone(),
            value: path,
            badge: "typed path".into(),
        }
    }

    /// A directory that does not exist yet, offered as a new project that
    /// is created when the agent starts.
    pub fn new_project(path: impl Into<String>) -> Self {
        Self {
            badge: "new project".into(),
            ..Self::directory(path)
        }
    }

    fn matches(&self, words: &[String]) -> bool {
        let haystack = format!("{} {} {}", self.label, self.value, self.detail).to_lowercase();
        words.iter().all(|word| haystack.contains(word.as_str()))
    }

    fn is(&self, text: &str) -> bool {
        self.value.eq_ignore_ascii_case(text) || self.label.eq_ignore_ascii_case(text)
    }
}

/// What the list does with filter text that names no listed choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreeText {
    /// Only listed choices can be chosen.
    None,
    /// The typed text is offered as a value of its own.
    Value,
    /// The typed text is an absolute directory path: directories that
    /// complete it are offered, and so is the path itself once it exists.
    /// A path that does not exist yet, or a bare name placed under
    /// [`ChoiceList::new_project_parent`], is offered as a new project.
    Directory,
}

/// A selector's open list: the choices, the filter typed over them, and the
/// row the cursor is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceList {
    pub choices: Vec<Choice>,
    pub filter: String,
    pub selected: usize,
    pub free_text: FreeText,
    /// Where a bare name typed into a directory list becomes a new project.
    pub new_project_parent: Option<String>,
}

impl ChoiceList {
    /// Opens the list on the row holding `current`, or on the first row.
    pub fn new(choices: Vec<Choice>, current: &str, free_text: FreeText) -> Self {
        let selected = choices
            .iter()
            .position(|choice| choice.value.eq_ignore_ascii_case(current.trim()))
            .unwrap_or(0);
        Self {
            choices,
            filter: String::new(),
            selected,
            free_text,
            new_project_parent: None,
        }
    }

    /// Lets a bare name typed into a directory list start a new project
    /// under `parent`.
    pub fn with_new_project_parent(mut self, parent: impl Into<String>) -> Self {
        self.new_project_parent = Some(parent.into());
        self
    }

    /// Swaps in a fresh set of choices (a model catalog that just arrived)
    /// while keeping the filter and, where possible, the highlighted row.
    pub fn replace_choices(&mut self, choices: Vec<Choice>) {
        let current = self.selected_choice().map(|choice| choice.value);
        self.choices = choices;
        self.selected = current
            .and_then(|value| self.rows().iter().position(|choice| choice.value == value))
            .unwrap_or(0);
    }

    /// The rows the filter leaves, in list order. Every word of the filter
    /// has to occur in the label, value, or detail, in any case. Depending on
    /// [`FreeText`], the filter itself follows as the last row(s).
    pub fn rows(&self) -> Vec<Choice> {
        let words: Vec<String> = self
            .filter
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let mut rows: Vec<Choice> = self
            .choices
            .iter()
            .filter(|choice| choice.matches(&words))
            .cloned()
            .collect();
        let typed = self.filter.trim();
        if typed.is_empty() {
            return rows;
        }
        match self.free_text {
            FreeText::None => {}
            FreeText::Value => {
                // Values never contain spaces; a filter that does is a
                // search, not a value of its own.
                if !typed.contains(char::is_whitespace)
                    && !self.choices.iter().any(|choice| choice.is(typed))
                {
                    rows.push(Choice::new(typed).badge("as typed"));
                }
            }
            FreeText::Directory => {
                let typed = expand_home(typed);
                let typed = typed.as_str();
                for path in path_completions(typed) {
                    if self.choices.iter().all(|choice| choice.value != path) {
                        rows.push(Choice::directory(path));
                    }
                }
                // A directory that ends in a slash lists its children above;
                // the directory itself stays choosable as the last row.
                if Path::new(typed).is_absolute()
                    && Path::new(typed).is_dir()
                    && self.choices.iter().all(|choice| choice.value != typed)
                    && rows.iter().all(|choice| choice.value != typed)
                {
                    rows.push(Choice::directory(typed));
                }
                if let Some(path) = self.new_project_path(typed) {
                    rows.push(Choice::new_project(path));
                }
            }
        }
        rows
    }

    /// Where the typed text would start a new project: an absolute path
    /// that does not exist yet, or a plain directory name under the new
    /// project parent that is not taken there.
    fn new_project_path(&self, typed: &str) -> Option<String> {
        let path = if Path::new(typed).is_absolute() {
            typed.trim_end_matches('/').to_string()
        } else {
            let parent = self.new_project_parent.as_deref()?;
            let is_name = !typed.contains('/')
                && !typed.contains(char::is_whitespace)
                && typed != "."
                && typed != "..";
            if !is_name {
                return None;
            }
            Path::new(parent).join(typed).to_string_lossy().into_owned()
        };
        (!path.is_empty() && !Path::new(&path).exists()).then_some(path)
    }

    /// The row under the cursor, if the filter left any.
    pub fn selected_choice(&self) -> Option<Choice> {
        let rows = self.rows();
        let index = self.selected.min(rows.len().saturating_sub(1));
        rows.into_iter().nth(index)
    }

    pub fn move_selection(&mut self, delta: isize) {
        let count = self.rows().len();
        if count == 0 {
            self.selected = 0;
            return;
        }
        let current = self.selected.min(count - 1) as isize;
        self.selected = (current + delta).rem_euclid(count as isize) as usize;
    }

    pub fn push_filter(&mut self, character: char) {
        self.filter.push(character);
        self.selected = 0;
    }

    pub fn pop_filter(&mut self) {
        self.filter.pop();
        self.selected = 0;
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Choice, ChoiceList, FreeText};
    use crate::test_support::ScratchDir;

    fn models() -> Vec<Choice> {
        vec![
            Choice::labelled("", "Harness default").badge("default"),
            Choice::new("opus"),
            Choice::new("opus[1m]"),
            Choice::new("sonnet"),
        ]
    }

    #[test]
    fn the_list_opens_on_the_current_value_and_filters_by_every_word() {
        let mut list = ChoiceList::new(models(), "sonnet", FreeText::Value);
        assert_eq!(list.selected, 3);
        assert_eq!(
            list.selected_choice().map(|choice| choice.value),
            Some("sonnet".into())
        );

        for character in "opus 1m".chars() {
            list.push_filter(character);
        }
        assert_eq!(
            list.rows()
                .iter()
                .map(|choice| choice.value.as_str())
                .collect::<Vec<_>>(),
            ["opus[1m]"]
        );
        assert_eq!(list.selected, 0);

        // A filter that matches a listed choice exactly offers no typed row,
        // and neither does a multi-word search.
        list.filter = "Opus".into();
        let rows = list.rows();
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter().all(|choice| choice.badge != "as typed"),
            "{rows:?}"
        );
        list.filter = "opus 1m".into();
        assert_eq!(list.rows().len(), 1);

        list.filter = "gpt-5".into();
        assert_eq!(list.rows(), vec![Choice::new("gpt-5").badge("as typed")]);
        assert_eq!(
            list.selected_choice().map(|choice| choice.value),
            Some("gpt-5".into())
        );
    }

    #[test]
    fn a_closed_list_offers_nothing_beyond_its_choices() {
        let mut list = ChoiceList::new(models(), "", FreeText::None);
        assert_eq!(list.selected, 0);
        list.filter = "nothing".into();
        assert!(list.rows().is_empty());
        assert_eq!(list.selected_choice(), None);
        list.move_selection(1);
        assert_eq!(list.selected, 0);
    }

    #[test]
    fn selection_wraps_around_the_filtered_rows() {
        let mut list = ChoiceList::new(models(), "", FreeText::None);
        list.move_selection(-1);
        assert_eq!(list.selected, 3);
        list.move_selection(1);
        assert_eq!(list.selected, 0);
        list.filter = "opus".into();
        list.move_selection(-1);
        assert_eq!(list.selected, 1);
    }

    #[test]
    fn directories_complete_a_typed_path_and_the_path_itself_stays_choosable() {
        let dir = ScratchDir::new("choices-completion");
        let alpha = dir.join("alpha");
        let alpine = dir.join("alpine");
        fs::create_dir_all(&alpha).expect("create alpha");
        fs::create_dir_all(&alpine).expect("create alpine");
        let mut list = ChoiceList::new(Vec::new(), "", FreeText::Directory);
        list.filter = format!("{}/al", dir.display());

        // The unfinished path could also be a new project, offered last.
        assert_eq!(
            list.rows(),
            vec![
                Choice::directory(alpha.to_string_lossy().into_owned()),
                Choice::directory(alpine.to_string_lossy().into_owned()),
                Choice::new_project(dir.join("al").to_string_lossy().into_owned()),
            ]
        );
        assert_eq!(list.rows()[0].label, "alpha");

        let typed_directory = format!("{}/", dir.display());
        list.filter = typed_directory.clone();
        assert_eq!(
            list.rows(),
            vec![
                Choice::directory(alpha.to_string_lossy().into_owned()),
                Choice::directory(alpine.to_string_lossy().into_owned()),
                Choice::directory(typed_directory),
            ]
        );

        // A relative path completes nothing.
        list.filter = "alp".into();
        assert!(list.rows().is_empty());
    }

    #[test]
    fn a_name_or_a_missing_path_is_offered_as_a_new_project() {
        let dir = ScratchDir::new("choices-new-project");
        let taken = dir.join("taken");
        fs::create_dir_all(&taken).expect("create taken");
        let parent = dir.to_string_lossy().into_owned();
        let mut list = ChoiceList::new(
            vec![Choice::directory(taken.to_string_lossy().into_owned())],
            "",
            FreeText::Directory,
        )
        .with_new_project_parent(parent.clone());

        list.filter = "fresh".into();
        let fresh = dir.join("fresh").to_string_lossy().into_owned();
        assert_eq!(list.rows(), vec![Choice::new_project(fresh.clone())]);
        assert_eq!(list.rows()[0].label, "fresh");

        // A name that already exists under the parent is not new, and a
        // search of several words is not a name.
        list.filter = "taken".into();
        assert!(list.rows().iter().all(|row| row.badge != "new project"));
        list.filter = "two words".into();
        assert!(list.rows().is_empty());

        // A full path works anywhere, and a trailing slash is dropped.
        let elsewhere = dir.join("sub").join("deep");
        list.filter = format!("{}/", elsewhere.display());
        assert_eq!(
            list.rows(),
            vec![Choice::new_project(
                elsewhere.to_string_lossy().into_owned()
            )]
        );

        // Without a parent a bare name stays a filter.
        list.new_project_parent = None;
        list.filter = "fresh".into();
        assert!(list.rows().is_empty());
    }

    #[test]
    fn replacing_choices_keeps_the_filter_and_the_highlighted_value() {
        let mut list = ChoiceList::new(models(), "opus", FreeText::Value);
        list.filter = "op".into();
        list.replace_choices(vec![
            Choice::labelled("", "Harness default"),
            Choice::new("gpt-5-codex"),
            Choice::new("opus"),
        ]);
        assert_eq!(list.filter, "op");
        assert_eq!(
            list.selected_choice().map(|choice| choice.value),
            Some("opus".into())
        );
    }
}
