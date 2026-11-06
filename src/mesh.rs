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

use crate::block::{BlockState, UNKNOWN};
use crate::coords::{ChunkPos, Face, LocalPos, CHUNK_SIZE};
use crate::registry::{BlockRegistry, RenderLayer};
use crate::world::ChunkStore;

const PADDED: usize = (CHUNK_SIZE + 2) as usize;

/// Block data for one chunk plus a one-voxel border. Border cells from unloaded
/// chunks are `UNKNOWN`, which hides the faces touching them.
pub struct PaddedChunk {
    blocks: Vec<BlockState>,
}

impl PaddedChunk {
    pub fn filled(state: BlockState) -> Self {
        PaddedChunk { blocks: vec![state; PADDED * PADDED * PADDED] }
    }

    /// Coordinates are chunk-relative, -1..=CHUNK_SIZE on each axis.
    #[inline]
    fn idx(x: i32, y: i32, z: i32) -> usize {
        debug_assert!(
            (-1..=CHUNK_SIZE).contains(&x) && (-1..=CHUNK_SIZE).contains(&y) && (-1..=CHUNK_SIZE).contains(&z)
        );
        (((y + 1) as usize * PADDED) + (z + 1) as usize) * PADDED + (x + 1) as usize
    }

    #[inline]
    pub fn at(&self, x: i32, y: i32, z: i32) -> BlockState {
        self.blocks[Self::idx(x, y, z)]
    }

    pub fn set(&mut self, x: i32, y: i32, z: i32, s: BlockState) {
        let i = Self::idx(x, y, z);
        self.blocks[i] = s;
    }

    pub fn capture(store: &ChunkStore, pos: ChunkPos) -> PaddedChunk {
        let mut out = PaddedChunk::filled(UNKNOWN);
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let Some(chunk) = store.get(pos.offset(dx, dy, dz)) else { continue };
                    let (sx, nx, ox) = axis_span(dx);
                    let (sy, ny, oy) = axis_span(dy);
                    let (sz, nz, oz) = axis_span(dz);
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

/// For a neighbour offset `d` in -1..=1: (source start in that chunk, length,
/// destination start in chunk-relative coordinates).
fn axis_span(d: i32) -> (i32, i32, i32) {
    match d {
        -1 => (CHUNK_SIZE - 1, 1, -1),
        0 => (0, CHUNK_SIZE, 0),
        _ => (0, 1, CHUNK_SIZE),
    }
}

/// 8 bytes:
///   a: x(8) | y(8)<<8 | z(8)<<16 | face(3)<<24 | ao(2)<<27
///   b: texture(16) | sky(4)<<16 | block(4)<<20 | 8 spare bits
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
const FACE_CORNERS: [[[i32; 3]; 4]; 6] = [
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
fn corner_ao(reg: &BlockRegistry, pc: &PaddedChunk, x: i32, y: i32, z: i32, face: Face) -> [u8; 4] {
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

#[allow(clippy::too_many_arguments)]
fn emit_quad(
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
    // Split along the brighter diagonal; rotate the order to flip it.
    let order: [usize; 4] = if ao[0] + ao[2] >= ao[1] + ao[3] { [0, 1, 2, 3] } else { [1, 2, 3, 0] };
    for &i in &order {
        layer.vertices.push(verts[i]);
    }
}

pub fn mesh_chunk(reg: &BlockRegistry, pc: &PaddedChunk) -> ChunkMesh {
    let mut mesh = ChunkMesh::default();
    for y in 0..CHUNK_SIZE {
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let state = pc.at(x, y, z);
                if state.is_air() {
                    continue;
                }
                let def = reg.get(state);
                let (ymin, ymax) = state.shape().y_range_q();
                for face in Face::ALL {
                    let (nx, ny, nz) = face.normal();
                    let nb = pc.at(x + nx, y + ny, z + nz);
                    if face_hidden(reg, state, face, nb) {
                        continue;
                    }
                    let ao = if def.render == RenderLayer::Translucent {
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
    fn index_pattern() {
        assert_eq!(quad_indices(2), vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
    }
}
