//! Natural structures: trees (from saplings to thousand-year-old giants), boulders, fallen
//! logs, stumps, bushes, rock spires and arches, ice spikes, crystals, cacti, giant mushrooms,
//! old skeletons, sinkholes.
//!
//! Every feature is a pure function of its anchor (position, kind, seed), so each chunk it
//! reaches draws its own part of it and the parts meet (the way Voxel Tools and Minecraft
//! place trees across chunk borders). Shapes are built from a few primitives (tapered
//! segments, rough ellipsoids, discs) the way Vintage Story's tree generator grows trunks and
//! branches that lose width as they go and the procedural-tree mods (Dynamic Trees,
//! Treecraft, Tom's Trees) give trees forked trunks, buttress roots, curving limbs and
//! layered crowns.

use crate::block::{BlockState, AIR};
use crate::coords::{LocalPos, CHUNK_SIZE};
use crate::rng::{hash3, Rng};

const S: i32 = CHUNK_SIZE;

/// How a feature's block goes in.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Replaces whatever is there (trunks, rock).
    Force,
    /// Only into air or water (leaves, so they never cut into trunks or ground).
    Air,
    /// Takes the block out (hollow trunks, sinkholes).
    Carve,
}

/// One chunk being generated, as a flat array (`LocalPos::index` order), with the ground
/// height of any column (for things that lie on the ground).
pub struct Canvas<'a> {
    pub buf: &'a mut [BlockState],
    pub origin: [i32; 3],
    pub water: u16,
    pub ground: &'a dyn Fn(i32, i32) -> i32,
}

impl Canvas<'_> {
    fn overlaps(&self, lo: [f32; 3], hi: [f32; 3]) -> bool {
        (0..3).all(|i| hi[i] >= self.origin[i] as f32 - 1.0 && lo[i] <= (self.origin[i] + S) as f32 + 1.0)
    }

    #[inline]
    pub fn put(&mut self, x: i32, y: i32, z: i32, s: BlockState, mode: Mode) {
        let (lx, ly, lz) = (x - self.origin[0], y - self.origin[1], z - self.origin[2]);
        if !(0..S).contains(&lx) || !(0..S).contains(&ly) || !(0..S).contains(&lz) {
            return;
        }
        let i = LocalPos { x: lx as u8, y: ly as u8, z: lz as u8 }.index();
        match mode {
            Mode::Force => self.buf[i] = s,
            Mode::Air => {
                let cur = self.buf[i];
                if cur.is_air() || cur.id() == self.water {
                    self.buf[i] = s;
                }
            }
            Mode::Carve => self.buf[i] = AIR,
        }
    }

    /// A ball of radius `r` (a single block below about 0.7).
    pub fn ball(&mut self, c: [f32; 3], r: f32, s: BlockState, mode: Mode) {
        if !self.overlaps([c[0] - r, c[1] - r, c[2] - r], [c[0] + r, c[1] + r, c[2] + r]) {
            return;
        }
        if r < 0.7 {
            self.put(c[0].floor() as i32, c[1].floor() as i32, c[2].floor() as i32, s, mode);
            return;
        }
        let r2 = r * r;
        let (x0, x1) = ((c[0] - r).floor() as i32, (c[0] + r).ceil() as i32);
        let (y0, y1) = ((c[1] - r).floor() as i32, (c[1] + r).ceil() as i32);
        let (z0, z1) = ((c[2] - r).floor() as i32, (c[2] + r).ceil() as i32);
        for y in y0..=y1 {
            for z in z0..=z1 {
                for x in x0..=x1 {
                    let (dx, dy, dz) = (x as f32 + 0.5 - c[0], y as f32 + 0.5 - c[1], z as f32 + 0.5 - c[2]);
                    if dx * dx + dy * dy + dz * dz <= r2 {
                        self.put(x, y, z, s, mode);
                    }
                }
            }
        }
    }

    /// A tapered limb from `a` (radius `ra`) to `b` (radius `rb`).
    pub fn segment(&mut self, a: [f32; 3], b: [f32; 3], ra: f32, rb: f32, s: BlockState, mode: Mode) {
        let r = ra.max(rb);
        let lo = [a[0].min(b[0]) - r, a[1].min(b[1]) - r, a[2].min(b[2]) - r];
        let hi = [a[0].max(b[0]) + r, a[1].max(b[1]) + r, a[2].max(b[2]) + r];
        if !self.overlaps(lo, hi) {
            return;
        }
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let steps = (len / 0.45).ceil().max(1.0) as i32;
        for k in 0..=steps {
            let t = k as f32 / steps as f32;
            let p = [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t];
            self.ball(p, ra + (rb - ra) * t, s, mode);
        }
    }

    /// An ellipsoid with a ragged edge (each block near the rim kept by chance), for leaf
    /// crowns, boulders and caps.
    pub fn blob(&mut self, c: [f32; 3], rad: [f32; 3], rough: f32, seed: u64, mut pick: impl FnMut(i32, i32, i32, f32) -> Option<(BlockState, Mode)>) {
        let r = rad[0].max(rad[1]).max(rad[2]);
        if !self.overlaps([c[0] - r, c[1] - r, c[2] - r], [c[0] + r, c[1] + r, c[2] + r]) {
            return;
        }
        let (x0, x1) = ((c[0] - rad[0]).floor() as i32, (c[0] + rad[0]).ceil() as i32);
        let (y0, y1) = ((c[1] - rad[1]).floor() as i32, (c[1] + rad[1]).ceil() as i32);
        let (z0, z1) = ((c[2] - rad[2]).floor() as i32, (c[2] + rad[2]).ceil() as i32);
        for y in y0..=y1 {
            if !(self.origin[1]..self.origin[1] + S).contains(&y) {
                continue;
            }
            for z in z0..=z1 {
                for x in x0..=x1 {
                    let (dx, dy, dz) = ((x as f32 + 0.5 - c[0]) / rad[0], (y as f32 + 0.5 - c[1]) / rad[1], (z as f32 + 0.5 - c[2]) / rad[2]);
                    let d = (dx * dx + dy * dy + dz * dz).sqrt();
                    if d > 1.0 {
                        continue;
                    }
                    if d > 1.0 - rough {
                        let h = (hash3(seed, x, y, z) >> 40) as f32 / 16_777_216.0;
                        if h < (d - (1.0 - rough)) / rough {
                            continue;
                        }
                    }
                    if let Some((s, m)) = pick(x, y, z, d) {
                        self.put(x, y, z, s, m);
                    }
                }
            }
        }
    }
}

