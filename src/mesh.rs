//! Chunk meshing: a padded snapshot goes in, packed quads come out.
//!
//! * Meshing works on a 34^3 copy (chunk + 1-voxel border), so a worker needs no
//!   locks and no access to the live world.
//! * Vertices are 8 bytes. Positions are in quarter-block units, which is what lets
//!   slabs and layers share the same vertex format as full blocks.
//! * Quads have NO index buffer: indices follow the fixed pattern
//!   `[0,1,2, 0,2,3] + 4*quad` (see `quad_indices`). The AO diagonal flip is done by
//!   rotating the vertex order instead of changing indices.
//! * Not yet implemented: greedy merging, real light propagation (sky/block light
//!   are placeholders), AO sampling across chunk edges/corners from unloaded neighbours.
//! * Planned for the next version: a binary greedy mesher (bitmask face detection plus
//!   merging of coplanar same-block faces into large quads). See README.
//!   (Implemented in `greedy.rs`: see `Mesher` and `mesh_chunk_greedy`.)

use crate::block::{BlockState, UNKNOWN};
use crate::coords::{ChunkPos, Face, LocalPos, CHUNK_SIZE, CHUNK_VOLUME};
use crate::registry::{BlockRegistry, RenderLayer};
use crate::world::ChunkStore;

/// How many cells of the neighbouring chunks a snapshot holds on each side: one for faces and
/// ambient occlusion, a second so water can tell the shore up to two blocks away.
const BORDER: i32 = 2;
const PADDED: usize = (CHUNK_SIZE + 2 * BORDER) as usize;

/// Block data for one chunk plus a two-voxel border. Border cells from unloaded
/// chunks are `UNKNOWN`, which hides the faces touching them.
pub struct PaddedChunk {
    blocks: Vec<BlockState>,
}

impl PaddedChunk {
    pub fn filled(state: BlockState) -> Self {
        PaddedChunk { blocks: vec![state; PADDED * PADDED * PADDED] }
    }

    /// Coordinates are chunk-relative, -BORDER..CHUNK_SIZE + BORDER on each axis.
    #[inline]
    fn idx(x: i32, y: i32, z: i32) -> usize {
        let r = -BORDER..CHUNK_SIZE + BORDER;
        debug_assert!(r.contains(&x) && r.contains(&y) && r.contains(&z));
        (((y + BORDER) as usize * PADDED) + (z + BORDER) as usize) * PADDED + (x + BORDER) as usize
    }

    #[inline]
    pub fn at(&self, x: i32, y: i32, z: i32) -> BlockState {
        self.blocks[Self::idx(x, y, z)]
    }

    pub fn set(&mut self, x: i32, y: i32, z: i32, s: BlockState) {
        let i = Self::idx(x, y, z);
        self.blocks[i] = s;
    }

    /// Snapshot of `pos` and the two-voxel shell around it. Runs on the caller's thread
    /// (the render thread, for the client), so it avoids per-voxel palette lookups where
    /// it can: uniform chunks are filled, and the centre chunk is decoded in one pass.
    pub fn capture(store: &ChunkStore, pos: ChunkPos) -> PaddedChunk {
        let mut out = PaddedChunk::filled(UNKNOWN);
        let n = CHUNK_SIZE as usize;
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let Some(chunk) = store.get(pos.offset(dx, dy, dz)) else { continue };
                    let (sx, nx, ox) = axis_span(dx);
                    let (sy, ny, oy) = axis_span(dy);
                    let (sz, nz, oz) = axis_span(dz);
                    if let Some(s) = chunk.uniform_state() {
                        for iy in 0..ny {
                            for iz in 0..nz {
                                let row = Self::idx(ox, oy + iy, oz + iz);
                                out.blocks[row..row + nx as usize].fill(s);
                            }
                        }
                        continue;
                    }
                    if (dx, dy, dz) == (0, 0, 0) {
                        // Both layouts are y-major with x contiguous: copy whole rows.
                        let mut cells = vec![UNKNOWN; CHUNK_VOLUME];
                        chunk.decode_into(&mut cells);
                        for iy in 0..n {
                            for iz in 0..n {
                                let src = (iy * n + iz) * n;
                                let dst = Self::idx(0, iy as i32, iz as i32);
                                out.blocks[dst..dst + n].copy_from_slice(&cells[src..src + n]);
                            }
                        }
                        continue;
                    }
                    for iy in 0..ny {
                        for iz in 0..nz {
                            for ix in 0..nx {
                                let l = LocalPos {
                                    x: (sx + ix) as u8,
                                    y: (sy + iy) as u8,
                                    z: (sz + iz) as u8,
                                };
                                out.set(ox + ix, oy + iy, oz + iz, chunk.get(l.index()));
                            }
                        }
                    }
                }
            }
        }
        out
    }
}

