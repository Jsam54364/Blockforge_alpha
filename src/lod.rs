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
//! * **Edits show far away too.** The server sends the top of every block column an edit
//!   has touched (`ColumnTop`, from the edit journal, so edits from earlier sessions count),
//!   and sections use those instead of the seed (`SurfaceEdits`). A cell shows the highest
//!   edited column inside it when that rises above the terrain (so a tower stays visible
//!   from kilometres away), or the edited column at its sample point otherwise (a pit shows
//!   once cells are small enough to land in it). Sections an edit touches are rebuilt,
//!   and drawn as they were until the new mesh is ready.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

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
// Edits
// ---------------------------------------------------------------------------------------

/// The top of one block column as edits left it: the height of its top surface in quarter
/// blocks and the block there.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ColumnTop {
    pub x: i32,
    pub z: i32,
    pub top_q: i32,
    pub block: u16,
}

/// Edited column tops, grouped by chunk column.
#[derive(Clone, Debug, Default)]
pub struct SurfaceEdits {
    by_chunk: HashMap<(i32, i32), Vec<ColumnTop>>,
}

/// What edits put in one LOD cell: the highest edited column in it, and the edited column
/// at its sample point if there is one.
#[derive(Copy, Clone, Debug)]
struct CellEdits {
    highest: ColumnTop,
    at_sample: Option<ColumnTop>,
}

impl SurfaceEdits {
    pub fn is_empty(&self) -> bool {
        self.by_chunk.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_chunk.values().map(|v| v.len()).sum()
    }

    pub fn get(&self, x: i32, z: i32) -> Option<ColumnTop> {
        let v = self.by_chunk.get(&(x.div_euclid(CHUNK_SIZE), z.div_euclid(CHUNK_SIZE)))?;
        v.iter().find(|t| t.x == x && t.z == z).copied()
    }

    /// Records `t`, replacing what was known of its column. Returns whether anything changed.
    pub fn set(&mut self, t: ColumnTop) -> bool {
        let v = self.by_chunk.entry((t.x.div_euclid(CHUNK_SIZE), t.z.div_euclid(CHUNK_SIZE))).or_default();
        match v.iter_mut().find(|o| o.x == t.x && o.z == t.z) {
            Some(o) if *o == t => false,
            Some(o) => {
                *o = t;
                true
            }
            None => {
                v.push(t);
                true
            }
        }
    }