/// The blocks of a kind of tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wood {
    pub log: u16,
    /// 0: none (dead and petrified trees).
    pub leaves: u16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// A broad-leaved tree: 0 a young one, 1 a grown one, 2 an ancient giant (buttress
    /// roots, sometimes hollow).
    Broadleaf(Wood, u8),
    /// A coast redwood: a straight column 45-70 blocks tall, crown only near the top.
    Redwood(Wood),
    /// An evergreen; `open` (pines) keeps its needles high on a bare trunk; `giant` is twice
    /// the size.
    Conifer { wood: Wood, open: bool, giant: bool },
    Palm(Wood),
    Acacia(Wood),
    Willow(Wood),
    Cypress(Wood),
    Bamboo(Wood),
    /// Bare twisted trees; `giant` for the old ones.
    Dead { wood: Wood, giant: bool },
    Mushroom { stem: u16, cap: u16, giant: bool },
    Boulder { stone: u16, moss: u16, big: bool },
    Spire { stone: u16, band: u16 },
    Arch { stone: u16, band: u16 },
    FallenLog { log: u16, moss: u16, big: bool },
    Stump(u16),
    Bush(Wood),
    IceSpike { ice: u16, core: u16, big: bool },
    Crystals(u16),
    Cactus(u16),
    Skeleton(u16),
    Sinkhole,
}

impl Kind {
    /// How far the feature can reach from its anchor: (sideways, down, up).
    pub fn reach(&self) -> (i32, i32, i32) {
        match self {
            Kind::Broadleaf(_, 2) => (19, 12, 46),
            Kind::Broadleaf(_, 1) => (9, 3, 22),
            Kind::Broadleaf(..) => (5, 1, 12),
            Kind::Redwood(_) => (12, 6, 76),
            Kind::Conifer { giant: true, .. } => (9, 2, 40),
            Kind::Conifer { .. } => (6, 2, 28),
            Kind::Palm(_) => (9, 1, 18),
            Kind::Acacia(_) => (9, 1, 14),
            Kind::Willow(_) => (9, 2, 16),
            Kind::Cypress(_) => (8, 3, 26),
            Kind::Bamboo(_) => (4, 1, 18),
            Kind::Dead { giant: true, .. } => (12, 6, 30),
            Kind::Dead { .. } => (6, 1, 16),
            Kind::Mushroom { giant: true, .. } => (9, 2, 24),
            Kind::Mushroom { .. } => (4, 1, 8),
            Kind::Boulder { big: true, .. } => (8, 4, 9),
            Kind::Boulder { .. } => (4, 2, 4),
            Kind::Spire { .. } => (8, 6, 48),
            Kind::Arch { .. } => (16, 6, 22),
            Kind::FallenLog { big: true, .. } => (14, 2, 5),
            Kind::FallenLog { .. } => (7, 2, 3),
            Kind::Stump(_) => (1, 1, 3),
            Kind::Bush(_) => (3, 1, 4),
            Kind::IceSpike { big: true, .. } => (5, 3, 30),
            Kind::IceSpike { .. } => (3, 2, 12),
            Kind::Crystals(_) => (5, 2, 7),
            Kind::Cactus(_) => (2, 1, 7),
            Kind::Skeleton(_) => (12, 2, 7),
            Kind::Sinkhole => (8, 200, 3),
        }
    }
}

/// A feature: what, and where it stands (`y`: the top block of the ground under it).
#[derive(Clone, Copy, Debug)]
pub struct Feature {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub kind: Kind,
    pub seed: u64,
}

fn unit3(seed: u64, y: i32) -> f32 {
    (hash3(seed, 7, y, 7) >> 40) as f32 / 16_777_216.0
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / l, v[1] / l, v[2] / l]
}

fn add(a: [f32; 3], b: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] + b[0] * k, a[1] + b[1] * k, a[2] + b[2] * k]
}

fn full(id: u16) -> BlockState {
    BlockState::full(id)
}

/// A random direction around the y axis: (cos, sin) of an angle.
fn around(rng: &mut Rng) -> (f32, f32) {
    let a = rng.next_f32() * std::f32::consts::TAU;
    (a.cos(), a.sin())
}

impl Feature {
    pub fn draw(&self, c: &mut Canvas) {
        let (rx, rd, ru) = self.kind.reach();
        let base = [self.x as f32 + 0.5, self.y as f32 + 1.0, self.z as f32 + 0.5];
        if !c.overlaps([(self.x - rx) as f32, (self.y - rd) as f32, (self.z - rx) as f32], [(self.x + rx) as f32, (self.y + ru) as f32, (self.z + rx) as f32]) {
            return;
        }
        let mut rng = Rng::new(self.seed);
        match self.kind {
            Kind::Broadleaf(w, size) => broadleaf(c, &mut rng, base, w, size, self.seed),
            Kind::Redwood(w) => redwood(c, &mut rng, base, w, self.seed),
            Kind::Conifer { wood, open, giant } => conifer(c, &mut rng, base, wood, open, giant, self.seed),
            Kind::Palm(w) => palm(c, &mut rng, base, w),
            Kind::Acacia(w) => acacia(c, &mut rng, base, w, self.seed),
            Kind::Willow(w) => willow(c, &mut rng, base, w, self.seed),
            Kind::Cypress(w) => cypress(c, &mut rng, base, w, self.seed),
            Kind::Bamboo(w) => bamboo(c, &mut rng, base, w),
            Kind::Dead { wood, giant } => dead(c, &mut rng, base, wood, giant, self.seed),
            Kind::Mushroom { stem, cap, giant } => mushroom(c, &mut rng, base, stem, cap, giant),
            Kind::Boulder { stone, moss, big } => boulder(c, &mut rng, base, stone, moss, big, self.seed),
            Kind::Spire { stone, band } => spire(c, &mut rng, base, stone, band, self.seed),
            Kind::Arch { stone, band } => arch(c, &mut rng, base, stone, band),
            Kind::FallenLog { log, moss, big } => fallen_log(c, &mut rng, base, log, moss, big),
            Kind::Stump(log) => {
                let h = 1 + rng.below(2) as i32;
                for y in -1..h {
                    c.put(self.x, self.y + 1 + y, self.z, full(log), Mode::Force);
                }
            }
            Kind::Bush(w) => {
                let r = 1.2 + rng.next_f32() * 1.0;
                c.put(self.x, self.y + 1, self.z, full(w.log), Mode::Force);
                c.blob(add(base, [0.0, 0.8, 0.0], 1.0), [r, r * 0.8, r], 0.4, self.seed, |_, _, _, _| Some((full(w.leaves), Mode::Air)));
            }
            Kind::IceSpike { ice, core, big } => ice_spike(c, &mut rng, base, ice, core, big),
            Kind::Crystals(b) => crystals(c, &mut rng, base, b, 1.0),
            Kind::Cactus(b) => cactus(c, &mut rng, base, b),
            Kind::Skeleton(b) => skeleton(c, &mut rng, base, b),
            Kind::Sinkhole => sinkhole(c, &mut rng, base, self.seed),
        }
    }
}

