//! Which repository an agent works in. An agent in any Git checkout belongs
//! to the repository `git rev-parse --git-common-dir` names, whether Herdr
//! opened its workspace as a linked worktree or not (decision 2026-11-03,
//! docs/orphaned-sessions.md): a plain `git worktree add` checkout, or an
//! agent whose cwd left its workspace's checkout, is grouped under its
//! repository and is a worktree for merge and close.
//!
//! Herdr's workspace provenance answers without running Git where it can:
//! a Herdr linked worktree the agent runs in, or the primary checkout's top
//! directory. Anything else asks Git once per directory, and the answer is
//! kept for [`REPO_TTL`] so a refresh every 850 ms does not run Git for each
//! agent each time.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::{
    git::git_output,
    model::{AgentInfo, WorkspaceInfo},
    paths::{dir_name, is_home},
    rows::agent_worktree,
};

/// How long Git's answer for one directory is kept.
pub const REPO_TTL: Duration = Duration::from_secs(60);

/// The repository and checkout a directory is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// The repository's primary checkout: where sibling worktrees are made
    /// from and merged into, and the project's root.
    pub root: String,
    /// The repository's name, as its heading shows it.
    pub name: String,
    /// The top of the checkout the directory is in: `root` itself, or a
    /// linked worktree's.
    pub checkout: String,
    /// The checkout is a linked worktree rather than the primary one.
    pub linked: bool,
    /// Herdr opened the agent's workspace on this linked worktree, so
    /// Herdr's `worktree.remove` takes it down; a plain `git worktree add`
    /// checkout is removed with Git instead.
    pub herdr_worktree: bool,
}

/// Git's answers by directory, each with when it was asked.
#[derive(Debug, Default)]
pub struct RepoCache {
    answers: HashMap<String, (Instant, Option<Repo>)>,
}

impl RepoCache {
    /// The repository `dir` is in, from Git, or `None` outside any.
    pub fn of_dir(&mut self, dir: &str) -> Option<Repo> {
        if let Some((asked, answer)) = self.answers.get(dir)
            && asked.elapsed() < REPO_TTL
        {
            return answer.clone();
        }
        let answer = repo_of_dir(Path::new(dir));
        self.answers
            .insert(dir.to_string(), (Instant::now(), answer.clone()));
        answer
    }

    /// Forgets the directories no agent works in any more.
    pub fn retain<'a>(&mut self, dirs: impl IntoIterator<Item = &'a str>) {
        let keep: std::collections::HashSet<&str> = dirs.into_iter().collect();
        self.answers.retain(|dir, _| keep.contains(dir.as_str()));
    }
}

/// The repository `info` works in: Herdr's provenance when it settles it,
/// else Git's answer for the agent's cwd. `None` for the home directory and
/// for a directory in no repository. A repository whose primary checkout is
/// the home directory (a dotfiles repository) is not one for an agent in a
/// subdirectory, which stays a project of its own.
pub fn agent_repo(
    info: &AgentInfo,
    workspace: Option<&WorkspaceInfo>,
    cache: &mut RepoCache,
) -> Option<Repo> {
    let cwd = info.cwd();
    if cwd.is_empty() || is_home(cwd) {
        return None;
    }
    if let Some(worktree) = agent_worktree(info, workspace).filter(|w| !w.repo_root.is_empty()) {
        let name = Some(worktree.repo_name.trim())
            .filter(|name| !name.is_empty())
            .or_else(|| dir_name(&worktree.repo_root))
            .unwrap_or_default()
            .to_string();
        if worktree.is_linked_worktree {
            return Some(Repo {
                root: worktree.repo_root.clone(),
                name,
                checkout: worktree.checkout_path.clone(),
                linked: true,
                herdr_worktree: true,
            });
        }
        if same_path(cwd, &worktree.checkout_path) {
            return Some(Repo {
                root: worktree.repo_root.clone(),
                name,
                checkout: worktree.checkout_path.clone(),
                linked: false,
                herdr_worktree: false,
            });
        }
    }
    cache.of_dir(cwd).filter(|repo| !is_home(&repo.root))
}

/// What Git says of `dir`: its primary checkout, its own checkout's top and
/// whether that is a linked worktree. One `git rev-parse`.
pub fn repo_of_dir(dir: &Path) -> Option<Repo> {
    let output = git_output(
        dir,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
            "--git-dir",
            "--show-toplevel",
        ],
    )
    .ok()?;
    let mut lines = output.lines().map(str::trim);
    let common = PathBuf::from(lines.next()?);
    let git_dir = PathBuf::from(lines.next()?);
    let toplevel = lines.next().filter(|line| !line.is_empty())?.to_string();
    // A repository's own `.git` sits in its primary checkout. Without one
    // (a bare repository's worktrees, a separate GIT_DIR) the checkout is
    // the only one Corgi can merge into, so it stands as the primary.
    let (root, linked) = match common.file_name().and_then(|name| name.to_str()) {
        Some(".git") => (
            common.parent()?.to_string_lossy().into_owned(),
            git_dir != common,
        ),
        _ => (toplevel.clone(), false),
    };
    Some(Repo {
        name: dir_name(&root).unwrap_or_default().to_string(),
        root,
        checkout: toplevel,
        linked,
        herdr_worktree: false,
    })
}

