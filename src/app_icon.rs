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
//! `herdr-app/mods/corgi-brand/corgi.png`, a finer pixel corgi beside
//! Herdr's own logo (see `brand_logo_svg`): `cargo brand-tile` draws it again.

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

/// The sidebar brand row's Corgi logo (`herdr-app/mods/corgi-brand/corgi.png`)
/// as pixel art finer than [`CORGI_PIXELS`]: the same face on a 32 by 32
/// grid, the left half drawn here and mirrored. `.` is clear; the others are
/// [`BRAND_COLOURS`]'. The face's painted cells run from row and column 5 to
/// 26, so it is centred on the canvas.
const BRAND_LEFT: [&str; 32] = [
    "................",
    "................",
    "................",
    "................",
    "................",
    "......oo........",
    "......ooo.......",
    ".....oodoo......",
    ".....odddoo.....",
    ".....oddddoo....",
    ".....oddddoooooo",
    ".....odddooooooo",
    ".....oddoooooooo",
    ".....ooooooooooo",
    ".....ooookkkooow",
    ".....ooookkhooww",
    ".....ooookkkooww",
    ".....oooooooowww",
    ".....wwwooowwwww",
    ".....wwwwwwwwkkk",
    ".....wwwwwwwwwkk",
    "......wwwwwwwwwk",
    "......wwwwwkwwwk",
    ".......wwwwwkkpp",
    ".......wwwwwwwpp",
    "........wwwwwwwp",
    "...........wwwww",
    "................",
    "................",
    "................",
    "................",
    "................",
];

/// The colours of [`BRAND_LEFT`]'s cells: coat, inner ear, white, glint,
/// dark (eyes, nose, mouth) and tongue.
const BRAND_COLOURS: [(char, &str); 6] = [
    ('o', "#ff8700"),
    ('d', "#d75f00"),
    ('w', "#f4f4f4"),
    ('h', "#ffffff"),
    ('k', "#1c1c1c"),
    ('p', "#ff8787"),
];

/// The jaw, from one cheek to the other: the curve the stepped cells make
/// once the 20 px logo smooths them, which the logo is cut to so that its
/// outline is that curve on any background.
const BRAND_JAW: &str = "M25 90 V104 C27 118 40 132 62 135 H98 C120 132 133 118 135 104 V90";

/// The edge drawn along [`BRAND_JAW`], inside the white, so that the white
/// face keeps its outline on a light sidebar, where it would vanish: a warm
/// grey that hardly shows on a dark one.
const BRAND_EDGE: &str = "#b0aa9e";

/// The brand logo as SVG on a [`TILE`] px canvas, the size of Herdr's logo
/// beside it (`frame/welcome/herdr.png` in the Herdr app's home), which the
/// app shows at 20 px too: the cells of [`BRAND_LEFT`] (each 5 px, crisp),
/// cut to [`BRAND_JAW`] below the eyes with white behind them up to it, and
/// the jaw edged in [`BRAND_EDGE`] 5 px wide, one cell. A cell edge would
/// step along the jaw and, where the white vanishes on a light sidebar, show
/// as a broken stair rather than the curve a dark one shows.
fn brand_logo_svg() -> String {
    let colour = |cell: char| {
        BRAND_COLOURS
            .iter()
            .find(|(key, _)| *key == cell)
            .map(|(_, hex)| *hex)
    };
    let white = colour('w').expect("white");
    let mut cells = String::new();
    for (y, left) in BRAND_LEFT.iter().enumerate() {
        let row: String = left.chars().chain(left.chars().rev()).collect();
        for (x, cell) in row.chars().enumerate() {
            if let Some(hex) = colour(cell) {
                let (x, y) = (x * 5, y * 5);
                cells.push_str(&format!(
                    r#"<rect x="{x}" y="{y}" width="5" height="5" fill="{hex}"/>"#
                ));
            }
        }
    }
    // Everything above the jaw's ends, and inside the jaw below them.
    let face = "M0 0 H160 V90 H135 V104 C133 118 120 132 98 135 H62 C40 132 27 118 25 104 V90 H0 Z";
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {TILE} {TILE}" width="{TILE}" height="{TILE}"><defs><clipPath id="face"><path d="{face}"/></clipPath></defs><g clip-path="url(#face)"><path d="{BRAND_JAW} Z" fill="{white}"/><g shape-rendering="crispEdges">{cells}</g><path d="{BRAND_JAW}" fill="none" stroke="{BRAND_EDGE}" stroke-width="10"/></g></svg>"##
    )
}

