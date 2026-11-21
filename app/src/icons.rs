//! Block pictures for the hotbar and the block picker: each block drawn as a small cube seen
//! from above and to the side, from its own textures, on the CPU when the renderer starts.

use blockforge::coords::Face;
use blockforge::block::{BlockState, Shape};
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

/// The icons drawn so far: thumbnail-texture slots handed out as items show on screen,
/// the least recently shown given up when all are taken (there are thousands of items and
/// room for `gfx::ICON_SLOTS` icons). The app draws the ones handed out (`take_pending`).
struct IconCache {
    /// item -> (slot, drawn yet)
    map: std::collections::HashMap<u16, (usize, bool)>,
    /// Each slot's item and the frame it last showed.
    slots: Vec<(u16, u64)>,
    frame: u64,
    pending: Vec<(usize, u16)>,
}

static CACHE: std::sync::Mutex<Option<IconCache>> = std::sync::Mutex::new(None);

fn with_cache<R>(f: impl FnOnce(&mut IconCache) -> R) -> R {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let c = g.get_or_insert_with(IconCache::new);
    f(c)
}

/// The slot of item `item`'s icon, once it is drawn (until then None; it is queued).
pub fn icon_slot(item: u16) -> Option<usize> {
    with_cache(|c| c.slot(item, crate::gfx::ICON_SLOTS))
}

impl IconCache {
    fn new() -> IconCache {
        IconCache { map: Default::default(), slots: Vec::new(), frame: 1, pending: Vec::new() }
    }

    /// `icon_slot` with room for `cap` icons.
    fn slot(&mut self, item: u16, cap: usize) -> Option<usize> {
        let c = self;
        let frame = c.frame;
        if let Some(&(slot, drawn)) = c.map.get(&item) {
            c.slots[slot].1 = frame;
            return drawn.then_some(slot);
        }
        let slot = if c.slots.len() < cap {
            c.slots.push((item, frame));
            c.slots.len() - 1
        } else {
            // The slot shown longest ago, if not this frame.
            let (slot, &(old, used)) = c.slots.iter().enumerate().min_by_key(|(_, s)| s.1)?;
            if used >= frame {
                return None;
            }
            c.map.remove(&old);
            c.pending.retain(|p| p.0 != slot);
            c.slots[slot] = (item, frame);
            slot
        };
        c.map.insert(item, (slot, false));
        c.pending.push((slot, item));
        None
    }
}

/// A new frame of the UI (icons shown before it may be given up).
pub fn next_icon_frame() {
    with_cache(|c| c.frame += 1);
}

/// Up to `max` icons to draw now: (slot, item). They count as drawn.
pub fn take_pending(max: usize) -> Vec<(usize, u16)> {
    with_cache(|c| {
        let n = c.pending.len().min(max);
        let out: Vec<_> = c.pending.drain(..n).collect();
        for &(_, item) in &out {
            if let Some(e) = c.map.get_mut(&item) {
                e.1 = true;
            }
        }
        out
    })
}

/// Forgets every icon (a new thumbnails texture).
pub fn reset_icons() {
    with_cache(|c| {
        c.map.clear();
        c.slots.clear();
        c.pending.clear();
    })
}

/// The inventory's groups, as tabs, in order: (key in blocks.toml, name, icon).
pub const GROUPS: [(&str, &str, &str); 4] =
    [("building", "Building", "fort"), ("natural", "Natural", "landscape"), ("wood", "Wood", "forest"), ("minerals", "Minerals", "diamond")];

/// The biomes blocks belong to (their `biome` in blocks.toml) and their names: the Blocks
/// window shows one at a time, or all (filter 0; biome n is filter n + 1).
pub const BIOMES: [(&str, &str); 15] = [
    ("wildwood", "Ancient Wildwood"),
    ("amberwood", "Amberwood"),
    ("frostpeak", "Frostpeak Highlands"),
    ("stonefang", "Stonefang Mountains"),
    ("sunscorched", "Sunscorched Expanse"),
    ("oasis", "Emerald Oasis"),
    ("murkwater", "Murkwater Swamp"),
    ("tundra", "Whitewind Tundra"),
    ("glacier", "Crystal Glacier"),
    ("taiga", "Winterpine Taiga"),
    ("blight", "Blightlands"),
    ("petrified", "Petrified Kingdom"),
    ("hollow", "Hollow Earth"),
    ("stormbreak", "Stormbreak Coast"),
    ("grasslands", "Giants' Grasslands"),
];