/// Whether two spellings of a directory are the same, ignoring a trailing
/// `/`.
fn same_path(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::WorkspaceWorktreeInfo,
        test_support::{ScratchDir, git_repo, git_worktree},
    };

    /// A repository with one commit, and a plain linked worktree of it made
    /// with `git worktree add`, as Claude Code's `--worktree` makes them.
    fn repository(scratch: &ScratchDir) -> (PathBuf, PathBuf) {
        let root = git_repo(&scratch.join("webshop"));
        let linked = git_worktree(&root, &root.join(".claude/worktrees/pay"), "pay");
        (root, linked)
    }

    fn agent(cwd: &Path) -> AgentInfo {
        AgentInfo {
            pane_id: "w1:p1".into(),
            workspace_id: "w1".into(),
            cwd: Some(cwd.to_string_lossy().into_owned()),
            ..AgentInfo::default()
        }
    }

    #[test]
    fn git_names_the_repository_of_a_primary_a_plain_linked_and_no_checkout() {
        let scratch = ScratchDir::new("repo-of-dir");
        let (root, linked) = repository(&scratch);
        let root_text = root.to_string_lossy().into_owned();

        let primary = repo_of_dir(&root.join(".")).expect("primary");
        assert_eq!(primary.root, root_text);
        assert_eq!(primary.name, "webshop");
        assert!(!primary.linked);

        let worktree = repo_of_dir(&linked).expect("linked worktree");
        assert_eq!(worktree.root, root_text);
        assert_eq!(worktree.checkout, linked.to_string_lossy());
        assert!(worktree.linked && !worktree.herdr_worktree);

        // A subdirectory of the primary checkout is the primary checkout.
        std::fs::create_dir_all(root.join("src")).unwrap();
        let sub = repo_of_dir(&root.join("src")).expect("subdirectory");
        assert_eq!((sub.root.as_str(), sub.linked), (root_text.as_str(), false));

        let plain = scratch.join("notes");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(repo_of_dir(&plain), None);
    }

    #[test]
    fn herdrs_provenance_settles_a_linked_worktree_without_git() {
        let info = agent(Path::new("/nowhere/.herdr/worktrees/corgi/worktree-a"));
        let workspace = WorkspaceInfo {
            workspace_id: "w1".into(),
            worktree: Some(WorkspaceWorktreeInfo {
                repo_name: "corgi".into(),
                repo_root: "/nowhere/repos/corgi".into(),
                checkout_path: "/nowhere/.herdr/worktrees/corgi/worktree-a".into(),
                is_linked_worktree: true,
            }),
            ..WorkspaceInfo::default()
        };
        let mut cache = RepoCache::default();
        let repo = agent_repo(&info, Some(&workspace), &mut cache).expect("repo");
        assert_eq!(repo.root, "/nowhere/repos/corgi");
        assert!(repo.linked && repo.herdr_worktree);
        assert!(cache.answers.is_empty(), "Git was not asked");
    }

    #[test]
    fn a_plain_linked_checkout_in_the_primarys_workspace_is_its_repositorys_worktree() {
        let scratch = ScratchDir::new("repo-plain-linked");
        let (root, linked) = repository(&scratch);
        let root_text = root.to_string_lossy().into_owned();
        // Herdr's workspace is the primary checkout's; the agent runs in a
        // `git worktree add` checkout inside it.
        let workspace = WorkspaceInfo {
            workspace_id: "w1".into(),
            worktree: Some(WorkspaceWorktreeInfo {
                repo_name: "webshop".into(),
                repo_root: root_text.clone(),
                checkout_path: root_text.clone(),
                is_linked_worktree: false,
            }),
            ..WorkspaceInfo::default()
        };
        let mut cache = RepoCache::default();
        let repo = agent_repo(&agent(&linked), Some(&workspace), &mut cache).expect("repo");
        assert_eq!(repo.root, root_text);
        assert!(repo.linked && !repo.herdr_worktree);

        // The primary's top directory needs no Git.
        let mut fresh = RepoCache::default();
        let primary = agent_repo(&agent(&root), Some(&workspace), &mut fresh).expect("primary");
        assert!(!primary.linked);
        assert!(fresh.answers.is_empty());

        // A plain workspace on the linked checkout: Git alone says.
        let plain = WorkspaceInfo {
            workspace_id: "w1".into(),
            ..WorkspaceInfo::default()
        };
        let repo = agent_repo(&agent(&linked), Some(&plain), &mut cache).expect("repo");
        assert_eq!(repo.root, root_text);
        assert!(repo.linked);

        // Outside any repository there is none.
        let notes = scratch.join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        assert_eq!(agent_repo(&agent(&notes), Some(&plain), &mut cache), None);
    }
}
