//! The window's fixed layout: the overview, the transcript split under it
//! and the side panel beside it, which open and close and keep their sizes
//! across runs in `$STATE/gui-layout.json`.

use std::{fs, path::Path, path::PathBuf};

use serde::{Deserialize, Serialize};

pub const SIDE_MIN: f32 = 240.0;
pub const SIDE_MAX: f32 = 900.0;
pub const TRANSCRIPT_MIN: f32 = 100.0;
pub const TRANSCRIPT_MAX: f32 = 1400.0;
/// The overview keeps at least this much height beside the transcript.
pub const OVERVIEW_MIN: f32 = 160.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    /// The side panel: the selected project's supervisor and its questions.
    pub side_open: bool,
    pub side_width: f32,
    /// The selected agent's transcript, under the overview.
    pub transcript_open: bool,
    pub transcript_height: f32,
    /// The window's size when it was last closed.
    pub window: Option<(f32, f32)>,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            side_open: true,
            side_width: 380.0,
            transcript_open: true,
            transcript_height: 260.0,
            window: None,
        }
    }
}

/// Which split a drag resizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    Side,
    Transcript,
}

impl Layout {
    /// Where the layout is kept.
    pub fn path() -> Option<PathBuf> {
        corgi_core::paths::corgi_state_dir().map(|dir| dir.join("gui-layout.json"))
    }

    /// The layout kept at `path`, the default when there is none or it
    /// cannot be read.
    pub fn load(path: Option<&Path>) -> Self {
        path.and_then(|path| fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<Layout>(&text).ok())
            .map(Layout::clamped)
            .unwrap_or_default()
    }

    /// Keeps the layout at `path`, replacing it whole through a rename.
    pub fn save(&self, path: Option<&Path>) {
        let Some(path) = path else { return };
        let Ok(text) = serde_json::to_string_pretty(self) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let temporary = path.with_extension("json.tmp");
        if fs::write(&temporary, text).is_ok() {
            let _ = fs::rename(&temporary, path);
        }
    }

    fn clamped(mut self) -> Self {
        self.side_width = self.side_width.clamp(SIDE_MIN, SIDE_MAX);
        self.transcript_height = self.transcript_height.clamp(TRANSCRIPT_MIN, TRANSCRIPT_MAX);
        self.window = self
            .window
            .filter(|(width, height)| *width >= 400.0 && *height >= 300.0);
        self
    }

    /// Resizes `split` by a drag of `delta` pixels: the side panel grows as
    /// its edge moves left, the transcript as its edge moves up.
    pub fn drag(&mut self, split: Split, delta: f32) {
        match split {
            Split::Side => {
                self.side_width = (self.side_width - delta).clamp(SIDE_MIN, SIDE_MAX);
            }
            Split::Transcript => {
                self.transcript_height =
                    (self.transcript_height - delta).clamp(TRANSCRIPT_MIN, TRANSCRIPT_MAX);
            }
        }
    }

    /// The transcript's height in a main column `available` pixels tall,
    /// leaving the overview its minimum.
    pub fn transcript_height_in(&self, available: f32) -> f32 {
        self.transcript_height
            .min(available - OVERVIEW_MIN)
            .max(TRANSCRIPT_MIN.min(available / 2.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("corgi-gui-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_layout_is_kept_across_runs_and_a_bad_file_gives_the_default() {
        let dir = scratch("layout");
        let path = dir.join("state/gui-layout.json");
        assert_eq!(Layout::load(Some(&path)), Layout::default());
        let mut layout = Layout {
            side_open: false,
            transcript_open: false,
            window: Some((1200.0, 800.0)),
            ..Layout::default()
        };
        layout.drag(Split::Side, -100.0);
        layout.save(Some(&path));
        assert_eq!(Layout::load(Some(&path)), layout);
        assert_eq!(layout.side_width, 480.0);
        fs::write(&path, "{not json").unwrap();
        assert_eq!(Layout::load(Some(&path)), Layout::default());
        // A file from a run with odd sizes is brought back into range.
        fs::write(&path, r#"{"side_width": 5, "window": [10, 10]}"#).unwrap();
        let loaded = Layout::load(Some(&path));
        assert_eq!((loaded.side_width, loaded.window), (SIDE_MIN, None));
        assert!(loaded.side_open, "a field left out keeps its default");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn drags_stay_in_range_and_the_overview_keeps_its_room() {
        let mut layout = Layout::default();
        layout.drag(Split::Transcript, 10_000.0);
        assert_eq!(layout.transcript_height, TRANSCRIPT_MIN);
        layout.drag(Split::Transcript, -10_000.0);
        assert_eq!(layout.transcript_height, TRANSCRIPT_MAX);
        assert_eq!(layout.transcript_height_in(700.0), 700.0 - OVERVIEW_MIN);
        // A tiny window splits its height rather than going negative.
        assert_eq!(layout.transcript_height_in(150.0), 75.0);
    }
}