impl Feature {
    /// How high the feature stands over the column `(dx, dz)` from its anchor, in blocks
    /// above the ground it stands on, and what shows there, for distant (LOD) terrain: the
    /// shape's outline (crowns as domes and cones, spires as tapering columns) using the
    /// same sizes as `draw` (its first random draws), without drawing a block. None where it
    /// doesn't reach.
    pub fn lod_top(&self, dx: i32, dz: i32) -> Option<(f32, u16)> {
        let mut rng = Rng::new(self.seed);
        let d = ((dx * dx + dz * dz) as f32).sqrt();
        let dome = |top: f32, r: f32, depth: f32, b: u16| (d < r).then(|| (top - depth * (d / r) * (d / r), b));
        let cone = |top: f32, r: f32, depth: f32, b: u16| (d < r).then(|| (top - depth * d / r, b));
        match self.kind {
            Kind::Broadleaf(w, size) => {
                let h = match size {
                    0 => 5.0 + rng.next_f32() * 3.0,
                    1 => 10.0 + rng.next_f32() * 6.0,
                    _ => 24.0 + rng.next_f32() * 12.0,
                };
                let leaf = [2.2, 2.8, 3.6][size.min(2) as usize];
                if w.leaves == 0 {
                    return dome(0.85 * h, h * 0.35 + 1.0, h * 0.3, w.log);
                }
                let cr = leaf * (1.2 + size as f32 * 0.6);
                dome(0.85 * h + cr * 0.95, h * 0.45 + leaf, h * 0.35 + cr * 0.5, w.leaves)
            }
            Kind::Redwood(w) => {
                let h = 45.0 + rng.next_f32() * 25.0;
                cone(h + 3.5, 12.0, 0.58 * h, w.leaves).or_else(|| (d < 3.0).then_some((h, w.log)))
            }
            Kind::Conifer { wood, open, giant } => {
                let h = (9.0 + rng.next_f32() * 12.0) * if giant { 2.0 } else { 1.0 };
                if open {
                    dome(h + 1.0, (h * 0.22 + 0.8) * 1.1, h * 0.35, wood.leaves)
                } else {
                    cone(h + 2.0, h * 0.3 + 1.4, h * 0.82, wood.leaves)
                }
            }
            Kind::Palm(w) => {
                let h = 7.0 + rng.next_f32() * 7.0;
                let a = rng.next_f32() * std::f32::consts::TAU;
                let bend = 0.25 + rng.next_f32() * 0.35;
                let (ox, oz) = (a.cos() * bend * h, a.sin() * bend * h);
                let d2 = ((dx as f32 - ox).powi(2) + (dz as f32 - oz).powi(2)).sqrt();
                (d2 < 6.5).then(|| (h + 1.3 - 3.0 * (d2 / 6.5).powi(2), w.leaves))
            }
            Kind::Acacia(w) => {
                let h = 4.0 + rng.next_f32() * 3.0;
                dome(h + 3.6, 6.5, 1.5, w.leaves)
            }
            Kind::Willow(w) => {
                let h = 5.0 + rng.next_f32() * 3.0;
                dome(h + 5.5, 6.5, 5.0, w.leaves)
            }
            Kind::Cypress(w) => {
                let h = 14.0 + rng.next_f32() * 9.0;
                dome(h + 2.5, 6.0, 3.0, w.leaves)
            }
            Kind::Bamboo(w) => dome(14.0, 3.0, 4.0, w.leaves),
            Kind::Dead { wood, giant } => {
                if giant {
                    let h = 16.0 + rng.next_f32() * 10.0;
                    dome(h, 3.0, h * 0.2, wood.log)
                } else {
                    let h = 5.0 + rng.next_f32() * 8.0;
                    (d < 1.0).then_some((h, wood.log))
                }
            }
            Kind::Mushroom { cap, giant, .. } => {
                let (h, _r, cr) = if giant {
                    (10.0 + rng.next_f32() * 9.0, rng.next_f32(), 5.0 + rng.next_f32() * 3.0)
                } else {
                    (3.0 + rng.next_f32() * 3.0, 0.0, 2.0 + rng.next_f32() * 1.2)
                };
                dome(h + cr * 0.55, cr, cr * 0.55, cap)
            }
            Kind::Boulder { stone, moss, big } => {
                let r = if big { 3.5 + rng.next_f32() * 3.5 } else { 1.2 + rng.next_f32() * 1.8 };
                let rx = r * (0.8 + rng.next_f32() * 0.5);
                let ry = r * (0.6 + rng.next_f32() * 0.3);
                let rz = r * (0.8 + rng.next_f32() * 0.5);
                dome(ry * 1.25 - 1.0, rx.max(rz), ry, if moss != 0 { moss } else { stone })
            }
            Kind::Spire { stone, .. } => {
                let h = 16.0 + rng.next_f32() * 30.0;
                let r = 2.5 + rng.next_f32() * 3.5;
                if d > r + 0.6 {
                    return None;
                }
                let t = 1.0 - ((d - 0.6).max(0.0) / r).powf(1.0 / 0.55);
                Some((h * t, stone))
            }
            Kind::Arch { stone, .. } => {
                let span = 7.0 + rng.next_f32() * 6.0;
                let a = rng.next_f32() * std::f32::consts::TAU;
                let thick = 2.0 + rng.next_f32() * 1.5;
                let (ax, az) = (a.cos(), a.sin());
                let u = dx as f32 * ax + dz as f32 * az;
                let v = -(dx as f32) * az + dz as f32 * ax;
                if v.abs() > thick * 1.3 || u.abs() > span + thick {
                    return None;
                }
                let k = (1.0 - (u / span).powi(2)).max(0.0).sqrt();
                Some((span * 1.25 * k + thick - 3.0, stone))
            }
            Kind::FallenLog { log, big: true, .. } => {
                let len = 12.0 + rng.next_f32() * 12.0;
                let r = 1.4 + rng.next_f32() * 1.0;
                let a = rng.next_f32() * std::f32::consts::TAU;
                let (ax, az) = (a.cos(), a.sin());
                let u = (dx as f32 * ax + dz as f32 * az).clamp(0.0, len);
                let (px, pz) = (dx as f32 - ax * u, dz as f32 - az * u);
                ((px * px + pz * pz).sqrt() < r).then_some((r * 1.6, log))
            }
            Kind::Bush(w) => dome(2.5, 1.8, 1.5, w.leaves),
            Kind::IceSpike { ice, big, .. } => {
                let (h, r) = if big { (14.0 + rng.next_f32() * 14.0, 2.0 + rng.next_f32() * 1.8) } else { (4.0 + rng.next_f32() * 6.0, 0.9 + rng.next_f32() * 0.7) };
                cone(h, r + 0.5, h, ice)
            }
            _ => None,
        }
    }
}

