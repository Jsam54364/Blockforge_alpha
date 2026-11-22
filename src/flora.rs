//! Trees, bushes, boulders and fallen logs. Each is a pure function of the grid cell it grows
//! from, so a chunk draws exactly the parts of every feature that reach into it, and the
//! chunk next door draws the rest: no tree is ever cut at a chunk border.
//!
//! Sizes are real ones, a block being a metre: oaks 9-14 m, birches 11-16 m, spruces and firs
//! 14-26 m, pines 14-20 m, redwoods 30-45 m, acacias 6-8 m wide and flat-topped. Crowns are
//! shaped from their species' silhouette (round, tall and narrow, conical in tiers, an
//! umbrella, drooping) with a ragged edge from a per-block hash, so no two are alike.

use crate::land::Biome;
use crate::rng::hash3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Oak,
    BigOak,
    Birch,
    Maple,
    Spruce,
    Fir,
    Pine,
    Redwood,
    Acacia,
    Willow,
    Palm,
    Jungle,
    Cactus,
    Bush,
    Boulder,
    FallenLog,
}

/// The blocks features are made of.
#[derive(Clone, Debug)]
pub struct Palette {
    /// (log, leaves) per tree kind; indexed by `Kind as usize` (unused entries are 0).
    pub wood: [(u16, u16); 16],
    pub cactus: u16,
    pub boulder: [u16; 3],
}

/// A placed feature: where its base is (the first block above the ground) and its size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feature {
    pub kind: Kind,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub seed: u64,
    /// Overall height and crown radius in blocks.
    pub height: i32,
    pub radius: i32,
}

/// The farthest any feature reaches from its base, horizontally (for finding the features
/// that can reach into a chunk).
pub const MAX_REACH: i32 = 11;

fn unit(seed: u64, k: u64) -> f32 {
    (hash3(seed, k as i32, 0x51, 0x7E) >> 40) as f32 / 16_777_216.0
}

fn range(seed: u64, k: u64, lo: f32, hi: f32) -> f32 {
    lo + (hi - lo) * unit(seed, k)
}

impl Kind {
    /// Which tree grows in a biome (`r` 0..1 picks among the biome's mix).
    pub fn tree_for(biome: Biome, r: f32) -> Kind {
        match biome {
            Biome::Forest => match r {
                r if r < 0.42 => Kind::Oak,
                r if r < 0.7 => Kind::Birch,
                r if r < 0.88 => Kind::Maple,
                _ => Kind::BigOak,
            },
            Biome::OldForest => match r {
                r if r < 0.35 => Kind::Redwood,
                r if r < 0.65 => Kind::Spruce,
                r if r < 0.85 => Kind::BigOak,
                _ => Kind::Fir,
            },
            Biome::Taiga => match r {
                r if r < 0.45 => Kind::Spruce,
                r if r < 0.8 => Kind::Fir,
                _ => Kind::Pine,
            },
            Biome::Tundra => Kind::Spruce,
            Biome::Plains => match r {
                r if r < 0.6 => Kind::Oak,
                r if r < 0.85 => Kind::Birch,
                _ => Kind::BigOak,
            },
            Biome::Steppe => Kind::Pine,
            Biome::Swamp => if r < 0.7 { Kind::Willow } else { Kind::Oak },
            Biome::Savanna => Kind::Acacia,
            Biome::Jungle => if r < 0.6 { Kind::Jungle } else { Kind::Palm },
            Biome::Desert | Biome::Mesa => Kind::Cactus,
            Biome::Beach => Kind::Palm,
            Biome::Peaks | Biome::Ocean => Kind::Bush,
        }
    }

    pub fn is_tree(self) -> bool {
        !matches!(self, Kind::Cactus | Kind::Bush | Kind::Boulder | Kind::FallenLog)
    }
}

