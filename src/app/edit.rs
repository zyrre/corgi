//! Opening an agent's checkout in the user's editor: the dashboard's `e` key
//! and `corgi edit`, which the plugin's `edit` action runs. Both end in
//! [`open_checkout`], so the two open the same directory the same way.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::{
    git::{git_current_branch, git_output},
    herdr::{HerdrClient, PaneInfo},
    paths::{dir_name, socket_state_file},
};

use super::App;

pub const EDIT_USAGE: &str = "\
Usage: corgi edit

Opens the checkout of the agent in Herdr's focused pane in your editor
($VISUAL, else $EDITOR, else vim), in a new tab of that agent's workspace
named after its branch. When the focused pane is a Corgi dashboard, the agent
selected there is the one opened. The plugin's edit action runs this; bind it
to a key in Herdr's config.toml.";

/// The editor command typed into the new tab's shell. It is expanded by
/// `sh`, not by the user's shell, so it works the same from bash, zsh or
/// fish; `sh` inherits the user's shell's environment, so an `EDITOR` set in
/// its config applies without restarting Herdr. Left unquoted, the variable
/// splits into words, so an editor with arguments such as `code --wait`
/// works, as it does for Git.
const EDITOR_SCRIPT: &str = r#"exec ${VISUAL:-${EDITOR:-vim}} "$1""#;