    /// The edits in each cell of `key`, its one-cell border included, by cell `(i, j)`.
    fn cells(&self, key: SectionKey) -> HashMap<(i32, i32), CellEdits> {
        let mut out: HashMap<(i32, i32), CellEdits> = HashMap::new();
        if self.by_chunk.is_empty() {
            return out;
        }
        let cell = key.cell();
        let (ox, oz) = key.origin();
        let (x0, z0) = (ox - cell, oz - cell);
        let (x1, z1) = (ox + key.width() + cell, oz + key.width() + cell);
        let (c0x, c0z, c1x, c1z) = (x0.div_euclid(CHUNK_SIZE), z0.div_euclid(CHUNK_SIZE), (x1 - 1).div_euclid(CHUNK_SIZE), (z1 - 1).div_euclid(CHUNK_SIZE));
        let mut add = |t: &ColumnTop| {
            if t.x < x0 || t.x >= x1 || t.z < z0 || t.z >= z1 {
                return;
            }
            let (i, j) = ((t.x - ox).div_euclid(cell), (t.z - oz).div_euclid(cell));
            let at = sample_at(ox + i * cell, cell) == t.x && sample_at(oz + j * cell, cell) == t.z;
            let e = out.entry((i, j)).or_insert(CellEdits { highest: *t, at_sample: None });
            if t.top_q > e.highest.top_q {
                e.highest = *t;
            }
            if at {
                e.at_sample = Some(*t);
            }
        };
        // Whichever is fewer: the section's chunk columns, or the edited ones.
        let span = (c1x - c0x + 1) as usize * (c1z - c0z + 1) as usize;
        if span <= self.by_chunk.len() {
            for cx in c0x..=c1x {
                for cz in c0z..=c1z {
                    self.by_chunk.get(&(cx, cz)).into_iter().flatten().for_each(&mut add);
                }
            }
        } else {
            for (&(cx, cz), v) in &self.by_chunk {
                if (c0x..=c1x).contains(&cx) && (c0z..=c1z).contains(&cz) {
                    v.iter().for_each(&mut add);
                }
            }
        }
        out
    }
}

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
///      | texture's high bits(3)<<29 (an 11-bit texture: up to 2048)
///   b: y in quarter blocks, as an i16
/// ```
///
/// `x` and `z` are corners in cells (0..=32) from the section origin. `in_x`/`in_z` point
/// from the corner into the quad's own voxel (0 = -1, 1 = 0, 2 = +1): the shader steps half
/// a cell that way (and a little into the voxel vertically: down from tops and from the
/// upper corners of sides, up from bottoms and lower corners) to find the chunk the quad
/// belongs to, and steps slightly the other way to grow merged quads over T-junction cracks.
/// `top` marks the upper corners of side faces.
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
        ((self.a >> 20) & 255) | (((self.a >> 29) & 7) << 8)
    }
    pub fn y_q(self) -> i32 {
        self.b as u16 as i16 as i32
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

/// Height of a voxel layer, in blocks, for cells `cell` blocks wide: cubes up to 8 blocks
/// (Voxy-style: the world downscaled into bigger blocks), then slabs a quarter as tall as
/// they are wide (8 to 32 blocks), so distant mountains keep their shape instead of turning
/// into a few giant steps.
pub fn voxel_height(cell: i32) -> i32 {
    if cell <= 8 {
        cell.max(1)
    } else {
        (cell / 4).clamp(8, 32)
    }
}

/// One column of a section as it is drawn, in voxels.
#[derive(Clone, Copy, Debug)]
struct Col {
    /// Top of its solid voxels, in blocks (a whole number of layers).
    top: i32,
    /// The water surface over it, in quarter blocks (drawn as a flat face at that height).
    water_q: Option<i32>,
    surface: u16,
    sub: u16,
    rock: u16,
    /// A forest canopy over it: bottom and top (blocks) and the leaves.
    canopy: Option<(i32, i32, u16)>,
}

impl Col {
    /// The block of the solid voxel whose top is at `y`: the surface block in the top layer,
    /// the subsurface block within 4 blocks of the surface, rock below (cliffs show stone).
    fn material(&self, y: i32) -> u16 {
        match self.top - y {
            d if d <= 0 => self.surface,
            d if d < 4 => self.sub,
            _ => self.rock,
        }
    }
}

fn div_round(a: i32, b: i32) -> i32 {
    (a + b / 2).div_euclid(b)
}

/// The lowest whole layer at or above the sea surface: land never sinks under the water (it
/// would read as a hole in the coast).
fn lowest_land(vh: i32) -> i32 {
    (crate::worldgen::SEA_LEVEL + 1 + vh - 1).div_euclid(vh) * vh
}

/// Column `(i, j)` of a section as drawn, with the edits in its cell.
fn column(gen: &WorldGen, key: SectionKey, i: i32, j: i32, edits: Option<&CellEdits>) -> Col {
    let cell = key.cell();
    let (ox, oz) = key.origin();
    let c = gen.lod_column(sample_at(ox + i * cell, cell), sample_at(oz + j * cell, cell), cell);
    column_from(gen, key, c, edits)
}

/// `column`, from the generator's column there.
fn column_from(gen: &WorldGen, key: SectionKey, c: crate::worldgen::LodColumn, edits: Option<&CellEdits>) -> Col {
    let vh = voxel_height(key.cell());
    let (mut top_q, mut surface, mut water) = (if c.water { c.top_q } else { c.ground_q }, c.surface, c.water);
    let mut canopy = c.canopy;
    if let Some(e) = edits {
        canopy = None;
        if e.highest.top_q > top_q {
            (top_q, surface, water) = (e.highest.top_q, e.highest.block, false);
        } else if let Some(t) = e.at_sample {
            (top_q, surface, water) = (t.top_q, t.block, t.block == gen.water_id());
        }
    }
    if water {
        // The bed under LOD water (seas and rivers): solid up to a layer below the surface.
        let below = (top_q.div_euclid(4) - 1).div_euclid(vh) * vh;
        let bed = div_round(c.ground_q, 4 * vh) * vh;
        Col { top: bed.min(below), water_q: Some(top_q), surface: c.sub, sub: c.sub, rock: c.rock, canopy: None }
    } else {
        let top = (div_round(top_q, 4 * vh) * vh).max(lowest_land(vh));
        Col { top, water_q: None, surface, sub: c.sub, rock: c.rock, canopy }
    }
}

/// The surface of cell `(i, j)` of a section as it is drawn, in quarter blocks, and its block:
/// the water surface over water, else the top of the solid voxels.
pub fn cell_surface(gen: &WorldGen, key: SectionKey, i: i32, j: i32) -> (i32, u16) {
    let c = column(gen, key, i, j, None);
    match c.water_q {
        Some(q) => (q, gen.water_id()),
        None => (4 * c.top, c.surface),
    }
}

/// Where a level-`cell` cell starting at block `b` is sampled: its centre (whole blocks).
fn sample_at(b: i32, cell: i32) -> i32 {
    if cell > 1 {
        b + cell / 2
    } else {
        b
    }
}

/// The lowest top a neighbouring section can show along one border cell, in blocks: the
/// neighbour may be at this level, one finer or one coarser (the quadtree keeps neighbours
/// within one level; see the tests), so sample all three, allow for each rounding to its own
/// layers, and add a block of slack. Border voxels' sides reach down to it (skirts): no crack
/// opens between sections of different detail.
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
    // Rounded to the coarser level's layers, at most half a layer down; and a block of slack.
    m.div_euclid(4) - voxel_height(c2) / 2 - voxel_height(c2) - 1
}

fn tex(reg: &BlockRegistry, block: u16, face: Face) -> u32 {
    reg.def(block).tex[face.index()] as u32 & 2047
}

/// Accumulates quads; tracks the vertical extent.
struct Emit {
    verts: Vec<LodVertex>,
    min_q: i32,
    max_q: i32,
}