impl Feature {
    /// A feature of `kind` with its base at `(x, y, z)`; its size comes from `seed`.
    pub fn new(kind: Kind, x: i32, y: i32, z: i32, seed: u64) -> Feature {
        let (h, r) = match kind {
            Kind::Oak => (range(seed, 1, 9.0, 13.0), range(seed, 2, 3.2, 4.3)),
            Kind::BigOak => (range(seed, 1, 13.0, 18.0), range(seed, 2, 5.0, 6.5)),
            Kind::Birch => (range(seed, 1, 11.0, 16.0), range(seed, 2, 2.3, 3.0)),
            Kind::Maple => (range(seed, 1, 10.0, 14.0), range(seed, 2, 3.5, 4.5)),
            Kind::Spruce => (range(seed, 1, 14.0, 24.0), range(seed, 2, 3.5, 4.8)),
            Kind::Fir => (range(seed, 1, 16.0, 26.0), range(seed, 2, 2.6, 3.4)),
            Kind::Pine => (range(seed, 1, 14.0, 20.0), range(seed, 2, 2.5, 3.5)),
            Kind::Redwood => (range(seed, 1, 30.0, 45.0), range(seed, 2, 5.0, 7.0)),
            Kind::Acacia => (range(seed, 1, 6.0, 8.0), range(seed, 2, 4.0, 5.5)),
            Kind::Willow => (range(seed, 1, 8.0, 11.0), range(seed, 2, 4.5, 5.5)),
            Kind::Palm => (range(seed, 1, 8.0, 12.0), range(seed, 2, 3.5, 4.5)),
            Kind::Jungle => (range(seed, 1, 18.0, 26.0), range(seed, 2, 5.0, 6.5)),
            Kind::Cactus => (range(seed, 1, 2.0, 5.0), 1.0),
            Kind::Bush => (range(seed, 1, 1.0, 2.5), range(seed, 2, 1.2, 2.2)),
            Kind::Boulder => (range(seed, 1, 1.5, 3.5), range(seed, 2, 1.5, 3.5)),
            Kind::FallenLog => (1.0, range(seed, 2, 4.0, 7.0)),
        };
        Feature { kind, x, y, z, seed, height: h.round() as i32, radius: r.ceil() as i32 }
    }

    /// The box the feature fills: (min, max) corners, inclusive.
    pub fn bounds(&self) -> ([i32; 3], [i32; 3]) {
        // Leaning trunks carry the crown off the base.
        let lean = match self.kind {
            Kind::Acacia => 4,
            Kind::Palm | Kind::Redwood | Kind::Jungle => 3,
            Kind::BigOak => 2,
            _ => 0,
        };
        let r = (self.radius + lean).min(MAX_REACH - 1) + 1;
        ([self.x - r, self.y - 3, self.z - r], [self.x + r + 1, self.y + self.height + 3, self.z + r + 1])
    }

    /// The tree's crown, for distant terrain: radius, and the bottom and top heights.
    pub fn crown(&self) -> Option<(i32, i32, i32)> {
        let top = self.y + self.height;
        let bottom = match self.kind {
            Kind::Oak | Kind::Maple => top - self.radius * 2,
            Kind::BigOak => top - self.radius * 2,
            Kind::Birch => top - 8,
            Kind::Spruce | Kind::Fir => self.y + 3,
            Kind::Pine => top - 4,
            Kind::Redwood => self.y + self.height / 2,
            Kind::Acacia => top - 2,
            Kind::Willow => self.y + 2,
            Kind::Palm => top - 2,
            Kind::Jungle => top - 7,
            _ => return None,
        };
        Some((self.radius, bottom, top))
    }