/// [`brand_logo_svg`] drawn as straight (not premultiplied) RGBA.
fn brand_tile() -> Vec<[u8; 4]> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_str(&brand_logo_svg(), &usvg::Options::default())
        .expect("the brand logo's SVG");
    let mut pixmap = tiny_skia::Pixmap::new(TILE, TILE).expect("pixmap");
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    pixmap
        .pixels()
        .iter()
        .map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect()
}

/// The brand logo's canvas, as Herdr's logo's.
const TILE: u32 = 160;

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
    fn the_brand_logo_is_the_finer_corgi_centred_with_its_jaw_edged() {
        assert!(BRAND_LEFT.iter().all(|row| row.len() == 16));
        let pixels = brand_tile();
        let at = |x: u32, y: u32| pixels[(y * TILE + x) as usize];
        let hex = |cell: char| {
            let (_, hex) = BRAND_COLOURS.iter().find(|(key, _)| *key == cell).unwrap();
            let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
            [channel(1), channel(3), channel(5), 255]
        };
        assert_eq!(at(0, 0)[3], 0, "no plate: the corner is clear");
        // A cell in the middle of each feature keeps its colour.
        assert_eq!(at(8 * 5 + 2, 10 * 5 + 2), hex('d'), "the inner ear");
        assert_eq!(at(10 * 5 + 2, 14 * 5 + 2), hex('k'), "the left eye");
        assert_eq!(at(11 * 5 + 2, 15 * 5 + 2), hex('h'), "its glint");
        assert_eq!(at(80, 24 * 5 + 2), hex('p'), "the tongue");
        // The jaw is edged inside the white, and clear outside it.
        let edge = [0xb0, 0xaa, 0x9e, 255];
        assert_eq!(at(80, 133), edge, "the chin's edge");
        assert_eq!(at(27, 98), edge, "a cheek's edge");
        assert_eq!(at(80, 136)[3], 0, "below the chin");
        assert_eq!(at(22, 98)[3], 0, "beside a cheek");
        // Measured in the drawing: the face is centred, left to right and
        // top to bottom, and mirrored.
        let painted = |x: u32, y: u32| at(x, y)[3] > 0;
        let columns: Vec<u32> = (0..TILE)
            .filter(|&x| (0..TILE).any(|y| painted(x, y)))
            .collect();
        let rows: Vec<u32> = (0..TILE)
            .filter(|&y| (0..TILE).any(|x| painted(x, y)))
            .collect();
        let (x0, x1) = (columns[0], columns[columns.len() - 1] + 1);
        let (y0, y1) = (rows[0], rows[rows.len() - 1] + 1);
        assert_eq!((x0, TILE - x1), (25, 25), "left and right");
        assert_eq!((y0, TILE - y1), (25, 25), "top and bottom");
        for y in 0..TILE {
            for x in 0..TILE / 2 {
                // The jaw's antialiasing may round a little differently on
                // each side; compared as they show, premultiplied.
                let shown = |[r, g, b, a]: [u8; 4]| {
                    [r, g, b]
                        .map(|c| u16::from(c) * u16::from(a) / 255)
                        .into_iter()
                        .chain([u16::from(a)])
                };
                let (left, right) = (at(x, y), at(TILE - 1 - x, y));
                assert!(
                    shown(left)
                        .zip(shown(right))
                        .all(|(a, b)| a.abs_diff(b) <= 16),
                    "mirrored at {x}, {y}: {left:?} {right:?}"
                );
            }
        }
    }

    /// Fails when the logo changed and the committed one was not drawn
    /// again with `cargo brand-tile`. It allows a step of antialiasing,
    /// which may round differently on another processor.
    #[test]
    fn the_committed_brand_tile_is_the_one_drawn_here() {
        let file = fs::File::open(brand_dir().join("corgi.png"))
            .expect("herdr-app/mods/corgi-brand/corgi.png");
        let mut reader = png::Decoder::new(std::io::BufReader::new(file))
            .read_info()
            .expect("a PNG");
        let mut committed = vec![0; reader.output_buffer_size().expect("a size")];
        let info = reader.next_frame(&mut committed).expect("its pixels");
        assert_eq!((info.width, info.height), (TILE, TILE));
        assert_eq!(info.color_type, png::ColorType::Rgba);
        let drawn = brand_tile();
        let close = committed
            .chunks(4)
            .zip(&drawn)
            .all(|(old, new)| old.iter().zip(new).all(|(a, b)| a.abs_diff(*b) <= 2));
        assert!(
            close,
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
