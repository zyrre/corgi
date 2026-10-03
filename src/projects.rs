//! The projects a new agent can be started in, and how the form lists them.
//!
//! Herdr represents a project with a repository workspace. Corgi creates one
//! separate, agentless and metadata-marked workspace per repository when it
//! first starts an agent there, so its workers do not depend on the dashboard
//! workspace that launched them. A project here is simply a directory a new
//! agent can be created from, and it becomes known in one of three ways: an
//! agent is running in it, a Herdr workspace is open in it, or Corgi used it
//! before and wrote it down. The written-down list lives in one plain text
//! file, one absolute path per line, most recently used first.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use crate::{
    choices::Choice,
    model::{DashboardAgent, WorkspaceInfo},
    paths::{corgi_state_dir, dir_name, home},
};

/// Most projects Corgi keeps in its file. Older, unused ones fall off the end.
const REMEMBERED_LIMIT: usize = 50;

/// Where a known project comes from, in the order the project list shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProjectSource {
    /// One or more agent sessions run in it right now.
    Agents(usize),
    /// A Herdr workspace is open in it, without an agent.
    Open,
    /// A sibling directory of a project currently open in Corgi or Herdr.
    Nearby,
    /// Corgi started an agent there before, or saw it open, and remembers it.
    Remembered,
}

impl ProjectSource {
    /// The short badge the project list shows after the path.
    pub fn badge(self) -> String {
        match self {
            Self::Agents(1) => "1 agent".into(),
            Self::Agents(count) => format!("{count} agents"),
            Self::Open => "open in Herdr".into(),
            Self::Nearby => "nearby".into(),
            Self::Remembered => "recent".into(),
        }
    }
}

/// A directory a new agent can be created from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// Absolute path: the repository's primary checkout, or a plain directory.
    pub root: String,
    /// The directory name, which is how the dashboard headings name projects.
    pub name: String,
    pub source: ProjectSource,
}

/// The projects Corgi has written down, in most-recently-used order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectMemory {
    /// `None` keeps the list in memory only, which tests use.
    path: Option<PathBuf>,
    roots: Vec<String>,
}

impl ProjectMemory {
    /// Reads the list from its file. A missing or unreadable file is an
    /// empty list, never an error: the projects on screen still work.
    pub fn load() -> Self {
        let path = memory_path();
        let roots = path
            .as_deref()
            .and_then(|path| fs::read_to_string(path).ok())
            .map(|text| parse_roots(&text))
            .unwrap_or_default();
        Self { path, roots }
    }

    /// The remembered roots, most recently used first.
    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    /// Records that an agent was just started in `root`, moving it to the
    /// front of the list.
    pub fn remember(&mut self, root: &str) {
        let root = root.trim();
        if root.is_empty() {
            return;
        }
        if self.roots.first().is_some_and(|first| first == root) {
            return;
        }
        self.roots.retain(|known| known != root);
        self.roots.insert(0, root.to_string());
        self.roots.truncate(REMEMBERED_LIMIT);
        self.save();
    }

    /// Records a project seen open in Herdr without changing the order of the
    /// ones already known, so a refresh never reshuffles the project list.
    pub fn note(&mut self, root: &str) {
        let root = root.trim();
        if root.is_empty() || self.roots.iter().any(|known| known == root) {
            return;
        }
        self.roots.push(root.to_string());
        self.roots.truncate(REMEMBERED_LIMIT);
        self.save();
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Some(directory) = path.parent() {
            let _ = fs::create_dir_all(directory);
        }
        let mut text = self.roots.join("\n");
        text.push('\n');
        let _ = fs::write(path, text);
    }
}

/// The projects file: `$CORGI_PROJECTS_FILE`, else `corgi/projects` under
/// `$XDG_STATE_HOME` or `~/.local/state`.
fn memory_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("CORGI_PROJECTS_FILE").filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    Some(corgi_state_dir()?.join("projects"))
}

/// The projects Corgi created itself, one canonical path per line, in
/// `created-projects` beside the project list (`$CORGI_CREATED_PROJECTS_FILE`
/// overrides it). Corgi made each of these as an empty directory for its
/// corgi, so it answers Claude Code's folder-trust question for them.
fn created_projects_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("CORGI_CREATED_PROJECTS_FILE").filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    Some(corgi_state_dir()?.join("created-projects"))
}

