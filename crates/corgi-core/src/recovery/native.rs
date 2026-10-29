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
pub struct Settings {
    pub model: String,
    pub effort: String,
    pub sandbox: Option<String>,
    pub approval: Option<String>,
    #[serde(default)]
    pub approvals_reviewer: Option<String>,
    pub permission: Option<String>,
    pub writable: Vec<PathBuf>,
    pub profile: Option<String>,
    pub network: Option<bool>,
    pub unknown: bool,
    pub native_cwd: Option<PathBuf>,
    pub native_pending: bool,
    #[serde(default)]
    pub permission_error: Option<String>,
    #[serde(default)]
    pub exclude_tmpdir_env_var: Option<bool>,
    #[serde(default)]
    pub exclude_slash_tmp: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub kind: String,
    pub id: String,
    pub cwd: PathBuf,
    pub settings: Settings,
}

impl Settings {
    pub fn apply_native(&mut self, observed: Self) {
        if !observed.model.is_empty() {
            self.model = observed.model;
        }
        if !observed.effort.is_empty() {
            self.effort = observed.effort;
        }
        if observed.sandbox.is_some() || observed.permission_error.is_some() {
            self.sandbox = observed.sandbox;
            self.network = observed.network;
            self.writable = observed.writable;
            self.exclude_tmpdir_env_var = observed.exclude_tmpdir_env_var;
            self.exclude_slash_tmp = observed.exclude_slash_tmp;
            self.permission_error = observed.permission_error;
        }
        if observed.approval.is_some() {
            self.approval = observed.approval;
        }
        if observed.approvals_reviewer.is_some() {
            self.approvals_reviewer = observed.approvals_reviewer;
        }
        self.native_cwd = observed.native_cwd;
        self.native_pending = observed.native_pending;
    }

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

pub fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

pub fn store_root(kind: &str) -> Result<PathBuf> {
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
pub fn locate(root: &Path, kind: &str, id: &str) -> Result<PathBuf> {
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

pub fn exact_file(root: &Path, kind: &str, id: &str, hint: Option<&Path>) -> Result<PathBuf> {
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

pub fn metadata(path: &Path, kind: &str, id: &str) -> Result<Settings> {
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
            if let Some(value) = meta["approvals_reviewer"].as_str() {
                settings.approvals_reviewer = Some(value.into());
            }
            if !meta["permission_profile"].is_null() || !meta["sandbox_policy"].is_null() {
                // Each permission record replaces the whole previous policy,
                // including empty roots and defaults, in JSONL order.
                settings.sandbox = None;
                settings.writable.clear();
                settings.network = None;
                settings.exclude_tmpdir_env_var = None;
                settings.exclude_slash_tmp = None;
                settings.permission_error = codex_permissions(&mut settings, meta)
                    .err()
                    .map(|error| error.to_string());
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

/// Translate only policies expressible by our CLI adapter. Keep unsupported
/// permissions as a blocking diagnostic, never fall back to an older policy.
fn codex_permissions(settings: &mut Settings, meta: &Value) -> Result<()> {
    let profile = &meta["permission_profile"];
    if profile.is_null() {
        let policy = &meta["sandbox_policy"];
        let kind = policy["type"]
            .as_str()
            .context("unrecognized Codex sandbox policy")?
            .replace('_', "-");
        ensure!(
            ["read-only", "workspace-write", "danger-full-access"].contains(&kind.as_str()),
            "Codex sandbox policy cannot be represented by recovery"
        );
        ensure!(
            policy["access"].is_null(),
            "restricted Codex reads cannot be represented by recovery"
        );
        settings.network = Some(
            policy["network_access"]
                .as_bool()
                .unwrap_or(kind == "danger-full-access"),
        );
        settings.sandbox = Some(kind);
        settings.exclude_tmpdir_env_var =
            Some(policy["exclude_tmpdir_env_var"].as_bool().unwrap_or(false));
        settings.exclude_slash_tmp = Some(policy["exclude_slash_tmp"].as_bool().unwrap_or(false));
        if let Some(roots) = policy["writable_roots"].as_array() {
            for root in roots {
                let path = root.as_str().context("unrecognized Codex writable root")?;
                ensure!(
                    Path::new(path).is_absolute(),
                    "Codex writable root must be absolute"
                );
                settings.writable.push(path.into());
            }
        }
    } else if profile["type"] == "disabled" {
        settings.sandbox = Some("danger-full-access".into());
        settings.network = Some(true);
    } else {
        ensure!(
            profile["type"] == "managed" && profile["file_system"]["type"] == "restricted",
            "Codex permission profile cannot be represented by recovery"
        );
        settings.network = Some(match profile["network"].as_str() {
            Some("enabled") => true,
            Some("restricted") => false,
            _ => bail!("Codex network policy cannot be represented by recovery"),
        });
        let entries = profile["file_system"]["entries"]
            .as_array()
            .context("Codex filesystem permissions missing")?;
        let mut reads_root = false;
        let mut guarded = std::collections::BTreeSet::new();
        settings.exclude_tmpdir_env_var = Some(true);
        settings.exclude_slash_tmp = Some(true);
        for entry in entries {
            let path = &entry["path"];
            if path["type"] == "path"
                && entry["access"] == "read"
                && entry["missing_path_behavior"] == "skip"
            {
                guarded.insert(PathBuf::from(
                    path["path"].as_str().context("invalid Codex read guard")?,
                ));
                continue;
            }
            ensure!(
                entry["missing_path_behavior"].is_null(),
                "Codex filesystem path behavior cannot be represented by recovery"
            );
            match (path["type"].as_str(), entry["access"].as_str()) {
                (Some("special"), Some("read"))
                    if path["value"] == serde_json::json!({"kind":"root"}) =>
                {
                    reads_root = true
                }
                (Some("path"), Some("write")) => {
                    let path = path["path"]
                        .as_str()
                        .context("invalid Codex writable path")?;
                    ensure!(
                        Path::new(path).is_absolute(),
                        "Codex writable root must be absolute"
                    );
                    settings.writable.push(path.into());
                }
                (Some("special"), Some("write"))
                    if path["value"] == serde_json::json!({"kind":"tmpdir"}) =>
                {
                    settings.exclude_tmpdir_env_var = Some(false)
                }
                (Some("special"), Some("write"))
                    if path["value"] == serde_json::json!({"kind":"slash_tmp"}) =>
                {
                    settings.exclude_slash_tmp = Some(false)
                }
                _ => bail!(
                    "Codex filesystem rules cannot be represented by recovery (restricted reads, deny rules or symbolic paths)"
                ),
            }
        }
        ensure!(
            reads_root,
            "restricted Codex reads cannot be represented by recovery"
        );
        if settings.writable.is_empty()
            && settings.exclude_tmpdir_env_var == Some(true)
            && settings.exclude_slash_tmp == Some(true)
        {
            settings.sandbox = Some("read-only".into());
        } else {
            ensure!(
                settings
                    .native_cwd
                    .as_ref()
                    .is_some_and(|cwd| settings.writable.iter().any(|root| cwd.starts_with(root))),
                "Codex write policy without writable cwd cannot be represented by recovery"
            );
            settings.sandbox = Some("workspace-write".into());
            // Codex 0.163 materializes these built-in guards for each write
            // root. --sandbox workspace-write reconstructs exactly this set.
            // Other read carveouts cannot be recreated with legacy CLI flags.
            let expected: std::collections::BTreeSet<_> = settings
                .writable
                .iter()
                .flat_map(|root| [".git", ".agents", ".codex", ".aws"].map(|name| root.join(name)))
                .collect();
            ensure!(
                guarded == expected,
                "Codex read guards differ from built-in workspace protections; recovery cannot represent them"
            );
        }
        if settings.sandbox.as_deref() == Some("read-only") {
            ensure!(
                guarded.is_empty(),
                "Codex read guards cannot be represented by read-only recovery"
            );
        }
    }
    ensure!(
        settings.sandbox.as_deref() != Some("read-only") || settings.network != Some(true),
        "Codex read-only with enabled network cannot be represented by recovery"
    );
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigReference {
    pub path: PathBuf,
    pub fingerprint: String,
}

/// Only references and change detectors leave this function, never settings,
/// hook commands, credentials or embedded instructions.
pub fn configuration(root: &Path, kind: &str, cwd: &Path) -> Result<(Vec<ConfigReference>, bool)> {
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

pub fn fingerprint(path: &Path) -> Result<String> {
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

pub fn resume_args(entry: &Entry) -> Result<Vec<String>> {
    let id = entry
        .native_id
        .as_deref()
        .context("native ID unavailable")?;
    ensure!(valid_id(id), "invalid native conversation ID");
    let harness = Harness::from_kind(&entry.kind);
    let settings = &entry.settings;
    if let Some(error) = &settings.permission_error {
        bail!("Saved permissions cannot be restored: {error}");
    }
    ensure!(
        entry.kind != "codex"
            || settings.sandbox.as_deref() != Some("read-only")
            || settings.writable.is_empty(),
        "read-only Codex cannot request writable roots"
    );
    ensure!(
        entry.kind != "codex"
            || settings.sandbox.as_deref() != Some("read-only")
            || settings.network != Some(true),
        "read-only Codex with enabled network cannot be represented by recovery"
    );
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
    if let Some(reviewer) = &settings.approvals_reviewer {
        ensure!(
            entry.kind == "codex" && ["user", "auto_review"].contains(&reviewer.as_str()),
            "unsupported saved approvals reviewer"
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
    if settings.sandbox.as_deref() == Some("workspace-write") {
        // Clear inherited extra roots; only the saved roots go through --add-dir.
        args.extend([
            "-c".into(),
            "sandbox_workspace_write.writable_roots=[]".into(),
        ]);
        for (key, value) in [
            ("exclude_tmpdir_env_var", settings.exclude_tmpdir_env_var),
            ("exclude_slash_tmp", settings.exclude_slash_tmp),
        ] {
            if let Some(value) = value {
                args.extend([
                    "-c".into(),
                    format!("sandbox_workspace_write.{key}={value}"),
                ]);
            }
        }
    }
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
    if let Some(reviewer) = &settings.approvals_reviewer {
        args.extend([
            "-c".into(),
            format!(
                "approvals_reviewer={}",
                crate::harness::toml_string(reviewer)
            ),
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
            ]);
            // Read-only already grants read access to durable state. --add-dir
            // requests writes and Codex rejects it with read-only permissions.
            // A launch profile can retain the launcher's intended state root;
            // observed native permissions already contain all permitted roots.
            if settings.sandbox.as_deref() == Some("workspace-write")
                && settings.exclude_tmpdir_env_var.is_none()
                && !settings
                    .writable
                    .iter()
                    .any(|root| supervisor.state_dir.starts_with(root))
            {
                args.extend([
                    "--add-dir".into(),
                    supervisor.state_dir.to_string_lossy().into_owned(),
                ]);
            }
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
