//! Block textures for the renderer: the 32 x 32 PNGs in `assets/textures` (original pixel
//! art, painted by `tools/paint_textures.py`), and `texgen`'s for any name without a file.
//!
//! The result is RGBA8 sRGB, but alpha is not transparency: it says how much of the biome's
//! grass colour to apply (255 = none, 0 = all). Grass tops and leaves are greyscale and fully
//! tinted; a grass block's side is dirt plus `<name>_overlay.png`, a greyscale fringe that is
//! tinted where the overlay is opaque (Minecraft's convention).
//!
//! Foliage (leaves and plants, see `is_foliage`) is see-through, drawn alpha-tested: its alpha
//! holds both, in two ranges that stay apart when mip levels average them: a tinted texture
//! is 0 (hole) .. 127 (leaf), an untinted one 128 (hole) .. 255 (leaf). Leaf textures without
//! an alpha channel get holes where they are darkest (the gaps between leaves).

use crate::texgen::{self, Pixels, SIZE};

macro_rules! files {
    ($($n:literal),* $(,)?) => {
        &[$(($n, include_bytes!(concat!("../../assets/textures/", $n, ".png")) as &[u8])),*]
    };
}

// Every texture in assets/textures (written by tools/blockset.py).
include!("texture_files.rs");

/// The plant sprites (`tools/paint_plants.py`), drawn on cross-shaped blocks.
pub const PLANTS: [&str; 15] = [
    "short_grass",
    "grass_tuft",
    "tall_grass",
    "dry_tuft",
    "fern",
    "leafy_bush",
    "berry_bush",
    "dandelion",
    "poppy",
    "cornflower",
    "daisy",
    "lavender",
    "dead_bush",
    "reeds",
    "heather",
];

/// Plant sprites in greyscale, tinted like the grass around them.
const TINTED_PLANTS: [&str; 5] = ["short_grass", "grass_tuft", "tall_grass", "fern", "leafy_bush"];

/// Textures multiplied by the biome colour all over.
fn fully_tinted(name: &str) -> bool {
    name.ends_with("grass_top") || name == "leaves" || TINTED_PLANTS.contains(&name)
}

/// See-through foliage: leaves and plants (alpha-tested, swaying; see the module docs).
pub fn is_foliage(name: &str) -> bool {
    name == "leaves" || name.ends_with("_leaves") || PLANTS.contains(&name)
}

/// Which pixels of a foliage texture are solid. Its own alpha where it has one; else (leaves
/// painted without holes) holes in clumps a few pixels across, where the texture is dark (the
/// shadowed gaps between leaves) and a blotchy noise that tiles with the texture agrees: about
/// a third, as Minecraft's leaves have.
fn coverage(px: &[u8]) -> Vec<bool> {
    if px.chunks(4).any(|p| p[3] < 255) {
        return px.chunks(4).map(|p| p[3] >= 128).collect();
    }
    let lum: Vec<u32> = px.chunks(4).map(|p| p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19).collect();
    let mut order: Vec<usize> = (0..lum.len()).collect();
    order.sort_by_key(|&i| lum[i]);
    let mut rank = vec![0.0f32; lum.len()];
    for (r, &i) in order.iter().enumerate() {
        rank[i] = r as f32 / lum.len() as f32;
    }
    // Value noise on an 8 x 8 lattice that wraps (cells of 4 pixels), smoothly blended.
    let cells = 8usize;
    let step = SIZE / cells;
    let lattice = |cx: usize, cy: usize| {
        let h = blockforge::rng::hash3(0x1EAF, (cx % cells) as i32, 0, (cy % cells) as i32);
        (h % 1000) as f32 / 1000.0
    };
    let score: Vec<f32> = (0..SIZE * SIZE)
        .map(|i| {
            let (x, y) = (i % SIZE, i / SIZE);
            let (cx, cy) = (x / step, y / step);
            let (fx, fy) = ((x % step) as f32 / step as f32, (y % step) as f32 / step as f32);
            let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
            let top = lattice(cx, cy) + (lattice(cx + 1, cy) - lattice(cx, cy)) * sx;
            let bottom = lattice(cx, cy + 1) + (lattice(cx + 1, cy + 1) - lattice(cx, cy + 1)) * sx;
            0.55 * rank[i] + 0.45 * (top + (bottom - top) * sy)
        })
        .collect();
    let mut sorted = score.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let cut = sorted[sorted.len() * 32 / 100];
    let mut solid: Vec<bool> = score.iter().map(|&s| s >= cut).collect();
    // No lone leaf pixels floating in a hole (they flicker at a distance).
    let snapshot = solid.clone();
    for i in 0..solid.len() {
        let (x, y) = (i % SIZE, i / SIZE);
        let n = [(1, 0), (SIZE - 1, 0), (0, 1), (0, SIZE - 1)]
            .iter()
            .filter(|(dx, dy)| snapshot[((y + dy) % SIZE) * SIZE + (x + dx) % SIZE])
            .count();
        if n == 0 {
            solid[i] = false;
        }
    }
    solid
}

