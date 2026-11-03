//! Corgi's macOS app icon: the supervisor's corgi from `corgi_core::art`,
//! each of its pixels a crisp square, on a rounded plate in Catppuccin
//! Mocha's base with the standard margin of a macOS icon (an 824 px plate on
//! a 1024 px canvas), transparent around it.
//!
//! Regenerate it with `cargo app-icon`, the alias for running the ignored
//! test at the bottom of this file, on macOS: it writes `AppIcon.png` and,
//! through `iconutil`, `AppIcon.icns` in `assets/icon/`. Every size of the
//! `.icns` is drawn from the pixel grid rather than scaled down from the
//! largest, so none of them is smoothed.

use corgi_core::{
    art::{CORGI_PIXELS, corgi_color},
    palette::{Palette, indexed_rgb},
};

/// Each size the icon is drawn at: the canvas, the plate centred on it and
/// how many device pixels wide each corgi pixel is. The plate is the
/// canvas's 824/1024, rounded to keep the margin whole, and the corgi about
/// two thirds of the plate's width, at a whole scale so its pixels stay
/// square. At 16 px the corgi's fourteen painted columns would not fit
/// inside the margin, so the plate fills the canvas there.
const SIZES: [(u32, u32, u32); 7] = [
    (16, 16, 1),
    (32, 26, 1),
    (64, 52, 3),
    (128, 104, 5),
    (256, 206, 10),
    (512, 412, 20),
    (1024, 824, 40),
];

/// The iconset's files: the name `iconutil` expects and the canvas size.
const ICONSET: [(&str, u32); 10] = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
];

/// The exponent of the plate's superellipse, the continuous curve of a
/// macOS icon's corners rather than a circular arc.
const PLATE_EXPONENT: i32 = 5;

/// Subsamples along each side of a pixel on the plate's edge.
const EDGE_SAMPLES: u32 = 16;

/// The icon drawn on a `canvas` px square, one of [`SIZES`], as straight
/// (not premultiplied) RGBA, so the plate's edge keeps its exact color
/// however transparent.
fn icon(canvas: u32) -> Vec<[u8; 4]> {
    let &(_, plate, scale) = SIZES
        .iter()
        .find(|(size, ..)| *size == canvas)
        .expect("a size the icon is drawn at");
    let mut pixels = vec![[0; 4]; (canvas * canvas) as usize];
    let (r, g, b) = Palette::CATPPUCCIN_MOCHA.background;
    let half = f64::from(plate) / 2.0;
    let centre = f64::from(canvas) / 2.0;
    let inside = |x: f64, y: f64| {
        ((x - centre) / half).abs().powi(PLATE_EXPONENT)
            + ((y - centre) / half).abs().powi(PLATE_EXPONENT)
            <= 1.0
    };
    for y in 0..canvas {
        for x in 0..canvas {
            let (left, top) = (f64::from(x), f64::from(y));
            let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
                .into_iter()
                .filter(|(dx, dy)| inside(left + dx, top + dy))
                .count();
            // The plate is convex and far larger than a pixel, so a pixel
            // whose corners are all on one side of its edge is all that side.
            let coverage = match corners {
                0 => 0.0,
                4 => 1.0,
                _ => {
                    let step = 1.0 / f64::from(EDGE_SAMPLES);
                    let hits = (0..EDGE_SAMPLES * EDGE_SAMPLES)
                        .filter(|sample| {
                            let dx = (f64::from(sample % EDGE_SAMPLES) + 0.5) * step;
                            let dy = (f64::from(sample / EDGE_SAMPLES) + 0.5) * step;
                            inside(left + dx, top + dy)
                        })
                        .count();
                    hits as f64 / f64::from(EDGE_SAMPLES * EDGE_SAMPLES)
                }
            };
            let alpha = (coverage * 255.0).round() as u8;
            pixels[(y * canvas + x) as usize] = [r, g, b, alpha];
        }
    }

    // The grid's blank border is even all round, so centring the grid
    // centres the corgi. Its ears are thin and its muzzle broad, so it is
    // raised by half a corgi pixel to sit in the middle to the eye.
    let columns = CORGI_PIXELS[0].len() as u32;
    let rows = CORGI_PIXELS.len() as u32;
    let left = (canvas - columns * scale) / 2;
    let top = (canvas - rows * scale) / 2 - scale / 2;
    for (row, line) in (0..).zip(CORGI_PIXELS) {
        for (column, pixel) in (0..).zip(line.chars()) {
            let Some(color) = corgi_color(pixel) else {
                continue;
            };
            let corgi_core::palette::Color::Indexed(index) = color else {
                panic!("the corgi's colors are fixed indexed colors");
            };
            let (r, g, b) = indexed_rgb(index);
            for y in top + row * scale..top + (row + 1) * scale {
                for x in left + column * scale..left + (column + 1) * scale {
                    pixels[(y * canvas + x) as usize] = [r, g, b, 255];
                }
            }
        }
    }
    pixels
}

