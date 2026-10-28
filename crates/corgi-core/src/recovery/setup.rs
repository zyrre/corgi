//! Explicit manual-recovery setup. Opening the UI only reads configuration.
use super::{Store, native, store};
use crate::{herdr::HerdrClient, paths::home};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Deserialize)]
struct Config {
    session: Option<Session>,
}
#[derive(Deserialize)]
struct Session {
    resume_agents_on_restore: Option<toml::Spanned<bool>>,
}
#[derive(Clone, Debug)]
pub struct Change {
    pub path: PathBuf,
    pub enabled: bool,
    original: String,
    candidate: String,
}
#[derive(Serialize, Deserialize)]
struct Confirmed {
    path: PathBuf,
    fingerprint: String,
    scope: String,
    boot: String,
    socket: String,
    host: String,
}

pub fn config_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("HERDR_CONFIG_PATH").filter(|s| !s.is_empty()) {
        return Ok(path.into());
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".config")))
        .context("Herdr config path unavailable")?;
    Ok(base.join("herdr/config.toml"))
}

pub fn configured(path: &Path) -> Result<bool> {
    let text = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let config: Config = toml::from_str(&text)?;
    Ok(config
        .session
        .and_then(|s| s.resume_agents_on_restore)
        .is_none_or(|s| *s.get_ref()))
}

/// Setup has been explicitly confirmed/reloaded, ready for the next normal boot.
pub fn prepared(client: &HerdrClient) -> bool {
    verified(client).is_ok()
}

/// A later server start excludes an old, already-queued automatic resume.
pub fn manual(client: &HerdrClient) -> bool {
    verified(client).is_ok_and(|proof| {
        store::boot().is_ok_and(|boot| proof.boot != boot)
            || socket_stamp(client.socket_path()).is_ok_and(|stamp| proof.socket != stamp)
    })
}

fn verified(client: &HerdrClient) -> Result<Confirmed> {
    let path = config_path()?;
    ensure!(!configured(&path)?, "automatic agent restore enabled");
    let store = Store::for_client(client).context("recovery storage unavailable")?;
    let proof: Confirmed = store::read(&store.0.join("manual-setup.json"))?;
    ensure!(
        proof.path == path
            && proof.scope == client.socket_path().to_string_lossy()
            && proof.fingerprint == native::fingerprint(&path)?
            && proof.host == store::host()?,
        "manual setup proof no longer matches"
    );
    Ok(proof)
}

pub fn prepare(path: PathBuf) -> Result<Change> {
    ensure!(path.is_absolute(), "Herdr config path must be absolute");
    let original = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let candidate = edit(&original)?;
    let config: Config = toml::from_str(&original)?;
    let enabled = config
        .session
        .and_then(|s| s.resume_agents_on_restore)
        .is_none_or(|s| *s.get_ref());
    Ok(Change {
        path,
        original,
        candidate,
        enabled,
    })
}

pub fn edit(text: &str) -> Result<String> {
    let parsed: Config = toml::from_str(text).context("Herdr configuration is invalid TOML")?;
    let mut expected: toml::Table = toml::from_str(text)?;
    let mut result = text.to_string();
    if let Some(value) = parsed.session.and_then(|s| s.resume_agents_on_restore) {
        result.replace_range(value.span(), "false");
    } else if expected.contains_key("session") {
        let header = regex::Regex::new(
            r#"(?m)^\s*\[\s*(?:session|'session'|"session")\s*\][ \t]*(?:#[^\n]*)?\r?$"#,
        )?;
        let found = header.find(text).context("nonstandard session table; set resume_agents_on_restore = false manually, then confirm setup")?;
        let end = found.end();
        result.insert_str(end, "\nresume_agents_on_restore = false");
    } else {
        result.push_str("\n[session]\nresume_agents_on_restore = false\n");
    }
    let session = expected
        .entry("session")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    session
        .as_table_mut()
        .context("session must be a table")?
        .insert(
            "resume_agents_on_restore".into(),
            toml::Value::Boolean(false),
        );
    let actual: toml::Table = toml::from_str(&result)?;
    ensure!(
        actual == expected,
        "setup would change unrelated configuration; edit the setting manually"
    );
    Ok(result)
}

pub fn apply(client: &HerdrClient, change: &Change) -> Result<String> {
    apply_with(client, change, |path| {
        let checked = Command::new("herdr")
            .args(["config", "check"])
            .env("HERDR_CONFIG_PATH", path)
            .output()
            .context("cannot run herdr config check")?;
        Ok(checked.status.success())
    })
}

pub fn apply_with(
    client: &HerdrClient,
    change: &Change,
    check: impl FnOnce(&Path) -> Result<bool>,
) -> Result<String> {
    let store = Store::for_client(client).context("recovery storage unavailable")?;
    let _lock = store.lock()?;
    let check_path = store
        .0
        .join(format!("config-check-{}.toml", store::nonce()?));
    store::atomic(&check_path, change.candidate.as_bytes())?;
    let checked = check(&check_path);
    let _ = fs::remove_file(&check_path);
    ensure!(
        checked?,
        "Herdr rejected the proposed configuration; installed file untouched"
    );
    let current = match fs::read_to_string(&change.path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).context("cannot recheck Herdr config before saving"),
    };
    ensure!(
        current == change.original,
        "configuration changed since confirmation; review setup again"
    );
    if current != change.candidate {
        if let Some(parent) = change.path.parent() {
            fs::create_dir_all(parent)?;
        }
        if change.path.exists() {
            let backup = change
                .path
                .with_extension(format!("corgi-recovery-{}.bak", store::nonce()?));
            store::atomic(&backup, current.as_bytes())?;
        }
        store::atomic(&change.path, change.candidate.as_bytes())?;
    }
    let reloaded = client.request("server.reload_config", serde_json::json!({}));
    if !reloaded.is_ok_and(|v| v["type"] == "config_reload" && v["status"] == "applied") {
        bail!(
            "Manual setting saved, but Herdr reload was not confirmed. Confirm setup again; no server restart was requested."
        );
    }
    let proof_path = store.0.join("manual-setup.json");
    let fingerprint = native::fingerprint(&change.path)?;
    let scope = client.socket_path().to_string_lossy().into_owned();
    let host = store::host()?;
    // Reconfirming unchanged, acknowledged setup must not invalidate an
    // already-crossed server-start boundary and demand another reboot.
    let proof = match store::read::<Confirmed>(&proof_path) {
        Ok(proof)
            if proof.path == change.path
                && proof.fingerprint == fingerprint
                && proof.scope == scope
                && proof.host == host =>
        {
            proof
        }
        _ => Confirmed {
            path: change.path.clone(),
            fingerprint,
            scope,
            host,
            boot: store::boot()?,
            socket: socket_stamp(client.socket_path())?,
        },
    };
    store::atomic(&proof_path, &serde_json::to_vec(&proof)?)?;
    Ok("Manual recovery configured and reloaded; layout restoration retained. Missing-session restores are enabled after the next normal Herdr start/reboot, to exclude already-queued automatic launches. No restart was requested.".into())
}

fn socket_stamp(path: &Path) -> Result<String> {
    let meta = fs::metadata(path)?;
    Ok(format!(
        "{}:{}:{}:{}",
        meta.dev(),
        meta.ino(),
        meta.ctime(),
        meta.ctime_nsec()
    ))
}