/// A block's biome filter (`BIOMES` index + 1; 0: none).
pub fn biome_of(reg: &BlockRegistry, id: u16) -> u8 {
    reg.def(id).biome.as_deref().and_then(|b| BIOMES.iter().position(|(k, _)| *k == b)).map_or(0, |i| i as u8 + 1)
}

/// The group of a block (its index in `GROUPS`; unknown groups are natural).
pub fn group_of(reg: &BlockRegistry, id: u16) -> u8 {
    let g = &reg.def(id).group;
    GROUPS.iter().position(|(k, _, _)| k == g).unwrap_or(1) as u8
}

/// The slab item of block `id` (its bottom-slab state: item ids are block states).
pub fn slab_item(id: u16) -> u16 {
    BlockState::new(id, Shape::SlabBottom).0
}

/// Whether an item is a slab.
pub fn is_slab(item: u16) -> bool {
    BlockState(item).shape() != Shape::Full
}

/// Everything the inventory offers, in its order: by group (`GROUPS`), then by family (in
/// the order families first appear in blocks.toml), then in the file's order, each block's
/// slab just after it. An item is a block state: a placeable block's id, or its slab's
/// state. The icons are drawn in this order (`gfx::set_block_icons`).
pub fn items(reg: &BlockRegistry) -> Vec<u16> {
    let blocks = placeable(reg);
    let mut families: Vec<&str> = Vec::new();
    for id in 1..reg.len() as u16 {
        let f = reg.def(id).family.as_str();
        if !families.contains(&f) {
            families.push(f);
        }
    }
    let rank = |id: u16| {
        let fam = families.iter().position(|f| *f == reg.def(id).family).unwrap_or(usize::MAX);
        (group_of(reg, id), fam, id)
    };
    let mut sorted = blocks.clone();
    sorted.sort_by_key(|&id| rank(id));
    let mut out = Vec::new();
    for id in sorted {
        out.push(id);
        if reg.def(id).slab {
            out.push(slab_item(id));
        }
    }
    out
}

/// An item's name: the block's label (or its name, "dry_grass" -> "Dry Grass"), "Slab" after
/// a slab's.
pub fn item_name(reg: &BlockRegistry, item: u16) -> String {
    let d = reg.def(item);
    let base = d.label.clone().unwrap_or_else(|| display_name(&d.name));
    if is_slab(item) {
        format!("{base} Slab")
    } else {
        base
    }
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

/// The item `id` as a cube icon (a slab: the lower half of one): `ICON` x `ICON` RGBA,
/// transparent around it.
pub fn render(reg: &BlockRegistry, id: u16) -> Vec<u8> {
    let def = reg.def(id);
    // How tall it is (a slab: half, its sides showing the lower half of their texture).
    let tall = if is_slab(id) { 0.5 } else { 1.0 };
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
    let ay = [0.0, -vh * tall];
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
                        Some((1, u, 1.0 - v * tall))
                    } else {
                        solve(p, add(c, ax), az, ay).map(|(u, v)| (2, 1.0 - u, 1.0 - v * tall))
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
    fn icons_are_drawn_as_they_show_and_the_oldest_give_way() {
        let mut c = IconCache::new();
        // Asked for: queued, not drawn yet.
        assert_eq!(c.slot(10, 2), None);
        assert_eq!(c.slot(11, 2), None);
        assert_eq!(c.pending, vec![(0, 10), (1, 11)]);
        c.map.values_mut().for_each(|e| e.1 = true);
        c.pending.clear();
        assert_eq!((c.slot(10, 2), c.slot(11, 2)), (Some(0), Some(1)));
        // Full, and both shown this frame: nothing gives way.
        assert_eq!(c.slot(12, 2), None);
        assert!(c.pending.is_empty());
        // Next frame, with 11 shown again, 10 gives way to 12.
        c.frame += 1;
        c.slot(11, 2);
        assert_eq!(c.slot(12, 2), None);
        assert_eq!(c.pending, vec![(0, 12)]);
        assert!(!c.map.contains_key(&10));
    }

    #[test]
    fn every_placeable_block_gets_a_cube_with_three_lit_faces() {
        let reg = BlockRegistry::builtin();
        let ids = placeable(&reg);
        let names: Vec<&str> = ids.iter().map(|&i| reg.def(i).name.as_str()).collect();
        assert!(names.contains(&"stone") && names.contains(&"grass") && names.contains(&"log"));
        assert!(!names.contains(&"air") && !names.contains(&"water") && !names.contains(&"bedrock"));
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