/// A limb that grows, bends and splits: Vintage Story's branches (they lose width as they
/// go, droop or rise, and fork into thinner ones) with leaf clusters where they end.
#[allow(clippy::too_many_arguments)]
fn limb(c: &mut Canvas, rng: &mut Rng, p: [f32; 3], dir: [f32; 3], len: f32, r: f32, depth: u32, w: Wood, leaf_r: f32, rise: f32, seed: u64) {
    let steps = 3;
    let mut at = p;
    let mut d = dir;
    let mut rad = r;
    for _ in 0..steps {
        let (cx, sz) = around(rng);
        d = norm([d[0] + cx * 0.25, d[1] + rise * 0.3 + (rng.next_f32() - 0.5) * 0.2, d[2] + sz * 0.25]);
        let next = add(at, d, len / steps as f32);
        let nr = (rad * 0.82).max(0.45);
        c.segment(at, next, rad, nr, full(w.log), Mode::Force);
        at = next;
        rad = nr;
    }
    if depth > 0 {
        let n = 2 + rng.below(2);
        for _ in 0..n {
            let (cx, sz) = around(rng);
            let nd = norm([d[0] * 0.6 + cx * 0.7, d[1] * 0.6 + 0.35 * rise, d[2] * 0.6 + sz * 0.7]);
            let nl = len * (0.55 + rng.next_f32() * 0.2);
            limb(c, rng, at, nd, nl, (rad * 0.75).max(0.45), depth - 1, w, leaf_r, rise, seed);
        }
    }
    if w.leaves != 0 && (depth == 0 || rng.next_f32() < 0.5) {
        let lr = leaf_r * (0.8 + rng.next_f32() * 0.45);
        leaf_cluster(c, at, lr, w.leaves, seed ^ (at[0] as u64).wrapping_mul(31) ^ (at[2] as u64).wrapping_mul(17));
    }
}

fn leaf_cluster(c: &mut Canvas, at: [f32; 3], r: f32, leaves: u16, seed: u64) {
    c.blob(add(at, [0.0, r * 0.25, 0.0], 1.0), [r, r * 0.72, r], 0.45, seed, |_, _, _, _| Some((full(leaves), Mode::Air)));
}

/// Roots spreading from the foot of a trunk into the ground (buttresses on big trees).
fn roots(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], r: f32, n: u32, reach: f32, log: u16) {
    for k in 0..n {
        let a = (k as f32 + rng.next_f32() * 0.6) / n as f32 * std::f32::consts::TAU;
        let (dx, dz) = (a.cos(), a.sin());
        let start = [base[0] + dx * r * 0.4, base[1] + r * 1.1, base[2] + dz * r * 0.4];
        let mid = [base[0] + dx * (r + reach * 0.45), base[1] + r * 0.2, base[2] + dz * (r + reach * 0.45)];
        let end = [base[0] + dx * (r + reach), base[1] - 2.5, base[2] + dz * (r + reach)];
        c.segment(start, mid, r * 0.55, r * 0.35, full(log), Mode::Force);
        c.segment(mid, end, r * 0.35, 0.45, full(log), Mode::Force);
    }
}

