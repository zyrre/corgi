//! Closing a worker (`corgi close`): a checkout with uncommitted or
//! untracked changes is refused once, unless forced; then the worker's
//! terminal is ended and removed, by its address, and its worktree removed
//! with `git worktree remove` in the primary checkout. The branch stays.
//!
//! A worker in a worktree has a space of its own in the Herdr app
//! ([`super::launch::worker_space`]), which the app closes by itself once
//! its last terminal is gone; one where the person opened a terminal of
//! their own stays, with it. So nothing here closes a space, and nothing
//! but the worker's own terminal is ended. A supervisor is never closed.

use std::{
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use serde_json::{Value, json};

use crate::{
    git::{git_current_branch, git_output},
    model::Agent,
};

use super::{HcClient, SPACE_LABEL};

/// How long a killed terminal may take to finish before it is removed.
const FINISH_TIMEOUT: Duration = Duration::from_secs(10);

/// What [`close_worker`] may do beyond the plain close.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CloseOptions {
    /// Remove a worktree with uncommitted or untracked changes too
    /// (`git worktree remove --force`), discarding them.
    pub force: bool,
    /// Leave the worktree on disk: end and remove the terminal alone.
    pub keep_worktree: bool,
}

/// How a close ended, as `corgi close --json` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseOutcome {
    /// `closed`, `refused` or `failed`.
    pub result: &'static str,
    pub message: String,
    /// Why it was refused: `dirty` (see `dirty`), `supervisor`, `no-agent`,
    /// `self` (the worker is the caller), or `unavailable`.
    pub reason: Option<&'static str>,
    /// The worktree's uncommitted and untracked changes, as
    /// `git status --porcelain` lists them, for a `dirty` refusal.
    pub dirty: Vec<String>,
    /// The worker's worktree, when it had one.
    pub worktree: Option<String>,
    /// The worktree's branch, which stays.
    pub branch: Option<String>,
    /// Whether the worktree was removed from disk.
    pub removed: bool,
}

impl CloseOutcome {
    pub fn refused(reason: &'static str, message: String) -> Self {
        Self {
            result: "refused",
            message,
            reason: Some(reason),
            dirty: Vec::new(),
            worktree: None,
            branch: None,
            removed: false,
        }
    }

    fn failed(message: String, worktree: Option<&Worktree>) -> Self {
        Self {
            result: "failed",
            message,
            reason: None,
            dirty: Vec::new(),
            worktree: worktree.map(Worktree::path),
            branch: worktree.and_then(|worktree| worktree.branch.clone()),
            removed: false,
        }
    }

    pub fn succeeded(&self) -> bool {
        self.result == "closed"
    }

    pub fn json(&self) -> Value {
        json!({
            "result": self.result,
            "message": self.message,
            "reason": self.reason,
            "dirty": self.dirty,
            "worktree": self.worktree,
            "branch": self.branch,
            "removed": self.removed,
        })
    }
}

/// A worker's linked worktree.
#[derive(Debug, Clone)]
struct Worktree {
    checkout: PathBuf,
    branch: Option<String>,
}

impl Worktree {
    fn path(&self) -> String {
        self.checkout.to_string_lossy().into_owned()
    }
}