    /// Every block of the feature: `put(x, y, z, block, solid)`. `solid` blocks (logs, rock)
    /// replace air and leaves; leaves only fill air (the caller enforces both).
    pub fn blocks(&self, pal: &Palette, put: &mut dyn FnMut(i32, i32, i32, u16, bool)) {
        let (log, leaves) = pal.wood[self.kind as usize];
        let (x0, y0, z0, s) = (self.x, self.y, self.z, self.seed);
        let h = self.height;
        let r = self.radius as f32;
        // A ragged edge: each block of a crown's outer shell is kept or dropped by a hash.
        let ragged = |x: i32, y: i32, z: i32| (hash3(s, x, y, z) >> 40) as f32 / 16_777_216.0;
        let blob = |put: &mut dyn FnMut(i32, i32, i32, u16, bool), cx: f32, cy: f32, cz: f32, rx: f32, ry: f32| {
            let (ix0, ix1) = ((cx - rx).floor() as i32, (cx + rx).ceil() as i32);
            let (iy0, iy1) = ((cy - ry).floor() as i32, (cy + ry).ceil() as i32);
            let (iz0, iz1) = ((cz - rx).floor() as i32, (cz + rx).ceil() as i32);
            for y in iy0..=iy1 {
                for z in iz0..=iz1 {
                    for x in ix0..=ix1 {
                        let (dx, dy, dz) = ((x as f32 + 0.5 - cx) / rx, (y as f32 + 0.5 - cy) / ry, (z as f32 + 0.5 - cz) / rx);
                        let d = dx * dx + dy * dy + dz * dz;
                        if d < 1.0 - 0.35 * ragged(x, y, z) {
                            put(x, y, z, leaves, false);
                        }
                    }
                }
            }
        };
        let trunk = |put: &mut dyn FnMut(i32, i32, i32, u16, bool), x: i32, z: i32, from: i32, to: i32| {
            for y in from..to {
                put(x, y, z, log, true);
            }
        };
        let (fx, fz) = (x0 as f32 + 0.5, z0 as f32 + 0.5);
        match self.kind {
            Kind::Oak | Kind::Maple => {
                let crown_y = (y0 + h) as f32 - r;
                blob(put, fx, crown_y, fz, r, r * 0.85);
                // A few lobes make it irregular.
                for k in 0..3 {
                    let a = unit(s, 10 + k) * std::f32::consts::TAU;
                    let d = r * 0.55;
                    blob(put, fx + a.cos() * d, crown_y - 0.5 - unit(s, 20 + k) * 1.5, fz + a.sin() * d, r * 0.62, r * 0.55);
                }
                trunk(put, x0, z0, y0, y0 + h - 2);
            }
            Kind::BigOak => {
                let crown_y = (y0 + h) as f32 - r * 0.9;
                blob(put, fx, crown_y, fz, r * 0.8, r * 0.65);
                for k in 0..5 {
                    let a = (k as f32 / 5.0 + unit(s, 10 + k) * 0.15) * std::f32::consts::TAU;
                    let d = r * 0.6;
                    let (bx, bz) = (fx + a.cos() * d, fz + a.sin() * d);
                    let by = crown_y - 1.0 - unit(s, 20 + k) * 2.5;
                    blob(put, bx, by, bz, r * 0.5, r * 0.42);
                    // The branch out to the lobe.
                    for t in 1..=3 {
                        let f = t as f32 / 4.0;
                        put((fx + (bx - fx) * f).floor() as i32, (by - 1.5 * (1.0 - f)).floor() as i32, (fz + (bz - fz) * f).floor() as i32, log, true);
                    }
                }
                trunk(put, x0, z0, y0, y0 + h - 3);
                // Roots flare at the base.
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    if unit(s, 40 + (dx + 2 * dz + 3) as u64) < 0.6 {
                        put(x0 + dx, y0, z0 + dz, log, true);
                    }
                }
            }
            Kind::Birch => {
                let top = (y0 + h) as f32;
                blob(put, fx, top - 3.5, fz, r, 4.2);
                trunk(put, x0, z0, y0, y0 + h - 1);
            }
            Kind::Spruce | Kind::Fir => {
                // Tiers of foliage tapering to the tip, each a little wider at its bottom.
                let base = y0 + 3;
                let top = y0 + h;
                for y in base..=top + 1 {
                    let f = (y - base) as f32 / (top + 1 - base) as f32;
                    let tier = ((y - base) % 3) as f32 / 2.0; // 0 at a tier's bottom
                    let rr = r * (1.0 - f).powf(0.9) * (1.0 - 0.3 * tier) + 0.6;
                    let ri = rr.ceil() as i32;
                    for dz in -ri..=ri {
                        for dx in -ri..=ri {
                            let d = ((dx * dx + dz * dz) as f32).sqrt();
                            if d <= rr - 0.4 * ragged(x0 + dx, y, z0 + dz) {
                                put(x0 + dx, y, z0 + dz, leaves, false);
                            }
                        }
                    }
                }
                trunk(put, x0, z0, y0, top);
            }
            Kind::Pine => {
                let top = (y0 + h) as f32;
                blob(put, fx, top - 1.5, fz, r, 2.2);
                blob(put, fx + 1.0, top - 3.5, fz - 0.5, r * 0.7, 1.4);
                trunk(put, x0, z0, y0, y0 + h - 1);
            }
            Kind::Redwood => {
                // A 2 x 2 trunk, bare for half its height, then a tall narrow cone.
                let base = y0 + h / 2;
                let top = y0 + h;
                let (cx, cz) = (x0 as f32 + 1.0, z0 as f32 + 1.0);
                for y in base..=top + 1 {
                    let f = (y - base) as f32 / (top + 1 - base) as f32;
                    let tier = ((y - base) % 4) as f32 / 3.0;
                    let rr = r * (1.0 - f).powf(0.8) * (1.0 - 0.25 * tier) + 0.8;
                    let ri = rr.ceil() as i32 + 1;
                    for dz in -ri..=ri {
                        for dx in -ri..=ri {
                            let (px, pz) = (x0 + dx, z0 + dz);
                            let d = ((px as f32 + 0.5 - cx).powi(2) + (pz as f32 + 0.5 - cz).powi(2)).sqrt();
                            if d <= rr - 0.4 * ragged(px, y, pz) {
                                put(px, y, pz, leaves, false);
                            }
                        }
                    }
                }
                for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    trunk(put, x0 + dx, z0 + dz, y0 - 1, top);
                }
                // Buttress roots.
                for (dx, dz) in [(-1, 0), (2, 1), (0, 2), (1, -1)] {
                    trunk(put, x0 + dx, z0 + dz, y0 - 1, y0 + 1 + (unit(s, 50 + (dx + 3 * dz + 9) as u64) * 3.0) as i32);
                }
            }
            Kind::Acacia => {
                // A trunk that leans, then a flat umbrella crown.
                let lean = h - 2;
                let a = unit(s, 7) * std::f32::consts::TAU;
                let (lx, lz) = (a.cos(), a.sin());
                let mut tx = fx;
                let mut tz = fz;
                for y in y0..y0 + lean {
                    if y - y0 >= 2 {
                        tx += lx * 0.7;
                        tz += lz * 0.7;
                    }
                    put(tx.floor() as i32, y, tz.floor() as i32, log, true);
                }
                let top = (y0 + lean) as f32;
                for layer in 0..2 {
                    let rr = r - layer as f32 * 1.5;
                    let ri = rr.ceil() as i32;
                    for dz in -ri..=ri {
                        for dx in -ri..=ri {
                            let (px, pz) = (tx.floor() as i32 + dx, tz.floor() as i32 + dz);
                            let d = ((dx * dx + dz * dz) as f32).sqrt();
                            if d <= rr - 0.5 * ragged(px, layer, pz) {
                                put(px, top as i32 + layer, pz, leaves, false);
                            }
                        }
                    }
                }
            }
            Kind::Willow => {
                let crown_y = (y0 + h) as f32 - 2.5;
                blob(put, fx, crown_y, fz, r, 2.6);
                // Curtains of leaves hanging from the crown's rim.
                let ri = r as i32;
                for dz in -ri..=ri {
                    for dx in -ri..=ri {
                        let d = ((dx * dx + dz * dz) as f32).sqrt() / r;
                        if (0.55..0.98).contains(&d) && ragged(x0 + dx, 7, z0 + dz) < 0.55 {
                            let len = 2 + (ragged(x0 + dx, 9, z0 + dz) * 5.0) as i32;
                            let from = (crown_y - 1.5) as i32;
                            for y in from - len..from {
                                put(x0 + dx, y, z0 + dz, leaves, false);
                            }
                        }
                    }
                }
                trunk(put, x0, z0, y0, y0 + h - 2);
            }
            Kind::Palm => {
                // A curved trunk and a star of drooping fronds.
                let a = unit(s, 7) * std::f32::consts::TAU;
                let (lx, lz) = (a.cos(), a.sin());
                let (mut tx, mut tz) = (fx, fz);
                for y in y0..y0 + h {
                    let f = (y - y0) as f32 / h as f32;
                    tx = fx + lx * f * f * 3.0;
                    tz = fz + lz * f * f * 3.0;
                    put(tx.floor() as i32, y, tz.floor() as i32, log, true);
                }
                let top = y0 + h;
                let (cx, cz) = (tx.floor() as i32, tz.floor() as i32);
                put(cx, top, cz, leaves, false);
                for k in 0..7 {
                    let b = (k as f32 / 7.0 + unit(s, 30 + k) * 0.1) * std::f32::consts::TAU;
                    for t in 1..=self.radius {
                        let droop = if t * 2 > self.radius { (t * 2 - self.radius) / 2 } else { 0 };
                        put(cx + (b.cos() * t as f32).round() as i32, top - droop, cz + (b.sin() * t as f32).round() as i32, leaves, false);
                    }
                }
            }
            Kind::Jungle => {
                let top = (y0 + h) as f32;
                blob(put, fx, top - 2.5, fz, r, 3.0);
                for k in 0..3 {
                    let a = unit(s, 10 + k) * std::f32::consts::TAU;
                    let d = r * 0.7;
                    blob(put, fx + a.cos() * d, top - 4.0 - unit(s, 20 + k) * 4.0, fz + a.sin() * d, r * 0.6, 2.2);
                }
                for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    trunk(put, x0 + dx, z0 + dz, y0 - 1, y0 + h - 2);
                }
            }
            Kind::Cactus => {
                for y in y0..y0 + h {
                    put(x0, y, z0, pal.cactus, true);
                }
                // An arm on the taller ones.
                if h >= 4 {
                    let (dx, dz) = [(1, 0), (-1, 0), (0, 1), (0, -1)][(unit(s, 8) * 4.0) as usize % 4];
                    let ay = y0 + 1 + (unit(s, 9) * 2.0) as i32;
                    put(x0 + dx, ay, z0 + dz, pal.cactus, true);
                    put(x0 + dx, ay + 1, z0 + dz, pal.cactus, true);
                }
            }
            Kind::Bush => {
                blob(put, fx, y0 as f32 + 0.4, fz, r, h as f32 * 0.6 + 0.4);
            }
            Kind::Boulder => {
                let stone = pal.boulder[(unit(s, 5) * 3.0) as usize % 3];
                let (rx, ry) = (r, h as f32 * 0.55);
                let cy = y0 as f32 - 0.6;
                let ri = rx.ceil() as i32;
                for y in (cy - ry).floor() as i32..=(cy + ry).ceil() as i32 {
                    for dz in -ri..=ri {
                        for dx in -ri..=ri {
                            let (u, v, w) = ((dx as f32) / rx, (y as f32 + 0.5 - cy) / ry, (dz as f32) / rx);
                            if u * u + v * v + w * w < 1.0 - 0.3 * ragged(x0 + dx, y, z0 + dz) {
                                put(x0 + dx, y, z0 + dz, stone, true);
                            }
                        }
                    }
                }
            }
            Kind::FallenLog => {
                let along_x = unit(s, 6) < 0.5;
                for t in 0..self.radius {
                    let (x, z) = if along_x { (x0 + t, z0) } else { (x0, z0 + t) };
                    put(x, y0, z, log, true);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        let mut wood = [(0u16, 0u16); 16];
        for (i, w) in wood.iter_mut().enumerate() {
            *w = (100 + i as u16, 200 + i as u16);
        }
        Palette { wood, cactus: 7, boulder: [1, 2, 3] }
    }

    #[test]
    fn every_block_lies_inside_the_bounds() {
        let pal = palette();
        let kinds = [
            Kind::Oak, Kind::BigOak, Kind::Birch, Kind::Maple, Kind::Spruce, Kind::Fir, Kind::Pine, Kind::Redwood,
            Kind::Acacia, Kind::Willow, Kind::Palm, Kind::Jungle, Kind::Cactus, Kind::Bush, Kind::Boulder, Kind::FallenLog,
        ];
        for (n, kind) in kinds.iter().enumerate() {
            for s in 0..20u64 {
                let f = Feature::new(*kind, 100, 50, -40, s * 7919 + n as u64);
                let (lo, hi) = f.bounds();
                let mut count = 0;
                f.blocks(&pal, &mut |x, y, z, _, _| {
                    count += 1;
                    assert!(x >= lo[0] && x <= hi[0] && y >= lo[1] && y <= hi[1] && z >= lo[2] && z <= hi[2], "{kind:?} block {x},{y},{z} outside {lo:?}..{hi:?}");
                    assert!((x - f.x).abs() <= MAX_REACH + 1 && (z - f.z).abs() <= MAX_REACH + 1);
                });
                assert!(count > 0, "{kind:?} has no blocks");
            }
        }
    }

    #[test]
    fn trees_have_a_trunk_under_a_crown_of_their_size() {
        let pal = palette();
        let f = Feature::new(Kind::Spruce, 0, 10, 0, 99);
        let mut logs = 0;
        let mut leaf_top = i32::MIN;
        f.blocks(&pal, &mut |x, y, z, b, solid| {
            if solid && b == pal.wood[Kind::Spruce as usize].0 && x == 0 && z == 0 {
                logs += 1;
            }
            if !solid {
                leaf_top = leaf_top.max(y);
            }
        });
        assert!(logs >= 13, "{logs}");
        assert!(leaf_top >= 10 + f.height - 1);
        assert!((14..=24).contains(&f.height));
    }
}
