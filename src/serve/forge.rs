//! Each project's open merge requests (GitLab) or pull requests (GitHub),
//! asked of its forge as the user, with `glab mr list` or `gh pr list`,
//! and kept when their source branch is one of the project's: a linked
//! worktree's, or one its supervisor's ledger names. The forge is told from
//! the `origin` remote's host. Nothing here runs on the snapshot path: the
//! server asks on a thread of its own every few minutes and keeps the last
//! answer per project.

use std::{
    collections::HashSet,
    env, fs,
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use corgi_core::{
    digest::lookup::{LEDGER_ARCHIVE_FILE, LEDGER_FILE},
    git::git_output,
    paths::home,
    protocol::MergeRequest,
    waker::Inboxes,
};
use serde::Deserialize;

/// How often the forges are asked again.
pub(super) const FORGE_INTERVAL: Duration = Duration::from_secs(180);
/// The longest one CLI call may take before it is killed.
const CLI_TIMEOUT: Duration = Duration::from_secs(30);
/// Set to log, once each, why a project's forge is not asked.
const DEBUG_VAR: &str = "CORGI_SERVE_DEBUG";

/// Runs `program` with `args` in a directory: its stdout, or why it failed.
pub(crate) type Runner = Arc<dyn Fn(&str, &[&str], &Path) -> Result<String, String> + Send + Sync>;

/// How the server asks the forges, so a test answers with canned output.
#[derive(Clone)]
pub(crate) struct ForgeCli {
    pub(crate) run: Runner,
    /// The hosts `glab` is configured for, which count as GitLab whatever
    /// their name.
    pub(crate) glab_hosts: fn() -> Vec<String>,
    /// Logs to stderr, once each, why a project's forge is not asked.
    pub(crate) debug: bool,
}

impl ForgeCli {
    /// The user's own `glab` and `gh`.
    pub(crate) fn live() -> Self {
        Self {
            run: Arc::new(run_cli),
            glab_hosts,
            debug: env::var_os(DEBUG_VAR).is_some_and(|value| !value.is_empty()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Forge {
    GitLab,
    GitHub,
}

/// The open merge requests of the project at `root` whose source branch is
/// one of its own, or why its forge was not asked.
pub(super) fn open_merge_requests(
    cli: &ForgeCli,
    inboxes: &Inboxes,
    root: &str,
) -> Result<Vec<MergeRequest>, String> {
    let root = Path::new(root);
    let url = git_output(root, &["remote", "get-url", "origin"])
        .map_err(|_| "no origin remote".to_string())?;
    let host = remote_host(&url).ok_or_else(|| format!("origin {} has no host", url.trim()))?;
    let forge = forge_of(&host, &(cli.glab_hosts)())
        .ok_or_else(|| format!("origin's host {host} is neither GitLab nor GitHub"))?;
    let mut branches = worktree_branches(root);
    if let Some(dir) = inboxes.dir(&root.to_string_lossy()) {
        for file in [LEDGER_FILE, LEDGER_ARCHIVE_FILE] {
            if let Ok(text) = fs::read_to_string(dir.join(file)) {
                branches.extend(ledger_branches(&text));
            }
        }
    }
    // No branch to match: the forge need not be asked.
    if branches.is_empty() {
        return Ok(Vec::new());
    }
    let (program, args): (&str, &[&str]) = match forge {
        Forge::GitLab => (
            "glab",
            &["mr", "list", "--output", "json", "--per-page", "100"],
        ),
        Forge::GitHub => (
            "gh",
            &[
                "pr",
                "list",
                "--state",
                "open",
                "--limit",
                "100",
                "--json",
                "number,title,isDraft,url,headRefName",
            ],
        ),
    };
    let output = (cli.run)(program, args, root)?;
    let open = parse(forge, &output)?;
    Ok(open
        .into_iter()
        .filter(|request| branches.contains(&request.branch))
        .collect())
}

/// The forge's list as the protocol's, in the forge's order. A notice the
/// CLI prints on stdout before the list (`glab` warns there of a deprecated
/// setting) is skipped.
pub(super) fn parse(forge: Forge, output: &str) -> Result<Vec<MergeRequest>, String> {
    let json = if output.trim_start().starts_with('[') {
        output
    } else {
        output.find("\n[").map_or(output, |at| &output[at + 1..])
    };
    let unreadable = |error: serde_json::Error| format!("cannot read the forge's list: {error}");
    Ok(match forge {
        Forge::GitLab => serde_json::from_str::<Vec<GlabMergeRequest>>(json)
            .map_err(unreadable)?
            .into_iter()
            .filter(|request| request.state.is_empty() || request.state == "opened")
            .map(|request| MergeRequest {
                id: format!("!{}", request.iid),
                title: request.title,
                branch: request.source_branch,
                draft: request.draft.or(request.work_in_progress).unwrap_or(false),
                url: request.web_url,
            })
            .collect(),
        Forge::GitHub => serde_json::from_str::<Vec<GhPullRequest>>(json)
            .map_err(unreadable)?
            .into_iter()
            .map(|request| MergeRequest {
                id: format!("#{}", request.number),
                title: request.title,
                branch: request.head_ref_name,
                draft: request.is_draft,
                url: request.url,
            })
            .collect(),
    })
}

/// One merge request of `glab mr list --output json`, GitLab's API's.
#[derive(Deserialize)]
struct GlabMergeRequest {
    iid: u64,
    title: String,
    source_branch: String,
    #[serde(default)]
    draft: Option<bool>,
    /// What older GitLabs call a draft.
    #[serde(default)]
    work_in_progress: Option<bool>,
    web_url: String,
    #[serde(default)]
    state: String,
}

/// One pull request of `gh pr list --json …`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPullRequest {
    number: u64,
    title: String,
    head_ref_name: String,
    #[serde(default)]
    is_draft: bool,
    url: String,
}

/// The branches of the linked worktrees of the repository at `root`, its
/// primary checkout left out.
fn worktree_branches(root: &Path) -> HashSet<String> {
    git_output(root, &["worktree", "list", "--porcelain"])
        .map(|list| porcelain_branches(&list))
        .unwrap_or_default()
}

/// The branches `git worktree list --porcelain` names, the first entry's
/// (the primary checkout's) left out.
pub(super) fn porcelain_branches(list: &str) -> HashSet<String> {
    list.split("\n\n")
        .skip(1)
        .flat_map(str::lines)
        .filter_map(|line| line.strip_prefix("branch refs/heads/"))
        .map(str::to_string)
        .collect()
}

/// Every `branch` a ledger's lines name, old lines too.
pub(super) fn ledger_branches(ledger: &str) -> impl Iterator<Item = String> + '_ {
    ledger.lines().filter_map(|line| {
        let entry: serde_json::Value = serde_json::from_str(line).ok()?;
        let branch = entry.get("branch")?.as_str()?.trim();
        let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
        (!branch.is_empty()).then(|| branch.to_string())
    })
}

/// The host of a remote URL, `https://host/…`, `ssh://user@host:22/…` or
/// `user@host:path`, lowercase; `None` for a local path.
pub(super) fn remote_host(url: &str) -> Option<String> {
    let url = url.trim();
    let authority = match url.split_once("://") {
        Some((_, rest)) => rest.split('/').next()?,
        None => {
            let (authority, _) = url.split_once(':')?;
            if authority.contains('/') {
                return None;
            }
            authority
        }
    };
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?.to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Which forge `host` is: GitLab when its name says so or `glab` is
/// configured for it (a self-hosted GitLab under any name), GitHub when its
/// name says so.
pub(super) fn forge_of(host: &str, glab_hosts: &[String]) -> Option<Forge> {
    let configured = glab_hosts
        .iter()
        .filter_map(|configured| remote_host(configured).or_else(|| Some(configured.clone())))
        .any(|configured| configured.eq_ignore_ascii_case(host));
    if host.contains("gitlab") || configured {
        Some(Forge::GitLab)
    } else if host.contains("github") {
        Some(Forge::GitHub)
    } else {
        None
    }
}

/// The hosts `glab` is configured for: those of its `config.yml`, wherever
/// it keeps it, and `$GITLAB_HOST`.
fn glab_hosts() -> Vec<String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = env::var_os("GLAB_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        dirs.push(dir.into());
    }
    if let Some(dir) = env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        dirs.push(PathBuf::from(dir).join("glab-cli"));
    }
    if let Some(home) = home() {
        dirs.push(home.join(".config/glab-cli"));
        dirs.push(home.join("Library/Application Support/glab-cli"));
    }
    let mut hosts: Vec<String> = dirs
        .iter()
        .filter_map(|dir| fs::read_to_string(dir.join("config.yml")).ok())
        .flat_map(|config| glab_config_hosts(&config))
        .collect();
    hosts.extend(env::var("GITLAB_HOST").ok().filter(|host| !host.is_empty()));
    hosts
}

/// The hosts under `hosts:` in `glab`'s `config.yml`, with their
/// `api_host` and `ssh_host` when set. Only that much YAML is read.
pub(super) fn glab_config_hosts(config: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    let mut in_hosts = false;
    let mut host_indent = None;
    for line in config.lines() {
        let text = line.trim();
        if text.is_empty() || text.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 {
            in_hosts = text == "hosts:";
            continue;
        }
        if !in_hosts {
            continue;
        }
        let unquote = |text: &str| text.trim().trim_matches(['"', '\'']).to_string();
        if *host_indent.get_or_insert(indent) == indent {
            if let Some(host) = text.strip_suffix(':') {
                hosts.push(unquote(host));
            }
        } else if let Some((key, value)) = text.split_once(':')
            && matches!(key.trim(), "api_host" | "ssh_host")
            && !unquote(value).is_empty()
        {
            hosts.push(unquote(value));
        }
    }
    hosts
}

/// Runs `program` in `cwd` with no prompt and no terminal, reading its
/// output as it comes (a forge's list outgrows a pipe's buffer), and kills
/// it after [`CLI_TIMEOUT`].
fn run_cli(program: &str, args: &[&str], cwd: &Path) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("GLAB_NO_PROMPT", "true")
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| match error.kind() {
            ErrorKind::NotFound => format!("{program} is not installed"),
            _ => format!("cannot run {program}: {error}"),
        })?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let stdout = drain(child.stdout.take().map(|pipe| Box::new(pipe) as _));
    let stderr = drain(child.stderr.take().map(|pipe| Box::new(pipe) as _));
    let deadline = Instant::now() + CLI_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} took longer than {}s",
                    CLI_TIMEOUT.as_secs()
                ));
            }
            Err(error) => return Err(format!("cannot wait for {program}: {error}")),
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    if status.success() {
        return Ok(stdout);
    }
    let stderr = stderr.join().unwrap_or_default();
    let reason = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no message");
    Err(format!(
        "{program} {} failed ({status}): {reason}",
        args[..2].join(" ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLAB: &str = r#"[
      {"iid":1715,"title":"relaypool: reject ICMP replies","draft":false,"work_in_progress":null,
       "source_branch":"relay-icmp","state":"opened","web_url":"https://git.example.com/g/r/-/merge_requests/1715",
       "author":{"username":"someone"},"labels":[]},
      {"iid":1713,"title":"Draft: weathermap neighbours","draft":true,
       "source_branch":"worktree/green-forest-025b","state":"opened","web_url":"https://git.example.com/g/r/-/merge_requests/1713"},
      {"iid":12,"title":"Old WIP","work_in_progress":true,
       "source_branch":"old","web_url":"https://git.example.com/g/r/-/merge_requests/12"},
      {"iid":11,"title":"Merged already","draft":false,
       "source_branch":"done","state":"merged","web_url":"https://git.example.com/g/r/-/merge_requests/11"}
    ]"#;

    const GH: &str = r#"[
      {"headRefName":"worktree/calm-harbor-4ea1","isDraft":false,"number":12,
       "title":"GUI: side panel","url":"https://github.com/o/r/pull/12"},
      {"headRefName":"feature","isDraft":true,"number":9,
       "title":"Someone's","url":"https://github.com/o/r/pull/9"}
    ]"#;

    fn request(id: &str, title: &str, branch: &str, draft: bool, url: &str) -> MergeRequest {
        MergeRequest {
            id: id.into(),
            title: title.into(),
            branch: branch.into(),
            draft,
            url: url.into(),
        }
    }

    /// Asks the real forge of the repository `CORGI_LIVE_FORGE_ROOT` names,
    /// as the user, reading its supervisor's ledger in Corgi's own state:
    /// `CORGI_LIVE_FORGE_ROOT=<repo> cargo test -- --ignored live_forge --nocapture`.
    /// It only lists; nothing is written anywhere.
    #[test]
    #[ignore = "asks the user's real forge"]
    fn live_forge() {
        let Some(root) = env::var_os("CORGI_LIVE_FORGE_ROOT") else {
            return;
        };
        let found = open_merge_requests(
            &ForgeCli::live(),
            &Inboxes::default(),
            &root.to_string_lossy(),
        );
        eprintln!("{found:#?}");
        assert!(found.is_ok(), "{found:?}");
    }

    #[test]
    fn glab_and_gh_lists_are_read_with_their_handles_and_drafts() {
        assert_eq!(
            parse(Forge::GitLab, GLAB).unwrap(),
            [
                request(
                    "!1715",
                    "relaypool: reject ICMP replies",
                    "relay-icmp",
                    false,
                    "https://git.example.com/g/r/-/merge_requests/1715"
                ),
                request(
                    "!1713",
                    "Draft: weathermap neighbours",
                    "worktree/green-forest-025b",
                    true,
                    "https://git.example.com/g/r/-/merge_requests/1713"
                ),
                request(
                    "!12",
                    "Old WIP",
                    "old",
                    true,
                    "https://git.example.com/g/r/-/merge_requests/12"
                ),
            ]
        );
        assert_eq!(
            parse(Forge::GitHub, GH).unwrap(),
            [
                request(
                    "#12",
                    "GUI: side panel",
                    "worktree/calm-harbor-4ea1",
                    false,
                    "https://github.com/o/r/pull/12"
                ),
                request(
                    "#9",
                    "Someone's",
                    "feature",
                    true,
                    "https://github.com/o/r/pull/9"
                ),
            ]
        );
        assert!(parse(Forge::GitLab, "Not logged in").is_err());
        let warned = format!("DEPRECATION WARNING: use GLAB_NO_PROMPT instead.\n{GLAB}");
        assert_eq!(parse(Forge::GitLab, &warned).unwrap().len(), 3);
        assert_eq!(parse(Forge::GitHub, "[]").unwrap(), []);
    }

    #[test]
    fn a_remote_url_is_its_host_and_the_host_its_forge() {
        for (url, host) in [
            (
                "ssh://git@git.int.example.com:7999/team/repo.git",
                "git.int.example.com",
            ),
            ("git@github.com:o/r.git", "github.com"),
            (
                "https://user@GitLab.Example.com/team/repo.git\n",
                "gitlab.example.com",
            ),
            ("http://gitlab.local:8080/team/repo", "gitlab.local"),
        ] {
            assert_eq!(remote_host(url).as_deref(), Some(host), "{url}");
        }
        assert_eq!(remote_host("/srv/git/repo.git"), None);
        assert_eq!(remote_host("../repo"), None);

        let none: &[String] = &[];
        assert_eq!(forge_of("gitlab.company.com", none), Some(Forge::GitLab));
        assert_eq!(forge_of("github.com", none), Some(Forge::GitHub));
        assert_eq!(forge_of("git.int.example.com", none), None);
        assert_eq!(forge_of("bitbucket.org", none), None);
        let configured = ["gitlab.com".to_string(), "Git.Int.Example.com".to_string()];
        assert_eq!(
            forge_of("git.int.example.com", &configured),
            Some(Forge::GitLab)
        );
        let as_url = ["https://git.int.example.com".to_string()];
        assert_eq!(
            forge_of("git.int.example.com", &as_url),
            Some(Forge::GitLab)
        );
    }

    #[test]
    fn glabs_hosts_are_the_keys_under_hosts_with_their_api_and_ssh_hosts() {
        let config = "\
git_protocol: ssh
host: gitlab.com
hosts:
    gitlab.com:
        api_protocol: https
        api_host: gitlab.com
        ssh_host:
        token: secret
    git.int.example.com:
        token: secret
    \"code.example.org\":
        ssh_host: ssh.code.example.org
aliases:
    ci: pipeline ci
";
        assert_eq!(
            glab_config_hosts(config),
            [
                "gitlab.com",
                "gitlab.com",
                "git.int.example.com",
                "code.example.org",
                "ssh.code.example.org"
            ]
        );
        assert_eq!(
            glab_config_hosts("host: gitlab.com\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn branches_come_from_linked_worktrees_and_every_ledger_line() {
        let porcelain = "\
worktree /repos/weather
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /home/me/.herdr/worktrees/weather/worktree-green-forest-025b
HEAD 2222222222222222222222222222222222222222
branch refs/heads/worktree/green-forest-025b

worktree /home/me/.herdr/worktrees/weather/detached
HEAD 3333333333333333333333333333333333333333
detached
";
        assert_eq!(
            porcelain_branches(porcelain),
            HashSet::from(["worktree/green-forest-025b".to_string()])
        );
        let ledger = r#"{"id":"a","agent":"w-a","status":"dispatched","branch":"worktree/a"}
{"id":"a","agent":"w-a","status":"reported","branch":"alarm-percent-whole"}
{"id":"b","agent":"w-b","status":"dispatched"}
not json
{"id":"c","branch":""}
{"id":"d","branch":"refs/heads/relay-icmp"}
"#;
        assert_eq!(
            ledger_branches(ledger).collect::<Vec<_>>(),
            ["worktree/a", "alarm-percent-whole", "relay-icmp"]
        );
    }
}