/// True when the chunk at `pos` cannot produce a single quad, judged without looking at
/// individual voxels: it is uniform (all air, or one block that hides its own faces) and
/// every loaded face neighbour is uniform and hides the faces at the seam. In a cubic-chunk
/// world that is most chunks (open sky, deep stone), so the client uses this to skip the
/// snapshot and the mesh job entirely. Gives exactly the answer `mesh_chunk` would.
pub fn has_no_faces(reg: &BlockRegistry, store: &ChunkStore, pos: ChunkPos) -> bool {
    let Some(chunk) = store.get(pos) else { return true };
    let Some(state) = chunk.uniform_state() else { return false };
    if state.is_air() {
        return true;
    }
    for face in Face::ALL {
        // Interior faces between identical cells must be hidden too (not true for slabs).
        if !face_hidden(reg, state, face, state) {
            return false;
        }
        let (dx, dy, dz) = face.normal();
        match store.get(pos.offset(dx, dy, dz)) {
            None => {} // unloaded: the mesher hides faces at the seam
            Some(nb) => match nb.uniform_state() {
                Some(ns) if face_hidden(reg, state, face, ns) => {}
                _ => return false,
            },
        }
    }
    true
}

/// For a neighbour offset `d` in -1..=1: (source start in that chunk, length,
/// destination start in chunk-relative coordinates).
fn axis_span(d: i32) -> (i32, i32, i32) {
    match d {
        -1 => (CHUNK_SIZE - BORDER, BORDER, -BORDER),
        0 => (0, CHUNK_SIZE, 0),
        _ => (0, BORDER, CHUNK_SIZE),
    }
}

/// 8 bytes:
///   a: x(8) | y(8)<<8 | z(8)<<16 | face(3)<<24 | ao(2)<<27 | liquid_top(1)<<29
///   b: texture(16) | sky(4)<<16 | block(4)<<20 | grow_u(1)<<24 | grow_v(1)<<25 | merged(1)<<26
///      | tex_top(2)<<27
/// grow_u/grow_v/merged are set on greedy-merged quads: which side of the quad this corner is
/// on along the face's two tangent axes, and the flag the shader uses to grow merged quads
/// slightly. tex_top (side faces) lifts the texture so its top row starts at the quad's top
/// when that top is not on a whole block (a half-slab shows the grass fringe, not dirt).
/// x/y/z are quarter-block units within the chunk (0..=128 fits in 8 bits).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PackedVertex {
    pub a: u32,
    pub b: u32,
}

impl PackedVertex {
    #[allow(clippy::too_many_arguments)]
    pub fn pack(x: u32, y: u32, z: u32, face: Face, ao: u8, tex: u16, sky: u8, block: u8) -> Self {
        debug_assert!(x < 256 && y < 256 && z < 256);
        let a = x | (y << 8) | (z << 16) | ((face as u32) << 24) | (((ao & 3) as u32) << 27);
        let b = (tex as u32) | (((sky & 15) as u32) << 16) | (((block & 15) as u32) << 20);
        PackedVertex { a, b }
    }

    /// Marks this vertex as part of a merged quad (see `greedy.rs`). `u_max`/`v_max` say
    /// whether the corner is on the high side of the quad along the face's first and second
    /// tangent axes ((axis + 1) % 3 and (axis + 2) % 3).
    pub fn with_grow(mut self, u_max: bool, v_max: bool) -> PackedVertex {
        self.b |= ((u_max as u32) << 24) | ((v_max as u32) << 25) | (1 << 26);
        self
    }

    /// Marks a corner at the top of a liquid's surface block: the shader lowers it by an
    /// eighth of a block, so water sits a little below the ground around it.
    pub fn with_liquid_top(mut self) -> PackedVertex {
        self.a |= 1 << 29;
        self
    }

    pub fn liquid_top(self) -> bool {
        (self.a >> 29) & 1 == 1
    }

    pub fn tex_top(self) -> u32 {
        (self.b >> 27) & 3
    }

