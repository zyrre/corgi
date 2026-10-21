//! Exact-ID native metadata and prompt-free adapters; no newest-by-directory lookup.
use super::Entry;
use crate::{harness::Harness, paths::home};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File},
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Settings {
    pub model: String,
    pub effort: String,
    pub sandbox: Option<String>,
    pub approval: Option<String>,
    pub permission: Option<String>,
    pub writable: Vec<PathBuf>,
    pub profile: Option<String>,
    pub network: Option<bool>,
    pub unknown: bool,
    pub native_cwd: Option<PathBuf>,
    pub native_pending: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Profile {
    pub kind: String,
    pub id: String,
    pub cwd: PathBuf,
    pub settings: Settings,
}

impl Settings {
    pub fn from_launch(model: &str, effort: &str, extra: &[String]) -> Self {
        let mut settings = Self {
            model: model.into(),
            effort: effort.into(),
            ..Self::default()
        };
        let mut iter = extra.iter();
        while let Some(arg) = iter.next() {
            match (arg.as_str(), iter.next()) {
                ("--sandbox" | "-s", Some(v)) => settings.sandbox = Some(v.clone()),
                ("--ask-for-approval" | "-a", Some(v)) => settings.approval = Some(v.clone()),
                ("--permission-mode", Some(v)) => settings.permission = Some(v.clone()),
                ("--add-dir", Some(v)) => settings.writable.push(PathBuf::from(v)),
                ("--profile", Some(v)) => settings.profile = Some(v.clone()),
                _ => settings.unknown = true,
            }
        }
        settings
    }
}

pub(crate) fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

pub(crate) fn store_root(kind: &str) -> Result<PathBuf> {
    let (var, folder) = match kind {
        "codex" => ("CODEX_HOME", ".codex"),
        "claude" => ("CLAUDE_CONFIG_DIR", ".claude"),
        _ => bail!("unsupported harness"),
    };
    let path = std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(folder)))
        .context("native store unavailable")?;
    path.canonicalize().context("native store missing")
}

/// Lists filenames, never unrelated conversation contents. A bounded directory
/// walk is needed for dated Codex rollouts and Claude's project directory names.
pub(crate) fn locate(root: &Path, kind: &str, id: &str) -> Result<PathBuf> {
    ensure!(valid_id(id), "invalid native conversation ID");
    let directory = root.join(if kind == "codex" {
        "sessions"
    } else {
        "projects"
    });
    let mut pending = vec![(directory, 0)];
    let mut count = 0;
    let mut matches = Vec::new();
    while let Some((dir, depth)) = pending.pop() {
        for item in fs::read_dir(&dir).with_context(|| "native conversation store unavailable")? {
            count += 1;
            ensure!(count <= 100_000, "native store lookup limit reached");
            let item = item?;
            let typ = item.file_type()?;
            if typ.is_dir() && depth < 3 {
                pending.push((item.path(), depth + 1));
            } else if typ.is_file() {
                let name = item.file_name().to_string_lossy().into_owned();
                let exact = if kind == "codex" {
                    name.ends_with(&format!("-{id}.jsonl"))
                } else {
                    name == format!("{id}.jsonl")
                };
                if exact {
                    matches.push(item.path());
                }
            }
        }
    }
    ensure!(
        matches.len() == 1,
        "native conversation missing or ambiguous"
    );
    Ok(matches.remove(0))
}

pub(crate) fn exact_file(
    root: &Path,
    kind: &str,
    id: &str,
    hint: Option<&Path>,
) -> Result<PathBuf> {
    ensure!(valid_id(id), "invalid native ID");
    if let Some(hint) = hint.and_then(|p| p.canonicalize().ok()) {
        let directory = root.join(if kind == "codex" {
            "sessions"
        } else {
            "projects"
        });
        let name = hint.file_name().unwrap_or_default().to_string_lossy();
        let matches = if kind == "codex" {
            name.ends_with(&format!("-{id}.jsonl"))
        } else {
            name == format!("{id}.jsonl")
        };
        if hint.starts_with(directory) && matches && hint.is_file() {
            return Ok(hint);
        }
    }
    locate(root, kind, id)
}

