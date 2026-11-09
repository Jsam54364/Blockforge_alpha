//! Greedy meshing of full opaque cubes using bit masks.
//!
//! The approach follows the published "binary greedy meshing" idea (cgerikj's repository, MIT,
//! and the 0fps greedy-meshing article before it): keep one 64-bit integer per (x, z) column
//! with one bit per voxel along y, find visible faces for a whole column at once with shifts
//! and masks, then merge neighbouring visible faces that share a texture into rectangles.
//! This is our own implementation, adapted to blockforge's 34^3 padded snapshot, quarter-block
//! shapes and per-vertex ambient occlusion:
//!
//! * Only full, opaque, opaque-layer cubes are handled here. Slabs, water, leaves and other
//!   shapes keep using the per-face mesher in `mesh.rs`, which still decides how they interact
//!   with these cubes. The occupancy masks below encode exactly the same hiding rules as
//!   `mesh::face_hidden`, and the tests prove both meshers cover the same unit faces.
//! * A merged quad has no interior vertices, so it cannot carry ambient occlusion that varies
//!   along the merge. Faces therefore merge only when their per-corner AO is identical AND
//!   unchanged across the merge direction (a straight cliff foot, with its dark bottom corners,
//!   merges along the cliff; a face shaded diagonally stays a unit quad). Linear interpolation
//!   across such a quad gives exactly the shading the individual faces had.
//! * Merged quads are flagged so the vertex shader can grow them by a sliver, which hides the
//!   pixel cracks that T-junctions against smaller neighbours would otherwise show.

use crate::block::{BlockState, UNKNOWN};
use crate::coords::{Face, CHUNK_SIZE};
use crate::mesh::{corner_ao, ChunkMesh, MeshLayer, PackedVertex, PaddedChunk, FACE_CORNERS};
use crate::registry::{BlockRegistry, RenderLayer};

const N: usize = CHUNK_SIZE as usize; // 32
const P: usize = N + 2; // 34: the padded snapshot
/// Bits 1..=32 of a column: the chunk's own voxels, excluding the one-voxel border.
const INTERIOR: u64 = ((1u64 << N) - 1) << 1;

#[derive(Clone, Copy, Default)]
struct Class {
    /// A full opaque cube that this module meshes.
    elig: bool,
    /// Hides a full block's side faces (and top/bottom): opaque and fills the whole cell.
    full: bool,
    /// Opaque and touching the bottom of its cell: hides the top face of the block below.
    bottom: bool,
    /// Opaque and touching the top of its cell: hides the bottom face of the block above.
    top: bool,
}

fn classify(reg: &BlockRegistry, s: BlockState) -> Class {
    if s == UNKNOWN {
        // Unloaded neighbour: hide faces at the seam, same as the per-face mesher.
        return Class { elig: false, full: true, bottom: true, top: true };
    }
    if s.is_air() {
        return Class::default();
    }
    let d = reg.get(s);
    if !d.opaque {
        return Class::default();
    }
    let (lo, hi) = s.shape().y_range_q();
    Class {
        elig: d.render == RenderLayer::Opaque && s.shape().is_full(),
        full: lo == 0 && hi == 4,
        bottom: lo == 0,
        top: hi == 4,
    }
}

/// Whether the greedy path (rather than the per-face path) owns this block's faces.
pub(crate) fn is_eligible(reg: &BlockRegistry, s: BlockState) -> bool {
    classify(reg, s).elig
}

struct Columns {
    elig: Vec<u64>,
    full: Vec<u64>,
    bot: Vec<u64>,
    top: Vec<u64>,
}

/// One u64 per padded (x, z) column; bit y = padded y.
fn build_columns(reg: &BlockRegistry, pc: &PaddedChunk) -> Columns {
    let mut c = Columns {
        elig: vec![0; P * P],
        full: vec![0; P * P],
        bot: vec![0; P * P],
        top: vec![0; P * P],
    };
    // Runs of identical blocks are common, so remember the last classification.
    let mut last = (BlockState(0), Class::default());
    for x in 0..P {
        for z in 0..P {
            let idx = x * P + z;
            for y in 0..P {
                let s = pc.at(x as i32 - 1, y as i32 - 1, z as i32 - 1);
                let cl = if s == last.0 {
                    last.1
                } else {
                    let k = classify(reg, s);
                    last = (s, k);
                    k
                };
                let bit = 1u64 << y;
                if cl.elig {
                    c.elig[idx] |= bit;
                }
                if cl.full {
                    c.full[idx] |= bit;
                }
                if cl.bottom {
                    c.bot[idx] |= bit;
                }
                if cl.top {
                    c.top[idx] |= bit;
                }
            }
        }
    }
    c
}