/// The directory an agent working in `cwd` changes: the top of its Git work
/// tree, uncommitted changes and all, or `cwd` itself outside one.
pub(crate) fn checkout_root(cwd: &Path) -> PathBuf {
    git_output(cwd, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(|top| top.trim_end_matches('\n').to_string())
        .filter(|top| !top.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.to_path_buf())
}

/// The editor tab's label: the checkout's branch, or its directory's name
/// when it has none (a detached HEAD, or no Git at all).
pub(crate) fn tab_label(root: &Path) -> String {
    git_current_branch(root)
        .ok()
        .or_else(|| dir_name(root).map(str::to_string))
        .unwrap_or_else(|| root.display().to_string())
}

/// The shell command line that opens `root` in the user's editor.
pub(crate) fn editor_command(root: &Path) -> String {
    format!(
        "sh -c {} corgi-edit {}",
        shell_quote(EDITOR_SCRIPT),
        shell_quote(&root.to_string_lossy())
    )
}

/// `text` as one single-quoted shell word. bash, zsh and fish all read
/// `'\''` as a quote inside it.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Opens the checkout of `cwd` in the user's editor in a new tab of
/// `workspace_id`, and goes to it. Quitting the editor leaves the tab's shell
/// in the checkout. Returns the tab's label.
pub(crate) fn open_checkout(client: &HerdrClient, workspace_id: &str, cwd: &str) -> Result<String> {
    if cwd.is_empty() {
        bail!("Herdr does not know the agent's working directory");
    }
    let root = checkout_root(Path::new(cwd));
    let label = tab_label(&root);
    let tab = client.create_tab(workspace_id, Some(&root), Some(&label))?;
    if let Err(error) = client.run_in_pane(&tab.root_pane_id, &editor_command(&root)) {
        // A tab without its editor is only in the way.
        let _ = client.close_tab(&tab.tab_id);
        return Err(error);
    }
    client.walk_to(workspace_id, &tab.tab_id)?;
    Ok(label)
}

/// Where the dashboard running as process `pid` keeps the pane ID of the
/// agent it has selected, empty when none is, for `corgi edit` to find.
pub(crate) fn selection_file(socket: &Path, pid: u32) -> Option<PathBuf> {
    socket_state_file("selection", socket, &pid.to_string())
}

/// Writes `pane_id` to `path` through a temporary file renamed over it, so
/// `corgi edit` never reads half of it.
pub(crate) fn write_selection(path: &Path, pane_id: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    fs::write(&temporary, pane_id)?;
    fs::rename(&temporary, path)
}

impl App {
    /// The dashboard's `e`: opens the selected agent's checkout in the
    /// user's editor.
    pub(super) fn edit_selected(&mut self) {
        let Some(info) = self.selected_agent().map(|agent| agent.info.clone()) else {
            self.status = "No agent selected".into();
            return;
        };
        match open_checkout(&self.client, &info.workspace_id, info.cwd()) {
            Ok(label) => self.status = format!("Opened {label} in the editor"),
            Err(error) => self.status = format!("Edit failed: {error:#}"),
        }
    }

    /// Writes the selected agent's pane to this dashboard's selection file
    /// whenever it changes, by a key or by a refresh that moved the rows.
    pub(super) fn share_selection(&mut self) {
        let Some(path) = &self.selection_file else {
            return;
        };
        let selected = self
            .selected_agent()
            .map_or("", |agent| agent.info.pane_id.as_str());
        if self.shared_selection.as_deref() == Some(selected) {
            return;
        }
        // A failed write is not retried until the selection changes again,
        // rather than on every frame; `corgi edit` then says what it found.
        let _ = write_selection(path, selected);
        self.shared_selection = Some(selected.to_string());
    }

    /// Removes this dashboard's selection file as it quits.
    pub(super) fn unshare_selection(&self) {
        if let Some(path) = &self.selection_file {
            let _ = fs::remove_file(path);
        }
    }
}

/// `corgi edit`, the plugin's `edit` action. It has no window of its own, so
/// a failure is also shown as a Herdr notification.
pub fn edit_command(args: &[String]) -> Result<()> {
    if let Some(arg) = args.first() {
        bail!("corgi edit takes no arguments, got {arg}\n\n{EDIT_USAGE}");
    }
    let client = HerdrClient::from_env()?;
    let opened = edit_target(&client)
        .and_then(|pane| open_checkout(&client, &pane.workspace_id, pane_cwd(&pane)));
    match opened {
        Ok(label) => {
            println!("opened {label} in the editor");
            Ok(())
        }
        Err(error) => {
            let _ = client.notify("Corgi: cannot open the editor", &format!("{error:#}"));
            Err(error)
        }
    }
}

/// The pane whose checkout `corgi edit` opens: the focused one, or, when that
/// is a Corgi dashboard, the agent selected in it. A dashboard is told apart
/// by the selection file of a process in its pane's foreground.
fn edit_target(client: &HerdrClient) -> Result<PaneInfo> {
    let focused = client
        .focused_pane()
        .context("cannot ask Herdr for the focused pane")?;
    let processes = client
        .foreground_processes(&focused.pane_id)
        .unwrap_or_default();
    let selection = processes.iter().find_map(|(pid, _)| {
        selection_file(client.socket_path(), *pid).and_then(|path| fs::read_to_string(path).ok())
    });
    match selection.as_deref().map(str::trim) {
        Some("") => bail!("No agent is selected in Corgi"),
        Some(pane_id) => client
            .pane(pane_id)
            .context("the agent selected in Corgi is gone"),
        None if processes.iter().any(|(_, name)| name == "corgi") => {
            bail!("This Corgi dashboard does not share its selection; reopen it")
        }
        None => Ok(focused),
    }
}

/// Where the shell or agent in `pane` works now.
fn pane_cwd(pane: &PaneInfo) -> &str {
    pane.foreground_cwd
        .as_deref()
        .or(pane.cwd.as_deref())
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;
    use crate::test_support::ScratchDir;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?}");
    }

    /// A repository with one commit on `branch`, in a directory whose name
    /// has a space, canonical so it compares with what Git reports.
    fn repository(scratch: &ScratchDir, branch: &str) -> PathBuf {
        let root = scratch.join("my repo");
        fs::create_dir_all(root.join("src")).unwrap();
        let root = root.canonicalize().unwrap();
        git(&root, &["init", "-q", "-b", branch]);
        git(
            &root,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "first",
            ],
        );
        root
    }

    #[test]
    fn the_checkout_is_the_top_of_the_work_tree_else_the_directory_itself() {
        let scratch = ScratchDir::new("edit-root");
        let root = repository(&scratch, "worktree/brave-fox");
        assert_eq!(checkout_root(&root.join("src")), root);
        assert_eq!(checkout_root(&root), root);

        let plain = scratch.join("plain dir");
        fs::create_dir_all(&plain).unwrap();
        assert_eq!(checkout_root(&plain), plain);
    }

    #[test]
    fn the_tab_is_named_after_the_branch_else_the_directory() {
        let scratch = ScratchDir::new("edit-label");
        let root = repository(&scratch, "worktree/brave-fox");
        assert_eq!(tab_label(&root), "worktree/brave-fox");

        git(&root, &["checkout", "-q", "--detach"]);
        assert_eq!(tab_label(&root), "my repo");

        let plain = scratch.join("plain dir");
        fs::create_dir_all(&plain).unwrap();
        assert_eq!(tab_label(&plain), "plain dir");
    }

    #[test]
    fn the_editor_command_quotes_the_path_as_one_word() {
        assert_eq!(
            editor_command(Path::new("/w/my repo")),
            r#"sh -c 'exec ${VISUAL:-${EDITOR:-vim}} "$1"' corgi-edit '/w/my repo'"#
        );
        assert_eq!(
            editor_command(Path::new("/w/it's")),
            r#"sh -c 'exec ${VISUAL:-${EDITOR:-vim}} "$1"' corgi-edit '/w/it'\''s'"#
        );
    }

    /// What the editor command runs, typed into a POSIX shell with `env`
    /// set: the program and arguments it ends in, one per line.
    fn editor_argv(root: &Path, env: &[(&str, &str)]) -> Vec<String> {
        let scratch = ScratchDir::new("edit-argv");
        // A stand-in editor that prints its arguments.
        let bin = scratch.join("bin");
        fs::create_dir_all(&bin).unwrap();
        for name in ["vim", "hx", "code"] {
            let path = bin.join(name);
            fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' {name} \"$@\"\n")).unwrap();
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
            fs::set_permissions(&path, permissions).unwrap();
        }
        let mut shell = Command::new("/bin/sh");
        shell
            .arg("-c")
            .arg(editor_command(root))
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
        for (key, value) in env {
            shell.env(key, value);
        }
        let output = shell.output().expect("run the editor command");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn the_editor_is_visual_else_editor_else_vim_and_gets_the_path_whole() {
        let root = Path::new("/w/my repo/it's");
        assert_eq!(editor_argv(root, &[]), ["vim", "/w/my repo/it's"]);
        assert_eq!(
            editor_argv(root, &[("EDITOR", "hx")]),
            ["hx", "/w/my repo/it's"]
        );
        assert_eq!(
            editor_argv(root, &[("EDITOR", "hx"), ("VISUAL", "code --wait")]),
            ["code", "--wait", "/w/my repo/it's"]
        );
    }

    #[test]
    fn the_selection_file_holds_the_selected_pane_until_the_dashboard_quits() {
        let scratch = ScratchDir::new("edit-selection");
        let mut app = crate::test_support::test_app();
        let path = scratch.join("selection").join("socket.42");
        app.selection_file = Some(path.clone());

        app.share_selection();
        assert_eq!(fs::read_to_string(&path).unwrap(), "");

        app.agents = vec![crate::model::DashboardAgent {
            info: crate::model::AgentInfo {
                pane_id: "w2:p3".into(),
                ..Default::default()
            },
            ..Default::default()
        }];
        app.share_selection();
        assert_eq!(fs::read_to_string(&path).unwrap(), "w2:p3");

        app.unshare_selection();
        assert!(!path.exists());
    }
}
