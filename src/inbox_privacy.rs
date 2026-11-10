//! Filesystem boundary for private inbox data. State-directory aliases are
//! intentional; content files and the reports directory must not be aliases.
use std::{
    collections::HashMap,
    fs::{self, DirBuilder, File, OpenOptions},
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use anyhow::{Context, Result, bail};

pub fn state_dir(path: &Path) -> Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .with_context(|| format!("create private state directory {}", path.display()))?;
    // State migration deliberately leaves aliases. Resolve only this boundary.
    let target = path.canonicalize()?;
    directory(&target)
}

fn directory(path: &Path) -> Result<()> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("open private directory {}", path.display()))?;
    restrict(&file, path, true)
}

fn restrict(file: &File, path: &Path, directory: bool) -> Result<()> {
    let meta = file.metadata()?;
    // Never chmod another user's inode or a hard link to unrelated content.
    if meta.uid() != unsafe { libc::geteuid() }
        || if directory {
            !meta.is_dir()
        } else {
            !meta.is_file() || meta.nlink() != 1
        }
    {
        bail!(
            "unsafe private inbox path {}: expected an owned {}",
            path.display(),
            if directory {
                "directory"
            } else {
                "regular file with one link"
            }
        );
    }
    let wanted = meta.mode() & if directory { 0o700 } else { 0o600 };
    if meta.mode() & 0o7777 != wanted {
        file.set_permissions(fs::Permissions::from_mode(wanted))
            .with_context(|| format!("protect {}", path.display()))?;
    }
    Ok(())
}

pub fn open(path: &Path, options: &mut OpenOptions) -> Result<File> {
    let file = options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("open private inbox file {}", path.display()))?;
    restrict(&file, path, false)?;
    Ok(file)
}

fn repair_file(path: &Path) -> Result<()> {
    match open(path, OpenOptions::new().read(true)) {
        Ok(_) => Ok(()),
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == io::ErrorKind::NotFound) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

pub fn reports_dir(dir: &Path) -> Result<PathBuf> {
    private_dir(dir, crate::inbox::REPORTS_DIR)
}

/// The private directory `name` in the state directory `dir`, made 0700 if
/// it is not there; like the reports directory, it must not be an alias.
pub fn private_dir(dir: &Path, name: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    match DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e).with_context(|| format!("create private directory {name}")),
    }
    directory(&path)?;
    Ok(path)
}

/// Repair all old reports, including unreferenced ones. Cache only successful
/// sweeps of an unchanged directory in this process; never persist a trust flag.
pub fn repair(dir: &Path) -> Result<()> {
    if !dir.try_exists()? {
        return Ok(());
    }
    state_dir(dir)?;
    for name in [crate::inbox::INBOX_FILE, "inbox.lock", "inbox.jsonl.tmp"] {
        repair_file(&dir.join(name))?;
    }
    let reports = dir.join(crate::inbox::REPORTS_DIR);
    match fs::symlink_metadata(&reports) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
        Ok(_) => directory(&reports)?,
    }
    type Stamp = (u64, u64, i64, i64, i64, i64);
    static SWEPT: OnceLock<Mutex<HashMap<PathBuf, Stamp>>> = OnceLock::new();
    let meta = fs::metadata(&reports)?;
    let stamp = (
        meta.dev(),
        meta.ino(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    );
    let mut swept = SWEPT
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let key = reports.canonicalize()?;
    if swept.get(&key) == Some(&stamp) {
        return Ok(());
    }
    for entry in fs::read_dir(&reports)? {
        // Missing entries during concurrent compaction are harmless.
        repair_file(&entry?.path())?;
    }
    swept.insert(key, stamp);
    Ok(())
}

pub fn report_path(dir: &Path, name: &str) -> Result<PathBuf> {
    let mut parts = Path::new(name).components();
    if parts.next() != Some(Component::Normal(crate::inbox::REPORTS_DIR.as_ref()))
        || !matches!(parts.next(), Some(Component::Normal(_)))
        || parts.next().is_some()
    {
        bail!("unsafe inbox report path {name:?}");
    }
    Ok(dir.join(name))
}