/// Closes the worker `name` of the project at `root` among `agents`
/// (Corgi's rows on hc): refuses a supervisor, a name Corgi did not start,
/// the caller's own terminal (`caller`, its hc address), and, unless
/// `options.force`, a worktree with uncommitted or untracked changes. Then
/// ends and removes the worker's terminal and removes its worktree, unless
/// `options.keep_worktree`. The branch always stays.
pub fn close_worker(
    client: &HcClient,
    agents: &[Agent],
    name: &str,
    root: &str,
    caller: Option<&str>,
    options: CloseOptions,
) -> CloseOutcome {
    let named = |agent: &&Agent| {
        agent.project_root == root
            && (agent.info.display_name() == name || agent.info.name.as_deref() == Some(name))
    };
    let Some(agent) = agents
        .iter()
        .filter(named)
        .find(|agent| !agent.supervisor)
        .or_else(|| agents.iter().find(named))
    else {
        return CloseOutcome::refused(
            "no-agent",
            format!("No worker {name} of Corgi's runs in {root}"),
        );
    };
    if agent.supervisor {
        return CloseOutcome::refused(
            "supervisor",
            format!("{name} is the supervisor of {root}: only a worker is closed this way"),
        );
    }
    let address = agent.info.address.as_str();
    if caller == Some(address) {
        return CloseOutcome::refused(
            "self",
            format!("{name} runs this command: it cannot close its own terminal"),
        );
    }
    let worktree = match linked_worktree(agent, root) {
        Ok(worktree) => worktree,
        Err(error) => {
            return CloseOutcome::failed(format!("Close of {name} failed: {error:#}"), None);
        }
    };
    let remove = worktree
        .as_ref()
        .filter(|worktree| !options.keep_worktree && worktree.checkout.exists());
    if let Some(worktree) = remove
        && !options.force
    {
        match changes(&worktree.checkout) {
            Ok(dirty) if !dirty.is_empty() => {
                let mut refused = CloseOutcome::refused(
                    "dirty",
                    format!(
                        "The worktree of {name} has uncommitted or untracked changes: {}",
                        named_changes(&dirty)
                    ),
                );
                refused.dirty = dirty;
                refused.worktree = Some(worktree.path());
                refused.branch = worktree.branch.clone();
                return refused;
            }
            Ok(_) => {}
            Err(error) => {
                return CloseOutcome::failed(
                    format!("Close of {name} failed: {error:#}"),
                    Some(worktree),
                );
            }
        }
    }
    if let Err(error) = end_terminal(client, address) {
        return CloseOutcome::failed(
            format!("Close of {name} failed: {error:#}"),
            worktree.as_ref(),
        );
    }
    let shared = shared_space(client, agent, agents);
    let mut message = format!("Closed {name}");
    let mut removed = false;
    match (&worktree, remove) {
        (_, Some(worktree)) => {
            let path = worktree.path();
            let mut args = vec!["worktree", "remove"];
            if options.force {
                args.push("--force");
            }
            args.push(&path);
            if let Err(error) = git_output(Path::new(root), &args) {
                return CloseOutcome::failed(
                    format!("Closed {name}, but its worktree {path} stays: {error:#}"),
                    Some(worktree),
                );
            }
            removed = true;
            message.push_str(&format!(" and removed its worktree {path}"));
        }
        (Some(worktree), None) if !worktree.checkout.exists() => {
            message.push_str(&format!(
                "; its worktree {} was gone already",
                worktree.path()
            ));
        }
        (Some(worktree), None) => {
            message.push_str(&format!("; its worktree {} stays", worktree.path()));
        }
        (None, None) => {}
    }
    if let Some(branch) = worktree
        .as_ref()
        .and_then(|worktree| worktree.branch.as_ref())
    {
        message.push_str(&format!("; branch {branch} stays"));
    }
    if shared > 0 {
        message.push_str(&format!(
            "; its space stays, with {shared} other terminal(s) in it"
        ));
    }
    CloseOutcome {
        result: "closed",
        message,
        reason: None,
        dirty: Vec::new(),
        worktree: worktree.as_ref().map(Worktree::path),
        branch: worktree.and_then(|worktree| worktree.branch),
        removed,
    }
}

/// The worker's checkout when it is a linked worktree of the repository at
/// `root` (never the primary checkout), with its branch. A worker in a
/// folder that is no worktree of the project has none: only its terminal
/// closes.
fn linked_worktree(agent: &Agent, root: &str) -> Result<Option<Worktree>> {
    let Some(checkout) = agent.worktree_checkout.as_deref() else {
        return Ok(None);
    };
    let listed = git_output(Path::new(root), &["worktree", "list", "--porcelain"])?;
    let same = |path: &Path| {
        let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        canonical(path) == canonical(Path::new(checkout))
    };
    // The first entry is the primary checkout.
    let linked = listed
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .skip(1)
        .any(|path| same(Path::new(path)));
    if !linked {
        return Ok(None);
    }
    let checkout = PathBuf::from(checkout);
    let branch = git_current_branch(&checkout).ok();
    Ok(Some(Worktree { checkout, branch }))
}

