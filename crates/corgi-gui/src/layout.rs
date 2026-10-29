//! The window's fixed layout: the header's measurements, and the overview,
//! the transcript split under it and the side panel beside it, and the focus
//! view's terminal with its own side panel, which open and close and keep
//! their sizes across runs in `$STATE/gui-layout.json`.

use std::{fs, path::Path, path::PathBuf};

use corgi_core::art::{CORGI_PIXELS, WORDMARK};
use serde::{Deserialize, Serialize};

/// One pixel of the corgi and the wordmark, drawn as a square about as wide
/// as a terminal cell, as the dashboard's half blocks have it.
pub const PIXEL: f32 = 8.0;
/// The header's band: as tall as the corgi, whose blank first and last
/// pixel rows hold its art one pixel clear of the band's edges.
pub const HEADER_HEIGHT: f32 = CORGI_PIXELS.len() as f32 * PIXEL;
/// The wordmark's rows are two pixels tall, one per half block.
pub const WORDMARK_HEIGHT: f32 = 2.0 * WORDMARK.len() as f32 * PIXEL;
/// The wordmark is centred against the corgi, flush left beside it.
pub const WORDMARK_TOP: f32 = (HEADER_HEIGHT - WORDMARK_HEIGHT) / 2.0;
/// The corgi to the letters, as wide as the wordmark's own letter spacing
/// in the dashboard; the letters to the first card twice the cards' gap.
pub const CORGI_GAP: f32 = 2.0 * PIXEL;
pub const CARD_GAP: f32 = PIXEL;
pub const WORDMARK_CARD_GAP: f32 = 2.0 * CARD_GAP;
/// A header card's title row. Its frame's top rule runs through the middle,
/// on the corgi's first painted pixel row.
pub const CARD_TITLE_ROW: f32 = 2.0 * PIXEL;
/// Every header card's height, its bottom rule on the corgi's last painted
/// pixel row, so the short usage cards stand as tall as HERD and the rules
/// bracket the letters by a pixel above and below.
pub const CARD_HEIGHT: f32 = HEADER_HEIGHT - PIXEL;
/// A line of a header card.
pub const CARD_LINE: f32 = 17.0;

/// The smallest the window can be made. Even this leaves the header room
/// for the corgi beside the letters; the cards wrap under them.
pub const WINDOW_MIN: (f32, f32) = (640.0, 420.0);
/// The header's padding either side, `px_4`.
pub const HEADER_PADDING: f32 = 16.0;

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
    /// The side panel beside the focus view's terminal.
    pub focus_side_open: bool,
    pub focus_side_width: f32,
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
            focus_side_open: true,
            focus_side_width: 340.0,
            window: None,
        }
    }
}

/// Which split a drag resizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    Side,
    Transcript,
    /// The focus view's terminal and its side panel.
    FocusSide,
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
        self.focus_side_width = self.focus_side_width.clamp(SIDE_MIN, SIDE_MAX);
        self.transcript_height = self.transcript_height.clamp(TRANSCRIPT_MIN, TRANSCRIPT_MAX);
        self.window = self
            .window
            .filter(|(width, height)| *width >= 400.0 && *height >= 300.0);
        self
    }

    /// Resizes `split` by a drag of `delta` pixels: a side panel grows as
    /// its edge moves left, the transcript as its edge moves up.
    pub fn drag(&mut self, split: Split, delta: f32) {
        match split {
            Split::Side => {
                self.side_width = (self.side_width - delta).clamp(SIDE_MIN, SIDE_MAX);
            }
            Split::FocusSide => {
                self.focus_side_width = (self.focus_side_width - delta).clamp(SIDE_MIN, SIDE_MAX);
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
    fn the_header_cards_share_one_height_and_bracket_the_lockup() {
        let painted = |row: &&str| row.contains(|pixel| pixel != '.');
        let first = CORGI_PIXELS.iter().position(painted).unwrap();
        let last = CORGI_PIXELS.iter().rposition(painted).unwrap();
        // The top rule, through the middle of the title row, is on the
        // corgi's first painted row; the bottom rule on its last.
        let top_rule = CARD_TITLE_ROW / 2.0;
        assert_eq!(top_rule, first as f32 * PIXEL);
        assert_eq!(CARD_HEIGHT, (last + 1) as f32 * PIXEL);
        // HERD's counts, the most lines a card holds, fit under the title
        // and above the bottom rule.
        let lines = crate::format::herd(&[]).len() as f32;
        assert!(CARD_TITLE_ROW + lines * CARD_LINE + 1.0 <= CARD_HEIGHT);
        // The letters sit inside the rules, a pixel clear of each.
        assert_eq!(WORDMARK_TOP, top_rule + PIXEL);
        assert_eq!(WORDMARK_TOP + WORDMARK_HEIGHT, CARD_HEIGHT - PIXEL);
        assert_eq!(
            corgi_core::art::wordmark_pixels().len() as f32 * PIXEL,
            WORDMARK_HEIGHT
        );
    }

    #[test]
    fn the_narrowest_window_still_holds_the_corgi_and_the_letters() {
        let corgi = CORGI_PIXELS[0].len() as f32 * PIXEL;
        let wordmark = corgi_core::art::wordmark_pixels()[0].len() as f32 * PIXEL;
        let lockup = corgi + CORGI_GAP + wordmark;
        assert_eq!(lockup, 448.0);
        assert!(lockup + 2.0 * HEADER_PADDING <= WINDOW_MIN.0);
    }

    #[test]
    fn the_focus_views_side_panel_keeps_its_own_width_and_visibility() {
        let dir = scratch("focus-layout");
        let path = dir.join("gui-layout.json");
        let mut layout = Layout::default();
        layout.drag(Split::FocusSide, -60.0);
        layout.focus_side_open = false;
        assert_eq!(layout.focus_side_width, 400.0);
        assert_eq!(
            layout.side_width,
            Layout::default().side_width,
            "the overview's is left alone"
        );
        layout.save(Some(&path));
        assert_eq!(Layout::load(Some(&path)), layout);
        // A file from before the focus view gives its defaults.
        fs::write(&path, r#"{"side_width": 300}"#).unwrap();
        let loaded = Layout::load(Some(&path));
        assert!(loaded.focus_side_open);
        assert_eq!(loaded.focus_side_width, Layout::default().focus_side_width);
        layout.drag(Split::FocusSide, 10_000.0);
        assert_eq!(layout.focus_side_width, SIDE_MIN);
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
