//! The adapter to Herdr's endpoint protocol, generation 1: the protocol
//! Herdr's own UI speaks on `herdr-client.sock`, beside the JSON API on
//! `herdr.sock`. Everything that knows that protocol is in this module, so a
//! Herdr upgrade that breaks it breaks it here.
//!
//! **Tested against Herdr 0.9.0.** Herdr promises generation 1 stays
//! available, with new fields optional and new encodings only behind hello
//! flags; its message order is frozen. The types in `protocol` were checked
//! field by field against 0.9.0's `src/protocol/wire.rs`.
//!
//! - `protocol`: the messages, adapted from Herdr UI's `herdr-protocol`.
//! - `connection`: one socket, its handshake, surfaces and requests.
//! - `viewer`: what the window's one terminal does with its connection:
//!   the park tab, activation, and a tab lost to someone else's navigation.
//!
//! The window never calls Herdr's JSON focus methods, which move every
//! client; it navigates only its own connection, with the endpoint's
//! `tab.focus`.

pub mod connection;
// Vendored whole: framing helpers the window does not call stay with it.
#[allow(dead_code)]
pub mod protocol;
pub mod viewer;

#[cfg(test)]
mod live_tests;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub use connection::{Connection, Event, Geometry};
use protocol::{SurfaceGraphicsAsset, SurfaceGraphicsAssetKey, SurfaceGraphicsFormat};

/// The Herdr version this adapter was written and tested against.
pub const TESTED_HERDR: &str = "0.9.0";

/// The endpoint socket beside a session's JSON socket, `herdr.sock`.
pub fn client_socket(json_socket: &Path) -> PathBuf {
    json_socket.with_file_name("herdr-client.sock")
}

/// The longest side of an image a pane may place: Herdr UI's bound.
pub const MAX_IMAGE_SIDE: u32 = 10_000;

/// Unique for the life of the process, so the painter's texture cache tells
/// two deliveries of one key apart.
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);

/// One image's encoded bytes, exactly as its key describes them. (Herdr UI's
/// `herdr-client` type of this name, which the painter takes.)
#[derive(Clone, Debug)]
pub struct SurfaceImage {
    serial: u64,
    data: Arc<[u8]>,
}

impl SurfaceImage {
    fn new(data: Vec<u8>) -> Self {
        Self {
            serial: NEXT_SERIAL.fetch_add(1, Ordering::Relaxed),
            data: Arc::from(data),
        }
    }

    pub fn serial(&self) -> u64 {
        self.serial
    }

    pub fn data(&self) -> &Arc<[u8]> {
        &self.data
    }
}

/// The images a connection's latest surface may place, by key.
#[derive(Clone, Debug, Default)]
pub struct SurfaceImages {
    images: HashMap<SurfaceGraphicsAssetKey, SurfaceImage>,
}

impl SurfaceImages {
    fn new(images: HashMap<SurfaceGraphicsAssetKey, SurfaceImage>) -> Self {
        Self { images }
    }

    pub fn get(&self, key: &SurfaceGraphicsAssetKey) -> Option<&SurfaceImage> {
        self.images.get(key)
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }
}

impl FromIterator<SurfaceGraphicsAsset> for SurfaceImages {
    fn from_iter<I: IntoIterator<Item = SurfaceGraphicsAsset>>(assets: I) -> Self {
        Self::new(
            assets
                .into_iter()
                .filter(|asset| valid_asset(&asset.key, &asset.data))
                .map(|asset| (asset.key, SurfaceImage::new(asset.data)))
                .collect(),
        )
    }
}

/// Whether `data` is exactly what `key` promises (Herdr UI's check).
pub fn valid_asset(key: &SurfaceGraphicsAssetKey, data: &[u8]) -> bool {
    let sides = 1..=MAX_IMAGE_SIDE;
    if data.is_empty()
        || u64::try_from(data.len()).ok() != Some(key.data_len)
        || !sides.contains(&key.image_width)
        || !sides.contains(&key.image_height)
    {
        return false;
    }
    let pixels = u64::from(key.image_width) * u64::from(key.image_height);
    match key.format {
        SurfaceGraphicsFormat::Rgb => pixels.checked_mul(3) == Some(key.data_len),
        SurfaceGraphicsFormat::Rgba => pixels.checked_mul(4) == Some(key.data_len),
        // Decoded later, under the painter's own limits.
        SurfaceGraphicsFormat::Png => true,
    }
}