/// The checkout's uncommitted and untracked changes, one line each.
fn changes(checkout: &Path) -> Result<Vec<String>> {
    let listed = git_output(
        checkout,
        &["status", "--porcelain", "--untracked-files=normal"],
    )?;
    Ok(listed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// The first few of `changes`, as a message names them.
fn named_changes(changes: &[String]) -> String {
    const NAMED: usize = 5;
    let mut named = changes[..changes.len().min(NAMED)].join(", ");
    if changes.len() > NAMED {
        named.push_str(&format!(" and {} more", changes.len() - NAMED));
    }
    named
}

/// Ends the program of the terminal at `address` (a kill stays final, even
/// for a kept terminal), waits for it to finish, and removes it. A terminal
/// that is gone already is fine.
fn end_terminal(client: &HcClient, address: &str) -> Result<()> {
    let Some(terminal) = client.find(address)? else {
        return Ok(());
    };
    if terminal.is_live() {
        client.kill(address)?;
        let deadline = Instant::now() + FINISH_TIMEOUT;
        loop {
            match client.find(address)? {
                None => return Ok(()),
                Some(terminal) if !terminal.is_live() => break,
                Some(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(100)),
                Some(_) => anyhow::bail!(
                    "terminal {address} did not finish within {} seconds of hc kill",
                    FINISH_TIMEOUT.as_secs()
                ),
            }
        }
    }
    client.remove(address)
}

/// How many other terminals of `agents` show in the worker's space, which
/// the app then keeps. Corgi's agents are read from the rows; any other
/// terminal of the space (the person's shell) is asked of hc. It is only
/// said, never acted on.
fn shared_space(client: &HcClient, agent: &Agent, agents: &[Agent]) -> usize {
    let space = agent.info.space.as_str();
    if space.is_empty() {
        return 0;
    }
    match client.list(&[(SPACE_LABEL, space)]) {
        Ok(terminals) => terminals
            .iter()
            // hc filtered by the label; a fake of it in a test does not.
            .filter(|terminal| {
                terminal.label(SPACE_LABEL) == Some(space) && terminal.address != agent.info.address
            })
            .count(),
        Err(_) => agents
            .iter()
            .filter(|other| other.info.space == space && other.info.address != agent.info.address)
            .count(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::{
        hc::tests::FakeHc,
        model::AgentInfo,
        test_support::{ScratchDir, git, git_repo, git_worktree},
    };

    /// A project with a worker `w-fix` in a linked worktree on
    /// `worktree/fix`, its terminal `t5` in the space `sp-fix`, and the
    /// project's supervisor `t1`, on a fake `hc` that has `t5` live until
    /// it is killed.
    struct Project {
        _scratch: ScratchDir,
        repo: PathBuf,
        worktree: PathBuf,
        hc: FakeHc,
        agents: Vec<Agent>,
    }

    fn row(name: &str, address: &str, space: &str, root: &Path, cwd: &Path) -> Agent {
        let checkout = (cwd != root).then(|| cwd.to_string_lossy().into_owned());
        Agent {
            info: AgentInfo {
                address: address.into(),
                name: Some(name.into()),
                space: space.into(),
                cwd: Some(cwd.to_string_lossy().into_owned()),
                ..AgentInfo::default()
            },
            project_root: root.to_string_lossy().into_owned(),
            worktree_checkout: checkout,
            ..Agent::default()
        }
    }

    impl Project {
        fn new(label: &str) -> Self {
            let scratch = ScratchDir::new(label);
            let repo = git_repo(&scratch.join("project"));
            let worktree = git_worktree(&repo, &scratch.join("worktree-fix"), "worktree/fix");
            fs::write(worktree.join("fix.txt"), "fixed\n").unwrap();
            git(&worktree, &["add", "fix.txt"]);
            git(&worktree, &["commit", "-q", "-m", "the fix"]);
            let mut supervisor = row("app-supervisor", "t1", "sp-app", &repo, &repo);
            supervisor.supervisor = true;
            let worker = row("w-fix", "t5", "sp-app-1a2b", &repo, &worktree);
            let hc = FakeHc::new();
            // Live at the first look, finished once killed.
            hc.answer("get.1", r#"{"address":"t5","state":"live"}"#);
            hc.answer("get.t5", r#"{"address":"t5","state":"finished"}"#);
            hc.answer("ls", r#"{"terminals":[{"address":"t5"}]}"#);
            Self {
                _scratch: scratch,
                repo,
                worktree,
                hc,
                agents: vec![supervisor, worker],
            }
        }

        fn root(&self) -> String {
            self.repo.to_string_lossy().into_owned()
        }

        fn close(&self, name: &str, options: CloseOptions) -> CloseOutcome {
            close_worker(
                &self.hc.client(),
                &self.agents,
                name,
                &self.root(),
                None,
                options,
            )
        }

        /// The calls that end or remove a terminal.
        fn ends(&self) -> Vec<String> {
            self.hc
                .calls()
                .into_iter()
                .filter(|call| call.starts_with("kill|") || call.starts_with("rm|"))
                .collect()
        }

        fn branch_stays(&self) {
            git(&self.repo, &["rev-parse", "--verify", "-q", "worktree/fix"]);
        }
    }

    #[test]
    fn a_clean_worker_is_ended_removed_and_its_worktree_removed_keeping_its_branch() {
        let project = Project::new("hc-close-clean");
        let outcome = project.close("w-fix", CloseOptions::default());
        let path = project.worktree.to_string_lossy().into_owned();
        assert_eq!(
            outcome,
            CloseOutcome {
                result: "closed",
                message: format!(
                    "Closed w-fix and removed its worktree {path}; branch worktree/fix stays"
                ),
                reason: None,
                dirty: Vec::new(),
                worktree: Some(path.clone()),
                branch: Some("worktree/fix".into()),
                removed: true,
            }
        );
        assert_eq!(project.ends(), ["kill|t5|--json", "rm|t5|--json"]);
        assert!(!project.worktree.exists());
        project.branch_stays();
        assert_eq!(
            outcome.json(),
            json!({"result": "closed", "message": outcome.message, "reason": null,
                   "dirty": [], "worktree": path, "branch": "worktree/fix", "removed": true})
        );
    }

    #[test]
    fn a_dirty_worktree_is_refused_with_its_changes_and_closed_when_forced() {
        let project = Project::new("hc-close-dirty");
        fs::write(project.worktree.join("fix.txt"), "edited\n").unwrap();
        fs::write(project.worktree.join("draft.txt"), "unsaved\n").unwrap();
        let outcome = project.close("w-fix", CloseOptions::default());
        assert_eq!((outcome.result, outcome.reason), ("refused", Some("dirty")));
        assert_eq!(outcome.dirty, ["M fix.txt", "?? draft.txt"]);
        assert_eq!(
            outcome.message,
            "The worktree of w-fix has uncommitted or untracked changes: M fix.txt, ?? draft.txt"
        );
        assert_eq!(outcome.branch.as_deref(), Some("worktree/fix"));
        assert!(project.hc.calls().is_empty(), "nothing asked of hc yet");
        assert!(project.worktree.join("draft.txt").exists());

        let outcome = project.close(
            "w-fix",
            CloseOptions {
                force: true,
                ..CloseOptions::default()
            },
        );
        assert_eq!(outcome.result, "closed", "{}", outcome.message);
        assert!(outcome.removed);
        assert_eq!(project.ends(), ["kill|t5|--json", "rm|t5|--json"]);
        assert!(!project.worktree.exists());
        project.branch_stays();
    }

    #[test]
    fn the_worktree_stays_when_asked_and_a_finished_terminal_is_only_removed() {
        let project = Project::new("hc-close-keep");
        project
            .hc
            .answer("get.1", r#"{"address":"t5","state":"finished"}"#);
        // Changes do not matter to a worktree that stays.
        fs::write(project.worktree.join("draft.txt"), "unsaved\n").unwrap();
        let outcome = project.close(
            "w-fix",
            CloseOptions {
                keep_worktree: true,
                ..CloseOptions::default()
            },
        );
        assert_eq!(outcome.result, "closed", "{}", outcome.message);
        assert!(!outcome.removed);
        assert_eq!(
            outcome.message,
            format!(
                "Closed w-fix; its worktree {} stays; branch worktree/fix stays",
                project.worktree.display()
            )
        );
        assert_eq!(project.ends(), ["rm|t5|--json"]);
        assert!(project.worktree.join("draft.txt").exists());
    }

    #[test]
    fn a_supervisor_an_unknown_name_and_the_callers_own_terminal_are_refused() {
        let project = Project::new("hc-close-refused");
        let outcome = project.close("app-supervisor", CloseOptions::default());
        assert_eq!(
            (outcome.result, outcome.reason),
            ("refused", Some("supervisor"))
        );
        // A terminal without Corgi's labels has no row: not Corgi's to close.
        let outcome = project.close("by-hand", CloseOptions::default());
        assert_eq!(
            (outcome.result, outcome.reason),
            ("refused", Some("no-agent"))
        );
        let outcome = close_worker(
            &project.hc.client(),
            &project.agents,
            "w-fix",
            &project.root(),
            Some("t5"),
            CloseOptions::default(),
        );
        assert_eq!((outcome.result, outcome.reason), ("refused", Some("self")));
        assert!(project.hc.calls().is_empty());
        assert!(project.worktree.exists());
    }

    #[test]
    fn a_space_shared_with_another_terminal_is_left_alone_and_said() {
        let project = Project::new("hc-close-shared");
        // The person opened a shell of their own in the worker's space.
        project
            .hc
            .answer("ls", r#"{"terminals":[{"address":"t5"},{"address":"t9"}]}"#);
        project.hc.answer(
            "get.t9",
            r#"{"address":"t9","state":"live","labels":{"herdr.space":"sp-app-1a2b"}}"#,
        );
        let outcome = project.close("w-fix", CloseOptions::default());
        assert_eq!(outcome.result, "closed", "{}", outcome.message);
        assert!(
            outcome
                .message
                .ends_with("; its space stays, with 1 other terminal(s) in it"),
            "{}",
            outcome.message
        );
        // Only the worker's own terminal is ended, by its address.
        assert_eq!(project.ends(), ["kill|t5|--json", "rm|t5|--json"]);
    }

    #[test]
    fn a_failed_kill_leaves_the_worktree() {
        let project = Project::new("hc-close-kill-failed");
        project.hc.fail(
            "kill",
            9,
            r#"{"error":{"kind":"blocked","message":"refused","hint":""}}"#,
        );
        let outcome = project.close("w-fix", CloseOptions::default());
        assert_eq!(outcome.result, "failed");
        assert!(
            outcome.message.contains("hc: refused"),
            "{}",
            outcome.message
        );
        assert!(project.worktree.exists());
        assert!(!project.ends().iter().any(|call| call.starts_with("rm|")));
    }

    #[test]
    fn a_worker_outside_any_worktree_closes_its_terminal_alone() {
        let project = Project::new("hc-close-folder");
        let elsewhere = project._scratch.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        let mut agents = project.agents.clone();
        agents[1] = row("w-fix", "t5", "sp-app", &project.repo, &elsewhere);
        let outcome = close_worker(
            &project.hc.client(),
            &agents,
            "w-fix",
            &project.root(),
            None,
            CloseOptions::default(),
        );
        assert_eq!(outcome.result, "closed", "{}", outcome.message);
        assert_eq!(outcome.message, "Closed w-fix");
        assert_eq!(outcome.worktree, None);
        assert!(elsewhere.exists());
    }
}