impl Emit {
    /// A box face over cells `[x0, x1] x [z0, z1]` and quarter heights `[y0, y1]`.
    #[allow(clippy::too_many_arguments)]
    fn quad(&mut self, face: Face, x0: i32, x1: i32, y0: i32, y1: i32, z0: i32, z1: i32, texture: u32) {
        let fi = face.index() as u32;
        for c in FACE_CORNERS[face.index()] {
            let (x, y, z) = (if c[0] == 1 { x1 } else { x0 }, if c[1] == 1 { y1 } else { y0 }, if c[2] == 1 { z1 } else { z0 });
            // Into the quad along its extent, into the owning voxel across a side face.
            let along = |v: i32, lo: i32| if v == lo { 1 } else { -1 };
            let (inx, inz) = match face {
                Face::PosX => (-1, along(z, z0)),
                Face::NegX => (1, along(z, z0)),
                Face::PosZ => (along(x, x0), -1),
                Face::NegZ => (along(x, x0), 1),
                _ => (along(x, x0), along(z, z0)),
            };
            // Upper corners of side faces (the shader looks down from them into the voxel).
            let top = matches!(face, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ) && y == y1 && y1 != y0;
            let a = (x as u32 & 63)
                | ((z as u32 & 63) << 6)
                | (fi << 12)
                | (((inx + 1) as u32) << 15)
                | (((inz + 1) as u32) << 17)
                | ((top as u32) << 19)
                | ((texture & 255) << 20)
                | ((texture >> 8 & 7) << 29);
            self.verts.push(LodVertex { a, b: y as i16 as u16 as u32 });
            self.min_q = self.min_q.min(y);
            self.max_q = self.max_q.max(y);
        }
    }
}

/// Greedy rectangles over a `w` x `h` grid of face textures (0 = no face), never crossing
/// a tile boundary (`tile_u`, `tile_v(v)`: the tile of each row). Calls `out(u, v, du, dv, tex)`.
fn greedy(w: i32, h: i32, mask: &mut [u32], tile_u: i32, tile_v: &dyn Fn(i32) -> i32, out: &mut dyn FnMut(i32, i32, i32, i32, u32)) {
    for v in 0..h {
        let mut u = 0;
        while u < w {
            let t = mask[(v * w + u) as usize];
            if t == 0 {
                u += 1;
                continue;
            }
            let u_end = ((u / tile_u + 1) * tile_u).min(w);
            let mut du = 1;
            while u + du < u_end && mask[(v * w + u + du) as usize] == t {
                du += 1;
            }
            let mut dv = 1;
            'grow: while v + dv < h && tile_v(v + dv) == tile_v(v) {
                for uu in u..u + du {
                    if mask[((v + dv) * w + uu) as usize] != t {
                        break 'grow;
                    }
                }
                dv += 1;
            }
            for vv in v..v + dv {
                for uu in u..u + du {
                    mask[(vv * w + uu) as usize] = 0;
                }
            }
            out(u, v, du, dv, t);
            u += du;
        }
    }
}

/// Builds the mesh of one section from the world seed.
pub fn build_section(gen: &WorldGen, reg: &BlockRegistry, key: SectionKey) -> LodMesh {
    build_section_edited(gen, reg, &SurfaceEdits::default(), key)
}