fn broadleaf(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], w: Wood, size: u8, seed: u64) {
    let (h, r, limbs, depth, leaf_r) = match size {
        0 => (5.0 + rng.next_f32() * 3.0, 0.45, 3, 0, 2.2),
        1 => (10.0 + rng.next_f32() * 6.0, 0.9 + rng.next_f32() * 0.5, 4, 1, 2.8),
        _ => (24.0 + rng.next_f32() * 12.0, 2.4 + rng.next_f32() * 1.1, 6, 2, 3.6),
    };
    let (lx, lz) = around(rng);
    let lean = 0.06 + rng.next_f32() * 0.08;
    let crown = h * 0.55;
    let top = [base[0] + lx * lean * crown, base[1] + crown, base[2] + lz * lean * crown];
    let foot = add(base, [0.0, -2.0, 0.0], 1.0);
    c.segment(foot, top, r * if size == 2 { 1.25 } else { 1.0 }, r * 0.7, full(w.log), Mode::Force);
    if size >= 1 {
        let n = if size == 2 { 6 + rng.below(3) } else { 3 };
        let reach = if size == 2 { 6.0 + rng.next_f32() * 4.0 } else { 2.0 };
        roots(c, rng, base, r, n, reach, w.log);
    }
    if size == 2 && rng.next_f32() < 0.45 {
        // A hollow trunk with a way in.
        let hr = r - 1.1;
        c.segment(add(base, [0.0, 0.0, 0.0], 1.0), add(base, [0.0, h * 0.3, 0.0], 1.0), hr, hr * 0.8, AIR, Mode::Carve);
        let (dx, dz) = around(rng);
        for k in 0..3 {
            let p = [base[0] + dx * (r * 0.5 + k as f32 * 0.6), base[1] + 1.0, base[2] + dz * (r * 0.5 + k as f32 * 0.6)];
            c.segment(p, add(p, [0.0, 2.2, 0.0], 1.0), 0.9, 0.9, AIR, Mode::Carve);
        }
    }
    // The main limbs from the top of the trunk and from its upper part.
    for k in 0..limbs {
        let a = (k as f32 + rng.next_f32() * 0.5) / limbs as f32 * std::f32::consts::TAU;
        let from = if k % 2 == 0 { top } else { add(base, [lx * lean * crown * 0.8, crown * (0.7 + rng.next_f32() * 0.25), lz * lean * crown * 0.8], 1.0) };
        let dir = norm([a.cos(), 0.55 + rng.next_f32() * 0.5, a.sin()]);
        limb(c, rng, from, dir, h * 0.42, r * 0.6, depth, w, leaf_r, 0.4, seed);
    }
    if w.leaves != 0 {
        let cr = leaf_r * (1.2 + size as f32 * 0.6);
        leaf_cluster(c, add(top, [0.0, h * 0.3, 0.0], 1.0), cr, w.leaves, seed ^ 0x55);
    }
}

fn redwood(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], w: Wood, seed: u64) {
    let h = 45.0 + rng.next_f32() * 25.0;
    let r = 2.2 + rng.next_f32() * 1.0;
    let foot = add(base, [0.0, -3.0, 0.0], 1.0);
    // A flared foot, then a straight column tapering to the top.
    c.segment(foot, add(base, [0.0, 4.0, 0.0], 1.0), r * 1.45, r, full(w.log), Mode::Force);
    let top = add(base, [0.0, h, 0.0], 1.0);
    c.segment(add(base, [0.0, 4.0, 0.0], 1.0), top, r, 0.6, full(w.log), Mode::Force);
    let (n, reach) = (6 + rng.below(3), 4.0 + rng.next_f32() * 3.0);
    roots(c, rng, base, r, n, reach, w.log);
    // Short branches drooping a little, only high up, shorter towards the top.
    let mut y = h * 0.42;
    while y < h - 2.0 {
        let t = y / h;
        let n = 1 + rng.below(3);
        for _ in 0..n {
            let (dx, dz) = around(rng);
            let len = (1.0 - t) * 9.0 + 2.0;
            let from = add(base, [0.0, y, 0.0], 1.0);
            let to = [from[0] + dx * len, from[1] - len * 0.15 + rng.next_f32(), from[2] + dz * len];
            c.segment(from, to, 0.8, 0.45, full(w.log), Mode::Force);
            leaf_cluster(c, to, 2.6 + (1.0 - t) * 2.4, w.leaves, seed ^ (y as u64 * 977) ^ (dx.to_bits() as u64));
            leaf_cluster(c, add(from, [dx * len * 0.5, 0.5, dz * len * 0.5], 1.0), 2.0, w.leaves, seed ^ (y as u64 * 31));
        }
        y += 2.0 + rng.next_f32() * 2.0;
    }
    c.blob(add(top, [0.0, -3.0, 0.0], 1.0), [4.0, 6.5, 4.0], 0.4, seed ^ 0x77, |_, _, _, _| Some((full(w.leaves), Mode::Air)));
}

fn conifer(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], w: Wood, open: bool, giant: bool, seed: u64) {
    let k = if giant { 2.0 } else { 1.0 };
    let h = (9.0 + rng.next_f32() * 12.0) * k;
    let r = if giant { 1.3 } else if h > 15.0 { 0.9 } else { 0.45 };
    c.segment(add(base, [0.0, -1.5, 0.0], 1.0), add(base, [0.0, h, 0.0], 1.0), r, 0.45, full(w.log), Mode::Force);
    if giant {
        roots(c, rng, base, r, 5, 3.0, w.log);
    }
    let bare = if open { 0.5 } else { 0.18 };
    let max_r = h * if open { 0.22 } else { 0.3 } + 0.8;
    let mut y = h * bare;
    while y < h {
        let t = (y - h * bare) / (h * (1.0 - bare));
        let lr = if open { max_r * (1.0 - (t - 0.45).abs() * 1.3).max(0.25) } else { max_r * (1.0 - t).powf(0.85) + 0.6 };
        let cy = base[1] + y;
        if open {
            leaf_cluster(c, [base[0], cy, base[2]], lr.max(1.2), w.leaves, seed ^ (y as u64 * 131));
        } else {
            // A layer: a disc of needles drooping at its rim, with short branches inside.
            c.blob([base[0], cy, base[2]], [lr, 1.0, lr], 0.35, seed ^ (y as u64 * 131), |_, yy, _, d| {
                let droop = d > 0.75;
                (!(droop && yy as f32 + 0.5 > cy)).then_some((full(w.leaves), Mode::Air))
            });
            c.blob([base[0], cy - 1.0, base[2]], [lr * 1.05, 0.6, lr * 1.05], 0.3, seed ^ (y as u64 * 37), |_, _, _, d| (d > 0.7).then_some((full(w.leaves), Mode::Air)));
            if lr > 2.5 {
                for (dx, dz) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                    let p = [base[0], cy, base[2]];
                    c.segment(p, [p[0] + dx * lr * 0.6, cy - 0.5, p[2] + dz * lr * 0.6], 0.45, 0.45, full(w.log), Mode::Force);
                }
            }
        }
        y += if open { 2.5 + rng.next_f32() } else { 2.0 + rng.next_f32() * 0.8 };
    }
    let tip = add(base, [0.0, h, 0.0], 1.0);
    c.segment(tip, add(tip, [0.0, 2.0, 0.0], 1.0), 0.45, 0.45, full(w.leaves), Mode::Air);
}