    pub fn grows(self) -> bool {
        (self.b >> 26) & 1 == 1
    }

    pub fn grow_u(self) -> bool {
        (self.b >> 24) & 1 == 1
    }

    pub fn grow_v(self) -> bool {
        (self.b >> 25) & 1 == 1
    }

    pub fn x(self) -> u32 {
        self.a & 255
    }
    pub fn y(self) -> u32 {
        (self.a >> 8) & 255
    }
    pub fn z(self) -> u32 {
        (self.a >> 16) & 255
    }
    pub fn face_index(self) -> u32 {
        (self.a >> 24) & 7
    }
    pub fn ao(self) -> u32 {
        (self.a >> 27) & 3
    }
    pub fn tex(self) -> u32 {
        self.b & 0xFFFF
    }
    pub fn sky(self) -> u32 {
        (self.b >> 16) & 15
    }
    pub fn block_light(self) -> u32 {
        (self.b >> 20) & 15
    }
}

#[derive(Default, Clone, Debug)]
pub struct MeshLayer {
    /// 4 vertices per quad, no indices (see `quad_indices`).
    pub vertices: Vec<PackedVertex>,
}

impl MeshLayer {
    pub fn quad_count(&self) -> usize {
        self.vertices.len() / 4
    }
}

#[derive(Default, Clone, Debug)]
pub struct ChunkMesh {
    pub opaque: MeshLayer,
    pub cutout: MeshLayer,
    pub translucent: MeshLayer,
}

impl ChunkMesh {
    pub fn is_empty(&self) -> bool {
        self.opaque.vertices.is_empty()
            && self.cutout.vertices.is_empty()
            && self.translucent.vertices.is_empty()
    }

    pub fn quad_count(&self) -> usize {
        self.opaque.quad_count() + self.cutout.quad_count() + self.translucent.quad_count()
    }

    pub fn vertex_bytes(&self) -> usize {
        (self.opaque.vertices.len() + self.cutout.vertices.len() + self.translucent.vertices.len())
            * std::mem::size_of::<PackedVertex>()
    }

    fn layer_mut(&mut self, l: RenderLayer) -> &mut MeshLayer {
        match l {
            RenderLayer::Opaque => &mut self.opaque,
            RenderLayer::Cutout => &mut self.cutout,
            RenderLayer::Translucent => &mut self.translucent,
        }
    }
}

/// One shared static index buffer serves every chunk: build it once at the largest
/// quad count you expect and reuse it.
pub fn quad_indices(quads: usize) -> Vec<u32> {
    let mut v = Vec::with_capacity(quads * 6);
    for q in 0..quads as u32 {
        let b = q * 4;
        v.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    v
}

/// Unit-cube corners per face, counter-clockwise seen from outside. Index = `Face::index()`.
pub(crate) const FACE_CORNERS: [[[i32; 3]; 4]; 6] = [
    [[1, 0, 1], [1, 0, 0], [1, 1, 0], [1, 1, 1]], // +X
    [[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]], // -X
    [[0, 1, 1], [1, 1, 1], [1, 1, 0], [0, 1, 0]], // +Y
    [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]], // -Y
    [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]], // +Z
    [[1, 0, 0], [0, 0, 0], [0, 1, 0], [1, 1, 0]], // -Z
];

/// Is the face of `state` pointing at `nb` invisible?
///
/// Partial blocks are handled geometrically: a side face is hidden only if the
/// neighbour covers its whole height range; a top/bottom face only if it touches the
/// block boundary and the neighbour fills the facing side of its cell.
fn face_hidden(reg: &BlockRegistry, state: BlockState, face: Face, nb: BlockState) -> bool {
    if nb == UNKNOWN {
        return true; // chunk not loaded yet: draw nothing at the seam
    }
    if nb.is_air() {
        return false;
    }
    let a = reg.get(state);
    let b = reg.get(nb);
    // Opaque neighbours always occlude. Transparent ones only hide identical blocks
    // (water next to water, leaves next to leaves).
    let material_ok = b.opaque || (!a.opaque && nb.id() == state.id());
    if !material_ok {
        return false;
    }
    let (amin, amax) = state.shape().y_range_q();
    let (bmin, bmax) = nb.shape().y_range_q();
    match face {
        Face::PosY => amax == 4 && bmin == 0,
        Face::NegY => amin == 0 && bmax == 4,
        _ => bmin <= amin && bmax >= amax,
    }
}