/// The icon drawn on a `canvas` px square as a PNG.
fn icon_png(canvas: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, canvas, canvas);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer
        .write_image_data(icon(canvas).as_flattened())
        .expect("png data");
    writer.finish().expect("png");
    out
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, process::Command};

    use super::*;

    fn icon_dir() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icon"))
    }

    #[test]
    fn every_size_paints_each_corgi_pixel_whole_and_leaves_the_corners_clear() {
        for (canvas, _, scale) in SIZES {
            let pixels = icon(canvas);
            let at = |x: u32, y: u32| pixels[(y * canvas + x) as usize];
            assert_eq!(at(0, 0)[3], 0, "{canvas}: the corner is clear");
            let columns = CORGI_PIXELS[0].len() as u32;
            let rows = CORGI_PIXELS.len() as u32;
            let left = (canvas - columns * scale) / 2;
            let top = (canvas - rows * scale) / 2 - scale / 2;
            for (row, line) in (0..).zip(CORGI_PIXELS) {
                for (column, pixel) in (0..).zip(line.chars()) {
                    let Some(corgi_core::palette::Color::Indexed(index)) = corgi_color(pixel)
                    else {
                        continue;
                    };
                    let (r, g, b) = indexed_rgb(index);
                    for y in top + row * scale..top + (row + 1) * scale {
                        for x in left + column * scale..left + (column + 1) * scale {
                            assert_eq!(
                                at(x, y),
                                [r, g, b, 255],
                                "{canvas}: corgi pixel {pixel:?} at row {row}, column {column}",
                            );
                        }
                    }
                }
            }
        }
    }

    /// Fails when the corgi or the plate changed and the committed icon was
    /// not drawn again with `cargo app-icon`.
    #[test]
    fn the_committed_icon_is_the_one_drawn_here() {
        let committed = fs::read(icon_dir().join("AppIcon.png")).expect("assets/icon/AppIcon.png");
        assert!(
            committed == icon_png(1024),
            "run `cargo app-icon` to draw the icon again"
        );
    }

    /// Draws the icon into `assets/icon/`; run it with `cargo app-icon` on
    /// macOS, which has `iconutil`.
    #[test]
    #[ignore = "writes the app icon; run it with `cargo app-icon`"]
    fn app_icon() {
        let dir = icon_dir();
        fs::create_dir_all(dir).expect("assets/icon");
        fs::write(dir.join("AppIcon.png"), icon_png(1024)).expect("write AppIcon.png");

        let scratch = std::env::temp_dir().join(format!("corgi-app-icon-{}", std::process::id()));
        let iconset = scratch.join("AppIcon.iconset");
        fs::create_dir_all(&iconset).expect("iconset");
        for (name, canvas) in ICONSET {
            fs::write(iconset.join(name), icon_png(canvas)).expect("write an iconset image");
        }
        let status = Command::new("iconutil")
            .args(["--convert", "icns", "--output"])
            .arg(dir.join("AppIcon.icns"))
            .arg(&iconset)
            .status()
            .expect("run iconutil, which comes with macOS");
        fs::remove_dir_all(&scratch).expect("remove the iconset");
        assert!(status.success(), "iconutil failed");
    }
}
