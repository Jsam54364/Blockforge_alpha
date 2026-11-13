//! Block textures for the renderer: the 32 x 32 PNGs in `assets/textures` (made from CC0
//! photos by `tools/make_textures.py`), and textures painted in code (`texgen`) for the rest.
//!
//! The result is RGBA8 sRGB, but alpha is not transparency: it says how much of the biome's
//! grass colour to apply (255 = none, 0 = all). Grass tops and leaves are greyscale and fully
//! tinted; a grass block's side is dirt plus `<name>_overlay.png`, a greyscale fringe that is
//! tinted where the overlay is opaque (Minecraft's convention).

use crate::texgen::{self, Pixels, SIZE};

macro_rules! files {
    ($($n:literal),* $(,)?) => {
        &[$(($n, include_bytes!(concat!("../../assets/textures/", $n, ".png")) as &[u8])),*]
    };
}

const FILES: &[(&str, &[u8])] = files!(
    "stone", "dirt", "sand", "gravel", "snow", "log_side", "bedrock",
    "grass_top", "grass_side", "grass_side_overlay",
    "dry_grass_top", "dry_grass_side", "dry_grass_side_overlay",
    "lush_grass_top", "lush_grass_side", "lush_grass_side_overlay",
    "cold_grass_top", "cold_grass_side", "cold_grass_side_overlay",
);

/// Textures multiplied by the biome colour all over.
fn fully_tinted(name: &str) -> bool {
    name.ends_with("grass_top") || name == "leaves"
}

fn file(name: &str) -> Option<&'static [u8]> {
    FILES.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
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
        assert!(load("leaves").chunks(4).all(|p| p[3] == 0));
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