pub(crate) fn metadata(path: &Path, kind: &str, id: &str) -> Result<Settings> {
    let mut file = File::open(path)?;
    // Verify identity in the exact record, rather than trust a renamed filename.
    let mut identified = false;
    let mut initial_cwd = None;
    for line in BufReader::new((&mut file).take(64 * 1024)).lines() {
        let first: Value =
            serde_json::from_str(&line?).context("invalid native session metadata")?;
        let actual = if kind == "codex" {
            first["payload"]["id"].as_str()
        } else {
            first["sessionId"].as_str()
        };
        if let Some(actual) = actual {
            ensure!(actual == id, "native session record identity differs");
            identified = true;
            initial_cwd = if kind == "codex" {
                first["payload"]["cwd"].as_str()
            } else {
                first["cwd"].as_str()
            }
            .map(PathBuf::from);
            break;
        }
    }
    ensure!(identified, "native session identity could not be verified");
    let len = file.metadata()?.len();
    let start = len.saturating_sub(2 * 1024 * 1024);
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::new(file.take(2 * 1024 * 1024));
    if start > 0 {
        let mut partial = String::new();
        reader.read_line(&mut partial)?;
    }
    let mut settings = Settings {
        native_cwd: initial_cwd,
        ..Settings::default()
    };
    let mut queued = 0usize;
    for line in reader.lines() {
        let Ok(record) = serde_json::from_str::<Value>(&line?) else {
            continue;
        };
        if kind == "codex" && record["type"] == "event_msg" {
            match record["payload"]["type"].as_str() {
                Some("task_started") => settings.native_pending = true,
                Some("task_complete" | "task_aborted" | "turn_aborted") => {
                    settings.native_pending = false
                }
                _ => {}
            }
        }
        if kind == "claude" && record["type"] == "queue-operation" {
            match record["operation"].as_str() {
                Some("enqueue") => queued += 1,
                Some("dequeue" | "remove") => queued = queued.saturating_sub(1),
                _ => {}
            }
            settings.native_pending = queued > 0;
        }
        // No text/tool fields are retained or returned.
        let meta = if kind == "codex" && record["type"] == "turn_context" {
            &record["payload"]
        } else if kind == "codex"
            && record["type"] == "event_msg"
            && record["payload"]["type"] == "thread_settings_applied"
        {
            &record["payload"]["thread_settings"]
        } else if kind == "claude" && record["type"] == "assistant" {
            &record["message"]
        } else {
            continue;
        };
        if let Some(value) = meta["model"].as_str() {
            settings.model = value.into();
        }
        if let Some(cwd) = meta["cwd"].as_str() {
            settings.native_cwd = Some(PathBuf::from(cwd));
        }
        if kind == "codex" {
            if let Some(value) = meta["approval_policy"].as_str() {
                settings.approval = Some(value.into());
            }
            if let Some(value) = meta["sandbox_policy"]["type"].as_str() {
                settings.sandbox = Some(value.replace('_', "-"));
                settings.network = meta["sandbox_policy"]["network_access"].as_bool();
                if let Some(roots) = meta["sandbox_policy"]["writable_roots"].as_array() {
                    settings.writable = roots
                        .iter()
                        .filter_map(|r| r.as_str().map(PathBuf::from))
                        .collect();
                }
            }
        }
        if let Some(value) = meta["effort"]
            .as_str()
            .or_else(|| meta["reasoning_effort"].as_str())
            .or_else(|| record["effort"].as_str())
            .or_else(|| record["effortLevel"].as_str())
        {
            settings.effort = value.into();
        }
    }
    Ok(settings)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConfigReference {
    pub path: PathBuf,
    pub fingerprint: String,
}

/// Only references and change detectors leave this function, never settings,
/// hook commands, credentials or embedded instructions.
pub(crate) fn configuration(
    root: &Path,
    kind: &str,
    cwd: &Path,
) -> Result<(Vec<ConfigReference>, bool)> {
    let paths = if kind == "codex" {
        vec![root.join("config.toml"), cwd.join(".codex/config.toml")]
    } else {
        vec![
            root.join("settings.json"),
            root.join("settings.local.json"),
            cwd.join(".claude/settings.json"),
            cwd.join(".claude/settings.local.json"),
        ]
    };
    let mut refs = Vec::new();
    let mut hooks = false;
    for path in paths.into_iter().filter(|p| p.is_file()) {
        let mut text = String::new();
        File::open(&path)?
            .take(128 * 1024 + 1)
            .read_to_string(&mut text)?;
        ensure!(
            text.len() <= 128 * 1024,
            "native settings file too large to verify"
        );
        hooks |= if kind == "codex" {
            toml::from_str::<toml::Table>(&text)
                .ok()
                .and_then(|t| t.get("hooks").cloned())
                .is_some()
        } else {
            serde_json::from_str::<Value>(&text)
                .ok()
                .is_some_and(|v| !v["hooks"].is_null())
        };
        refs.push(ConfigReference {
            fingerprint: fingerprint(&path)?,
            path,
        });
    }
    Ok((refs, hooks))
}

pub(crate) fn fingerprint(path: &Path) -> Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(128 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 128 * 1024, "referenced setup file too large");
    // A local change detector, not a signature or a trust boundary. Records
    // and referenced instructions are owned by the same account.
    let mut hash = DefaultHasher::new();
    bytes.hash(&mut hash);
    Ok(format!("{:016x}", hash.finish()))
}

