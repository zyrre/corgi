//! The filterable list of projects behind `corgi projects`, which the Herdr
//! app's "Open project" field shows.
//!
//! Typing filters the known projects, and a directory Corgi has never seen
//! is offered as a row too: the directories that complete a typed path, the
//! path itself, and a path or name that does not exist yet as a new project.

use std::path::Path;

use crate::{
    paths::{dir_name, expand_home},
    projects::path_completions,
};

/// One row of the list: the directory it stands for, the label the row
/// shows, and an optional detail and badge shown after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
    pub detail: String,
    pub badge: String,
}

impl Choice {
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
}

/// The list: the known projects and the filter typed over them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceList {
    pub choices: Vec<Choice>,
    pub filter: String,
    /// Where a bare name typed into the list becomes a new project.
    pub new_project_parent: Option<String>,
}

impl ChoiceList {
    pub fn new(choices: Vec<Choice>) -> Self {
        Self {
            choices,
            filter: String::new(),
            new_project_parent: None,
        }
    }

    /// Lets a bare name typed into the list start a new project under
    /// `parent`.
    pub fn with_new_project_parent(mut self, parent: impl Into<String>) -> Self {
        self.new_project_parent = Some(parent.into());
        self
    }

    /// The rows the filter leaves, in list order. Every word of the filter
    /// has to occur in the label, value, or detail, in any case. The typed
    /// text is an absolute directory path: directories that complete it
    /// follow as the last rows, and so does the path itself once it exists.
    /// A path that does not exist yet, or a bare name placed under
    /// [`ChoiceList::new_project_parent`], is offered as a new project.
    pub fn rows(&self) -> Vec<Choice> {
        // A directory typed from ~ is matched as the full path the choices
        // hold, so a known project stays listed while its path is typed.
        let filter = expand_home(self.filter.trim());
        let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
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
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Choice, ChoiceList};
    use crate::test_support::ScratchDir;

    #[test]
    fn directories_complete_a_typed_path_and_the_path_itself_stays_choosable() {
        let dir = ScratchDir::new("choices-completion");
        let alpha = dir.join("alpha");
        let alpine = dir.join("alpine");
        fs::create_dir_all(&alpha).expect("create alpha");
        fs::create_dir_all(&alpine).expect("create alpine");
        let mut list = ChoiceList::new(Vec::new());
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
    fn a_known_directory_stays_listed_while_its_path_is_typed_from_the_home_directory() {
        let Some(home) = crate::paths::home().and_then(|home| home.to_str().map(str::to_string))
        else {
            return;
        };
        let known = format!("{home}/corgi-choices-no-such-folder/weather");
        let mut list = ChoiceList::new(vec![
            Choice::labelled(known.clone(), "weather").detail(known.clone()),
        ]);
        list.filter = "~/corgi-choices-no-such-folder/wea".into();
        let rows = list.rows();
        assert_eq!(rows[0].value, known, "{rows:?}");
        // What is typed could also be a new project, offered after it.
        assert_eq!(
            rows[1],
            Choice::new_project(format!("{home}/corgi-choices-no-such-folder/wea"))
        );
        list.filter = "~/corgi-choices-no-such-folder/other".into();
        assert!(list.rows().iter().all(|row| row.value != known));
    }

    #[test]
    fn a_name_or_a_missing_path_is_offered_as_a_new_project() {
        let dir = ScratchDir::new("choices-new-project");
        let taken = dir.join("taken");
        fs::create_dir_all(&taken).expect("create taken");
        let parent = dir.to_string_lossy().into_owned();
        let mut list = ChoiceList::new(vec![Choice::directory(
            taken.to_string_lossy().into_owned(),
        )])
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
}