fn occludes_ao(reg: &BlockRegistry, s: BlockState) -> bool {
    if s == UNKNOWN || s.is_air() {
        return false;
    }
    let d = reg.get(s);
    d.solid && d.opaque
}

/// Per-corner ambient occlusion, 0 (dark) ..= 3 (bright).
/// Approximation: samples the cell in front of the face, even for partial-height tops.
pub(crate) fn corner_ao(reg: &BlockRegistry, pc: &PaddedChunk, x: i32, y: i32, z: i32, face: Face) -> [u8; 4] {
    let (nx, ny, nz) = face.normal();
    let axis = if nx != 0 {
        0
    } else if ny != 0 {
        1
    } else {
        2
    };
    let t1 = (axis + 1) % 3;
    let t2 = (axis + 2) % 3;
    let base = [x + nx, y + ny, z + nz];
    let corners = &FACE_CORNERS[face.index()];
    let mut out = [3u8; 4];
    for (i, c) in corners.iter().enumerate() {
        let s1 = if c[t1] == 1 { 1 } else { -1 };
        let s2 = if c[t2] == 1 { 1 } else { -1 };
        let mut p1 = base;
        p1[t1] += s1;
        let mut p2 = base;
        p2[t2] += s2;
        let mut pcn = base;
        pcn[t1] += s1;
        pcn[t2] += s2;
        let o1 = occludes_ao(reg, pc.at(p1[0], p1[1], p1[2]));
        let o2 = occludes_ao(reg, pc.at(p2[0], p2[1], p2[2]));
        let oc = occludes_ao(reg, pc.at(pcn[0], pcn[1], pcn[2]));
        out[i] = if o1 && o2 { 0 } else { 3 - (o1 as u8 + o2 as u8 + oc as u8) };
    }
    out
}