fn file(name: &str) -> Option<&'static [u8]> {
    // (FILES is sorted by name: tools/blockset.py writes it so)
    FILES.binary_search_by(|(n, _)| (*n).cmp(name)).ok().map(|i| FILES[i].1)
}

/// Decodes a 32 x 32 PNG (grey, grey + alpha, RGB or RGBA) to RGBA8.
pub fn decode(bytes: &[u8]) -> Result<Pixels, String> {
    let mut dec = png::Decoder::new(bytes);
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.width as usize != SIZE || info.height as usize != SIZE {
        return Err(format!("{}x{}, not {SIZE}x{SIZE}", info.width, info.height));
    }
    let px = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Rgb => px.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::Rgba => px.to_vec(),
        other => return Err(format!("unsupported colour type {other:?}")),
    };
    Ok(rgba)
}

/// The texture called `name`, alpha = 255 minus how much to tint (see the module docs).
pub fn load(name: &str) -> Pixels {
    let mut px = match file(name).map(decode) {
        Some(Ok(p)) => p,
        Some(Err(e)) => panic!("assets/textures/{name}.png: {e}"),
        None => texgen::generate(name),
    };
    let tint = fully_tinted(name);
    if is_foliage(name) {
        let solid = coverage(&px);
        // Holes take the average colour of the leaves, so mip levels blend no dark fringe in.
        let mut sum = [0u64; 3];
        let n = solid.iter().filter(|&&s| s).count().max(1) as u64;
        for (p, _) in px.chunks(4).zip(&solid).filter(|(_, s)| **s) {
            for c in 0..3 {
                sum[c] += p[c] as u64;
            }
        }
        for (p, &s) in px.chunks_mut(4).zip(&solid) {
            if !s {
                for c in 0..3 {
                    p[c] = (sum[c] / n) as u8;
                }
            }
            p[3] = match (tint, s) {
                (true, true) => 127,
                (true, false) => 0,
                (false, true) => 255,
                (false, false) => 128,
            };
        }
        return px;
    }
    for p in px.chunks_mut(4) {
        p[3] = if tint { 0 } else { 255 };
    }
    if let Some(Ok(over)) = file(&format!("{name}_overlay")).map(decode) {
        for (p, o) in px.chunks_mut(4).zip(over.chunks(4)) {
            let a = o[3] as u32;
            for c in 0..3 {
                p[c] = ((p[c] as u32 * (255 - a) + o[c] as u32 * a) / 255) as u8;
            }
            p[3] = 255 - o[3];
        }
    }
    px
}

/// Whether `name` has a real texture (a file or a painter), not the missing-texture pattern.
pub fn known(name: &str) -> bool {
    file(name).is_some() || texgen::known(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_decodes_to_32_by_32() {
        assert!(FILES.windows(2).all(|w| w[0].0 < w[1].0), "FILES is sorted");
        for (name, bytes) in FILES {
            let px = decode(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(px.len(), SIZE * SIZE * 4, "{name}");
        }
    }

    #[test]
    fn grass_is_tinted_where_it_grows_and_nowhere_else() {
        let top = load("grass_top");
        assert!(top.chunks(4).all(|p| p[3] == 0 && p[0] == p[1] && p[1] == p[2]), "greyscale, fully tinted");
        let side = load("grass_side");
        let tinted_rows = |y: usize| (0..SIZE).filter(|&x| side[(y * SIZE + x) * 4 + 3] < 128).count();
        assert_eq!(tinted_rows(0), SIZE, "the fringe starts at the top");
        assert_eq!(tinted_rows(SIZE - 1), 0, "dirt below");
        assert!(load("stone").chunks(4).all(|p| p[3] == 255));
    }

    #[test]
    fn foliage_is_see_through_with_tint_and_holes_in_two_alpha_ranges() {
        // Tinted leaves: 127 (leaf) or 0 (hole), about a fifth holes.
        let l = load("leaves");
        assert!(l.chunks(4).all(|p| p[3] == 127 || p[3] == 0));
        let holes = l.chunks(4).filter(|p| p[3] == 0).count();
        assert!((280..400).contains(&holes), "{holes}");
        // Untinted leaves: 255 or 128.
        assert!(load("birch_leaves").chunks(4).all(|p| p[3] == 255 || p[3] == 128));
        // A plant sprite is mostly hole; a tinted one in the low range.
        let g = load("tall_grass");
        assert!(g.chunks(4).all(|p| p[3] <= 127));
        assert!(g.chunks(4).filter(|p| p[3] == 0).count() > SIZE * SIZE / 4);
        let f = load("poppy");
        assert!(f.chunks(4).all(|p| p[3] >= 128));
        for p in PLANTS {
            assert!(file(p).is_some(), "{p}.png");
        }
    }

    #[test]
    fn every_builtin_block_texture_exists() {
        let reg = blockforge::registry::BlockRegistry::builtin();
        for name in reg.texture_names() {
            if name != "air" {
                assert!(known(name), "no texture for {name}");
            }
        }
    }
}
