//! Level-of-detail terrain past the loaded chunks, for an "infinite" view distance.
//!
//! The design follows Distant Horizons (the Minecraft LOD mod), rebuilt for this engine:
//!
//! * **Sections in a quadtree.** A section is 32 x 32 cells; a cell at level `L` is a
//!   column `2^L` blocks wide, so a level-`L` section covers `32 * 2^L` blocks. Level 0
//!   cells are single blocks; level 10 sections are 32 km wide.
//! * **Detail drops with distance in rings that widen exponentially**, as in Distant
//!   Horizons, but counted from where the real chunks end: the first ring past them has
//!   2-block cells, the next is twice as wide with 4-block cells, then 8, 16 ... 512. Far
//!   out each ring holds about the same number of cells, so the cost of the view grows with
//!   the *logarithm* of the distance. That is what makes a very long view affordable.
//! * **Built straight from the world seed.** The generator is a pure function of
//!   `(seed, x, z)`, so a far cell is one height sample, filtered for the cell size
//!   (`WorldGen::lod_surface`). Nothing is generated chunk by chunk, nothing is stored.
//! * **Blocky meshes like Distant Horizons'**: a box per column, a side face only where the
//!   neighbour is lower, coplanar faces merged into rectangles.
//!
//! What this adds over Distant Horizons, for phones:
//!
//! * **No holes and no overlap with real chunks.** Distant Horizons has to choose between
//!   drawing LODs under the loaded chunks (overlap) and cutting them out by distance (holes
//!   while chunks stream in). Here the renderer hides LOD faces per chunk column, exactly
//!   where the real terrain is meshed and drawn (`Client::lod_coverage`), from the vertex
//!   shader. Faces are merged only inside a chunk column for sections that can meet real
//!   chunks, so every vertex of a quad agrees on which column it belongs to.
//! * **Drawn after the real terrain**, nearest first, so the depth test rejects LOD
//!   fragments hidden behind real terrain before they are shaded.
//! * **Crack-free borders between detail levels** with skirts: border side faces reach
//!   down to the lowest height a neighbour at the same, the next finer or the next coarser
//!   level can have there.
//! * **A section is only replaced once its replacement is ready**, so switching detail as
//!   the camera moves never opens a hole (`LodManager::render_set`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::coords::{Face, CHUNK_SIZE};
use crate::jobs::JobPool;
use crate::mesh::FACE_CORNERS;
use crate::registry::BlockRegistry;
use crate::worldgen::WorldGen;

/// Cells per section side.
pub const CELLS: i32 = 32;
/// Coarsest level: 512-block cells, 16 km sections.
pub const MAX_LEVEL: u8 = 9;
/// Sections at this level or finer can meet loaded chunks: their faces are merged only
/// within a chunk column, the renderer hides them per column, and every chunk-column
/// boundary through them gets side faces deep enough to meet the real terrain beside it.
/// Sections within a chunk of the real terrain are never coarser (see `LodSettings::walk`).
pub const MASKED_MAX_LEVEL: u8 = 2;
/// A typical surface height, used to measure how far above the terrain the camera is.
const REFERENCE_Y: f32 = 8.0;

// ---------------------------------------------------------------------------------------
// Positions
// ---------------------------------------------------------------------------------------

/// A section of the quadtree: level and position in units of its own width.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SectionKey {
    pub level: u8,
    pub x: i32,
    pub z: i32,
}

impl SectionKey {
    pub const fn new(level: u8, x: i32, z: i32) -> SectionKey {
        SectionKey { level, x, z }
    }

    /// The level-`level` section containing block column `(bx, bz)`.
    pub fn containing(level: u8, bx: i32, bz: i32) -> SectionKey {
        let w = CELLS << level;
        SectionKey { level, x: bx.div_euclid(w), z: bz.div_euclid(w) }
    }

    /// Cell width in blocks.
    pub const fn cell(self) -> i32 {
        1 << self.level
    }

    /// Section width in blocks.
    pub const fn width(self) -> i32 {
        CELLS << self.level
    }

    /// Minimum block corner.
    pub fn origin(self) -> (i32, i32) {
        (self.x.wrapping_mul(self.width()), self.z.wrapping_mul(self.width()))
    }

    pub fn parent(self) -> SectionKey {
        SectionKey { level: self.level + 1, x: self.x >> 1, z: self.z >> 1 }
    }

    pub fn children(self) -> [SectionKey; 4] {
        let (l, x, z) = (self.level - 1, self.x * 2, self.z * 2);
        [
            SectionKey::new(l, x, z),
            SectionKey::new(l, x + 1, z),
            SectionKey::new(l, x, z + 1),
            SectionKey::new(l, x + 1, z + 1),
        ]
    }

    /// Does this section cover `other` (strictly coarser and containing it)?
    pub fn is_ancestor_of(self, other: SectionKey) -> bool {
        if other.level >= self.level {
            return false;
        }
        let s = (self.level - other.level) as u32;
        (other.x >> s) == self.x && (other.z >> s) == self.z
    }

    /// Horizontal distance from `(px, pz)` to the nearest point of the section.
    pub fn distance_to(self, px: f32, pz: f32) -> f32 {
        let (ox, oz) = self.origin();
        let w = self.width() as f32;
        let axis = |lo: f32, p: f32| -> f32 {
            if p < lo {
                lo - p
            } else if p > lo + w {
                p - lo - w
            } else {
                0.0
            }
        };
        let (dx, dz) = (axis(ox as f32, px), axis(oz as f32, pz));
        (dx * dx + dz * dz).sqrt()
    }
}

// ---------------------------------------------------------------------------------------
// Which sections to draw
// ---------------------------------------------------------------------------------------

/// How far and how detailed the LOD terrain is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodSettings {
    /// LOD view distance in blocks; 0 turns LODs off.
    pub radius: f32,
    /// Width in blocks of the first ring past the real chunks. Each further ring is `base`
    /// times wider than the one before and its cells twice as coarse.
    pub ring: f32,
    pub base: f32,
    /// Level of the first ring (and of the stand-in terrain under real chunks that haven't
    /// loaded yet): cells of `2^min_level` blocks.
    pub min_level: u8,
    /// Where the real chunks end (their draw distance from the camera), in blocks. The first
    /// ring starts here; nearer than this LOD terrain only stands in for chunks still loading.
    pub near_edge: f32,
}

