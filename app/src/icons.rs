//! Block pictures for the hotbar and the block picker: each block drawn as a small cube seen
//! from above and to the side, from its own textures, on the CPU when the renderer starts.

use blockforge::coords::Face;
use blockforge::registry::BlockRegistry;

use crate::texgen::SIZE as TEX;
use crate::textures;

/// Icons are `ICON` x `ICON` RGBA pixels.
pub const ICON: u32 = 64;

/// Grass and leaves are grey in their textures and coloured by the biome where they grow;
/// their icons use the plains' green (as the terrain does before the biome colours arrive).
const GREEN: [f32; 3] = [0x91 as f32 / 255.0, 0xBD as f32 / 255.0, 0x59 as f32 / 255.0];

/// How each face is lit: top, left (+Z) and right (+X).
const SHADE: [f32; 3] = [1.0, 0.80, 0.62];

/// The blocks a player can place, in the registry's order: solid, breakable, not water.
pub fn placeable(reg: &BlockRegistry) -> Vec<u16> {
    (1..reg.len() as u16)
        .filter(|&id| {
            let d = reg.def(id);
            d.solid && d.hardness >= 0.0 && d.name != "water"
        })
        .collect()
}

/// "dry_grass" -> "Dry Grass".
pub fn display_name(name: &str) -> String {
    name.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A face's texture as linear-ish RGB (0..1), tinted where the texture asks for it.
fn face_texels(reg: &BlockRegistry, tex: u16) -> Vec<[f32; 3]> {
    let name = reg.texture_names().get(tex as usize).map(String::as_str).unwrap_or("");
    let px = textures::load(name);
    px.chunks(4)
        .map(|p| {
            // alpha = 255 minus how much of the biome colour to apply
            let t = 1.0 - p[3] as f32 / 255.0;
            let mut c = [0.0; 3];
            for i in 0..3 {
                c[i] = p[i] as f32 / 255.0 * (1.0 + (GREEN[i] - 1.0) * t);
            }
            c
        })
        .collect()
}

/// The block `id` as a cube icon: `ICON` x `ICON` RGBA, transparent around the cube.
pub fn render(reg: &BlockRegistry, id: u16) -> Vec<u8> {
    let def = reg.def(id);
    let faces = [
        face_texels(reg, def.tex[Face::PosY.index()]),
        face_texels(reg, def.tex[Face::PosZ.index()]),
        face_texels(reg, def.tex[Face::PosX.index()]),
    ];
    let s = ICON as f32;
    // A 2:1 cube: x runs right and down, z left and down, y up.
    let hx = 0.44 * s;
    let hy = hx * 0.5;
    let vh = hx * 1.155;
    let top = (s - (vh + 2.0 * hy)) * 0.5;
    let c = [s * 0.5, top + vh];
    let ax = [hx, hy];
    let az = [-hx, hy];
    let ay = [0.0, -vh];
    // p = o + a*u + b*v  ->  (u, v), or None if outside the face.
    let solve = |p: [f32; 2], o: [f32; 2], a: [f32; 2], b: [f32; 2]| -> Option<(f32, f32)> {
        let det = a[0] * b[1] - a[1] * b[0];
        let (dx, dy) = (p[0] - o[0], p[1] - o[1]);
        let u = (dx * b[1] - dy * b[0]) / det;
        let v = (a[0] * dy - a[1] * dx) / det;
        ((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)).then_some((u, v))
    };
    let add = |a: [f32; 2], b: [f32; 2]| [a[0] + b[0], a[1] + b[1]];
    let texel = |f: &Vec<[f32; 3]>, u: f32, v: f32| {
        let x = ((u * TEX as f32) as usize).min(TEX - 1);
        let y = ((v * TEX as f32) as usize).min(TEX - 1);
        f[y * TEX + x]
    };
    const SS: usize = 4;
    let mut out = vec![0u8; (ICON * ICON * 4) as usize];
    for py in 0..ICON as usize {
        for px in 0..ICON as usize {
            let mut acc = [0.0f32; 3];
            let mut hits = 0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let p = [px as f32 + (sx as f32 + 0.5) / SS as f32, py as f32 + (sy as f32 + 0.5) / SS as f32];
                    let found = if let Some((u, v)) = solve(p, add(c, ay), ax, az) {
                        Some((0, u, v))
                    } else if let Some((u, v)) = solve(p, add(c, az), ax, ay) {
                        Some((1, u, 1.0 - v))
                    } else {
                        solve(p, add(c, ax), az, ay).map(|(u, v)| (2, 1.0 - u, 1.0 - v))
                    };
                    if let Some((f, u, v)) = found {
                        let t = texel(&faces[f], u, v);
                        for i in 0..3 {
                            acc[i] += t[i] * SHADE[f];
                        }
                        hits += 1;
                    }
                }
            }
            if hits > 0 {
                let o = (py * ICON as usize + px) * 4;
                for i in 0..3 {
                    out[o + i] = ((acc[i] / hits as f32).clamp(0.0, 1.0) * 255.0).round() as u8;
                }
                out[o + 3] = (hits * 255 / (SS * SS)) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_placeable_block_gets_a_cube_with_three_lit_faces() {
        let reg = BlockRegistry::builtin();
        let ids = placeable(&reg);
        let names: Vec<&str> = ids.iter().map(|&i| reg.def(i).name.as_str()).collect();
        assert!(names.contains(&"stone") && names.contains(&"grass") && names.contains(&"log"));
        assert!(!names.contains(&"air") && !names.contains(&"water") && !names.contains(&"bedrock"));
        assert!(ids.len() <= 32, "{} icons: the thumbnails texture has room for 32", ids.len());
        for &id in &ids {
            let px = render(&reg, id);
            assert_eq!(px.len(), (ICON * ICON * 4) as usize);
            let at = |x: u32, y: u32| &px[((y * ICON + x) * 4) as usize..((y * ICON + x) * 4 + 4) as usize];
            // Corners are empty, the middle of each face is solid.
            assert_eq!(at(0, 0)[3], 0);
            assert_eq!(at(ICON - 1, ICON - 1)[3], 0);
            let (top, left, right) = (at(ICON / 2, 12), at(ICON / 4, 40), at(ICON * 3 / 4, 40));
            for p in [top, left, right] {
                assert_eq!(p[3], 255, "{}", reg.def(id).name);
            }
            // The top is lit more than the right side (on average over the face).
            let lum = |p: &[u8]| p[0] as u32 + p[1] as u32 + p[2] as u32;
            let avg = |x0: u32, x1: u32, y0: u32, y1: u32| {
                let mut s = 0;
                for y in y0..y1 {
                    for x in x0..x1 {
                        s += lum(at(x, y));
                    }
                }
                s
            };
            assert!(avg(26, 38, 8, 16) > avg(44, 52, 34, 46) * 9 / 10, "{}", reg.def(id).name);
        }
        // Grass is green on top.
        let grass = render(&reg, reg.id("grass").unwrap());
        let i = ((12 * ICON + ICON / 2) * 4) as usize;
        assert!(grass[i + 1] > grass[i] && grass[i + 1] > grass[i + 2], "{:?}", &grass[i..i + 3]);
    }

    #[test]
    fn names_read_well() {
        assert_eq!(display_name("dry_grass"), "Dry Grass");
        assert_eq!(display_name("stone"), "Stone");
    }
}