/// How far each corner of a liquid's surface is from the shore: the nearest block standing in
/// the liquid at the surface's own level, within two blocks. 0 = the corner touches one,
/// 1 = one block away, 2 = one block away diagonally, 3 = open water (the same as "no
/// occlusion", so the code travels in the AO bits, which water doesn't otherwise use). The
/// shader turns it into foam and calmer waves along the shore. A corner's code depends only
/// on the 4 x 4 cells around it, so the chunks on either side of a seam agree.
pub(crate) fn shore_corners(reg: &BlockRegistry, pc: &PaddedChunk, x: i32, y: i32, z: i32) -> [u8; 4] {
    let blocks = |cx: i32, cz: i32| {
        let s = pc.at(cx, y, cz);
        s != UNKNOWN && !s.is_air() && reg.get(s).solid
    };
    let corners = &FACE_CORNERS[Face::PosY.index()];
    let mut out = [3u8; 4];
    for (k, c) in corners.iter().enumerate() {
        // The corner point; the cells around it start 2 before it and end 1 after it.
        let (px, pz) = (x + c[0], z + c[2]);
        for i in -2..=1 {
            for j in -2..=1 {
                if blocks(px + i, pz + j) {
                    let code = match (i == -2 || i == 1, j == -2 || j == 1) {
                        (false, false) => 0,
                        (true, true) => 2,
                        _ => 1,
                    };
                    out[k] = out[k].min(code);
                }
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_quad(
    layer: &mut MeshLayer,
    face: Face,
    ox: i32,
    oy: i32,
    oz: i32,
    ymin: i32,
    ymax: i32,
    ao: [u8; 4],
    tex: u16,
    sky: u8,
    block: u8,
) {
    let corners = &FACE_CORNERS[face.index()];
    let mut verts = [PackedVertex::default(); 4];
    for (i, c) in corners.iter().enumerate() {
        let px = ox + c[0] * 4;
        let py = oy + if c[1] == 1 { ymax } else { ymin };
        let pz = oz + c[2] * 4;
        verts[i] = PackedVertex::pack(px as u32, py as u32, pz as u32, face, ao[i], tex, sky, block);
    }
    anchor_texture_top(face, &mut verts);
    // Split along the brighter diagonal; rotate the order to flip it.
    let order: [usize; 4] = if ao[0] + ao[2] >= ao[1] + ao[3] { [0, 1, 2, 3] } else { [1, 2, 3, 0] };
    for &i in &order {
        layer.vertices.push(verts[i]);
    }
}

/// For side faces, sets each corner's `tex_top`: how far (in quarter blocks) the quad's top
/// is below the next whole block, so the texture's top row lines up with the quad's top.
pub fn anchor_texture_top(face: Face, verts: &mut [PackedVertex; 4]) {
    if matches!(face, Face::PosY | Face::NegY) {
        return;
    }
    let top = verts.iter().map(|v| v.y()).max().unwrap_or(0);
    let lift = (4 - top % 4) % 4;
    for v in verts.iter_mut() {
        v.b = (v.b & !(3 << 27)) | (lift << 27);
    }
}

/// Which mesher to use. Both produce the same visible faces; greedy merges coplanar faces
/// of full opaque cubes into larger quads.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mesher {
    Simple,
    Greedy,
}

pub fn mesh_chunk_with(kind: Mesher, reg: &BlockRegistry, pc: &PaddedChunk) -> ChunkMesh {
    match kind {
        Mesher::Simple => mesh_chunk(reg, pc),
        Mesher::Greedy => mesh_chunk_greedy(reg, pc),
    }
}

/// One quad per visible face.
pub fn mesh_chunk(reg: &BlockRegistry, pc: &PaddedChunk) -> ChunkMesh {
    mesh_chunk_impl(reg, pc, false)
}

/// Hybrid: full opaque cubes go through the bit-mask greedy mesher, everything else (slabs,
/// water, leaves, ...) through the per-face path.
pub fn mesh_chunk_greedy(reg: &BlockRegistry, pc: &PaddedChunk) -> ChunkMesh {
    let mut mesh = mesh_chunk_impl(reg, pc, true);
    crate::greedy::mesh_eligible(reg, pc, &mut mesh);
    mesh
}

fn mesh_chunk_impl(reg: &BlockRegistry, pc: &PaddedChunk, skip_greedy: bool) -> ChunkMesh {
    let mut mesh = ChunkMesh::default();
    for y in 0..CHUNK_SIZE {
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let state = pc.at(x, y, z);
                if state.is_air() {
                    continue;
                }
                if skip_greedy && crate::greedy::is_eligible(reg, state) {
                    continue;
                }
                let def = reg.get(state);
                let (ymin, ymax) = state.shape().y_range_q();
                // A liquid's surface: the top of its block is lowered a little (see shader).
                let surface = def.render == RenderLayer::Translucent && !def.solid && pc.at(x, y + 1, z) != state;
                for face in Face::ALL {
                    let (nx, ny, nz) = face.normal();
                    let nb = pc.at(x + nx, y + ny, z + nz);
                    if face_hidden(reg, state, face, nb) {
                        continue;
                    }
                    let ao = if surface && face == Face::PosY {
                        shore_corners(reg, pc, x, y, z)
                    } else if def.render == RenderLayer::Translucent {
                        [3; 4]
                    } else {
                        corner_ao(reg, pc, x, y, z, face)
                    };
                    // Sky light is a placeholder until the light propagation pass exists.
                    emit_quad(
                        mesh.layer_mut(def.render),
                        face,
                        x * 4,
                        y * 4,
                        z * 4,
                        ymin,
                        ymax,
                        ao,
                        def.tex[face.index()],
                        15,
                        def.light_emission,
                    );
                    if surface {
                        let verts = &mut mesh.layer_mut(def.render).vertices;
                        let n = verts.len();
                        for v in &mut verts[n - 4..] {
                            if v.y() as i32 == y * 4 + ymax {
                                *v = v.with_liquid_top();
                            }
                        }
                    }
                }
            }
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{Shape, AIR};
    use crate::chunk::Chunk;

    fn reg() -> BlockRegistry {
        BlockRegistry::builtin()
    }

    #[test]
    fn capture_matches_a_per_voxel_read_of_the_store() {
        let r = reg();
        let ids = [r.state("stone").unwrap(), r.state("dirt").unwrap(), r.state("water").unwrap(), AIR];
        let mut store = ChunkStore::new();
        // A mix of paletted, uniform and missing chunks around the centre.
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let k = (dx + 1) + 3 * (dz + 1) + 9 * (dy + 1);
                    if k % 5 == 3 {
                        continue; // unloaded
                    }
                    let mut c = Chunk::uniform(ids[(k % 4) as usize]);
                    if k % 3 != 1 {
                        for i in (0..CHUNK_VOLUME).step_by(7 + k as usize) {
                            c.set(i, ids[(i / 3 + k as usize) % 4]);
                        }
                    }
                    store.insert(ChunkPos::new(dx, dy, dz), c);
                }
            }
        }
        for centre in [ChunkPos::new(0, 0, 0), ChunkPos::new(1, 0, 0), ChunkPos::new(0, -1, 1)] {
            let pc = PaddedChunk::capture(&store, centre);
            let o = centre.origin();
            for y in -1..=CHUNK_SIZE {
                for z in -1..=CHUNK_SIZE {
                    for x in -1..=CHUNK_SIZE {
                        let want = store.block(o.offset(x, y, z)).unwrap_or(UNKNOWN);
                        assert_eq!(pc.at(x, y, z), want, "centre {centre:?} at {x},{y},{z}");
                    }
                }
            }
        }
    }

    #[test]
    fn has_no_faces_agrees_with_the_mesher() {
        let r = reg();
        let stone = r.state("stone").unwrap();
        let water = r.state("water").unwrap();
        let slab = stone.with_shape(Shape::SlabBottom);
        let centre = ChunkPos::new(0, 0, 0);
        let mut mixed = Chunk::uniform(stone);
        mixed.set(0, AIR);
        let neighbours = [None, Some(Chunk::air()), Some(Chunk::uniform(stone)), Some(Chunk::uniform(water)), Some(mixed)];
        for own in [AIR, stone, water, slab] {
            for face in Face::ALL {
                for nb in &neighbours {
                    let mut store = ChunkStore::new();
                    store.insert(centre, Chunk::uniform(own));
                    let (dx, dy, dz) = face.normal();
                    if let Some(c) = nb {
                        store.insert(centre.offset(dx, dy, dz), c.clone());
                    }
                    let empty = mesh_chunk(&r, &PaddedChunk::capture(&store, centre)).is_empty();
                    let claim = has_no_faces(&r, &store, centre);
                    // Never claim "empty" for a chunk that has geometry; the reverse is allowed
                    // only for the mixed neighbour, which the check does not look inside.
                    assert!(!claim || empty, "wrongly skipped: {own:?} {face:?}");
                    if nb.as_ref().map_or(true, |c| c.is_uniform()) {
                        assert_eq!(claim, empty, "{own:?} {face:?} {:?}", nb.as_ref().map(|c| c.uniform_state()));
                    }
                }
            }
        }
    }

    #[test]
    fn single_block_has_six_quads() {
        let r = reg();
        let mut pc = PaddedChunk::filled(AIR);
        pc.set(5, 5, 5, r.state("stone").unwrap());
        let m = mesh_chunk(&r, &pc);
        assert_eq!(m.opaque.quad_count(), 6);
        assert_eq!(m.quad_count(), 6);
    }

    #[test]
    fn adjacent_blocks_hide_shared_faces() {
        let r = reg();
        let stone = r.state("stone").unwrap();
        let mut pc = PaddedChunk::filled(AIR);
        pc.set(5, 5, 5, stone);
        pc.set(6, 5, 5, stone);
        assert_eq!(mesh_chunk(&r, &pc).quad_count(), 10);
    }

    #[test]
    fn slab_top_is_half_height() {
        let r = reg();
        let slab = BlockState::new(r.id("stone").unwrap(), Shape::SlabBottom);
        let mut pc = PaddedChunk::filled(AIR);
        pc.set(5, 5, 5, slab);
        let m = mesh_chunk(&r, &pc);
        assert_eq!(m.quad_count(), 6);
        let tops: Vec<_> = m.opaque.vertices.iter().filter(|v| v.face_index() == Face::PosY as u32).collect();
        assert_eq!(tops.len(), 4);
        assert!(tops.iter().all(|v| v.y() == 5 * 4 + 2));
    }

    #[test]
    fn slab_on_block_hides_touching_faces_only() {
        let r = reg();
        let stone = r.state("stone").unwrap();
        let slab = BlockState::new(r.id("stone").unwrap(), Shape::SlabBottom);
        let mut pc = PaddedChunk::filled(AIR);
        pc.set(5, 5, 5, stone);
        pc.set(5, 6, 5, slab);
        assert_eq!(mesh_chunk(&r, &pc).quad_count(), 10);
    }

    #[test]
    fn full_block_beside_slab_keeps_upper_side_face() {
        let r = reg();
        let stone = r.state("stone").unwrap();
        let slab = BlockState::new(r.id("stone").unwrap(), Shape::SlabBottom);
        let mut pc = PaddedChunk::filled(AIR);
        pc.set(5, 5, 5, stone);
        pc.set(6, 5, 5, slab);
        // block: 6 faces (its +X side is taller than the slab so stays visible)
        // slab: 5 faces (its -X face is fully covered by the block)
        assert_eq!(mesh_chunk(&r, &pc).quad_count(), 11);
    }

    #[test]
    fn water_goes_to_translucent_layer_and_merges_internally() {
        let r = reg();
        let water = r.state("water").unwrap();
        let mut pc = PaddedChunk::filled(AIR);
        pc.set(5, 5, 5, water);
        pc.set(6, 5, 5, water);
        let m = mesh_chunk(&r, &pc);
        assert_eq!(m.opaque.quad_count(), 0);
        assert_eq!(m.translucent.quad_count(), 10);
    }

    #[test]
    fn water_surfaces_know_where_the_shore_is() {
        let r = reg();
        let (water, stone) = (r.state("water").unwrap(), r.state("stone").unwrap());
        let mut pc = PaddedChunk::filled(AIR);
        for x in 4..9 {
            for z in 4..9 {
                pc.set(x, 5, z, water);
            }
        }
        // A pillar standing in the pond, and the pond's west side is land.
        pc.set(6, 5, 6, stone);
        for z in 3..10 {
            pc.set(3, 5, z, stone);
        }
        let m = mesh_chunk(&r, &pc);
        let shore = |x: u32, z: u32| -> Vec<u32> {
            let mut v: Vec<u32> = m
                .translucent
                .vertices
                .iter()
                .filter(|v| v.face_index() == Face::PosY as u32 && v.x() == x * 4 && v.z() == z * 4)
                .map(|v| v.ao())
                .collect();
            v.sort_unstable();
            v
        };
        // Two blocks from both the land and the pillar: open water.
        assert_eq!(shore(9, 4), vec![3], "the pond's far corner is 2+ blocks from everything");
        // On the shore line: touching. A block out from it: one away.
        assert_eq!(shore(4, 5), vec![0, 0]);
        assert_eq!(shore(5, 4), vec![1, 1]);
        // Around the pillar: its corner touches it, from all three water cells.
        assert_eq!(shore(6, 6), vec![0, 0, 0]);
        assert_eq!(shore(8, 6), vec![1, 1, 1, 1], "a block east of the pillar");
        assert_eq!(shore(8, 8), vec![2, 2, 2, 2], "diagonally a block from the pillar's corner");
        // Sides and the bottom of the pond are not surfaces: no shore there.
        assert!(m.translucent.vertices.iter().filter(|v| v.face_index() != Face::PosY as u32).all(|v| v.ao() == 3));
    }

    #[test]
    fn unloaded_neighbours_hide_seam_faces() {
        let r = reg();
        let stone = r.state("stone").unwrap();
        let mut store = ChunkStore::new();
        store.insert(ChunkPos::new(0, 0, 0), Chunk::uniform(stone));
        let m = mesh_chunk(&r, &PaddedChunk::capture(&store, ChunkPos::new(0, 0, 0)));
        assert_eq!(m.quad_count(), 0);

        // Now an air chunk appears to the +X side: only that wall becomes visible.
        store.insert(ChunkPos::new(1, 0, 0), Chunk::air());
        let m = mesh_chunk(&r, &PaddedChunk::capture(&store, ChunkPos::new(0, 0, 0)));
        assert_eq!(m.quad_count(), (CHUNK_SIZE * CHUNK_SIZE) as usize);
    }

    #[test]
    fn ambient_occlusion_darkens_inner_corners() {
        let r = reg();
        let stone = r.state("stone").unwrap();
        let mut pc = PaddedChunk::filled(AIR);
        pc.set(5, 5, 5, stone);
        pc.set(6, 6, 5, stone); // diagonal neighbour above the +X edge
        let m = mesh_chunk(&r, &pc);
        let mut ao: Vec<u32> = m
            .opaque
            .vertices
            .iter()
            .filter(|v| v.face_index() == Face::PosY as u32 && v.y() == 6 * 4 && v.x() <= 24 && v.z() <= 24 && v.x() >= 20)
            .map(|v| v.ao())
            .collect();
        ao.sort_unstable();
        assert_eq!(ao, vec![2, 2, 3, 3]);
    }

    #[test]
    fn every_triangle_winds_counter_clockwise_seen_from_outside() {
        // The renderer culls back faces, so a face wound the wrong way would vanish.
        // A lumpy structure with slabs also exercises the AO diagonal flip.
        let r = reg();
        let id = r.id("stone").unwrap();
        let stone = BlockState::full(id);
        let slab_b = BlockState::new(id, Shape::SlabBottom);
        let slab_t = BlockState::new(id, Shape::SlabTop);
        let quarter = BlockState::new(id, Shape::Quarter);
        let mut pc = PaddedChunk::filled(AIR);
        for (x, y, z, s) in [
            (5, 5, 5, stone),
            (6, 5, 5, stone),
            (6, 6, 5, stone),
            (5, 6, 6, slab_b),
            (7, 5, 6, slab_t),
            (4, 5, 5, quarter),
            (5, 5, 6, stone),
            (6, 5, 6, stone),
            (6, 6, 6, slab_b),
        ] {
            pc.set(x, y, z, s);
        }
        let m = mesh_chunk(&r, &pc);
        assert!(m.opaque.quad_count() > 20);
        let idx = quad_indices(m.opaque.quad_count());
        let v = &m.opaque.vertices;
        let pos = |i: u32| {
            let p = v[i as usize];
            [p.x() as f32, p.y() as f32, p.z() as f32]
        };
        for tri in idx.chunks(3) {
            let (a, b, c) = (pos(tri[0]), pos(tri[1]), pos(tri[2]));
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let face = Face::ALL[v[tri[0] as usize].face_index() as usize];
            let (fx, fy, fz) = face.normal();
            let dot = n[0] * fx as f32 + n[1] * fy as f32 + n[2] * fz as f32;
            assert!(dot > 0.0, "triangle faces away from {face:?}: {a:?} {b:?} {c:?}");
        }
    }

    #[test]
    fn packed_vertex_roundtrip() {
        let v = PackedVertex::pack(128, 7, 99, Face::NegZ, 2, 513, 15, 9);
        assert_eq!((v.x(), v.y(), v.z()), (128, 7, 99));
        assert_eq!(v.face_index(), Face::NegZ as u32);
        assert_eq!((v.ao(), v.tex(), v.sky(), v.block_light()), (2, 513, 15, 9));
        assert_eq!(std::mem::size_of::<PackedVertex>(), 8);
    }

    #[test]
    fn side_textures_start_at_the_top_of_half_slabs() {
        let quad = |y0: u32, y1: u32| {
            let mut v = [
                PackedVertex::pack(0, y0, 0, Face::PosX, 3, 1, 15, 0),
                PackedVertex::pack(0, y1, 0, Face::PosX, 3, 1, 15, 0),
                PackedVertex::pack(0, y1, 4, Face::PosX, 3, 1, 15, 0),
                PackedVertex::pack(0, y0, 4, Face::PosX, 3, 1, 15, 0),
            ];
            anchor_texture_top(Face::PosX, &mut v);
            v.map(|v| v.tex_top())
        };
        assert_eq!(quad(8, 10), [2; 4]); // a half-slab from y 2 to 2.5: lift by half a block
        assert_eq!(quad(8, 12), [0; 4]); // a full block
        assert_eq!(quad(4, 11), [1; 4]);
        let mut top = [PackedVertex::pack(0, 10, 0, Face::PosY, 3, 1, 15, 0); 4];
        anchor_texture_top(Face::PosY, &mut top);
        assert!(top.iter().all(|v| v.tex_top() == 0));
    }

    #[test]
    fn only_the_top_of_a_water_surface_is_lowered() {
        let reg = BlockRegistry::builtin();
        let water = reg.state("water").unwrap();
        let mut pc = PaddedChunk::filled(AIR);
        for y in 0..3 {
            pc.set(5, y, 5, water);
        }
        let m = mesh_chunk(&reg, &pc);
        let v = &m.translucent.vertices;
        assert!(!v.is_empty());
        for q in v {
            assert_eq!(q.liquid_top(), q.y() == 12, "only corners at the top of the column (y 3): {q:?}");
        }
        assert!(v.iter().any(|q| q.liquid_top()));
    }

    #[test]
    fn index_pattern() {
        assert_eq!(quad_indices(2), vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
    }
}