impl LodSettings {
    pub const OFF: LodSettings = LodSettings { radius: 0.0, ring: 64.0, base: 2.0, min_level: 1, near_edge: 0.0 };

    /// Presets for the menu: (name, first ring width). Cells per ring grow with its square.
    pub const QUALITY: [(&'static str, f32); 3] = [("LOW", 32.0), ("MED", 64.0), ("HIGH", 128.0)];

    pub fn new(radius: f32, ring: f32) -> LodSettings {
        LodSettings { radius, ring, ..LodSettings::OFF }
    }

    /// The same settings with the rings starting `edge` blocks from the camera.
    pub fn with_near_edge(self, edge: f32) -> LodSettings {
        LodSettings { near_edge: edge.max(0.0), ..self }
    }

    pub fn enabled(&self) -> bool {
        self.radius > 0.0
    }

    /// The level a section this far away should be drawn at: the ring it falls in. Ring `k`
    /// (from 0) starts `ring * (base^k - 1) / (base - 1)` blocks past the near edge.
    pub fn level_for_distance(&self, d: f32) -> u8 {
        let x = (d - self.near_edge).max(0.0) / self.ring;
        let k = ((1.0 + x * (self.base - 1.0)).ln() / self.base.ln()).floor();
        (self.min_level as f32 + k.max(0.0)).min(MAX_LEVEL as f32) as u8
    }

    /// Distance from the camera at which `level` starts.
    pub fn ring_start(&self, level: u8) -> f32 {
        let k = level.saturating_sub(self.min_level) as i32;
        self.near_edge + self.ring * (self.base.powi(k) - 1.0) / (self.base - 1.0)
    }


    /// The leaf sections to draw around the camera: Distant Horizons' quadtree walk from the
    /// coarsest level down, splitting while a section is coarser than its distance asks for.
    pub fn select(&self, cam: [f32; 3]) -> Vec<SectionKey> {
        let mut out = Vec::new();
        if !self.enabled() {
            return out;
        }
        let r = self.radius.ceil() as i32;
        let (px, pz) = (cam[0].floor() as i32, cam[2].floor() as i32);
        let lo = SectionKey::containing(MAX_LEVEL, px.saturating_sub(r), pz.saturating_sub(r));
        let hi = SectionKey::containing(MAX_LEVEL, px.saturating_add(r), pz.saturating_add(r));
        let dy = (cam[1] - REFERENCE_Y).abs();
        for x in lo.x..=hi.x {
            for z in lo.z..=hi.z {
                self.walk(SectionKey::new(MAX_LEVEL, x, z), cam, dy, &mut out);
            }
        }
        self.balance(out, cam)
    }

    /// Splits sections until no two neighbours are more than one level apart (a "restricted"
    /// quadtree). Narrow rings would otherwise put, say, 64-block cells next to 16-block
    /// cells, which the crack-proof borders (`border_min`) don't cover.
    fn balance(&self, leaves: Vec<SectionKey>, cam: [f32; 3]) -> Vec<SectionKey> {
        let mut set: HashSet<SectionKey> = leaves.into_iter().collect();
        loop {
            let mut split: HashSet<SectionKey> = HashSet::new();
            for &b in &set {
                // Same-size neighbours of `b`; whatever leaf covers each must be at most one
                // level coarser than `b`.
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let mut n = SectionKey::new(b.level, b.x + dx, b.z + dz);
                    while n.level < MAX_LEVEL && !set.contains(&n) {
                        n = n.parent();
                    }
                    if set.contains(&n) && n.level >= b.level + 2 {
                        split.insert(n);
                    }
                }
            }
            if split.is_empty() {
                break;
            }
            for n in split {
                set.remove(&n);
                for c in n.children() {
                    if c.distance_to(cam[0], cam[2]) <= self.radius {
                        set.insert(c);
                    }
                }
            }
        }
        let mut out: Vec<SectionKey> = set.into_iter().collect();
        out.sort_unstable();
        out
    }

    fn walk(&self, node: SectionKey, cam: [f32; 3], dy: f32, out: &mut Vec<SectionKey>) {
        let dh = node.distance_to(cam[0], cam[2]);
        if dh > self.radius {
            return;
        }
        let d = (dh * dh + dy * dy).sqrt();
        let mut want = self.level_for_distance(d);
        if dh <= self.near_edge + CHUNK_SIZE as f32 {
            // Can touch real chunks (from high up, distance alone would allow coarser).
            want = want.min(MASKED_MAX_LEVEL.max(self.min_level));
        }
        if node.level > want && node.level > self.min_level {
            for c in node.children() {
                self.walk(c, cam, dy, out);
            }
        } else {
            out.push(node);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Meshing
// ---------------------------------------------------------------------------------------

/// One LOD vertex, 8 bytes (shared quad index buffer, like chunk meshes):
///
/// ```text
///   a: x(6) | z(6)<<6 | face(3)<<12 | in_x(2)<<15 | in_z(2)<<17 | top(1)<<19 | texture(8)<<20
///   b: y in quarter blocks, as an i32
/// ```
///
/// `x` and `z` are corners in cells (0..=32) from the section origin. `in_x`/`in_z` point
/// from the corner into the quad's own cell (0 = -1, 1 = 0, 2 = +1): the shader steps half
/// a cell that way to find the chunk column the quad belongs to, and steps slightly the
/// other way to grow merged quads over T-junction cracks. `top` marks the upper corners of
/// side faces (grown upwards for the same reason).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LodVertex {
    pub a: u32,
    pub b: u32,
}

impl LodVertex {
    pub fn x(self) -> u32 {
        self.a & 63
    }
    pub fn z(self) -> u32 {
        (self.a >> 6) & 63
    }
    pub fn face(self) -> u32 {
        (self.a >> 12) & 7
    }
    pub fn inward(self) -> (i32, i32) {
        (((self.a >> 15) & 3) as i32 - 1, ((self.a >> 17) & 3) as i32 - 1)
    }
    pub fn is_top(self) -> bool {
        (self.a >> 19) & 1 == 1
    }
    pub fn texture(self) -> u32 {
        (self.a >> 20) & 255
    }
    pub fn y_q(self) -> i32 {
        self.b as i32
    }
}

/// A built section, ready to upload.
#[derive(Clone, Debug)]
pub struct LodMesh {
    pub key: SectionKey,
    pub vertices: Vec<LodVertex>,
    /// Vertical extent in blocks, for frustum culling.
    pub min_y: f32,
    pub max_y: f32,
}

impl LodMesh {
    pub fn quads(&self) -> usize {
        self.vertices.len() / 4
    }
}

/// Side length of the sampled grid: the section plus a one-cell ring of neighbours.
const GRID: i32 = CELLS + 2;

struct Grid {
    surf: Vec<i32>,
    block: Vec<u16>,
}

impl Grid {
    fn idx(i: i32, j: i32) -> usize {
        ((j + 1) * GRID + (i + 1)) as usize
    }
    fn surf(&self, i: i32, j: i32) -> i32 {
        self.surf[Self::idx(i, j)]
    }
    fn block(&self, i: i32, j: i32) -> u16 {
        self.block[Self::idx(i, j)]
    }
}

/// Height step for a level, in quarter blocks: a quarter of the cell width, at least one
/// block. Rounding to it is invisible at the distance each level is drawn from, and it
/// turns smooth slopes into terraces whose faces merge: about half the quads of exact heights.
fn height_step(cell: i32) -> i32 {
    cell.max(4)
}

fn quantize(surf_q: i32, block: u16, water: u16, cell: i32) -> i32 {
    if block == water || cell <= 1 {
        return surf_q;
    }
    let step = height_step(cell);
    // Land never drops below the sea surface (it would read as a hole in the coast).
    ((surf_q + step / 2).div_euclid(step) * step).max(4 * (crate::worldgen::SEA_LEVEL + 1))
}

/// The surface of cell `(i, j)` of a section as it is drawn (quantized), in quarter blocks.
pub fn cell_surface(gen: &WorldGen, key: SectionKey, i: i32, j: i32) -> (i32, u16) {
    let cell = key.cell();
    let (ox, oz) = key.origin();
    let (s, b) = gen.lod_surface(sample_at(ox + i * cell, cell), sample_at(oz + j * cell, cell), cell);
    (quantize(s, b, gen.water_id(), cell), b)
}

/// Where a level-`cell` cell starting at block `b` is sampled: its centre (whole blocks).
fn sample_at(b: i32, cell: i32) -> i32 {
    if cell > 1 {
        b + cell / 2
    } else {
        b
    }
}

/// The lowest surface a neighbouring section can show along one border cell, in quarter
/// blocks: the neighbour may be at this level, one finer or one coarser (the quadtree keeps
/// neighbours within one level; see the tests), so sample all three and add a block of slack.
fn border_min(gen: &WorldGen, key: SectionKey, i: i32, j: i32) -> i32 {
    let cell = key.cell();
    let (ox, oz) = key.origin();
    let (bx, bz) = (ox + i * cell, oz + j * cell); // the neighbour cell's corner, this level
    let mut m = gen.lod_height(sample_at(bx, cell), sample_at(bz, cell), cell);
    // One coarser: the 2-cell block containing the neighbour cell.
    let c2 = cell * 2;
    let (cx, cz) = (bx.div_euclid(c2) * c2, bz.div_euclid(c2) * c2);
    m = m.min(gen.lod_height(sample_at(cx, c2), sample_at(cz, c2), c2));
    // One finer: the two half-cells of the neighbour that touch this section's edge.
    if cell > 1 {
        let h = cell / 2;
        let (edge_x, edge_z) = match (i, j) {
            (-1, _) => (Some(bx + h), None),
            (CELLS, _) => (Some(bx), None),
            (_, -1) => (None, Some(bz + h)),
            _ => (None, Some(bz)),
        };
        for k in 0..2 {
            let (fx, fz) = match (edge_x, edge_z) {
                (Some(x), _) => (x, bz + k * h),
                (_, Some(z)) => (bx + k * h, z),
                _ => unreachable!(),
            };
            m = m.min(gen.lod_height(sample_at(fx, h), sample_at(fz, h), h));
        }
    }
    // Neighbours round their heights to their own step, at most half the coarser step down.
    m - height_step(cell * 2) / 2 - 4
}

fn tex(reg: &BlockRegistry, block: u16, face: Face) -> u32 {
    // Tall LOD side faces repeat the side texture once per block, grass fringe and all: that
    // is what the stepped slopes they stand for look like up close.
    reg.def(block).tex[face.index()] as u32 & 255
}

/// Accumulates quads; tracks the vertical extent.
struct Emit {
    verts: Vec<LodVertex>,
    min_q: i32,
    max_q: i32,
}

impl Emit {
    /// A box face over cells `[x0, x1] x [z0, z1]` and quarter heights `[y0, y1]`.
    /// `owner` is the horizontal direction from a side face into its own cell.
    #[allow(clippy::too_many_arguments)]
    fn quad(&mut self, face: Face, x0: i32, x1: i32, y0: i32, y1: i32, z0: i32, z1: i32, texture: u32) {
        let fi = face.index() as u32;
        for c in FACE_CORNERS[face.index()] {
            let (x, y, z) = (if c[0] == 1 { x1 } else { x0 }, if c[1] == 1 { y1 } else { y0 }, if c[2] == 1 { z1 } else { z0 });
            // Into the quad along its extent, into the owning cell across a side face.
            let along = |v: i32, lo: i32| if v == lo { 1 } else { -1 };
            let (inx, inz) = match face {
                Face::PosX => (-1, along(z, z0)),
                Face::NegX => (1, along(z, z0)),
                Face::PosZ => (along(x, x0), -1),
                Face::NegZ => (along(x, x0), 1),
                _ => (along(x, x0), along(z, z0)),
            };
            let top = face != Face::PosY && c[1] == 1;
            let a = (x as u32 & 63)
                | ((z as u32 & 63) << 6)
                | (fi << 12)
                | (((inx + 1) as u32) << 15)
                | (((inz + 1) as u32) << 17)
                | ((top as u32) << 19)
                | ((texture & 255) << 20);
            self.verts.push(LodVertex { a, b: y as u32 });
            self.min_q = self.min_q.min(y);
            self.max_q = self.max_q.max(y);
        }
    }
}

/// Builds the mesh of one section from the world seed.
pub fn build_section(gen: &WorldGen, reg: &BlockRegistry, key: SectionKey) -> LodMesh {
    let mut grid = Grid { surf: vec![0; (GRID * GRID) as usize], block: vec![0; (GRID * GRID) as usize] };
    for j in -1..=CELLS {
        for i in -1..=CELLS {
            let corner = (i == -1 || i == CELLS) && (j == -1 || j == CELLS);
            if corner {
                continue; // never a neighbour of a face
            }
            let (s, b) = cell_surface(gen, key, i, j);
            let k = Grid::idx(i, j);
            grid.surf[k] = s;
            grid.block[k] = b;
        }
    }
    // Neighbour heights for border sides: the lowest any neighbouring level can show.
    for t in 0..CELLS {
        for (i, j) in [(-1, t), (CELLS, t), (t, -1), (t, CELLS)] {
            grid.surf[Grid::idx(i, j)] = border_min(gen, key, i, j);
        }
    }

    // Faces are merged only inside a tile. For sections that can meet loaded chunks a tile
    // is one chunk column, so the renderer can hide every quad by its column.
    let tile = if key.level <= MASKED_MAX_LEVEL { (CHUNK_SIZE >> key.level).max(1) } else { CELLS };
    let mut e = Emit { verts: Vec::new(), min_q: i32::MAX, max_q: i32::MIN };

    // Tops: greedy rectangles of equal height and texture.
    let top_key = |i: i32, j: i32| (grid.surf(i, j), tex(reg, grid.block(i, j), Face::PosY));
    let mut used = vec![false; (CELLS * CELLS) as usize];
    for j in 0..CELLS {
        for i in 0..CELLS {
            if used[(j * CELLS + i) as usize] {
                continue;
            }
            let k = top_key(i, j);
            let (i_end, j_end) = ((i / tile + 1) * tile, (j / tile + 1) * tile);
            let mut w = 1;
            while i + w < i_end && !used[(j * CELLS + i + w) as usize] && top_key(i + w, j) == k {
                w += 1;
            }
            let mut h = 1;
            'grow: while j + h < j_end {
                for ii in i..i + w {
                    if used[((j + h) * CELLS + ii) as usize] || top_key(ii, j + h) != k {
                        break 'grow;
                    }
                }
                h += 1;
            }
            for jj in j..j + h {
                for ii in i..i + w {
                    used[(jj * CELLS + ii) as usize] = true;
                }
            }
            e.quad(Face::PosY, i, i + w, k.0, k.0, j, j + h, k.1);
        }
    }

    // Sides: where the neighbour is lower, from its height up to ours, merged along the edge.
    // On a chunk-column boundary the neighbour may be real terrain instead (the LOD there is
    // hidden), which can be lower than its LOD cell: the face reaches below the real ground
    // along that edge, so no slit opens between LOD and real terrain.
    let cell = key.cell();
    let (ox, oz) = key.origin();
    let masked = key.level <= MASKED_MAX_LEVEL;
    let water = gen.water_id();
    let real_edge_min = |face: Face, i: i32, j: i32| -> i32 {
        // The ground just across this cell's `face` edge, at full detail. The ground, not
        // the water surface: real water is see-through, so the face must reach the sea floor.
        let (x0, z0) = (ox + i * cell, oz + j * cell);
        let mut m = i32::MAX;
        for k in 0..cell {
            let (x, z) = match face {
                Face::PosX => (x0 + cell, z0 + k),
                Face::NegX => (x0 - 1, z0 + k),
                Face::PosZ => (x0 + k, z0 + cell),
                _ => (x0 + k, z0 - 1),
            };
            m = m.min(gen.ground_q(x, z));
        }
        m - 4 // and a block of slack
    };
    for face in [Face::PosX, Face::NegX, Face::PosZ, Face::NegZ] {
        let (dx, _, dz) = face.normal();
        for a in 0..CELLS {
            // `a` walks across the edges, `b` along them. Is this edge a chunk-column line?
            let edge_cell = if dx > 0 || dz > 0 { a + 1 } else { a };
            let edge_block = if dx != 0 { ox + edge_cell * cell } else { oz + edge_cell * cell };
            let on_line = masked && edge_block.rem_euclid(CHUNK_SIZE) == 0;
            let bottom_at = |i: i32, j: i32| -> i32 {
                let nb = grid.surf(i + dx, j + dz);
                // LOD water is level with real water, which is a full block at the same height:
                // nothing to close there, and a face under the see-through real water would
                // show through it as a dark curtain.
                if on_line && grid.block(i, j) != water {
                    nb.min(real_edge_min(face, i, j))
                } else {
                    nb
                }
            };
            let mut b = 0;
            while b < CELLS {
                let (i, j) = if dx != 0 { (a, b) } else { (b, a) };
                let own = grid.surf(i, j);
                let bottom = bottom_at(i, j);
                if own <= bottom {
                    b += 1;
                    continue;
                }
                let t = tex(reg, grid.block(i, j), face);
                let end = (b / tile + 1) * tile;
                let mut len = 1;
                while b + len < end {
                    let (i2, j2) = if dx != 0 { (a, b + len) } else { (b + len, a) };
                    if grid.surf(i2, j2) != own || bottom_at(i2, j2) != bottom || tex(reg, grid.block(i2, j2), face) != t {
                        break;
                    }
                    len += 1;
                }
                match face {
                    Face::PosX => e.quad(face, a + 1, a + 1, bottom, own, b, b + len, t),
                    Face::NegX => e.quad(face, a, a, bottom, own, b, b + len, t),
                    Face::PosZ => e.quad(face, b, b + len, bottom, own, a + 1, a + 1, t),
                    _ => e.quad(face, b, b + len, bottom, own, a, a, t),
                }
                b += len;
            }
        }
    }

    let (min_y, max_y) = if e.verts.is_empty() { (0.0, 0.0) } else { (e.min_q as f32 / 4.0, e.max_q as f32 / 4.0) };
    LodMesh { key, vertices: e.verts, min_y, max_y }
}

// ---------------------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------------------

/// What the manager knows about a built section after the renderer took its vertices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SectionInfo {
    pub quads: u32,
    pub min_y: f32,
    pub max_y: f32,
    /// Taken by the renderer (`take_built`). Only uploaded sections are drawn, and a section
    /// is only retired once its replacement is uploaded: no hole while the GPU catches up.
    pub uploaded: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LodStats {
    pub built: u64,
    pub wanted: usize,
    pub drawn: usize,
    pub drawn_quads: u64,
    pub pending: usize,
}

/// Keeps the wanted sections built on worker threads and decides what to draw.
pub struct LodManager {
    settings: LodSettings,
    workers: usize,
    pool: Option<JobPool<SectionKey, LodMesh>>,
    built: HashMap<SectionKey, SectionInfo>,
    outbox: Vec<LodMesh>,
    evicted: Vec<SectionKey>,
    pending: HashSet<SectionKey>,
    wanted: Vec<SectionKey>,
    render: Vec<SectionKey>,
    planned_at: Option<[f32; 3]>,
    camera: [f32; 3],
    dirty: bool,
    pub stats: LodStats,
}

impl LodManager {
    pub fn new(workers: usize) -> LodManager {
        LodManager {
            settings: LodSettings::OFF,
            workers: workers.max(1),
            pool: None,
            built: HashMap::new(),
            outbox: Vec::new(),
            evicted: Vec::new(),
            pending: HashSet::new(),
            wanted: Vec::new(),
            render: Vec::new(),
            planned_at: None,
            camera: [0.0; 3],
            dirty: true,
            stats: LodStats::default(),
        }
    }

    /// Starts the workers once the world (seed and blocks) is known.
    pub fn start(&mut self, gen: Arc<WorldGen>, reg: Arc<BlockRegistry>) {
        self.pool = Some(JobPool::new(self.workers, move |key: SectionKey| build_section(&gen, &reg, key)));
        self.clear();
    }

    pub fn settings(&self) -> LodSettings {
        self.settings
    }

    pub fn set_settings(&mut self, s: LodSettings) {
        if s == self.settings {
            return;
        }
        // A section's mesh depends only on its key, so built sections stay valid whatever
        // the settings; only which ones are wanted changes.
        self.settings = s;
        if !s.enabled() {
            self.clear();
        }
        self.planned_at = None;
        self.dirty = true;
    }

    fn clear(&mut self) {
        self.evicted.extend(self.built.keys().copied());
        self.built.clear();
        self.outbox.clear();
        self.wanted.clear();
        self.render.clear();
        self.planned_at = None;
        self.dirty = true;
    }

    /// Plans, collects finished sections, submits new jobs and picks what to draw.
    pub fn update(&mut self, camera: [f32; 3]) {
        self.camera = camera;
        let Some(pool) = self.pool.as_ref() else { return };
        // Replan when the camera has moved a fraction of the finest ring's width.
        let replan = match self.planned_at {
            None => true,
            Some(p) => {
                let (dx, dy, dz) = (camera[0] - p[0], camera[1] - p[1], camera[2] - p[2]);
                dx * dx + dy * dy + dz * dz > (self.settings.ring * 0.25).powi(2)
            }
        };
        if replan {
            self.wanted = self.settings.select(camera);
            self.planned_at = Some(camera);
            self.dirty = true;
        }

        for mesh in pool.poll() {
            let key = mesh.key;
            self.pending.remove(&key);
            if !self.settings.enabled() {
                continue;
            }
            self.built.insert(key, SectionInfo { quads: mesh.quads() as u32, min_y: mesh.min_y, max_y: mesh.max_y, uploaded: false });
            self.outbox.retain(|m| m.key != key);
            self.outbox.push(mesh);
            self.stats.built += 1;
            self.dirty = true;
        }

        // Nearest first; a shallow queue keeps priorities fresh as the camera moves.
        let max_in_flight = self.workers * 4;
        if pool.in_flight() < max_in_flight {
            let mut missing: Vec<(f32, SectionKey)> = self
                .wanted
                .iter()
                .filter(|k| !self.built.contains_key(k) && !self.pending.contains(k))
                .map(|&k| (k.distance_to(camera[0], camera[2]), k))
                .collect();
            missing.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            for (d, k) in missing.into_iter().take(max_in_flight - pool.in_flight()) {
                self.pending.insert(k);
                pool.submit(k, d as u32);
            }
        }

        if self.dirty {
            self.dirty = false;
            self.choose_render_set();
            self.evict();
        }
        self.stats.wanted = self.wanted.len();
        self.stats.pending = self.pending.len();
        self.stats.drawn = self.render.len();
        self.stats.drawn_quads = self.render.iter().filter_map(|k| self.built.get(k)).map(|i| i.quads as u64).sum();
    }

    /// Every wanted section that is built is drawn. One that isn't is stood in for by a
    /// built ancestor (which then also stands in for its built wanted descendants, so
    /// nothing is drawn twice) or by built descendants that cover it completely. So detail
    /// switches only once the new sections are ready, and nothing that was visible vanishes.
    fn choose_render_set(&mut self) {
        let ready: HashSet<SectionKey> = self.built.iter().filter(|(_, i)| i.uploaded).map(|(k, _)| *k).collect();
        let built = &ready;
        let mut ancestors: HashSet<SectionKey> = HashSet::new();
        let mut from_children: Vec<SectionKey> = Vec::new();
        for &w in &self.wanted {
            if built.contains(&w) {
                continue;
            }
            let mut p = w;
            let mut found = false;
            while p.level < MAX_LEVEL {
                p = p.parent();
                if built.contains(&p) {
                    ancestors.insert(p);
                    found = true;
                    break;
                }
            }
            if !found {
                cover_with_children(built, w, &mut from_children);
            }
        }
        // An ancestor may itself sit under another chosen ancestor: keep the coarsest.
        let ancestor_list: Vec<SectionKey> = ancestors.iter().copied().collect();
        ancestors.retain(|a| !ancestor_list.iter().any(|b| b.is_ancestor_of(*a)));
        let covered = |k: &SectionKey| ancestors.iter().any(|a| a.is_ancestor_of(*k));
        let mut render: Vec<SectionKey> = self.wanted.iter().copied().filter(|k| built.contains(k) && !covered(k)).collect();
        render.extend(from_children.into_iter().filter(|k| !covered(k)));
        render.extend(ancestors);
        render.sort_unstable();
        render.dedup();
        self.render = render;
    }

    /// Drops built sections that are neither wanted nor drawn.
    fn evict(&mut self) {
        let keep: HashSet<SectionKey> = self.wanted.iter().chain(self.render.iter()).copied().collect();
        let drop: Vec<SectionKey> = self.built.keys().filter(|k| !keep.contains(k)).copied().collect();
        for k in drop {
            self.built.remove(&k);
            self.outbox.retain(|m| m.key != k);
            self.evicted.push(k);
        }
    }

    /// Sections to draw this frame (built, possibly not yet uploaded by the renderer).
    pub fn render_set(&self) -> &[SectionKey] {
        &self.render
    }

    pub fn info(&self, key: SectionKey) -> Option<&SectionInfo> {
        self.built.get(&key)
    }

    /// Newly built meshes for the renderer to upload (nearest first, at most `max`).
    pub fn take_built(&mut self, max: usize) -> Vec<LodMesh> {
        let cam = self.camera;
        self.outbox.sort_by(|a, b| {
            let da = a.key.distance_to(cam[0], cam[2]);
            let db = b.key.distance_to(cam[0], cam[2]);
            db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
        });
        let n = self.outbox.len().saturating_sub(max);
        let taken = self.outbox.split_off(n);
        for m in &taken {
            if let Some(i) = self.built.get_mut(&m.key) {
                i.uploaded = true;
            }
        }
        if !taken.is_empty() {
            self.dirty = true;
        }
        taken
    }

    /// Sections whose GPU buffers the renderer should free.
    pub fn take_evicted(&mut self) -> Vec<SectionKey> {
        std::mem::take(&mut self.evicted)
    }

    /// For a renderer that lost its GPU buffers (Android drops the surface in the
    /// background): forget what was built so it is built and handed over again. Sections are
    /// cheap to rebuild from the seed, so no CPU copy of every mesh is kept.
    pub fn requeue_all(&mut self) {
        self.built.clear();
        self.outbox.clear();
        self.render.clear();
        self.dirty = true;
    }

    /// Nothing queued or running, everything wanted is built.
    pub fn is_idle(&self) -> bool {
        self.pending.is_empty() && self.wanted.iter().all(|k| self.built.contains_key(k))
    }
}

fn cover_with_children(built: &HashSet<SectionKey>, k: SectionKey, out: &mut Vec<SectionKey>) -> bool {
    if built.contains(&k) {
        out.push(k);
        return true;
    }
    if k.level == 0 {
        return false;
    }
    let mark = out.len();
    for c in k.children() {
        if !cover_with_children(built, c, out) {
            out.truncate(mark);
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn world() -> (BlockRegistry, WorldGen) {
        let reg = BlockRegistry::builtin();
        let gen = WorldGen::new(1337, &reg);
        (reg, gen)
    }

    fn medium(radius: f32) -> LodSettings {
        LodSettings::new(radius, 64.0)
    }

    #[test]
    fn section_keys_nest() {
        let k = SectionKey::containing(3, -1, 700);
        assert_eq!(k, SectionKey::new(3, -1, 2));
        assert_eq!(k.origin(), (-256, 512));
        for c in k.children() {
            assert_eq!(c.parent(), k);
            assert!(k.is_ancestor_of(c));
            assert!(k.parent().is_ancestor_of(c));
            assert!(!c.is_ancestor_of(k));
        }
        assert!(!k.is_ancestor_of(k));
        assert_eq!(SectionKey::new(0, 0, 0).distance_to(10.0, 10.0), 0.0);
        assert_eq!(SectionKey::new(0, 0, 0).distance_to(-3.0, 36.0), 5.0);
    }

    #[test]
    fn rings_start_at_the_real_chunks_and_double_in_width() {
        let s = medium(16384.0).with_near_edge(200.0);
        assert_eq!(s.level_for_distance(0.0), 1, "stand-in terrain under loading chunks: 2-block cells");
        assert_eq!(s.level_for_distance(199.0), 1);
        assert_eq!(s.level_for_distance(263.0), 1, "first ring: 64 blocks of 2-block cells");
        assert_eq!(s.level_for_distance(265.0), 2);
        assert_eq!(s.level_for_distance(391.0), 2, "second ring: 128 blocks of 4-block cells");
        assert_eq!(s.level_for_distance(393.0), 3);
        assert_eq!(s.level_for_distance(647.0), 3, "third ring: 256 blocks of 8-block cells");
        assert_eq!(s.level_for_distance(649.0), 4);
        assert_eq!(s.level_for_distance(1.0e9), MAX_LEVEL);
        for level in 1..MAX_LEVEL {
            let d = s.ring_start(level);
            assert_eq!(s.level_for_distance(d + 0.01), level);
            let width = s.ring_start(level + 1) - d;
            assert!((width - 64.0 * 2f32.powi(level as i32 - 1)).abs() < 1e-2, "ring {level} is {width} wide");
        }
    }

    #[test]
    fn selection_tiles_the_view_without_gaps_or_overlaps() {
        for (radius, cam) in [(2048.0, [100.0, 70.0, -40.0]), (16384.0, [-5000.5, 12.0, 777.0])] {
            let s = medium(radius);
            let keys = s.select(cam);
            // Sample points across the view: each must be covered exactly once.
            let mut checked = 0;
            for i in -40..=40 {
                for j in -40..=40 {
                    let (x, z) = (cam[0] + i as f32 * radius / 41.0, cam[2] + j as f32 * radius / 41.0);
                    let d = ((x - cam[0]).powi(2) + (z - cam[2]).powi(2)).sqrt();
                    let n = keys.iter().filter(|k| k.distance_to(x, z) == 0.0 && {
                        let (ox, oz) = k.origin();
                        x >= ox as f32 && x < (ox + k.width()) as f32 && z >= oz as f32 && z < (oz + k.width()) as f32
                    }).count();
                    if d < radius * 0.95 {
                        assert_eq!(n, 1, "point {x},{z} covered {n} times");
                        checked += 1;
                    } else {
                        assert!(n <= 1);
                    }
                }
            }
            assert!(checked > 1000);
        }
    }

    /// Every render distance the menu offers, as the game passes it (`Game::view_blocks`).
    fn near_edges() -> impl Iterator<Item = f32> {
        (2..=12).map(|r| (r as f32 + 0.5) * CHUNK_SIZE as f32).chain([0.0])
    }

    #[test]
    fn neighbouring_sections_differ_by_at_most_one_level() {
        // The border skirts assume it (see `border_min`).
        for (_, ring) in LodSettings::QUALITY {
            for edge in near_edges() {
                let s = LodSettings::new(16384.0, ring).with_near_edge(edge);
                for cam in [[0.0, 70.0, 0.0], [12345.6, 300.0, -999.0], [-31.0, -40.0, 4000.0]] {
                    let keys = s.select(cam);
                    let mut level_at: HashMap<(i32, i32), u8> = HashMap::new();
                    // Paint each section's footprint on a grid of the finest sections.
                    let fine = CELLS << s.min_level;
                    for k in &keys {
                        let (ox, oz) = k.origin();
                        let n = k.width() / fine;
                        if n > 64 {
                            continue; // far coarse sections: checked through their finer neighbours
                        }
                        for a in 0..n {
                            for b in 0..n {
                                level_at.insert((ox.div_euclid(fine) + a, oz.div_euclid(fine) + b), k.level);
                            }
                        }
                    }
                    for (&(x, z), &l) in &level_at {
                        for (dx, dz) in [(1, 0), (0, 1)] {
                            if let Some(&m) = level_at.get(&(x + dx, z + dz)) {
                                assert!((l as i32 - m as i32).abs() <= 1, "levels {l} and {m} touch (ring {ring}, edge {edge})");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn coarse_sections_never_reach_the_loaded_chunks() {
        // Real chunks are only drawn (and LOD terrain only cut out) within the near edge.
        for (_, ring) in LodSettings::QUALITY {
            for edge in near_edges() {
                // Also from high up, where distance alone would pick coarse sections.
                for cam in [[5.0, 20.0, -7.0], [5.0, 420.0, -7.0]] {
                    let s = LodSettings::new(32768.0, ring).with_near_edge(edge);
                    for k in s.select(cam) {
                        if k.level > MASKED_MAX_LEVEL {
                            assert!(k.distance_to(cam[0], cam[2]) > edge + CHUNK_SIZE as f32, "{k:?} at {cam:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_view_costs_the_log_of_its_distance() {
        let s = medium(1024.0);
        let near = s.select([0.0, 20.0, 0.0]).len();
        let far = LodSettings { radius: 32768.0, ..s }.select([0.0, 20.0, 0.0]).len();
        // 32x the distance (1024x the area) costs only a few more rings of sections.
        assert!(far < near * 5, "{near} -> {far}");
    }

    fn check_winding(m: &LodMesh) {
        for q in m.vertices.chunks(4) {
            let p = |v: &LodVertex| [v.x() as f32, v.y_q() as f32 / 4.0, v.z() as f32];
            let (a, b, c) = (p(&q[0]), p(&q[1]), p(&q[2]));
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            let f = Face::ALL[q[0].face() as usize].normal();
            let dot = n[0] * f.0 as f32 + n[1] * f.1 as f32 + n[2] * f.2 as f32;
            assert!(dot > 0.0, "quad {q:?} is wound inward");
        }
    }

    #[test]
    fn meshes_are_closed_and_outward_facing() {
        let (reg, gen) = world();
        for key in [SectionKey::new(0, 0, 0), SectionKey::new(2, -3, 1), SectionKey::new(5, 1, -2), SectionKey::new(8, 0, 0)] {
            let m = build_section(&gen, &reg, key);
            assert!(m.quads() > 0);
            check_winding(&m);
            // Every cell's top is covered by exactly one top quad.
            let mut cover = vec![0u8; (CELLS * CELLS) as usize];
            for q in m.vertices.chunks(4).filter(|q| q[0].face() == Face::PosY.index() as u32) {
                let xs: Vec<u32> = q.iter().map(|v| v.x()).collect();
                let zs: Vec<u32> = q.iter().map(|v| v.z()).collect();
                for z in *zs.iter().min().unwrap()..*zs.iter().max().unwrap() {
                    for x in *xs.iter().min().unwrap()..*xs.iter().max().unwrap() {
                        cover[(z * CELLS as u32 + x) as usize] += 1;
                    }
                }
            }
            assert!(cover.iter().all(|&c| c == 1), "{key:?}");
            assert!(m.min_y <= m.max_y);
        }
    }

    #[test]
    fn side_faces_close_every_step() {
        // Walking along any row, every change of height has a side face covering it.
        let (reg, gen) = world();
        let key = SectionKey::new(1, 3, -2);
        let m = build_section(&gen, &reg, key);
        let surf = |i: i32, j: i32| cell_surface(&gen, key, i, j).0;
        for j in 0..CELLS {
            for i in 0..CELLS - 1 {
                let (a, b) = (surf(i, j), surf(i + 1, j));
                if a == b {
                    continue;
                }
                let (face, x) = if a > b { (Face::PosX, i + 1) } else { (Face::NegX, i + 1) };
                let (lo, hi) = (a.min(b), a.max(b));
                let ok = m.vertices.chunks(4).any(|q| {
                    q[0].face() == face.index() as u32
                        && q[0].x() as i32 == x
                        && q.iter().map(|v| v.z() as i32).min().unwrap() <= j
                        && q.iter().map(|v| v.z() as i32).max().unwrap() > j
                        && q.iter().map(|v| v.y_q()).min().unwrap() <= lo
                        && q.iter().map(|v| v.y_q()).max().unwrap() >= hi
                });
                assert!(ok, "no side between cells {i} and {} in row {j}", i + 1);
            }
        }
    }

    #[test]
    fn quads_near_real_chunks_stay_inside_one_chunk_column() {
        let (reg, gen) = world();
        for level in 0..=MASKED_MAX_LEVEL {
            let key = SectionKey::new(level, 2, -1);
            let m = build_section(&gen, &reg, key);
            let (ox, oz) = key.origin();
            let cell = key.cell();
            for q in m.vertices.chunks(4) {
                let cols: HashSet<(i32, i32)> = q
                    .iter()
                    .map(|v| {
                        let (ix, iz) = v.inward();
                        // Half a cell into the quad, as the shader does.
                        let x = ox as f32 + (v.x() as f32 + ix as f32 * 0.5) * cell as f32;
                        let z = oz as f32 + (v.z() as f32 + iz as f32 * 0.5) * cell as f32;
                        ((x / CHUNK_SIZE as f32).floor() as i32, (z / CHUNK_SIZE as f32).floor() as i32)
                    })
                    .collect();
                assert_eq!(cols.len(), 1, "level {level}: quad spans columns {cols:?}");
            }
        }
    }

    #[test]
    fn faces_on_chunk_lines_reach_below_the_real_terrain_beside_them() {
        // Where LOD meets real chunks, the real ground (or sea floor: water is see-through)
        // may be lower than its LOD cell. The LOD side on every chunk-column line must reach
        // below the real ground across it.
        let (reg, gen) = world();
        for key in [SectionKey::new(1, -2, 3), SectionKey::new(2, 1, 1)] {
            let m = build_section(&gen, &reg, key);
            let (ox, oz) = key.origin();
            let cell = key.cell();
            let mut checked = 0;
            for a in 0..CELLS {
                for (face, x) in [(Face::NegX, a), (Face::PosX, a + 1)] {
                    if (ox + x * cell).rem_euclid(CHUNK_SIZE) != 0 {
                        continue;
                    }
                    for j in 0..CELLS {
                        let (own, block) = cell_surface(&gen, key, a, j);
                        if block == gen.water_id() {
                            continue; // level with the real water beside it
                        }
                        let across = if face == Face::PosX { ox + (a + 1) * cell } else { ox + a * cell - 1 };
                        let real = (0..cell).map(|k| gen.ground_q(across, oz + j * cell + k)).min().unwrap();
                        if own <= real {
                            continue;
                        }
                        checked += 1;
                        let ok = m.vertices.chunks(4).any(|q| {
                            q[0].face() == face.index() as u32
                                && q[0].x() as i32 == x
                                && q.iter().map(|v| v.z() as i32).min().unwrap() <= j
                                && q.iter().map(|v| v.z() as i32).max().unwrap() > j
                                && q.iter().map(|v| v.y_q()).min().unwrap() < real
                                && q.iter().map(|v| v.y_q()).max().unwrap() >= own
                        });
                        assert!(ok, "{key:?}: no face down to the real ground at line x={x}, cell {j}");
                    }
                }
            }
            assert!(checked > 0, "{key:?}: nothing to check");
        }
    }

    #[test]
    fn flat_sea_merges_into_few_quads() {
        let (reg, gen) = world();
        // Somewhere coarse enough that the far terrain is mostly flat: few quads per cell.
        let m = build_section(&gen, &reg, SectionKey::new(8, 3, 3));
        assert!(m.quads() < (CELLS * CELLS) as usize, "{} quads", m.quads());
    }

    #[test]
    fn manager_never_shows_a_hole_even_when_uploads_lag_behind() {
        let reg = Arc::new(BlockRegistry::builtin());
        let gen = Arc::new(WorldGen::new(7, &reg));
        let mut m = LodManager::new(2);
        m.start(gen, reg);
        m.set_settings(medium(4096.0).with_near_edge(208.0));
        // What a renderer would have on the GPU.
        let mut gpu: HashSet<SectionKey> = HashSet::new();
        let frame = |m: &mut LodManager, cam: [f32; 3], uploads: usize, gpu: &mut HashSet<SectionKey>| {
            m.update(cam);
            for k in m.take_evicted() {
                gpu.remove(&k);
            }
            for mesh in m.take_built(uploads) {
                gpu.insert(mesh.key);
            }
        };
        let cam = [0.0, 30.0, 0.0];
        let start = Instant::now();
        loop {
            frame(&mut m, cam, usize::MAX, &mut gpu);
            if m.stats.wanted > 0 && m.is_idle() && m.render_set().len() == m.stats.wanted {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(60), "never settled");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(m.render_set().iter().all(|k| k.level >= 1));

        // Fly fast with a renderer that uploads only two sections a frame. Every frame, what
        // is drawn must be on the GPU and must cover the ground once: no holes, no doubles.
        let probe: Vec<(f32, f32)> = (0..400).map(|i| ((i % 20) as f32 * 97.0 - 950.0, (i / 20) as f32 * 97.0 - 950.0)).collect();
        let mut c = cam;
        for step in 0..150 {
            c[0] += 12.0;
            c[2] -= 5.0;
            frame(&mut m, c, 2, &mut gpu);
            let set = m.render_set();
            for k in set {
                assert!(gpu.contains(k), "step {step}: {k:?} drawn but not uploaded");
            }
            for &(x, z) in &probe {
                let (x, z) = (x + c[0], z + c[2]);
                let n = set.iter().filter(|k| {
                    let (ox, oz) = k.origin();
                    x >= ox as f32 && x < (ox + k.width()) as f32 && z >= oz as f32 && z < (oz + k.width()) as f32
                }).count();
                if n != 1 {
                    let has = |k: &SectionKey| {
                        let (ox, oz) = k.origin();
                        x >= ox as f32 && x < (ox + k.width()) as f32 && z >= oz as f32 && z < (oz + k.width()) as f32
                    };
                    let w: Vec<_> = m.wanted.iter().filter(|k| has(k)).map(|k| (*k, m.built.get(k).copied())).collect();
                    let r: Vec<_> = set.iter().filter(|k| has(k)).collect();
                    panic!("step {step}: point {x},{z} drawn {n} times; wanted {w:?}; drawn {r:?}; cam {c:?}");
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        m.set_settings(LodSettings::OFF);
        m.update(c);
        assert!(m.render_set().is_empty());
        assert!(!m.take_evicted().is_empty());
    }
}
