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
//!
//! It also draws the Corgi logo of the Herdr app's sidebar brand row,
//! `herdr-app/mods/corgi-brand/corgi.png`, on Herdr's cream plate beside
//! Herdr's own logo (see `brand_tile`): `cargo brand-tile` draws it again.

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
            let alpha = (coverage(x, y, inside) * 255.0).round() as u8;
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
    paint_corgi(&mut pixels, canvas, (left, top), scale);
    pixels
}

/// How much of the pixel at (`x`, `y`) is inside a plate: `inside` says
/// whether a point is. The plate is convex and far larger than a pixel, so
/// a pixel whose corners are all on one side of its edge is all that side;
/// one it crosses is subsampled.
fn coverage(x: u32, y: u32, inside: impl Fn(f64, f64) -> bool) -> f64 {
    let (left, top) = (f64::from(x), f64::from(y));
    let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
        .into_iter()
        .filter(|(dx, dy)| inside(left + dx, top + dy))
        .count();
    match corners {
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
    }
}

/// The RGB of a pixel of [`CORGI_PIXELS`], `None` for a blank one.
fn corgi_rgb(pixel: char) -> Option<(u8, u8, u8)> {
    let color = corgi_color(pixel)?;
    let corgi_core::palette::Color::Indexed(index) = color else {
        panic!("the corgi's colors are fixed indexed colors");
    };
    Some(indexed_rgb(index))
}

/// Paints [`CORGI_PIXELS`] onto a `canvas` px square with its grid's
/// top-left corner at `origin`, each corgi pixel `scale` px wide.
fn paint_corgi(pixels: &mut [[u8; 4]], canvas: u32, (left, top): (u32, u32), scale: u32) {
    for (row, line) in (0..).zip(CORGI_PIXELS) {
        for (column, pixel) in (0..).zip(line.chars()) {
            let Some((r, g, b)) = corgi_rgb(pixel) else {
                continue;
            };
            for y in top + row * scale..top + (row + 1) * scale {
                for x in left + column * scale..left + (column + 1) * scale {
                    pixels[(y * canvas + x) as usize] = [r, g, b, 255];
                }
            }
        }
    }
}

/// The sidebar brand row's Corgi logo (`herdr-app/mods/corgi-brand/corgi.png`),
/// drawn to match Herdr's logo beside it (`frame/welcome/herdr.png` in the
/// Herdr app's home), whose measures these are: a [`TILE`] px canvas,
/// transparent round a plate [`TILE_MARGIN`] px in, of Herdr's cream with
/// circular corners of [`TILE_RADIUS`] px. The app shows both at 20 px.
///
/// The corgi is as wide on the plate as Herdr's ram (about nine tenths of
/// it), at [`TILE_SCALE`] px a corgi pixel, which the 20 px logo shows as two
/// device pixels at 2x, so it stays crisp. Its white is almost the cream's
/// colour, so where the white meets the plate it is edged, inside its own
/// pixels, with [`TILE_OUTLINE`]; the art is otherwise as drawn.
fn brand_tile() -> Vec<[u8; 4]> {
    let canvas = TILE;
    let mut pixels = vec![[0; 4]; (canvas * canvas) as usize];
    let (r, g, b) = TILE_PLATE;
    let (low, high) = (f64::from(TILE_MARGIN), f64::from(canvas - TILE_MARGIN));
    let radius = f64::from(TILE_RADIUS);
    let inside = |x: f64, y: f64| {
        let cx = x.clamp(low + radius, high - radius);
        let cy = y.clamp(low + radius, high - radius);
        (low..=high).contains(&x)
            && (low..=high).contains(&y)
            && (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
    };
    for y in 0..canvas {
        for x in 0..canvas {
            let alpha = (coverage(x, y, inside) * 255.0).round() as u8;
            pixels[(y * canvas + x) as usize] = [r, g, b, alpha];
        }
    }

    // Centred as on the app icon, raised by half a corgi pixel, both whole
    // multiples of four so the 20 px logo keeps each edge on a pixel.
    let (left, top) = tile_corgi_origin();
    paint_corgi(&mut pixels, canvas, (left, top), TILE_SCALE);

    let blank = |row: i64, column: i64| {
        usize::try_from(row)
            .ok()
            .and_then(|row| CORGI_PIXELS.get(row))
            .and_then(|line| {
                usize::try_from(column)
                    .ok()
                    .and_then(|c| line.chars().nth(c))
            })
            .is_none_or(|pixel| corgi_color(pixel).is_none())
    };
    let (or, og, ob) = TILE_OUTLINE;
    for (row, line) in (0..).zip(CORGI_PIXELS) {
        for (column, pixel) in (0..).zip(line.chars()) {
            if pixel != 'w' {
                continue;
            }
            let (r, c) = (i64::from(row), i64::from(column));
            let (x0, y0) = (left + column * TILE_SCALE, top + row * TILE_SCALE);
            let (x1, y1) = (x0 + TILE_SCALE, y0 + TILE_SCALE);
            let w = TILE_OUTLINE_WIDTH;
            let bands = [
                (blank(r - 1, c), (x0, y0, x1, y0 + w)),
                (blank(r + 1, c), (x0, y1 - w, x1, y1)),
                (blank(r, c - 1), (x0, y0, x0 + w, y1)),
                (blank(r, c + 1), (x1 - w, y0, x1, y1)),
            ];
            for (_, (bx0, by0, bx1, by1)) in bands.into_iter().filter(|(edge, _)| *edge) {
                for y in by0..by1 {
                    for x in bx0..bx1 {
                        pixels[(y * canvas + x) as usize] = [or, og, ob, 255];
                    }
                }
            }
        }
    }
    pixels
}

/// The brand tile's canvas, as Herdr's logo's.
const TILE: u32 = 160;
/// The transparent margin round the brand tile's plate, as Herdr's logo's.
const TILE_MARGIN: u32 = 16;
/// The radius of the plate's corners, fitted to Herdr's logo's alpha.
const TILE_RADIUS: u32 = 26;
/// Herdr's logo's cream, sampled from its plate.
const TILE_PLATE: (u8, u8, u8) = (244, 241, 233);
/// Device pixels to a corgi pixel on the brand tile.
const TILE_SCALE: u32 = 8;
/// The edge of the corgi's white where it meets the cream: a warm grey,
/// light enough to read as the white's shadow rather than a drawn line.
const TILE_OUTLINE: (u8, u8, u8) = (176, 170, 158);
/// How wide that edge is: one device pixel of the 20 px logo at 2x.
const TILE_OUTLINE_WIDTH: u32 = 4;

/// Where the corgi's grid starts on the brand tile.
fn tile_corgi_origin() -> (u32, u32) {
    let columns = CORGI_PIXELS[0].len() as u32;
    let rows = CORGI_PIXELS.len() as u32;
    let left = (TILE - columns * TILE_SCALE) / 2;
    let top = (TILE - rows * TILE_SCALE) / 2 - TILE_SCALE / 2;
    (left, top)
}

/// `pixels`, a `canvas` px square of straight RGBA, as a PNG.
fn png_bytes(canvas: u32, pixels: &[[u8; 4]]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, canvas, canvas);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer
        .write_image_data(pixels.as_flattened())
        .expect("png data");
    writer.finish().expect("png");
    out
}