fn palm(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], w: Wood) {
    let h = 7.0 + rng.next_f32() * 7.0;
    let (dx, dz) = around(rng);
    let bend = 0.25 + rng.next_f32() * 0.35;
    let mut prev = add(base, [0.0, -1.0, 0.0], 1.0);
    let n = h as i32;
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let p = [base[0] + dx * bend * h * t * t, base[1] + h * t, base[2] + dz * bend * h * t * t];
        c.segment(prev, p, 0.5, 0.5, full(w.log), Mode::Force);
        prev = p;
    }
    let top = prev;
    c.ball(top, 1.3, full(w.leaves), Mode::Air);
    let fronds = 6 + rng.below(3);
    for k in 0..fronds {
        let a = (k as f32 + rng.next_f32() * 0.4) / fronds as f32 * std::f32::consts::TAU;
        let (fx, fz) = (a.cos(), a.sin());
        let len = 4.5 + rng.next_f32() * 2.5;
        let steps = (len * 2.0) as i32;
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            let p = [top[0] + fx * len * t, top[1] + 1.2 * t * 2.0 - 3.6 * t * t, top[2] + fz * len * t];
            c.ball(p, 0.5, full(w.leaves), Mode::Air);
        }
    }
}

fn acacia(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], w: Wood, seed: u64) {
    let h = 4.0 + rng.next_f32() * 3.0;
    let (dx, dz) = around(rng);
    let fork = [base[0] + dx * 1.5, base[1] + h, base[2] + dz * 1.5];
    c.segment(add(base, [0.0, -1.0, 0.0], 1.0), fork, 0.6, 0.5, full(w.log), Mode::Force);
    let n = 2 + rng.below(2);
    for k in 0..n {
        let a = (k as f32 + rng.next_f32() * 0.5) / n as f32 * std::f32::consts::TAU;
        let len = 3.0 + rng.next_f32() * 3.0;
        let end = [fork[0] + a.cos() * len, fork[1] + len * 0.8, fork[2] + a.sin() * len];
        c.segment(fork, end, 0.5, 0.45, full(w.log), Mode::Force);
        let r = 3.0 + rng.next_f32() * 2.5;
        c.blob(add(end, [0.0, 0.8, 0.0], 1.0), [r, 0.9, r], 0.3, seed ^ k as u64, |_, _, _, _| Some((full(w.leaves), Mode::Air)));
    }
}

fn willow(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], w: Wood, seed: u64) {
    let h = 5.0 + rng.next_f32() * 3.0;
    let r = 0.9 + rng.next_f32() * 0.4;
    let top = add(base, [0.0, h, 0.0], 1.0);
    c.segment(add(base, [0.0, -1.5, 0.0], 1.0), top, r * 1.2, r * 0.8, full(w.log), Mode::Force);
    roots(c, rng, base, r, 4, 2.0, w.log);
    for k in 0..5 {
        let a = (k as f32 + rng.next_f32() * 0.5) / 5.0 * std::f32::consts::TAU;
        limb(c, rng, top, norm([a.cos(), 0.7, a.sin()]), 4.0, 0.6, 0, w, 2.6, 0.2, seed);
    }
    let cr = 5.0 + rng.next_f32() * 2.0;
    let centre = add(top, [0.0, 2.5, 0.0], 1.0);
    c.blob(centre, [cr, 3.0, cr], 0.35, seed ^ 0x99, |_, _, _, _| Some((full(w.leaves), Mode::Air)));
    // Strands hanging from the crown's rim.
    let n = (cr * 7.0) as i32;
    for k in 0..n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU + rng.next_f32() * 0.2;
        let rr = cr * (0.75 + rng.next_f32() * 0.25);
        let (x, z) = ((centre[0] + a.cos() * rr).floor() as i32, (centre[2] + a.sin() * rr).floor() as i32);
        let len = 2 + rng.below(6) as i32;
        let y0 = (centre[1] - 1.0) as i32;
        for y in (y0 - len)..y0 {
            c.put(x, y, z, full(w.leaves), Mode::Air);
        }
    }
}

fn cypress(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], w: Wood, seed: u64) {
    let h = 14.0 + rng.next_f32() * 9.0;
    // A swollen, buttressed foot, then a slim trunk.
    c.segment(add(base, [0.0, -3.0, 0.0], 1.0), add(base, [0.0, 3.0, 0.0], 1.0), 2.6, 1.0, full(w.log), Mode::Force);
    c.segment(add(base, [0.0, 3.0, 0.0], 1.0), add(base, [0.0, h, 0.0], 1.0), 1.0, 0.5, full(w.log), Mode::Force);
    roots(c, rng, base, 1.6, 5, 2.5, w.log);
    // Knees: stubs poking out of the water or mud around it.
    for _ in 0..(3 + rng.below(4)) {
        let (dx, dz) = around(rng);
        let d = 3.0 + rng.next_f32() * 3.0;
        let (x, z) = ((base[0] + dx * d) as i32, (base[2] + dz * d) as i32);
        let gy = (c.ground)(x, z);
        for y in gy..gy + 1 + rng.below(2) as i32 {
            c.put(x, y + 1, z, full(w.log), Mode::Force);
        }
    }
    for k in 0..(3 + rng.below(3)) {
        let (dx, dz) = around(rng);
        let y = h * (0.62 + 0.35 * rng.next_f32());
        let from = add(base, [0.0, y, 0.0], 1.0);
        let to = [from[0] + dx * 3.0, from[1] + 1.0, from[2] + dz * 3.0];
        c.segment(from, to, 0.6, 0.45, full(w.log), Mode::Force);
        let r = 3.0 + rng.next_f32() * 1.5;
        c.blob(to, [r, 1.5, r], 0.35, seed ^ k as u64, |_, _, _, _| Some((full(w.leaves), Mode::Air)));
        for _ in 0..4 {
            let (sx, sz) = around(rng);
            let (x, z) = ((to[0] + sx * r * 0.8) as i32, (to[2] + sz * r * 0.8) as i32);
            for yy in 0..(2 + rng.below(4) as i32) {
                c.put(x, to[1] as i32 - 1 - yy, z, full(w.leaves), Mode::Air);
            }
        }
    }
}

