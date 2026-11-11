//! The Corgi logo of the Herdr app's sidebar brand row,
//! `herdr-app/mods/corgi/corgi.png`: a pixel corgi beside Herdr's own
//! logo (see `brand_logo_svg`). `cargo brand-tile`, the alias for running
//! the ignored test at the bottom of this file, draws it again; another
//! test fails when the committed file is not the one drawn here.

/// The sidebar brand row's Corgi logo (`herdr-app/mods/corgi/corgi.png`)
/// as pixel art: a corgi's face on a 32 by 32 grid, the left half drawn here
/// and mirrored. `.` is clear; the others are
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
#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::*;

    fn brand_dir() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/herdr-app/mods/corgi"))
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
        let file =
            fs::File::open(brand_dir().join("corgi.png")).expect("herdr-app/mods/corgi/corgi.png");
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

    /// Draws the brand row's logo into `herdr-app/mods/corgi/`; run it
    /// with `cargo brand-tile`.
    #[test]
    #[ignore = "writes the brand row's logo; run it with `cargo brand-tile`"]
    fn brand_tile_png_file() {
        fs::write(brand_dir().join("corgi.png"), brand_tile_png()).expect("write corgi.png");
    }
}