/// `build_section` over the terrain as edits left it.
///
/// Voxy-style: the section is a grid of voxels `cell` blocks wide and `voxel_height` tall,
/// each one block type, filled from the generator's columns; every voxel face open to air is
/// drawn with that block's texture (one texture per voxel: it looks like the world in bigger
/// blocks), and coplanar faces of one texture are merged into rectangles.
pub fn build_section_edited(gen: &WorldGen, reg: &BlockRegistry, edits: &SurfaceEdits, key: SectionKey) -> LodMesh {
    let edited = edits.cells(key);
    let cell = key.cell();
    let vh = voxel_height(cell);
    let (ox, oz) = key.origin();
    let water = gen.water_id();

    // Columns, a one-cell ring included (the ring only decides which border faces show; it
    // is lowered to what any neighbouring section can show there).
    let gi = |i: i32, j: i32| ((j + 1) * GRID + (i + 1)) as usize;
    let mut cols = vec![Col { top: 0, water_q: None, surface: 0, sub: 0, rock: 0, canopy: None }; (GRID * GRID) as usize];
    let generated = gen.lod_grid(sample_at(ox - cell, cell), sample_at(oz - cell, cell), cell, GRID as usize);
    for j in -1..=CELLS {
        for i in -1..=CELLS {
            let ring = i == -1 || i == CELLS || j == -1 || j == CELLS;
            let corner = (i == -1 || i == CELLS) && (j == -1 || j == CELLS);
            if corner {
                continue; // never a neighbour of a face
            }
            let mut c = column_from(gen, key, generated[((j + 1) * GRID + i + 1) as usize], edited.get(&(i, j)));
            if ring {
                let mut m = border_min(gen, key, i, j);
                if edited.contains_key(&(i, j)) {
                    m = m.min(c.top - voxel_height(cell * 2) - 1); // an edit may have lowered it
                }
                c.top = m.div_euclid(vh) * vh;
                c.water_q = None;
                c.canopy = None;
            }
            cols[gi(i, j)] = c;
        }
    }

    // The voxels between the lowest and the highest top: below is solid, above is air.
    let lowest = (-1..=CELLS)
        .flat_map(|j| (-1..=CELLS).map(move |i| (i, j)))
        .filter(|&(i, j)| !((i == -1 || i == CELLS) && (j == -1 || j == CELLS)))
        .map(|(i, j)| cols[gi(i, j)].top)
        .min()
        .unwrap_or(0);
    // Where this section can meet real chunks, sides on chunk-column lines may reach down to
    // the real ground beside them (see `real_min` below): the voxels must go that deep.
    let masked = key.level <= MASKED_MAX_LEVEL;
    // The real ground on both sides of every chunk-column line through the section, at
    // full detail (fetched in one batch: one at a time costs a lot more).
    let mut line_ground: HashMap<(i32, i32), i32> = HashMap::new();
    if masked {
        let mut pts = Vec::new();
        for a in 0..=CELLS {
            if (ox + a * cell).rem_euclid(CHUNK_SIZE) == 0 {
                for t in 0..CELLS * cell {
                    pts.push((ox + a * cell, oz + t));
                    pts.push((ox + a * cell - 1, oz + t));
                }
            }
            if (oz + a * cell).rem_euclid(CHUNK_SIZE) == 0 {
                for t in 0..CELLS * cell {
                    pts.push((ox + t, oz + a * cell));
                    pts.push((ox + t, oz + a * cell - 1));
                }
            }
        }
        pts.sort_unstable();
        pts.dedup();
        let qs = gen.ground_q_many(&pts);
        line_ground = pts.into_iter().zip(qs).collect();
    }
    let lines_low = if masked { line_ground.values().copied().min().unwrap_or(i32::MAX).div_euclid(4) - 1 } else { i32::MAX };
    let y0 = (lowest.min(lines_low.div_euclid(vh) * vh)) - vh;
    // Trees near the real chunks are drawn one by one (each crown in voxels); farther out
    // forests are a canopy over each column (`LodColumn::canopy`).
    let trees = if key.level <= MASKED_MAX_LEVEL {
        let m = 8;
        gen.lod_trees(ox - m, oz - m, ox + key.width() + m, oz + key.width() + m, cell)
    } else {
        Vec::new()
    };
    let highest = (0..CELLS)
        .flat_map(|j| (0..CELLS).map(move |i| (i, j)))
        .map(|(i, j)| {
            let c = &cols[gi(i, j)];
            c.top.max(c.canopy.map_or(i32::MIN, |k| k.1 + 1))
        })
        .max()
        .unwrap_or(0)
        .max(trees.iter().filter_map(|t| t.0.crown()).map(|c| c.2 + 1).max().unwrap_or(i32::MIN));
    let y1 = highest.max(y0 + vh);
    let layers = (y1 - y0) / vh;
    let vi = |i: i32, k: i32, j: i32| ((k * GRID + j + 1) * GRID + i + 1) as usize;
    let mut vox = vec![0u16; (GRID * GRID * layers) as usize];
    for j in -1..=CELLS {
        for i in -1..=CELLS {
            let c = &cols[gi(i, j)];
            for k in 0..layers {
                let top = y0 + (k + 1) * vh;
                if top <= c.top {
                    // Ring voxels only need to be solid (any non-zero block).
                    vox[vi(i, k, j)] = if c.surface == 0 { 1 } else { c.material(top) };
                }
            }
        }
    }
    // Canopies: leaves in the layers between their bottom and top, over the ground.
    for j in 0..CELLS {
        for i in 0..CELLS {
            let c = cols[gi(i, j)];
            if let Some((bottom, top, leaves)) = c.canopy {
                for k in 0..layers {
                    let mid = y0 + k * vh + vh / 2;
                    if mid >= bottom && mid <= top && y0 + k * vh >= c.top {
                        vox[vi(i, k, j)] = leaves;
                    }
                }
            }
        }
    }
    // Single trees: their crowns as voxels (rounded to the voxels' size), and at the finest
    // level their trunks.
    for (f, log, leaves) in &trees {
        let Some((r, bottom, top)) = f.crown() else { continue };
        let (cx, cz) = (f.x as f32 + 0.5, f.z as f32 + 0.5);
        let rr = r as f32 + cell as f32 * 0.35;
        let half = (top - bottom) as f32 * 0.5 + vh as f32 * 0.5;
        let mid_y = (top + bottom) as f32 * 0.5;
        let (i0, i1) = (((cx - rr - ox as f32) / cell as f32).floor() as i32, ((cx + rr - ox as f32) / cell as f32).floor() as i32);
        let (j0, j1) = (((cz - rr - oz as f32) / cell as f32).floor() as i32, ((cz + rr - oz as f32) / cell as f32).floor() as i32);
        for j in j0.max(0)..=j1.min(CELLS - 1) {
            for i in i0.max(0)..=i1.min(CELLS - 1) {
                let (px, pz) = (ox as f32 + (i as f32 + 0.5) * cell as f32, oz as f32 + (j as f32 + 0.5) * cell as f32);
                let d = ((px - cx).powi(2) + (pz - cz).powi(2)).sqrt() / rr;
                if d > 1.0 {
                    continue;
                }
                // A flat-topped crown, a little narrower toward the top and bottom layers: the
                // tops of neighbouring trees merge into large faces, as a canopy reads afar.
                for k in 0..layers {
                    let y = (y0 + k * vh) as f32 + vh as f32 * 0.5;
                    let e = ((y - mid_y) / half).abs();
                    if e <= 1.0 && d <= 1.0 - 0.35 * (e - 0.6).max(0.0) / 0.4 && vox[vi(i, k, j)] == 0 {
                        vox[vi(i, k, j)] = *leaves;
                    }
                }
            }
        }
        if cell <= 2 {
            let (i, j) = ((f.x - ox).div_euclid(cell), (f.z - oz).div_euclid(cell));
            if (0..CELLS).contains(&i) && (0..CELLS).contains(&j) {
                for k in 0..layers {
                    let y = y0 + k * vh;
                    if y >= f.y && y < bottom && vox[vi(i, k, j)] == 0 {
                        vox[vi(i, k, j)] = *log;
                    }
                }
            }
        }
    }
    let solid = |i: i32, k: i32, j: i32| -> bool {
        if k < 0 {
            return true;
        }
        if k >= layers {
            return false;
        }
        vox[vi(i, k, j)] != 0
    };

    // Where this section can meet real chunks, faces are merged only within one chunk (in
    // all three directions), so the renderer can hide each quad by the chunk it belongs to.
    let tile = if masked { (CHUNK_SIZE / cell).max(1) } else { CELLS };
    let layer_tile = |k: i32| if masked { (y0 + k * vh).div_euclid(CHUNK_SIZE) } else { 0 };
    // On a chunk-column line the voxel beside may be real terrain instead (its LOD hidden),
    // which can be lower than its LOD column: sides there reach down to the real ground, so
    // no slit opens between LOD and real terrain.
    let real_min = |face: Face, i: i32, j: i32| -> i32 {
        let (x0, z0) = (ox + i * cell, oz + j * cell);
        let mut m = i32::MAX;
        for k in 0..cell {
            let (x, z) = match face {
                Face::PosX => (x0 + cell, z0 + k),
                Face::NegX => (x0 - 1, z0 + k),
                Face::PosZ => (x0 + k, z0 + cell),
                _ => (x0 + k, z0 - 1),
            };
            m = m.min(line_ground.get(&(x, z)).copied().unwrap_or_else(|| gen.ground_q(x, z)));
        }
        m.div_euclid(4) - 1
    };

    let mut e = Emit { verts: Vec::new(), min_q: i32::MAX, max_q: i32::MIN };
    let yq = |k: i32| 4 * (y0 + k * vh);

    // Tops and bottoms, layer by layer.
    for (face, up) in [(Face::PosY, 1), (Face::NegY, -1)] {
        for k in 0..layers {
            let mut mask = vec![0u32; (CELLS * CELLS) as usize];
            let mut any = false;
            for j in 0..CELLS {
                for i in 0..CELLS {
                    if solid(i, k, j) && !solid(i, k + up, j) && !(up < 0 && k == 0) {
                        let b = vox[vi(i, k, j)];
                        let c = &cols[gi(i, j)];
                        if up > 0 && c.water_q.is_some_and(|w| w >= 4 * (y0 + (k + 1) * vh)) {
                            continue; // under the water surface
                        }
                        mask[(j * CELLS + i) as usize] = tex(reg, b, face) + 1;
                        any = true;
                    }
                }
            }
            if !any {
                continue;
            }
            let y = if up > 0 { yq(k + 1) } else { yq(k) };
            greedy(CELLS, CELLS, &mut mask, tile, &|v| v / tile, &mut |u, v, du, dv, t| e.quad(face, u, u + du, y, y, v, v + dv, t - 1));
        }
    }
    // Water surfaces: flat, at their exact height (level with real water).
    {
        let mut by_height: HashMap<i32, Vec<u32>> = HashMap::new();
        for j in 0..CELLS {
            for i in 0..CELLS {
                if let Some(w) = cols[gi(i, j)].water_q {
                    by_height.entry(w).or_insert_with(|| vec![0; (CELLS * CELLS) as usize])[(j * CELLS + i) as usize] = tex(reg, water, Face::PosY) + 1;
                }
            }
        }
        let mut heights: Vec<i32> = by_height.keys().copied().collect();
        heights.sort_unstable();
        for w in heights {
            let mut mask = by_height.remove(&w).unwrap();
            greedy(CELLS, CELLS, &mut mask, tile, &|v| v / tile, &mut |u, v, du, dv, t| e.quad(Face::PosY, u, u + du, w, w, v, v + dv, t - 1));
        }
    }

    // Sides: a voxel face is drawn where the voxel beside is air (and not under water there).
    for face in [Face::PosX, Face::NegX, Face::PosZ, Face::NegZ] {
        let (dx, _, dz) = face.normal();
        for a in 0..CELLS {
            // `a` walks across the planes; the mask runs along them (u) and up (v = layer).
            let mut mask = vec![0u32; (CELLS * layers) as usize];
            let mut any = false;
            let edge_cell = if dx > 0 || dz > 0 { a + 1 } else { a };
            let edge_block = if dx != 0 { ox + edge_cell * cell } else { oz + edge_cell * cell };
            let on_line = masked && edge_block.rem_euclid(CHUNK_SIZE) == 0;
            for u in 0..CELLS {
                let (i, j) = if dx != 0 { (a, u) } else { (u, a) };
                let (ni, nj) = (i + dx, j + dz);
                let own = &cols[gi(i, j)];
                if own.water_q.is_some() {
                    continue; // the water surface stands for the whole column
                }
                let across = &cols[gi(ni, nj)];
                let real = if on_line { real_min(face, i, j) } else { i32::MAX };
                for k in 0..layers {
                    if !solid(i, k, j) {
                        continue;
                    }
                    let top = y0 + (k + 1) * vh;
                    let open = !solid(ni, k, nj) || top > real;
                    // Under LOD water a face can't be seen; under real water (see-through, across
                    // a chunk line) it closes the slit down to the real bed.
                    let drowned = across.water_q.is_some_and(|w| 4 * top <= w) && top <= real;
                    if open && !drowned {
                        mask[(k * CELLS + u) as usize] = tex(reg, vox[vi(i, k, j)], face) + 1;
                        any = true;
                    }
                }
            }
            if !any {
                continue;
            }
            greedy(CELLS, layers, &mut mask, tile, &layer_tile, &mut |u, v, du, dv, t| {
                let (ya, yb) = (yq(v), yq(v + dv));
                let t = t - 1;
                match face {
                    Face::PosX => e.quad(face, a + 1, a + 1, ya, yb, u, u + du, t),
                    Face::NegX => e.quad(face, a, a, ya, yb, u, u + du, t),
                    Face::PosZ => e.quad(face, u, u + du, ya, yb, a + 1, a + 1, t),
                    _ => e.quad(face, u, u + du, ya, yb, a, a, t),
                }
            });
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
    /// Edited column tops, shared with the workers. Replaced, never changed in place, so a
    /// worker only holds the lock long enough to take a reference.
    edits: Arc<Mutex<Arc<SurfaceEdits>>>,
    /// Built (or building) sections that edits have changed since: built again, and drawn
    /// as they were until then.
    redo: HashSet<SectionKey>,
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
            edits: Arc::new(Mutex::new(Arc::new(SurfaceEdits::default()))),
            redo: HashSet::new(),
            stats: LodStats::default(),
        }
    }

    /// Starts the workers once the world (seed and blocks) is known.
    pub fn start(&mut self, gen: Arc<WorldGen>, reg: Arc<BlockRegistry>) {
        let edits = self.edits.clone();
        self.pool = Some(JobPool::new(self.workers, move |key: SectionKey| {
            let e = edits.lock().map(|e| e.clone()).unwrap_or_default();
            build_section_edited(&gen, &reg, &e, key)
        }));
        self.clear();
    }

    /// The edited column tops so far.
    pub fn edits(&self) -> Arc<SurfaceEdits> {
        self.edits.lock().map(|e| e.clone()).unwrap_or_default()
    }

    /// Takes in edited column tops (`ServerMsg::Surface`); the sections they change are
    /// built again.
    pub fn set_surface(&mut self, tops: &[ColumnTop]) {
        let mut next = (*self.edits()).clone();
        let changed: Vec<ColumnTop> = tops.iter().copied().filter(|&t| next.set(t)).collect();
        if changed.is_empty() {
            return;
        }
        if let Ok(mut e) = self.edits.lock() {
            *e = Arc::new(next);
        }
        for t in changed {
            for level in 0..=MAX_LEVEL {
                // The section holding the column, and those holding it in their border.
                let c = 1 << level;
                for (dx, dz) in [(0, 0), (-c, 0), (c, 0), (0, -c), (0, c)] {
                    let k = SectionKey::containing(level, t.x + dx, t.z + dz);
                    if self.built.contains_key(&k) || self.pending.contains(&k) {
                        self.redo.insert(k);
                    }
                }
            }
        }
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
        self.redo.clear();
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
            // A rebuild keeps drawing the old mesh until the renderer has the new one.
            let uploaded = self.built.get(&key).is_some_and(|i| i.uploaded);
            self.built.insert(key, SectionInfo { quads: mesh.quads() as u32, min_y: mesh.min_y, max_y: mesh.max_y, uploaded });
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
                .filter(|k| (!self.built.contains_key(k) || self.redo.contains(k)) && !self.pending.contains(k))
                .map(|&k| (k.distance_to(camera[0], camera[2]), k))
                .collect();
            missing.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            for (d, k) in missing.into_iter().take(max_in_flight - pool.in_flight()) {
                self.pending.insert(k);
                self.redo.remove(&k);
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
            self.redo.remove(&k);
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
        self.pending.is_empty() && self.wanted.iter().all(|k| self.built.contains_key(k) && !self.redo.contains(k))
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

    #[test]
    fn textures_past_255_keep_their_high_bits() {
        let mut e = Emit { verts: Vec::new(), min_q: i32::MAX, max_q: i32::MIN };
        e.quad(Face::PosY, 0, 1, 4, 4, 0, 1, 1733);
        assert!(e.verts.iter().all(|v| v.texture() == 1733));
    }

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

    #[test]
    fn edits_show_in_lod_sections_far_and_near() {
        let (reg, g) = world();
        let stone = reg.id("stone").unwrap();
        // A 40-block tower somewhere in a 8-block-cell section, and a deep pit in a 1-block one.
        let key = SectionKey::new(3, 2, -1);
        let (ox, oz) = key.origin();
        let (tx, tz) = (ox + 37, oz + 101);
        let base = g.lod_surface(tx, tz, 1).0;
        let mut edits = SurfaceEdits::default();
        assert!(edits.set(ColumnTop { x: tx, z: tz, top_q: base + 160, block: stone }));
        assert!(!edits.set(ColumnTop { x: tx, z: tz, top_q: base + 160, block: stone }), "no change");
        // The height of the top face over cell (i, j).
        let top_at = |m: &LodMesh, i: u32, j: u32| -> i32 {
            m.vertices
                .chunks(4)
                .filter(|q| q[0].face() == Face::PosY as u32)
                .find(|q| {
                    let (x0, x1) = (q.iter().map(|v| v.x()).min().unwrap(), q.iter().map(|v| v.x()).max().unwrap());
                    let (z0, z1) = (q.iter().map(|v| v.z()).min().unwrap(), q.iter().map(|v| v.z()).max().unwrap());
                    (x0..x1).contains(&i) && (z0..z1).contains(&j)
                })
                .map(|q| q[0].y_q())
                .expect("a top face over the cell")
        };
        let (i, j) = (((tx - ox) / key.cell()) as u32, ((tz - oz) / key.cell()) as u32);
        let plain = build_section(&g, &reg, key);
        let edited = build_section_edited(&g, &reg, &edits, key);
        assert!(top_at(&edited, i, j) >= base + 160 - 16, "{} vs {}", top_at(&edited, i, j), base + 160);
        assert!(top_at(&plain, i, j) < base + 100);
        check_winding(&edited);

        let near = SectionKey::containing(0, tx + 40, tz);
        let (px, pz) = (tx + 40, tz);
        let ground = g.lod_surface(px, pz, 1).0;
        edits.set(ColumnTop { x: px, z: pz, top_q: ground - 40, block: stone });
        let m = build_section_edited(&g, &reg, &edits, near);
        let (no, _) = near.origin();
        let (pi, pj) = ((px - no) as u32, (pz - near.origin().1) as u32);
        assert_eq!(top_at(&m, pi, pj), ground - 40);
        assert_eq!(top_at(&build_section(&g, &reg, near), pi, pj), ground);
    }

    #[test]
    fn edited_sections_are_built_again_and_drawn_meanwhile() {
        let (reg, g) = world();
        let mut m = LodManager::new(2);
        m.start(Arc::new(g), Arc::new(reg));
        m.set_settings(medium(2000.0));
        let cam = [0.0, 40.0, 0.0];
        let settle = |m: &mut LodManager| {
            for _ in 0..4000 {
                m.update(cam);
                m.take_built(usize::MAX);
                if m.is_idle() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            panic!("never settled");
        };
        settle(&mut m);
        m.update(cam);
        let before = m.stats.built;
        let drawn = m.render_set().to_vec();
        let at = |k: SectionKey| (k.origin().0 + 5, k.origin().1 + 5);
        let (x, z) = at(*drawn.iter().find(|k| k.level == 2).expect("a level-2 section"));
        m.set_surface(&[ColumnTop { x, z, top_q: 4 * 150, block: 1 }]);
        assert_eq!(m.edits().get(x, z).map(|t| t.top_q), Some(600));
        assert!(!m.is_idle(), "sections to rebuild");
        // Nothing stops being drawn while they are rebuilt.
        m.update(cam);
        assert_eq!(m.render_set().len(), drawn.len());
        settle(&mut m);
        m.update(cam);
        assert!(m.stats.built > before, "rebuilt");
        assert_eq!(m.render_set().len(), drawn.len());
        let k = SectionKey::containing(2, x, z);
        assert!(m.info(k).unwrap().max_y >= 149.0);
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
            // Every cell's top is covered by a top quad (the ground's, and a tree crown's or
            // a canopy's over it).
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
            assert!(cover.iter().all(|&c| c >= 1), "{key:?}");
            assert!(m.min_y <= m.max_y);
        }
    }

    #[test]
    fn voxels_are_cubes_near_and_at_most_eight_blocks_tall_far() {
        let (reg, gen) = world();
        for (level, vh) in [(1u8, 2), (2, 4), (3, 8), (5, 8)] {
            let key = SectionKey::new(level, 3, -2);
            let m = build_section(&gen, &reg, key);
            // Every land vertex sits on a layer boundary (water surfaces are exact).
            let water = tex(&reg, gen.water_id(), Face::PosY);
            for v in m.vertices.iter().filter(|v| v.texture() != water) {
                assert_eq!(v.y_q().rem_euclid(4 * vh), 0, "level {level}: y {} not on a {vh}-block layer", v.y_q());
            }
            assert_eq!(voxel_height(key.cell()), vh);
        }
    }

    #[test]
    fn side_faces_close_every_step() {
        // Walking along any row, every change of height has a side face covering it.
        let (reg, gen) = world();
        let key = SectionKey::new(1, 3, -2);
        let m = build_section(&gen, &reg, key);
        let surf = |i: i32, j: i32| cell_surface(&gen, key, i, j).0;
        // (Where a tree stands beside a step its leaves may cover the step instead.)
        let (ox, oz) = key.origin();
        let trees = gen.lod_trees(ox - 8, oz - 8, ox + key.width() + 8, oz + key.width() + 8, key.cell());
        let near_tree = |i: i32, j: i32| {
            let (x, z) = (ox + i * key.cell(), oz + j * key.cell());
            trees.iter().any(|(f, ..)| (f.x - x).abs() <= f.radius + 4 && (f.z - z).abs() <= f.radius + 4)
        };
        for j in 0..CELLS {
            for i in 0..CELLS - 1 {
                let (a, b) = (surf(i, j), surf(i + 1, j));
                if a == b || near_tree(i, j) || near_tree(i + 1, j) {
                    continue;
                }
                let (face, x) = if a > b { (Face::PosX, i + 1) } else { (Face::NegX, i + 1) };
                let (lo, hi) = (a.min(b), a.max(b));
                // The side faces there, together (bands of surface, subsurface and rock).
                let mut spans: Vec<(i32, i32)> = m
                    .vertices
                    .chunks(4)
                    .filter(|q| {
                        q[0].face() == face.index() as u32
                            && q[0].x() as i32 == x
                            && q.iter().map(|v| v.z() as i32).min().unwrap() <= j
                            && q.iter().map(|v| v.z() as i32).max().unwrap() > j
                    })
                    .map(|q| (q.iter().map(|v| v.y_q()).min().unwrap(), q.iter().map(|v| v.y_q()).max().unwrap()))
                    .collect();
                spans.sort_unstable();
                let mut reach: Option<(i32, i32)> = None;
                for (a, b) in spans {
                    reach = match reach {
                        None => Some((a, b)),
                        Some((l, h)) if a <= h => Some((l, h.max(b))),
                        r => r,
                    };
                }
                let ok = reach.is_some_and(|(l, h)| l <= lo && h >= hi);
                assert!(ok, "no side between cells {i} and {} in row {j}: {reach:?} vs {lo}..{hi}", i + 1);
            }
        }
    }

    #[test]
    fn quads_near_real_chunks_stay_inside_one_chunk() {
        let (reg, gen) = world();
        for level in 0..=MASKED_MAX_LEVEL {
            // Over mountains too, where sides are tall and cross chunks vertically.
            for key in [SectionKey::new(level, 2, -1), SectionKey::new(level, -7, 12), SectionKey::new(level, 40, 3)] {
                let m = build_section(&gen, &reg, key);
                let (ox, oz) = key.origin();
                let cell = key.cell();
                for q in m.vertices.chunks(4) {
                    let chunks: HashSet<(i32, i32, i32)> = q
                        .iter()
                        .map(|v| {
                            let (ix, iz) = v.inward();
                            // Half a cell into the quad, and a little into its voxel
                            // vertically, as the shader does.
                            let x = ox as f32 + (v.x() as f32 + ix as f32 * 0.5) * cell as f32;
                            let z = oz as f32 + (v.z() as f32 + iz as f32 * 0.5) * cell as f32;
                            let face = v.face();
                            let down = face == Face::PosY.index() as u32 || (face != Face::NegY.index() as u32 && v.is_top());
                            let y = v.y_q() as f32 / 4.0 + if down { -0.25 } else { 0.25 };
                            let c = |p: f32| (p / CHUNK_SIZE as f32).floor() as i32;
                            (c(x), c(y), c(z))
                        })
                        .collect();
                    assert_eq!(chunks.len(), 1, "level {level} {key:?}: quad spans chunks {chunks:?}");
                }
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
                        // The faces on this line over this cell, together, cover from below
                        // the real ground up to the LOD top (bands of grass, dirt and rock).
                        let mut spans: Vec<(i32, i32)> = m
                            .vertices
                            .chunks(4)
                            .filter(|q| {
                                q[0].face() == face.index() as u32
                                    && q[0].x() as i32 == x
                                    && q.iter().map(|v| v.z() as i32).min().unwrap() <= j
                                    && q.iter().map(|v| v.z() as i32).max().unwrap() > j
                            })
                            .map(|q| (q.iter().map(|v| v.y_q()).min().unwrap(), q.iter().map(|v| v.y_q()).max().unwrap()))
                            .collect();
                        spans.sort_unstable();
                        let mut reach = None::<(i32, i32)>;
                        for (a, b) in spans {
                            reach = match reach {
                                None => Some((a, b)),
                                Some((lo, hi)) if a <= hi => Some((lo, hi.max(b))),
                                Some(r) => Some(r), // a gap: stop extending
                            };
                        }
                        let ok = reach.is_some_and(|(lo, hi)| lo < real && hi >= own);
                        assert!(ok, "{key:?}: no face down to the real ground at line x={x}, cell {j}: {reach:?} vs {real}..{own}");
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