/// Meshes every eligible block's faces into `out.opaque`.
pub(crate) fn mesh_eligible(reg: &BlockRegistry, pc: &PaddedChunk, out: &mut ChunkMesh) {
    let cols = build_columns(reg, pc);
    // rows[slice * N + v]: bit u set = a mergeable visible face at (u, v) in that slice.
    let mut rows = vec![0u32; N * N];
    let mut keys = vec![0u32; N * N * N];
    for face in Face::ALL {
        mesh_direction(reg, pc, &cols, face, &mut rows, &mut keys, out);
    }
}

fn mesh_direction(
    reg: &BlockRegistry,
    pc: &PaddedChunk,
    cols: &Columns,
    face: Face,
    rows: &mut [u32],
    keys: &mut [u32],
    out: &mut ChunkMesh,
) {
    let d = face.index();
    let axis = d / 2; // 0 = X, 1 = Y, 2 = Z
    let t1 = (axis + 1) % 3; // merge plane axes: u along t1, v along t2
    let t2 = (axis + 2) % 3;
    // Which entry of FACE_CORNERS sits at (u side, v side) of the face.
    let mut corner_at = [[0usize; 2]; 2];
    for (i, c) in FACE_CORNERS[d].iter().enumerate() {
        corner_at[c[t1] as usize][c[t2] as usize] = i;
    }

    // 1. Visible faces for whole columns at once; scatter them into per-slice planes.
    for x in 1..=N {
        for z in 1..=N {
            let idx = x * P + z;
            let occluded = match face {
                Face::PosX => cols.full[(x + 1) * P + z],
                Face::NegX => cols.full[(x - 1) * P + z],
                Face::PosZ => cols.full[x * P + z + 1],
                Face::NegZ => cols.full[x * P + z - 1],
                Face::PosY => cols.bot[idx] >> 1,
                Face::NegY => cols.top[idx] << 1,
            };
            let mut vis = cols.elig[idx] & !occluded & INTERIOR;
            while vis != 0 {
                let y = vis.trailing_zeros() as usize;
                vis &= vis - 1;
                let c = [x - 1, y - 1, z - 1]; // chunk-relative voxel coordinates
                let (cx, cy, cz) = (c[0] as i32, c[1] as i32, c[2] as i32);
                let state = pc.at(cx, cy, cz);
                let def = reg.get(state);
                let tex = def.tex[d];
                let emission = def.light_emission;
                let ao = corner_ao(reg, pc, cx, cy, cz, face);
                // Key = texture | emission | AO of the four corners (2 bits each, in
                // FACE_CORNERS order). Faces merge only if their keys are equal.
                let sig = ao[0] as u32 | ((ao[1] as u32) << 2) | ((ao[2] as u32) << 4) | ((ao[3] as u32) << 6);
                let row = c[axis] * N + c[t2];
                rows[row] |= 1u32 << c[t1];
                keys[row * N + c[t1]] = tex as u32 | ((emission as u32) << 16) | (sig << 24);
            }
        }
    }

    // 2. Greedy merge inside each slice.
    for slice in 0..N {
        let base = slice * N;
        for v in 0..N {
            while rows[base + v] != 0 {
                let row = rows[base + v];
                let u0 = row.trailing_zeros() as usize;
                // Longest run of adjacent visible faces starting at u0 ...
                let run = (row >> u0).trailing_ones() as usize;
                let key = keys[(base + v) * N + u0];
                let (same_along_u, same_along_v) = ao_symmetry(key >> 24, &corner_at);
                // ... shortened to the part sharing the first face's key, and only if widening
                // cannot change the shading.
                let mut w = 1;
                if same_along_u {
                    while w < run && keys[(base + v) * N + u0 + w] == key {
                        w += 1;
                    }
                }
                let mask: u32 = if w == 32 { u32::MAX } else { ((1u32 << w) - 1) << u0 };
                // Grow downward while the whole run is visible with the same texture.
                let mut h = 1;
                while same_along_v && v + h < N {
                    let r = base + v + h;
                    if rows[r] & mask != mask {
                        break;
                    }
                    if (0..w).any(|i| keys[r * N + u0 + i] != key) {
                        break;
                    }
                    h += 1;
                }
                for k in 0..h {
                    rows[base + v + k] &= !mask;
                }
                emit_merged(&mut out.opaque, face, slice, u0, v, w, h, key);
            }
        }
    }
}

