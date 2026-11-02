//! The harness of the most recent spawn, kept in one small file under Corgi's
//! state directory so the next new-agent form opens on it.
//!
//! Every launch records its harness once the agent has started (see
//! [`record_launch`]), whichever way it was asked for: the dashboard form, the
//! desktop window through `corgi serve`, `corgi spawn`, or a supervisor start.
//! A missing, unreadable or corrupt file means nothing was recorded.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{harness::Harness, herdr::HerdrClient, paths::corgi_state_dir};

const FILE_NAME: &str = "last-harness.json";

#[derive(Serialize, Deserialize)]
struct Record {
    harness: String,
}

/// Where the record lives for the real state directory. Tests of the core
/// keep none, so they never read or write the developer's own state.
pub fn state_file() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    corgi_state_dir().map(|dir| dir.join(FILE_NAME))
}

/// The kind recorded in the file at `path`, or `None` when there is no file,
/// it cannot be read or parsed, or it names no harness.
pub fn read(path: &Path) -> Option<String> {
    let record: Record = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let kind = Harness::from_typed(&record.harness).kind().to_string();
    (!kind.is_empty()).then_some(kind)
}

/// Replaces the record at `path` in one step: a temporary file of this
/// process's own is renamed over it, so a reader sees the old record or the
/// new one and never half of either.
pub fn write(path: &Path, harness: &Harness) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let record = Record {
        harness: harness.kind().to_string(),
    };
    let written = fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(&serde_json::to_vec(&record)?)?;
        file.sync_all()
    });
    if let Err(error) = written.and_then(|()| fs::rename(&temporary, path)) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

/// Records `harness` as the last one used, in the state directory `client`
/// keeps its markers in. A client with no such directory, as in tests,
/// records nothing. Failing to record is not a failed launch.
pub fn record_launch(client: &HerdrClient, harness: &Harness) {
    let Some(markers) = client.marker_record().and_then(Path::parent) else {
        return;
    };
    if markers.file_name().is_none_or(|name| name != "markers") {
        return;
    }
    if let Some(state) = markers.parent() {
        let _ = write(&state.join(FILE_NAME), harness);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ScratchDir;

    #[test]
    fn a_written_harness_reads_back_and_is_replaced_by_the_next() {
        let scratch = ScratchDir::new("last-harness");
        let path = scratch.join("state/last-harness.json");
        assert_eq!(read(&path), None);
        write(&path, &Harness::Claude).unwrap();
        assert_eq!(read(&path).as_deref(), Some("claude"));
        write(&path, &Harness::Codex).unwrap();
        assert_eq!(read(&path).as_deref(), Some("codex"));
        let leftovers = fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1, "no temporary file is left behind");
    }

    #[test]
    fn a_corrupt_or_empty_record_reads_as_nothing_recorded() {
        let scratch = ScratchDir::new("last-harness-corrupt");
        let path = scratch.join("last-harness.json");
        for text in [
            "",
            "not json",
            "{}",
            "[1]",
            r#"{"harness":""}"#,
            r#"{"harness":"  "}"#,
        ] {
            fs::write(&path, text).unwrap();
            assert_eq!(read(&path), None, "{text:?}");
        }
        fs::write(&path, [0xff, 0xfe]).unwrap();
        assert_eq!(read(&path), None);
    }

    #[test]
    fn a_launch_records_in_the_state_directory_beside_the_markers() {
        let scratch = ScratchDir::new("last-harness-launch");
        let client = HerdrClient::from_socket_path(scratch.join("herdr.sock"));
        record_launch(&client, &Harness::Claude);
        assert!(
            !scratch.join(FILE_NAME).exists(),
            "a test client records nothing"
        );

        let client = client.with_marker_record(scratch.join("markers/session.json"));
        record_launch(&client, &Harness::Claude);
        assert_eq!(read(&scratch.join(FILE_NAME)).as_deref(), Some("claude"));

        // A marker record elsewhere is a test's own and is left alone.
        let other = ScratchDir::new("last-harness-other");
        let client = client.with_marker_record(other.join("markers.json"));
        record_launch(&client, &Harness::Codex);
        assert!(!other.join(FILE_NAME).exists());
        assert!(!other.parent().unwrap().join(FILE_NAME).exists());
    }
}