pub(crate) fn resume_args(entry: &Entry) -> Result<Vec<String>> {
    let id = entry
        .native_id
        .as_deref()
        .context("native ID unavailable")?;
    ensure!(valid_id(id), "invalid native conversation ID");
    let harness = Harness::from_kind(&entry.kind);
    let settings = &entry.settings;
    ensure!(
        !settings.model.chars().any(char::is_control),
        "invalid saved model"
    );
    ensure!(
        [
            "", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"
        ]
        .contains(&settings.effort.as_str()),
        "unsupported saved effort"
    );
    if let Some(policy) = &settings.sandbox {
        ensure!(
            entry.kind == "codex"
                && ["read-only", "workspace-write", "danger-full-access"]
                    .contains(&policy.as_str()),
            "unsupported saved sandbox policy"
        );
    }
    if let Some(policy) = &settings.approval {
        ensure!(
            entry.kind == "codex" && ["on-request", "never"].contains(&policy.as_str()),
            "unsupported saved approval policy"
        );
    }
    if let Some(policy) = &settings.permission {
        ensure!(
            entry.kind == "claude"
                && [
                    "default",
                    "plan",
                    "acceptEdits",
                    "auto",
                    "bypassPermissions",
                    "dontAsk"
                ]
                .contains(&policy.as_str()),
            "unsupported saved Claude permission mode"
        );
    }
    if settings.profile.is_some() {
        ensure!(entry.kind == "codex", "saved profile requires Codex");
    }
    for root in &settings.writable {
        ensure!(
            root.is_absolute() && root.is_dir(),
            "saved writable directory is unavailable"
        );
    }
    let mut args = match harness {
        Harness::Codex => vec![
            "resume".into(),
            id.into(),
            "--cd".into(),
            entry.cwd.to_string_lossy().into_owned(),
        ],
        Harness::Claude => vec!["--resume".into(), id.into()],
        _ => bail!("unsupported harness"),
    };
    args.extend(harness.launch_args(&entry.settings.model, &entry.settings.effort));
    if let Some(network) = entry.settings.network {
        ensure!(
            entry.kind == "codex",
            "network sandbox override belongs to Codex"
        );
        args.extend([
            "-c".into(),
            format!("sandbox_workspace_write.network_access={network}"),
        ]);
    }
    for (key, value) in [
        ("--sandbox", &entry.settings.sandbox),
        ("--ask-for-approval", &entry.settings.approval),
        ("--permission-mode", &entry.settings.permission),
        ("--profile", &entry.settings.profile),
    ] {
        if let Some(value) = value {
            args.extend([key.into(), value.clone()]);
        }
    }
    for path in &entry.settings.writable {
        args.extend(["--add-dir".into(), path.to_string_lossy().into_owned()]);
    }
    if let Some(supervisor) = &entry.supervisor {
        ensure!(
            fingerprint(&supervisor.role_file)? == supervisor.role_fingerprint,
            "supervisor role changed; save/review its setup again"
        );
        ensure!(
            supervisor.state_dir.is_dir(),
            "supervisor memory directory missing"
        );
        if let Some(command) = &supervisor.command {
            ensure!(
                command.is_file(),
                "Corgi command referenced by supervisor instructions is missing"
            );
        }
        let role = fs::read_to_string(&supervisor.role_file)?;
        if entry.kind == "codex" {
            // Resume must not overwrite a saved restrictive policy with the
            // new-supervisor launcher's default workspace-write policy.
            args.extend([
                "-c".into(),
                format!(
                    "developer_instructions={}",
                    crate::harness::toml_string(&role)
                ),
                "--add-dir".into(),
                supervisor.state_dir.to_string_lossy().into_owned(),
            ]);
        } else {
            args.extend(harness.supervisor_args(
                &supervisor.role_file,
                &role,
                &supervisor.state_dir,
            ));
        }
    }
    Ok(args)
}