fn bamboo(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], w: Wood) {
    for _ in 0..(5 + rng.below(8)) {
        let (dx, dz) = around(rng);
        let d = rng.next_f32() * 2.5;
        let (x, z) = ((base[0] + dx * d).floor() as i32, (base[2] + dz * d).floor() as i32);
        let gy = (c.ground)(x, z);
        let h = 8 + rng.below(9) as i32;
        for y in gy..=gy + h {
            c.put(x, y, z, full(w.log), Mode::Force);
        }
        let mut y = gy + 5;
        while y <= gy + h {
            c.ball([x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5], 1.2, full(w.leaves), Mode::Air);
            y += 3 + rng.below(2) as i32;
        }
    }
}

fn dead(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], w: Wood, giant: bool, seed: u64) {
    let (h, r) = if giant { (16.0 + rng.next_f32() * 10.0, 1.8 + rng.next_f32() * 0.8) } else { (5.0 + rng.next_f32() * 8.0, 0.5 + rng.next_f32() * 0.4) };
    // A twisted trunk.
    let mut at = add(base, [0.0, -1.5, 0.0], 1.0);
    let mut rad = r;
    let n = 5;
    for _ in 0..n {
        let (dx, dz) = around(rng);
        let next = [at[0] + dx * h * 0.07, at[1] + h / n as f32, at[2] + dz * h * 0.07];
        c.segment(at, next, rad, rad * 0.85, full(w.log), Mode::Force);
        at = next;
        rad *= 0.85;
    }
    if giant {
        let (n, reach) = (5 + rng.below(3), 4.0 + rng.next_f32() * 3.0);
        roots(c, rng, base, r, n, reach, w.log);
    }
    let limbs = 2 + rng.below(3);
    for k in 0..limbs {
        let a = (k as f32 + rng.next_f32()) / limbs as f32 * std::f32::consts::TAU;
        let from = add(base, [0.0, h * (0.5 + rng.next_f32() * 0.4), 0.0], 1.0);
        limb(c, rng, from, norm([a.cos(), 0.35, a.sin()]), h * 0.4, (rad * 1.3).max(0.45), if giant { 2 } else { 1 }, w, 1.4, 0.2, seed);
    }
}

fn mushroom(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], stem: u16, cap: u16, giant: bool) {
    let (h, r, cr) = if giant { (10.0 + rng.next_f32() * 9.0, 1.2 + rng.next_f32() * 0.8, 5.0 + rng.next_f32() * 3.0) } else { (3.0 + rng.next_f32() * 3.0, 0.45, 2.0 + rng.next_f32() * 1.2) };
    let (dx, dz) = around(rng);
    let bend = rng.next_f32() * 0.15;
    let top = [base[0] + dx * h * bend, base[1] + h, base[2] + dz * h * bend];
    let mid = [base[0] + dx * h * bend * 0.3, base[1] + h * 0.5, base[2] + dz * h * bend * 0.3];
    c.segment(add(base, [0.0, -1.0, 0.0], 1.0), mid, r, r * 0.9, full(stem), Mode::Force);
    c.segment(mid, top, r * 0.9, r * 0.8, full(stem), Mode::Force);
    // A dome: a shell, open underneath.
    let ch = cr * 0.55;
    c.blob(top, [cr, ch, cr], 0.15, (top[0] as u64) ^ 0xC0FFEE, |_, y, _, d| {
        (y as f32 + 0.5 >= top[1] - 0.5 && d > 0.62).then_some((full(cap), Mode::Air))
    });
}

fn boulder(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], stone: u16, moss: u16, big: bool, seed: u64) {
    let r = if big { 3.5 + rng.next_f32() * 3.5 } else { 1.2 + rng.next_f32() * 1.8 };
    let rad = [r * (0.8 + rng.next_f32() * 0.5), r * (0.6 + rng.next_f32() * 0.3), r * (0.8 + rng.next_f32() * 0.5)];
    let centre = add(base, [0.0, rad[1] * 0.25 - 1.0, 0.0], 1.0);
    let top_from = centre[1] + rad[1] * 0.35;
    c.blob(centre, rad, 0.35, seed, |_, y, _, _| {
        let s = if moss != 0 && y as f32 > top_from { moss } else { stone };
        Some((full(s), Mode::Force))
    });
    // Sometimes a smaller one leaning on it.
    if big && rng.next_f32() < 0.6 {
        let (dx, dz) = around(rng);
        let r2 = r * 0.5;
        let p = [centre[0] + dx * r, centre[1] - r2 * 0.3, centre[2] + dz * r];
        c.blob(p, [r2, r2 * 0.8, r2], 0.35, seed ^ 7, |_, _, _, _| Some((full(stone), Mode::Force)));
    }
}

fn spire(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], stone: u16, band: u16, seed: u64) {
    let h = 16.0 + rng.next_f32() * 30.0;
    let r = 2.5 + rng.next_f32() * 3.5;
    let (lx, lz) = around(rng);
    let lean = rng.next_f32() * 0.12;
    let y0 = base[1] as i32 - 6;
    let y1 = (base[1] + h) as i32;
    let band_every = 6 + (seed % 5) as i32;
    for y in y0..=y1 {
        let t = ((y as f32 - base[1]).max(0.0) / h).min(1.0);
        let rr = r * (1.0 - t).powf(0.55) + 0.6 + 0.5 * (((hash3(seed, 0, y, 0) >> 40) as f32 / 16_777_216.0) - 0.5);
        let cx = base[0] + lx * lean * (y as f32 - base[1]).max(0.0);
        let cz = base[2] + lz * lean * (y as f32 - base[1]).max(0.0);
        let s = if y.rem_euclid(band_every) == 0 && unit3(seed, y) < 0.5 { band } else { stone };
        c.blob([cx, y as f32 + 0.5, cz], [rr, 0.5, rr], 0.25, seed ^ y as u64, |_, _, _, _| Some((full(s), Mode::Force)));
    }
}

fn arch(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], stone: u16, band: u16) {
    let span = 7.0 + rng.next_f32() * 6.0;
    let (dx, dz) = around(rng);
    let thick = 2.0 + rng.next_f32() * 1.5;
    let n = 40;
    let mut prev: Option<[f32; 3]> = None;
    for k in 0..=n {
        let a = k as f32 / n as f32 * std::f32::consts::PI;
        let p = [base[0] + dx * span * a.cos(), base[1] - 3.0 + span * 1.25 * a.sin(), base[2] + dz * span * a.cos()];
        let r = thick * (1.0 + 0.6 * (1.0 - a.sin()));
        if let Some(q) = prev {
            c.segment(q, p, r, r, full(if k % 7 == 0 { band } else { stone }), Mode::Force);
        }
        prev = Some(p);
    }
}