/// Records `project`, a directory Corgi has just created, as one of its own.
pub fn record_created_project(project: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let Some(path) = created_projects_path() else {
        return Ok(());
    };
    let project = fs::canonicalize(project)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{}", project.display())
}

/// Whether Corgi created the project whose directory is `root`.
pub fn is_created_project(root: &Path) -> bool {
    let Ok(root) = fs::canonicalize(root) else {
        return false;
    };
    created_projects_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .is_some_and(|text| lists_project(&text, &root))
}

fn lists_project(text: &str, root: &Path) -> bool {
    text.lines()
        .map(str::trim)
        .any(|line| !line.is_empty() && Path::new(line) == root)
}

fn parse_roots(text: &str) -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || roots.iter().any(|root| root == line) {
            continue;
        }
        roots.push(line.to_string());
    }
    roots
}

/// Every project a new agent can be created in, in the order the project list shows
/// them: projects with agents first, then open Herdr workspaces, nearby
/// directories beside those open projects, then remembered ones most recently
/// used first. A path appears once, under its most informative source, and a
/// remembered directory that no longer exists is left out.
pub fn known_projects(
    agents: &[DashboardAgent],
    workspaces: &[WorkspaceInfo],
    memory: &ProjectMemory,
) -> Vec<Project> {
    let mut projects: Vec<Project> = Vec::new();
    for agent in agents {
        let root = agent.project_root.trim();
        // A scratch agent's home directory is never a project.
        if root.is_empty() || agent.scratch {
            continue;
        }
        match projects.iter_mut().find(|project| project.root == root) {
            Some(Project {
                source: ProjectSource::Agents(count),
                ..
            }) => *count += 1,
            Some(_) => {}
            None => projects.push(project(root, ProjectSource::Agents(1))),
        }
    }
    projects.sort_by_key(|project| project.name.to_lowercase());
    let mut open: Vec<Project> = Vec::new();
    for root in workspaces.iter().filter_map(WorkspaceInfo::repo_root) {
        if projects
            .iter()
            .chain(&open)
            .all(|project| project.root != root)
        {
            open.push(project(root, ProjectSource::Open));
        }
    }
    open.sort_by_key(|project| project.name.to_lowercase());
    projects.extend(open);
    let nearby_roots: Vec<String> = projects
        .iter()
        .map(|project| project.root.clone())
        .collect();
    let mut nearby = Vec::new();
    for root in nearby_roots {
        discover_siblings(&root, &projects, &mut nearby);
    }
    nearby.sort_by_key(|project| project.name.to_lowercase());
    projects.extend(nearby);
    for root in memory.roots() {
        if projects.iter().all(|project| &project.root != root) && Path::new(root).is_dir() {
            projects.push(project(root, ProjectSource::Remembered));
        }
    }
    projects
}

/// Adds visible directory siblings of `root` that Corgi does not know yet.
/// Open repositories tend to be collected under one `projects` directory, so
/// this makes the next repository available without first opening it in Herdr.
fn discover_siblings(root: &str, known: &[Project], nearby: &mut Vec<Project>) {
    let Some(parent) = Path::new(root).parent() else {
        return;
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let root = path.to_string_lossy().into_owned();
        if known
            .iter()
            .chain(nearby.iter())
            .all(|project| project.root != root)
        {
            nearby.push(project(&root, ProjectSource::Nearby));
        }
    }
}

fn project(root: &str, source: ProjectSource) -> Project {
    Project {
        root: root.to_string(),
        name: dir_name(root).unwrap_or(root).to_string(),
        source,
    }
}

/// The known projects as selector rows: the directory name, its path, and
/// where Corgi knows it from.
pub fn project_choices(projects: &[Project]) -> Vec<Choice> {
    projects
        .iter()
        .map(|project| {
            Choice::labelled(project.root.clone(), project.name.clone())
                .detail(project.root.clone())
                .badge(project.source.badge())
        })
        .collect()
}