/// For an AO signature (see the key layout above): is the shading unchanged along the face's
/// u axis, and along its v axis? Only then can faces merge in that direction without changing
/// how they are lit.
fn ao_symmetry(sig: u32, corner_at: &[[usize; 2]; 2]) -> (bool, bool) {
    let ao = |cu: usize, cv: usize| (sig >> (2 * corner_at[cu][cv])) & 3;
    let along_u = ao(0, 0) == ao(1, 0) && ao(0, 1) == ao(1, 1);
    let along_v = ao(0, 0) == ao(0, 1) && ao(1, 0) == ao(1, 1);
    (along_u, along_v)
}

#[allow(clippy::too_many_arguments)]
fn emit_merged(
    layer: &mut MeshLayer,
    face: Face,
    slice: usize,
    u0: usize,
    v0: usize,
    w: usize,
    h: usize,
    key: u32,
) {
    let d = face.index();
    let axis = d / 2;
    let t1 = (axis + 1) % 3;
    let t2 = (axis + 2) % 3;
    let tex = (key & 0xFFFF) as u16;
    let emission = (key >> 16) as u8;
    let sig = key >> 24;
    let ao_of = |i: usize| ((sig >> (2 * i)) & 3) as u8;
    let grows = w > 1 || h > 1;
    let mut verts = [PackedVertex::default(); 4];
    for (i, c) in FACE_CORNERS[d].iter().enumerate() {
        let mut p = [0i32; 3];
        // c[axis] is 1 on the positive face and 0 on the negative one.
        p[axis] = (slice as i32 + c[axis]) * 4;
        p[t1] = (u0 as i32 + c[t1] * w as i32) * 4;
        p[t2] = (v0 as i32 + c[t2] * h as i32) * 4;
        let mut vertex =
            PackedVertex::pack(p[0] as u32, p[1] as u32, p[2] as u32, face, ao_of(i), tex, 15, emission);
        if grows {
            vertex = vertex.with_grow(c[t1] == 1, c[t2] == 1);
        }
        verts[i] = vertex;
    }
    // Same diagonal rule as the per-face mesher (rotating the order flips the split).
    let order: [usize; 4] = if ao_of(0) + ao_of(2) >= ao_of(1) + ao_of(3) { [0, 1, 2, 3] } else { [1, 2, 3, 0] };
    for &i in &order {
        layer.vertices.push(verts[i]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{Shape, AIR};
    use crate::coords::ChunkPos;
    use crate::mesh::{mesh_chunk, mesh_chunk_greedy, quad_indices};
    use crate::rng::hash3;
    use crate::world::ChunkStore;
    use crate::worldgen::WorldGen;

    fn reg() -> BlockRegistry {
        BlockRegistry::builtin()
    }

    fn stone(r: &BlockRegistry) -> BlockState {
        r.state("stone").unwrap()
    }

    /// One face, described independently of how it was meshed.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    struct Rec {
        layer: u8,
        face: u32,
        plane: u32,
        a_min: u32,
        b_min: u32,
        a_len: u32,
        b_len: u32,
        tex: u32,
        sky: u32,
        blk: u32,
        ao: [u32; 4],
    }

    fn coord(v: &PackedVertex, axis: usize) -> u32 {
        match axis {
            0 => v.x(),
            1 => v.y(),
            _ => v.z(),
        }
    }

    /// Every quad as a list of unit-cell records, so merged and unmerged meshes compare equal
    /// when they cover the same faces. Duplicates (overlaps) show up as extra records.
    fn canon(mesh: &ChunkMesh) -> Vec<Rec> {
        let mut out = Vec::new();
        for (li, layer) in [&mesh.opaque, &mesh.cutout, &mesh.translucent].iter().enumerate() {
            for q in layer.vertices.chunks(4) {
                let face = q[0].face_index();
                let axis = (face / 2) as usize;
                let (t1, t2) = ((axis + 1) % 3, (axis + 2) % 3);
                let lo = |a: usize| q.iter().map(|v| coord(v, a)).min().unwrap();
                let hi = |a: usize| q.iter().map(|v| coord(v, a)).max().unwrap();
                let (plane, a0, a1, b0, b1) = (lo(axis), lo(t1), hi(t1), lo(t2), hi(t2));
                // AO by corner position (u side, v side), not as a sorted bag, so a mix-up
                // between corners would be caught.
                let mut ao = [0u32; 4];
                for v in q.iter() {
                    let su = (coord(v, t1) == a1) as usize;
                    let sv = (coord(v, t2) == b1) as usize;
                    ao[su + 2 * sv] = v.ao();
                }
                let (la, lb) = (a1 - a0, b1 - b0);
                let rec = |a_min, b_min, a_len, b_len| Rec {
                    layer: li as u8,
                    face,
                    plane,
                    a_min,
                    b_min,
                    a_len,
                    b_len,
                    tex: q[0].tex(),
                    sky: q[0].sky(),
                    blk: q[0].block_light(),
                    ao,
                };
                if la % 4 == 0 && lb % 4 == 0 && la >= 4 && lb >= 4 && (la > 4 || lb > 4) {
                    for i in 0..la / 4 {
                        for j in 0..lb / 4 {
                            out.push(rec(a0 + i * 4, b0 + j * 4, 4, 4));
                        }
                    }
                } else {
                    out.push(rec(a0, b0, la, lb));
                }
            }
        }
        out.sort();
        out
    }

    fn assert_same(r: &BlockRegistry, pc: &PaddedChunk, what: &str) -> (usize, usize) {
        let simple = mesh_chunk(r, pc);
        let greedy = mesh_chunk_greedy(r, pc);
        assert_eq!(canon(&simple), canon(&greedy), "{what}");
        assert!(greedy.quad_count() <= simple.quad_count(), "{what}: greedy used more quads");
        (simple.quad_count(), greedy.quad_count())
    }

    fn assert_outward(mesh: &ChunkMesh) {
        for layer in [&mesh.opaque, &mesh.cutout, &mesh.translucent] {
            let idx = quad_indices(layer.quad_count());
            let v = &layer.vertices;
            for tri in idx.chunks(3) {
                let p = |i: u32| {
                    let q = v[i as usize];
                    [q.x() as f32, q.y() as f32, q.z() as f32]
                };
                let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
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
    }

    fn palette(r: &BlockRegistry) -> Vec<BlockState> {
        let id = |n: &str| r.id(n).unwrap();
        let stone = id("stone");
        vec![
            AIR,
            AIR,
            BlockState::full(stone),
            BlockState::full(stone),
            BlockState::full(id("dirt")),
            BlockState::full(id("grass")),
            BlockState::full(id("sand")),
            BlockState::full(id("water")),
            BlockState::full(id("leaves")),
            BlockState::full(id("glowstone")),
            BlockState::new(stone, Shape::SlabBottom),
            BlockState::new(stone, Shape::SlabTop),
            BlockState::new(stone, Shape::Quarter),
            BlockState::new(id("grass"), Shape::ThreeQuarter),
        ]
    }

    /// Blocky random scene: `cluster`-sized cubes of a random block type, so bigger clusters
    /// give bigger mergeable areas. The border is either unloaded or random.
    fn scene(r: &BlockRegistry, seed: u64, cluster: i32, unknown_border: bool) -> PaddedChunk {
        let pal = palette(r);
        let mut pc = PaddedChunk::filled(UNKNOWN);
        for y in -1..=CHUNK_SIZE {
            for z in -1..=CHUNK_SIZE {
                for x in -1..=CHUNK_SIZE {
                    let border = x < 0 || y < 0 || z < 0 || x >= CHUNK_SIZE || y >= CHUNK_SIZE || z >= CHUNK_SIZE;
                    if border && unknown_border {
                        continue;
                    }
                    let h = hash3(seed, x.div_euclid(cluster), y.div_euclid(cluster), z.div_euclid(cluster));
                    pc.set(x, y, z, pal[(h % pal.len() as u64) as usize]);
                }
            }
        }
        pc
    }

    /// Interior filled with `f(x, z)` below y = 16 and air above; unloaded border.
    fn plate(f: impl Fn(i32, i32) -> BlockState) -> PaddedChunk {
        let mut pc = PaddedChunk::filled(UNKNOWN);
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    pc.set(x, y, z, if y < 16 { f(x, z) } else { AIR });
                }
            }
        }
        pc
    }

    #[test]
    fn greedy_covers_exactly_the_same_faces_as_the_simple_mesher() {
        let r = reg();
        for seed in 0..4u64 {
            for &cluster in &[1, 2, 4, 8] {
                for &unknown in &[true, false] {
                    let pc = scene(&r, seed, cluster, unknown);
                    assert_same(&r, &pc, &format!("seed {seed} cluster {cluster} unknown_border {unknown}"));
                }
            }
        }
    }

    #[test]
    fn greedy_matches_simple_on_real_terrain_and_uses_fewer_quads() {
        let r = reg();
        let g = WorldGen::new(1337, &r);
        for &(cx, cz) in &[(0, 0), (5, 3), (-4, 2)] {
            // A chunk around the middle of the column's surface heights, so it holds terrain.
            let col = g.column(cx, cz);
            let mid = (col.min_top + col.max_top).div_euclid(2);
            let cy = mid.div_euclid(CHUNK_SIZE);
            let mut store = ChunkStore::new();
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        let p = ChunkPos::new(cx + dx, cy + dy, cz + dz);
                        let c = g.column(p.x, p.z);
                        store.insert(p, g.generate_chunk(p, &c));
                    }
                }
            }
            let pc = PaddedChunk::capture(&store, ChunkPos::new(cx, cy, cz));
            let (simple, greedy) = assert_same(&r, &pc, &format!("terrain chunk {cx},{cy},{cz}"));
            assert!(simple > 0, "no geometry in test chunk {cx},{cy},{cz}");
            assert!(greedy < simple, "chunk {cx},{cy},{cz}: greedy {greedy} vs simple {simple}");
        }
    }

    #[test]
    fn a_flat_plate_becomes_one_quad() {
        let r = reg();
        let s = stone(&r);
        let pc = plate(|_, _| s);
        let g = mesh_chunk_greedy(&r, &pc);
        assert_eq!(g.opaque.quad_count(), 1);
        assert_eq!(g.quad_count(), 1);
        for v in &g.opaque.vertices {
            assert_eq!(v.y(), 64); // the plate's top is at block y = 16
            assert!(v.x() == 0 || v.x() == 128);
            assert!(v.z() == 0 || v.z() == 128);
            assert!(v.grows());
        }
        assert_eq!(mesh_chunk(&r, &pc).quad_count(), 1024);
        assert_outward(&g);
    }

    #[test]
    fn different_textures_do_not_merge_but_matching_ones_do() {
        let r = reg();
        let (s, grass) = (stone(&r), r.state("grass").unwrap());
        let pc = plate(|x, _| if x < 16 { grass } else { s });
        let m = mesh_chunk_greedy(&r, &pc);
        assert_eq!(m.opaque.quad_count(), 2);
        assert_same(&r, &pc, "two halves");
    }

    #[test]
    fn shading_blocks_merging_around_a_bump_but_the_rest_still_merges() {
        let r = reg();
        let s = stone(&r);
        let mut pc = plate(|_, _| s);
        pc.set(16, 16, 16, s); // a one-block bump in the middle
        let (simple, greedy) = assert_same(&r, &pc, "bump");
        assert!(greedy < simple, "greedy {greedy} vs simple {simple}");
        assert_outward(&mesh_chunk_greedy(&r, &pc));
    }

    #[test]
    fn checkerboards_and_slab_neighbours_stay_equivalent() {
        let r = reg();
        let s = stone(&r);
        assert_same(&r, &plate(|x, z| if (x + z) % 2 == 0 { s } else { AIR }), "checkerboard");

        let slab_b = BlockState::new(r.id("stone").unwrap(), Shape::SlabBottom);
        let slab_t = BlockState::new(r.id("stone").unwrap(), Shape::SlabTop);
        let mut pc = PaddedChunk::filled(AIR);
        pc.set(5, 5, 5, s);
        pc.set(5, 6, 5, slab_b); // a bottom slab hides the block's top face
        pc.set(5, 3, 5, slab_t);
        pc.set(5, 4, 5, s); // a top slab below hides this block's bottom face
        assert_same(&r, &pc, "slab neighbours");
    }

    #[test]
    fn grow_flags_mark_exactly_the_merged_quads_and_the_right_corners() {
        let r = reg();
        let pc = scene(&r, 3, 4, true);
        let m = mesh_chunk_greedy(&r, &pc);
        let mut merged_seen = 0;
        for q in m.opaque.vertices.chunks(4) {
            let axis = (q[0].face_index() / 2) as usize;
            let (t1, t2) = ((axis + 1) % 3, (axis + 2) % 3);
            let span = |a: usize| {
                let lo = q.iter().map(|v| coord(v, a)).min().unwrap();
                let hi = q.iter().map(|v| coord(v, a)).max().unwrap();
                (lo, hi)
            };
            let ((u_lo, u_hi), (v_lo, v_hi)) = (span(t1), span(t2));
            let merged = u_hi - u_lo > 4 || v_hi - v_lo > 4;
            for v in q {
                assert_eq!(v.grows(), merged);
                if merged {
                    assert_eq!(v.grow_u(), coord(v, t1) == u_hi);
                    assert_eq!(v.grow_v(), coord(v, t2) == v_hi);
                }
            }
            if merged {
                merged_seen += 1;
            }
        }
        assert!(merged_seen > 0, "the scene should contain merged quads");
    }

    #[test]
    fn a_straight_cliff_foot_merges_along_the_cliff_despite_its_shading() {
        let r = reg();
        let s = stone(&r);
        // A plateau over half the plate: a straight riser along x = 16 whose bottom corners are
        // shaded (dark at the foot of the cliff). Only the ends of the strip differ.
        let mut pc = plate(|_, _| s);
        for z in 0..CHUNK_SIZE {
            for x in 0..16 {
                pc.set(x, 16, z, s);
            }
        }
        let g = mesh_chunk_greedy(&r, &pc);
        let riser_quads: Vec<&[PackedVertex]> = g
            .opaque
            .vertices
            .chunks(4)
            .filter(|q| {
                q[0].face_index() == Face::PosX as u32
                    && q.iter().all(|v| v.x() == 64 && v.y() >= 64 && v.y() <= 68)
            })
            .collect();
        assert!(riser_quads.len() <= 3, "riser used {} quads instead of merging", riser_quads.len());
        let longest = riser_quads
            .iter()
            .map(|q| q.iter().map(|v| v.z()).max().unwrap() - q.iter().map(|v| v.z()).min().unwrap())
            .max()
            .unwrap();
        assert!(longest >= 29 * 4, "longest merged riser spans only {longest} quarter units");
        assert_same(&r, &pc, "plateau");
        assert_outward(&g);
    }

    #[test]
    fn ao_symmetry_flags_follow_the_signature() {
        // Corner order for a +Y face is c0=(0,1,1) c1=(1,1,1) c2=(1,1,0) c3=(0,1,0); there the
        // merge axes are u = z (t1) and v = x (t2), so corner_at[cu][cv] with cu = z side, cv = x side.
        let (t1, t2) = (2usize, 0usize);
        let mut corner_at = [[0usize; 2]; 2];
        for (i, c) in FACE_CORNERS[Face::PosY.index()].iter().enumerate() {
            corner_at[c[t1] as usize][c[t2] as usize] = i;
        }
        let sig = |ao: [u32; 4]| ao[0] | (ao[1] << 2) | (ao[2] << 4) | (ao[3] << 6);
        // Uniform shading is symmetric both ways.
        assert_eq!(ao_symmetry(sig([3, 3, 3, 3]), &corner_at), (true, true));
        // c1 and c2 are the two corners on the x = 1 edge. Dark there, bright on the x = 0 edge:
        // unchanged along z (u), changing along x (v).
        assert_eq!(ao_symmetry(sig([3, 1, 1, 3]), &corner_at), (true, false));
        // A diagonal pattern is symmetric along neither axis.
        assert_eq!(ao_symmetry(sig([1, 3, 1, 3]), &corner_at), (false, false));
    }

    #[test]
    fn eligibility_is_full_opaque_cubes_only() {
        let r = reg();
        assert!(is_eligible(&r, stone(&r)));
        assert!(is_eligible(&r, r.state("grass").unwrap()));
        assert!(!is_eligible(&r, AIR));
        assert!(!is_eligible(&r, UNKNOWN));
        assert!(!is_eligible(&r, r.state("water").unwrap()));
        assert!(!is_eligible(&r, r.state("leaves").unwrap()));
        assert!(!is_eligible(&r, BlockState::new(r.id("stone").unwrap(), Shape::SlabBottom)));
    }
}