fn fallen_log(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], log: u16, moss: u16, big: bool) {
    let len = if big { 12.0 + rng.next_f32() * 12.0 } else { 4.0 + rng.next_f32() * 6.0 };
    let r = if big { 1.4 + rng.next_f32() * 1.0 } else { 0.45 };
    let (dx, dz) = around(rng);
    let a = [base[0], 0.0, base[2]];
    let b = [base[0] + dx * len, 0.0, base[2] + dz * len];
    let ga = (c.ground)(a[0] as i32, a[2] as i32) as f32 + 1.0 + r * 0.6;
    let gb = (c.ground)(b[0].floor() as i32, b[2].floor() as i32) as f32 + 1.0 + r * 0.6;
    let (a, b) = ([a[0], ga, a[2]], [b[0], gb, b[2]]);
    c.segment(a, b, r, r * 0.9, full(log), Mode::Force);
    if moss != 0 {
        let steps = len as i32;
        for k in 0..steps {
            if rng.next_f32() < 0.45 {
                let t = k as f32 / steps as f32;
                let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t + r + 0.2, a[2] + (b[2] - a[2]) * t];
                c.put(p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32, full(moss), Mode::Air);
            }
        }
    }
}

fn ice_spike(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], ice: u16, core: u16, big: bool) {
    let (h, r) = if big { (14.0 + rng.next_f32() * 14.0, 2.0 + rng.next_f32() * 1.8) } else { (4.0 + rng.next_f32() * 6.0, 0.9 + rng.next_f32() * 0.7) };
    let (dx, dz) = around(rng);
    let lean = rng.next_f32() * 0.15;
    let top = [base[0] + dx * h * lean, base[1] + h, base[2] + dz * h * lean];
    c.segment(add(base, [0.0, -2.0, 0.0], 1.0), top, r, 0.45, full(ice), Mode::Force);
    if big {
        c.segment(add(base, [0.0, -2.0, 0.0], 1.0), add(top, [0.0, -h * 0.25, 0.0], 1.0), r * 0.45, 0.45, full(core), Mode::Force);
    }
}

fn crystals(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], b: u16, up: f32) {
    for _ in 0..(3 + rng.below(5)) {
        let (dx, dz) = around(rng);
        let tilt = rng.next_f32() * 0.8;
        let len = 2.0 + rng.next_f32() * 4.0;
        let dir = norm([dx * tilt, up, dz * tilt]);
        let from = add(base, [dx * 0.6, -1.0 * up, dz * 0.6], 1.0);
        c.segment(from, add(from, dir, len), 0.8, 0.45, full(b), Mode::Force);
    }
}

fn cactus(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], b: u16) {
    let (x, y, z) = (base[0] as i32, base[1] as i32, base[2] as i32);
    let h = 2 + rng.below(4) as i32;
    for k in 0..h {
        c.put(x, y + k, z, full(b), Mode::Force);
    }
    if h >= 3 {
        for _ in 0..rng.below(3) {
            let (dx, dz) = [(1, 0), (-1, 0), (0, 1), (0, -1)][rng.below(4) as usize];
            let at = y + 1 + rng.below((h - 1) as u32) as i32;
            c.put(x + dx, at, z + dz, full(b), Mode::Air);
            for k in 1..=(1 + rng.below(2) as i32) {
                c.put(x + dx, at + k, z + dz, full(b), Mode::Air);
            }
        }
    }
}

fn skeleton(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], b: u16) {
    let len = 10.0 + rng.next_f32() * 9.0;
    let (dx, dz) = around(rng);
    let a = add(base, [0.0, 0.5, 0.0], 1.0);
    let e = [a[0] + dx * len, a[1] + rng.next_f32() * 1.5, a[2] + dz * len];
    c.segment(a, e, 0.6, 0.45, full(b), Mode::Force);
    // Ribs: arches across the middle of the spine, half sunk in the ground.
    let (px, pz) = (-dz, dx);
    let mut t = 0.25;
    while t < 0.7 {
        let p = [a[0] + (e[0] - a[0]) * t, a[1] + (e[1] - a[1]) * t, a[2] + (e[2] - a[2]) * t];
        let rr = 2.5 + (1.0 - ((t - 0.47) * 4.0).abs()) * 2.5;
        let mut prev: Option<[f32; 3]> = None;
        for k in 0..=12 {
            let ang = k as f32 / 12.0 * std::f32::consts::PI;
            let q = [p[0] + px * rr * ang.cos(), p[1] + rr * 1.1 * ang.sin() - 1.5, p[2] + pz * rr * ang.cos()];
            if let Some(o) = prev {
                c.segment(o, q, 0.45, 0.45, full(b), Mode::Force);
            }
            prev = Some(q);
        }
        t += 0.09;
    }
    // The skull.
    c.blob(add(e, [dx * 1.5, 0.5, dz * 1.5], 1.0), [2.0, 1.4, 1.6], 0.25, len.to_bits() as u64, |_, _, _, d| (d > 0.5).then_some((full(b), Mode::Force)));
}

fn sinkhole(c: &mut Canvas, rng: &mut Rng, base: [f32; 3], seed: u64) {
    let r = 3.0 + rng.next_f32() * 3.0;
    let y1 = base[1] as i32 + 2;
    let y0 = -64;
    for y in y0..=y1 {
        if !(c.origin[1]..c.origin[1] + S).contains(&y) {
            continue;
        }
        let w = r * (1.0 + 0.25 * ((y as f32) * 0.21).sin()) + 0.8 * (((hash3(seed, 1, y, 1) >> 40) as f32 / 16_777_216.0) - 0.5);
        let ox = 2.0 * ((y as f32) * 0.05).sin();
        c.blob([base[0] + ox, y as f32 + 0.5, base[2]], [w, 0.5, w], 0.2, seed ^ y as u64, |_, _, _, _| Some((AIR, Mode::Carve)));
    }
}
