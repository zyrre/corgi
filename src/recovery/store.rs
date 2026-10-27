//! Private durable recovery records. A committed manifest also arms its next-boot hold.
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::de::DeserializeOwned;

use super::{Journal, Snapshot};

const MAX_FILE: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct Store(pub PathBuf);

pub(crate) struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl Store {
    pub fn for_client(client: &crate::herdr::HerdrClient) -> Option<Self> {
        let record = client.marker_record()?;
        let parent = record.parent()?;
        // Custom marker records in existing tests are intentionally isolated.
        (parent.file_name()? == "markers").then(|| Self(parent.parent().unwrap().join("recovery")))
    }

    pub fn lock(&self) -> Result<Lock> {
        private_dir(&self.0)?;
        ensure!(self.0.is_absolute(), "recovery state home must be absolute");
        ensure!(
            cfg!(test) || !volatile(&self.0),
            "recovery state is in a volatile directory; use a persistent XDG_STATE_HOME before Save"
        );
        let file = crate::inbox_privacy::open(
            &self.0.join("recovery.lock"),
            OpenOptions::new().read(true).write(true).create(true),
        )?;
        file.lock()?;
        Ok(Lock(file))
    }

    pub fn save(&self, snapshot: &Snapshot) -> Result<()> {
        let _lock = self.lock()?;
        snapshot.validate()?;
        let mut snapshot = snapshot.clone();
        snapshot.sequence = self
            .snapshots()?
            .iter()
            .map(|s| s.sequence)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .context("snapshot sequence exhausted")?;
        let dir = self.0.join("snapshots");
        private_dir(&dir)?;
        let path = dir.join(format!("{}.json", snapshot.id));
        ensure!(!path.exists(), "snapshot ID already exists");
        atomic(&path, &serde_json::to_vec_pretty(&snapshot)?)
    }

    pub fn snapshots(&self) -> Result<Vec<Snapshot>> {
        let dir = self.0.join("snapshots");
        if !dir.try_exists()? {
            return Ok(Vec::new());
        }
        private_dir(&dir)?;
        let mut entries = Vec::new();
        for (count, entry) in fs::read_dir(dir)?.enumerate() {
            ensure!(
                count < 1000,
                "too many saved snapshots; archive older records first"
            );
            let path = entry?.path();
            if path.extension().is_some_and(|x| x == "json") {
                let snapshot: Snapshot = read(&path)?;
                snapshot.validate()?;
                entries.push(snapshot);
            }
        }
        entries.sort_by(|a, b| b.sequence.cmp(&a.sequence).then_with(|| b.id.cmp(&a.id)));
        Ok(entries)
    }

    pub fn journal(&self) -> Result<Journal> {
        let path = self.0.join("progress.json");
        if !path.try_exists()? {
            return Ok(Journal::default());
        }
        let journal: Journal = read(&path)?;
        ensure!(
            journal.version == 1,
            "unsupported recovery progress version"
        );
        Ok(journal)
    }

    /// Caller holds the recovery lock across intent and its side effect.
    pub fn write_journal(&self, journal: &Journal) -> Result<()> {
        atomic(&self.0.join("progress.json"), &serde_json::to_vec(journal)?)
    }
}

pub(crate) fn volatile(path: &Path) -> bool {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    ["/tmp", "/private/tmp", "/run", "/var/run"]
        .iter()
        .any(|p| resolved.starts_with(p))
        || resolved.starts_with(
            std::env::temp_dir()
                .canonicalize()
                .unwrap_or_else(|_| std::env::temp_dir()),
        )
}

pub(crate) fn private_dir(path: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        ensure!(
            !meta.file_type().is_symlink(),
            "recovery directory must not be a symlink"
        );
    }
    crate::inbox_privacy::state_dir(path)?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub(crate) fn read<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = crate::inbox_privacy::open(path, OpenOptions::new().read(true))?;
    ensure!(
        file.metadata()?.len() <= MAX_FILE,
        "recovery record is too large"
    );
    serde_json::from_reader(file.take(MAX_FILE + 1)).context("invalid recovery record")
}

pub(crate) fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() as u64 <= MAX_FILE,
        "recovery record is too large"
    );
    if let Ok(meta) = fs::symlink_metadata(path) {
        ensure!(
            meta.is_file() && meta.nlink() == 1 && meta.uid() == unsafe { libc::geteuid() },
            "unsafe recovery destination"
        );
    }
    let temporary = path.with_extension(format!("{}.tmp", nonce()?));
    let result = (|| {
        let mut file = crate::inbox_privacy::open(
            &temporary,
            OpenOptions::new().write(true).create_new(true),
        )?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(path.parent().context("recovery file has no directory")?)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn nonce() -> Result<String> {
    let mut bytes = [0u8; 12];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub(crate) fn boot() -> Result<String> {
    #[cfg(target_os = "linux")]
    return fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|s| s.trim().to_string())
        .context("cannot identify this host boot");
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "kern.boottime"])
            .output()?;
        ensure!(out.status.success(), "cannot identify this host boot");
        Ok(String::from_utf8(out.stdout)?.trim().to_string())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    anyhow::bail!("host boot identity is unsupported")
}

pub(crate) fn host() -> Result<String> {
    #[cfg(target_os = "linux")]
    return fs::read_to_string("/etc/machine-id")
        .map(|s| s.trim().to_string())
        .context("local machine identity unavailable");
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/usr/sbin/ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()?;
        ensure!(out.status.success(), "local machine identity unavailable");
        let text = String::from_utf8(out.stdout)?;
        let re = regex::Regex::new(r#""IOPlatformUUID"\s*=\s*"([A-Fa-f0-9-]+)""#)?;
        return Ok(re
            .captures(&text)
            .context("local machine identity unavailable")?[1]
            .to_string());
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    anyhow::bail!("local machine identity unsupported")
}