/// Where a project typed by name alone is created: the directory most known
/// projects live in, so new repositories land beside the existing ones. Ties
/// go to the parent of the project listed first; with no project known it is
/// the home directory.
pub fn new_project_parent(projects: &[Project]) -> Option<String> {
    let mut parents: Vec<(String, usize)> = Vec::new();
    for project in projects {
        // Nearby directories are siblings of a listed project by definition
        // and would only repeat its parent.
        if project.source == ProjectSource::Nearby {
            continue;
        }
        let Some(parent) = Path::new(&project.root).parent() else {
            continue;
        };
        let parent = parent.to_string_lossy().into_owned();
        if parent.is_empty() || parent == "/" {
            continue;
        }
        match parents.iter_mut().find(|(known, _)| *known == parent) {
            Some((_, count)) => *count += 1,
            None => parents.push((parent, 1)),
        }
    }
    let mut best: Option<(String, usize)> = None;
    for (parent, count) in parents {
        if best.as_ref().is_none_or(|(_, most)| count > *most) {
            best = Some((parent, count));
        }
    }
    best.map(|(parent, _)| parent)
        .or_else(|| home().and_then(|home| home.into_os_string().into_string().ok()))
}

/// Direct child directories that complete the last component of an absolute
/// path. Reading only one directory keeps suggestions fast and predictable.
pub fn path_completions(typed: &str) -> Vec<String> {
    let path = Path::new(typed);
    if !path.is_absolute() {
        return Vec::new();
    }
    let (parent, prefix) = if typed.ends_with(std::path::MAIN_SEPARATOR) {
        (path, "")
    } else {
        (
            path.parent().unwrap_or_else(|| Path::new("/")),
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default(),
        )
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut paths: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            (entry.path().is_dir() && name.starts_with(prefix))
                .then(|| entry.path().to_string_lossy().into_owned())
        })
        .collect();
    paths.sort_by_key(|path| path.to_lowercase());
    paths
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        Project, ProjectMemory, ProjectSource, known_projects, lists_project, new_project_parent,
        parse_roots, path_completions, project_choices,
    };
    use crate::choices::Choice;
    use crate::model::{DashboardAgent, WorkspaceInfo, WorkspaceWorktreeInfo};
    use crate::paths::dir_name;
    use crate::test_support::ScratchDir;

    #[test]
    fn only_a_listed_directory_counts_as_created_by_corgi() {
        let list = "/repos/weather\n\n  /repos/test/hello  \n";
        assert!(lists_project(list, std::path::Path::new("/repos/weather")));
        assert!(lists_project(
            list,
            std::path::Path::new("/repos/test/hello")
        ));
        // Neither a parent, a child, nor a sibling of a listed project.
        for other in ["/repos", "/repos/weather/sub", "/repos/weather-2", ""] {
            assert!(!lists_project(list, std::path::Path::new(other)), "{other}");
        }
    }

    fn agent_in(root: &str) -> DashboardAgent {
        DashboardAgent {
            project_root: root.into(),
            ..DashboardAgent::default()
        }
    }

    fn primary_workspace(root: &str) -> WorkspaceInfo {
        WorkspaceInfo {
            worktree: Some(WorkspaceWorktreeInfo {
                repo_root: root.into(),
                checkout_path: root.into(),
                is_linked_worktree: false,
                ..WorkspaceWorktreeInfo::default()
            }),
            ..WorkspaceInfo::default()
        }
    }

    #[test]
    fn the_memory_file_is_one_path_per_line_most_recent_first() {
        let dir = ScratchDir::new("projects-memory");
        let file = dir.join("projects");
        let mut memory = ProjectMemory {
            path: Some(file.clone()),
            roots: Vec::new(),
        };

        memory.note("/repos/a");
        memory.note("/repos/b");
        memory.remember("/repos/b");
        memory.remember("/repos/c");
        memory.note("/repos/a");

        let text = fs::read_to_string(&file).expect("memory file written");
        assert_eq!(text, "/repos/c\n/repos/b\n/repos/a\n");
        assert_eq!(parse_roots("# comment\n/x\n\n/x\n/y\n"), vec!["/x", "/y"]);
    }

    #[test]
    fn known_projects_list_agents_then_open_workspaces_then_remembered_directories() {
        let dir = ScratchDir::new("projects-known");
        let remembered = dir.join("remembered");
        fs::create_dir_all(&remembered).expect("create remembered dir");
        let gone = dir.join("gone");
        let memory = ProjectMemory {
            path: None,
            roots: vec![
                remembered.to_string_lossy().into_owned(),
                gone.to_string_lossy().into_owned(),
                "/repos/corgi".into(),
            ],
        };

        let projects = known_projects(
            &[
                agent_in("/repos/zeta"),
                agent_in("/repos/corgi"),
                agent_in("/repos/corgi"),
            ],
            &[
                primary_workspace("/repos/corgi"),
                primary_workspace("/repos/open"),
            ],
            &memory,
        );

        assert_eq!(
            projects,
            vec![
                Project {
                    root: "/repos/corgi".into(),
                    name: "corgi".into(),
                    source: ProjectSource::Agents(2),
                },
                Project {
                    root: "/repos/zeta".into(),
                    name: "zeta".into(),
                    source: ProjectSource::Agents(1),
                },
                Project {
                    root: "/repos/open".into(),
                    name: "open".into(),
                    source: ProjectSource::Open,
                },
                Project {
                    root: remembered.to_string_lossy().into_owned(),
                    name: "remembered".into(),
                    source: ProjectSource::Remembered,
                },
            ]
        );
        assert_eq!(ProjectSource::Agents(2).badge(), "2 agents");
        assert_eq!(ProjectSource::Agents(1).badge(), "1 agent");
    }

    #[test]
    fn projects_become_rows_named_after_their_directory_with_a_source_badge() {
        let choices = project_choices(&[
            Project {
                root: "/repos/corgi".into(),
                name: "corgi".into(),
                source: ProjectSource::Agents(1),
            },
            Project {
                root: "/work/webshop-backend".into(),
                name: "webshop-backend".into(),
                source: ProjectSource::Open,
            },
        ]);
        assert_eq!(
            choices,
            vec![
                Choice::labelled("/repos/corgi", "corgi")
                    .detail("/repos/corgi")
                    .badge("1 agent"),
                Choice::labelled("/work/webshop-backend", "webshop-backend")
                    .detail("/work/webshop-backend")
                    .badge("open in Herdr"),
            ]
        );
    }

    #[test]
    fn new_projects_go_where_most_known_projects_live() {
        let known = |root: &str, source| Project {
            root: root.into(),
            name: dir_name(root).unwrap_or(root).to_string(),
            source,
        };
        assert_eq!(
            new_project_parent(&[
                known("/work/webshop", ProjectSource::Agents(3)),
                known("/repos/corgi", ProjectSource::Open),
                known("/repos/zeta", ProjectSource::Remembered),
                known("/work/a", ProjectSource::Nearby),
                known("/work/b", ProjectSource::Nearby),
            ]),
            Some("/repos".into())
        );
        assert_eq!(
            new_project_parent(&[
                known("/work/webshop", ProjectSource::Open),
                known("/repos/corgi", ProjectSource::Open),
            ]),
            Some("/work".into())
        );
    }

    #[test]
    fn nearby_directories_of_open_projects_are_suggested() {
        let dir = ScratchDir::new("projects-nearby");
        let corgi = dir.join("corgi");
        let other = dir.join("other-project");
        fs::create_dir_all(&corgi).expect("create open project");
        fs::create_dir_all(&other).expect("create sibling project");

        let projects = known_projects(
            &[],
            &[primary_workspace(corgi.to_string_lossy().as_ref())],
            &ProjectMemory::default(),
        );

        assert_eq!(
            projects,
            vec![
                Project {
                    root: corgi.to_string_lossy().into_owned(),
                    name: "corgi".into(),
                    source: ProjectSource::Open,
                },
                Project {
                    root: other.to_string_lossy().into_owned(),
                    name: "other-project".into(),
                    source: ProjectSource::Nearby,
                },
            ]
        );
    }

    #[test]
    fn absolute_path_prefixes_complete_to_their_sibling_directories() {
        let dir = ScratchDir::new("projects-completion");
        let alpha = dir.join("alpha");
        let alpine = dir.join("alpine");
        fs::create_dir_all(&alpha).expect("create alpha");
        fs::create_dir_all(&alpine).expect("create alpine");
        fs::write(dir.join("alpha.txt"), "").expect("create a file");

        assert_eq!(
            path_completions(&format!("{}/al", dir.display())),
            [
                alpha.to_string_lossy().into_owned(),
                alpine.to_string_lossy().into_owned(),
            ]
        );
        assert_eq!(
            path_completions(&format!("{}/", dir.display())),
            [
                alpha.to_string_lossy().into_owned(),
                alpine.to_string_lossy().into_owned(),
            ]
        );
        assert!(path_completions("relative/al").is_empty());
    }
}