/// The brand tile as a PNG.
fn brand_tile_png() -> Vec<u8> {
    png_bytes(TILE, &brand_tile())
}

/// The icon drawn on a `canvas` px square as a PNG.
fn icon_png(canvas: u32) -> Vec<u8> {
    png_bytes(canvas, &icon(canvas))
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

    fn brand_dir() -> &'static Path {
        Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/herdr-app/mods/corgi-brand"
        ))
    }

    #[test]
    fn the_brand_tile_is_herdrs_plate_with_the_corgi_whole_and_its_white_edged() {
        let pixels = brand_tile();
        let at = |x: u32, y: u32| pixels[(y * TILE + x) as usize];
        assert_eq!(at(0, 0)[3], 0, "the margin is clear");
        assert_eq!(
            at(TILE_MARGIN + 1, TILE_MARGIN + 1)[3],
            0,
            "the corner is rounded"
        );
        let (r, g, b) = TILE_PLATE;
        assert_eq!(
            at(TILE_MARGIN + 2, TILE / 2),
            [r, g, b, 255],
            "the plate is cream"
        );
        let (left, top) = tile_corgi_origin();
        assert_eq!(
            (left % 4, top % 4),
            (0, 0),
            "each corgi pixel sits on a 20 px logo's pixel"
        );
        let (or, og, ob) = TILE_OUTLINE;
        for (row, line) in (0..).zip(CORGI_PIXELS) {
            for (column, pixel) in (0..).zip(line.chars()) {
                let Some((r, g, b)) = corgi_rgb(pixel) else {
                    continue;
                };
                let (x, y) = (left + column * TILE_SCALE, top + row * TILE_SCALE);
                let square: Vec<_> = (0..TILE_SCALE * TILE_SCALE)
                    .map(|d| at(x + d % TILE_SCALE, y + d / TILE_SCALE))
                    .collect();
                if pixel == 'w' {
                    // Edged only on its rim: some of the white is left.
                    assert!(
                        square
                            .iter()
                            .all(|&p| p == [r, g, b, 255] || p == [or, og, ob, 255])
                            && square.contains(&[r, g, b, 255]),
                        "white at {row}, {column}"
                    );
                } else {
                    assert!(
                        square.iter().all(|&p| p == [r, g, b, 255]),
                        "only the white is edged: {pixel:?} at {row}, {column}"
                    );
                }
            }
        }
        // The muzzle's lowest white pixel meets the cream below it.
        let (x, y) = (left + 5 * TILE_SCALE, top + 13 * TILE_SCALE);
        assert_eq!(at(x, y - 1), [or, og, ob, 255]);
        assert_eq!(at(x, y), [r, g, b, 255]);
    }

    /// Fails when the corgi or the tile changed and the committed logo was
    /// not drawn again with `cargo brand-tile`.
    #[test]
    fn the_committed_brand_tile_is_the_one_drawn_here() {
        let committed =
            fs::read(brand_dir().join("corgi.png")).expect("herdr-app/mods/corgi-brand/corgi.png");
        assert!(
            committed == brand_tile_png(),
            "run `cargo brand-tile` to draw the brand row's logo again"
        );
    }

    /// Draws the brand row's logo into `herdr-app/mods/corgi-brand/`; run it
    /// with `cargo brand-tile`.
    #[test]
    #[ignore = "writes the brand row's logo; run it with `cargo brand-tile`"]
    fn brand_tile_png_file() {
        fs::write(brand_dir().join("corgi.png"), brand_tile_png()).expect("write corgi.png");
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
