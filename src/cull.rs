//! Occlusion culling for chunks, after Tommaso Checchi's "advanced cave culling" (the method
//! Minecraft has used since 1.8, described on his blog in 2014).
//!
//! When a chunk is meshed, the empty space inside it is flood filled, and for every pair of
//! its six faces we note whether one region of see-through cells touches both: whether a
//! line of sight could enter through one face and leave through the other (`Links`). Each
//! frame a breadth-first walk starts at the camera's chunk and steps to a neighbour only
//! through faces its chunk connects to the face it was entered by, and never back against a
//! direction it has already moved in (sight lines are straight, so a path that doubles back
//! is not one). Chunks the walk never reaches are hidden behind terrain: deep caves under
//! the ground when standing on it, and everything above when standing in a cave.
//!
//! It is conservative: a reached chunk may still be hidden, but a chunk that could be seen
//! is always reached. Chunks with nothing known yet (not loaded, not meshed) count as open.

use std::collections::VecDeque;

use crate::block::{BlockState, UNKNOWN};
use crate::coords::{ChunkPos, Face, CHUNK_SIZE};
use crate::mesh::PaddedChunk;
use crate::registry::BlockRegistry;

/// For each pair of a chunk's faces, whether see-through space inside it touches both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Links(pub u16);

impl Links {
    /// Every face sees every other (air, water, an unknown chunk).
    pub const ALL: Links = Links(0x7fff);
    /// No face sees any other (solid ground).
    pub const NONE: Links = Links(0);

    /// The bit for faces `a` and `b` (a != b): the 15 pairs in order.
    fn bit(a: Face, b: Face) -> u16 {
        let (i, j) = if a.index() < b.index() { (a.index(), b.index()) } else { (b.index(), a.index()) };
        1 << (i * 6 - i * (i + 1) / 2 + (j - i - 1))
    }

    pub fn connects(self, a: Face, b: Face) -> bool {
        a == b || self.0 & Self::bit(a, b) != 0
    }

    /// A region touching all of `faces` (a bit per face index) links each pair of them.
    fn add_region(&mut self, faces: u8) {
        for a in Face::ALL {
            for b in Face::ALL {
                if a.index() < b.index() && faces & (1 << a.index()) != 0 && faces & (1 << b.index()) != 0 {
                    self.0 |= Self::bit(a, b);
                }
            }
        }
    }
}

/// Whether a block stops sight through its whole cell: an opaque full cube.
fn walls(reg: &BlockRegistry, s: BlockState) -> bool {
    s != UNKNOWN && !s.is_air() && s.shape().y_range_q() == (0, 4) && reg.get(s).opaque
}

/// The links of a chunk that is one block throughout.
pub fn links_uniform(reg: &BlockRegistry, s: BlockState) -> Links {
    if walls(reg, s) {
        Links::NONE
    } else {
        Links::ALL
    }
}

/// The links of the chunk in `pc` (its own cells; the border is not looked at).
pub fn links_of(reg: &BlockRegistry, pc: &PaddedChunk) -> Links {
    const N: i32 = CHUNK_SIZE;
    let row = |y: i32, z: i32| (y * N + z) as usize;
    // One bit per cell along x, per (y, z) row: wall cells, then cells already filled.
    let mut wall = vec![0u32; (N * N) as usize];
    let mut last = (UNKNOWN, false);
    let (mut any_wall, mut all_wall) = (false, true);
    for y in 0..N {
        for z in 0..N {
            let mut bits = 0u32;
            for x in 0..N {
                let s = pc.at(x, y, z);
                if s != last.0 {
                    last = (s, walls(reg, s));
                }
                if last.1 {
                    bits |= 1 << x;
                }
            }
            any_wall |= bits != 0;
            all_wall &= bits == u32::MAX;
            wall[row(y, z)] = bits;
        }
    }
    if !any_wall {
        return Links::ALL;
    }
    if all_wall {
        return Links::NONE;
    }
    let mut done = wall.clone();
    let mut links = Links::NONE;
    let mut stack: Vec<(i32, i32, i32)> = Vec::new();
    for y in 0..N {
        for z in 0..N {
            loop {
                let free = !done[row(y, z)];
                if free == 0 {
                    break;
                }
                // Flood the region holding the first open cell of this row.
                let mut faces = 0u8;
                let x0 = free.trailing_zeros() as i32;
                done[row(y, z)] |= 1 << x0;
                stack.push((x0, y, z));
                while let Some((x, y, z)) = stack.pop() {
                    faces |= (x == N - 1) as u8
                        | ((x == 0) as u8) << 1
                        | ((y == N - 1) as u8) << 2
                        | ((y == 0) as u8) << 3
                        | ((z == N - 1) as u8) << 4
                        | ((z == 0) as u8) << 5;
                    for (dx, dy, dz) in [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)] {
                        let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                        if !(0..N).contains(&nx) || !(0..N).contains(&ny) || !(0..N).contains(&nz) {
                            continue;
                        }
                        let r = &mut done[row(ny, nz)];
                        if *r & (1 << nx) == 0 {
                            *r |= 1 << nx;
                            stack.push((nx, ny, nz));
                        }
                    }
                }
                links.add_region(faces);
                if links == Links::ALL {
                    return links;
                }
            }
        }
    }
    links
}

/// The chunks that may be seen from a camera in chunk `from`, within `radius` chunks
/// horizontally and `vertical` up and down. `links` gives each chunk's links (`Links::ALL`
/// where nothing is known). Returned in the order the walk reached them (nearest first).
pub fn visible(from: ChunkPos, radius: i32, vertical: i32, links: impl Fn(ChunkPos) -> Links) -> Vec<ChunkPos> {
    let (w, h) = (2 * radius + 1, 2 * vertical + 1);
    let slot = |p: ChunkPos| -> Option<usize> {
        let (dx, dy, dz) = (p.x - from.x + radius, p.y - from.y + vertical, p.z - from.z + radius);
        if (0..w).contains(&dx) && (0..h).contains(&dy) && (0..w).contains(&dz) {
            Some(((dy * w + dz) * w + dx) as usize)
        } else {
            None
        }
    };
    let mut seen = vec![false; (w * w * h) as usize];
    let mut out = vec![from];
    if let Some(i) = slot(from) {
        seen[i] = true;
    }
    // (chunk, the face it was entered through, the directions moved so far)
    let mut queue: VecDeque<(ChunkPos, Option<Face>, u8)> = VecDeque::new();
    queue.push_back((from, None, 0));
    while let Some((p, entered, moved)) = queue.pop_front() {
        let l = if entered.is_some() { links(p) } else { Links::ALL };
        for dir in Face::ALL {
            if moved & (1 << dir.opposite().index()) != 0 {
                continue;
            }
            if let Some(e) = entered {
                if !l.connects(e, dir) {
                    continue;
                }
            }
            let (dx, dy, dz) = dir.normal();
            let n = p.offset(dx, dy, dz);
            let Some(i) = slot(n) else { continue };
            if seen[i] {
                continue;
            }
            seen[i] = true;
            out.push(n);
            queue.push_back((n, Some(dir.opposite()), moved | (1 << dir.index())));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::AIR;
    use std::collections::{HashMap, HashSet};

    fn reg() -> BlockRegistry {
        BlockRegistry::builtin()
    }

    fn solid(r: &BlockRegistry) -> PaddedChunk {
        PaddedChunk::filled(r.state("stone").unwrap())
    }

    #[test]
    fn the_fifteen_pairs_have_their_own_bits() {
        let mut bits = HashSet::new();
        for a in Face::ALL {
            for b in Face::ALL {
                if a != b {
                    assert_eq!(Links::bit(a, b), Links::bit(b, a));
                    bits.insert(Links::bit(a, b));
                }
            }
        }
        assert_eq!(bits.len(), 15);
        assert_eq!(bits.iter().fold(0, |m, b| m | b), Links::ALL.0);
    }

    #[test]
    fn air_and_stone_chunks() {
        let r = reg();
        assert_eq!(links_of(&r, &PaddedChunk::filled(AIR)), Links::ALL);
        assert_eq!(links_of(&r, &solid(&r)), Links::NONE);
        assert_eq!(links_uniform(&r, AIR), Links::ALL);
        assert_eq!(links_uniform(&r, r.state("water").unwrap()), Links::ALL);
        assert_eq!(links_uniform(&r, r.state("stone").unwrap()), Links::NONE);
    }

    #[test]
    fn a_tunnel_links_only_its_two_ends() {
        let r = reg();
        let mut pc = solid(&r);
        // A tunnel along x, then turning up through the top at x = 20.
        for x in 0..=20 {
            pc.set(x, 10, 10, AIR);
        }
        for y in 10..CHUNK_SIZE {
            pc.set(20, y, 10, AIR);
        }
        let l = links_of(&r, &pc);
        assert!(l.connects(Face::NegX, Face::PosY));
        assert_eq!(l.0.count_ones(), 1, "{l:?}");
        // A sealed pocket links nothing; a slab or water lets sight through.
        let mut pc = solid(&r);
        pc.set(5, 5, 5, AIR);
        assert_eq!(links_of(&r, &pc), Links::NONE);
        let mut pc = solid(&r);
        let slab = crate::block::BlockState::new(r.id("stone").unwrap(), crate::block::Shape::SlabBottom);
        for z in 0..CHUNK_SIZE {
            pc.set(3, 3, z, if z % 2 == 0 { slab } else { r.state("water").unwrap() });
        }
        assert_eq!(links_of(&r, &pc), Links(Links::bit(Face::NegZ, Face::PosZ)));
    }

    #[test]
    fn the_walk_stops_at_solid_ground_and_follows_caves() {
        // Ground: every chunk with y < 0 is solid, except a cave going straight down at
        // (0, *, 0) to y = -3, then along +x.
        let pair = |a: Face, b: Face| Links(Links::bit(a, b));
        let links = |p: ChunkPos| -> Links {
            match (p.x, p.y, p.z) {
                (_, y, _) if y >= 0 => Links::ALL,
                (0, -2..=-1, 0) => pair(Face::PosY, Face::NegY),
                (0, -3, 0) => pair(Face::PosY, Face::PosX),
                (x, -3, 0) if x > 0 => pair(Face::NegX, Face::PosX),
                _ => Links::NONE,
            }
        };
        let seen: HashSet<ChunkPos> = visible(ChunkPos::new(0, 1, 0), 6, 6, links).into_iter().collect();
        // Above ground, everything; the first layer of ground is reached (its top shows).
        assert!(seen.contains(&ChunkPos::new(5, 3, -4)));
        assert!(seen.contains(&ChunkPos::new(3, -1, 2)));
        // Below that, only the cave.
        assert!(!seen.contains(&ChunkPos::new(3, -2, 2)));
        assert!(seen.contains(&ChunkPos::new(0, -3, 0)));
        assert!(seen.contains(&ChunkPos::new(4, -3, 0)), "along the cave");
        assert!(!seen.contains(&ChunkPos::new(4, -4, 0)), "nothing under it");

        // Standing in the cave at its far end: back along the cave and up the shaft (the sky
        // above it could show), but not the surface off to the side or the ground around.
        let seen: HashSet<ChunkPos> = visible(ChunkPos::new(5, -3, 0), 6, 6, links).into_iter().collect();
        assert!(seen.contains(&ChunkPos::new(1, -3, 0)));
        assert!(seen.contains(&ChunkPos::new(0, 2, 0)));
        assert!(!seen.contains(&ChunkPos::new(3, 1, 3)));
        assert!(!seen.contains(&ChunkPos::new(5, -5, 3)));
    }

    #[test]
    fn open_space_reaches_everything_in_range() {
        let seen = visible(ChunkPos::new(2, 0, -1), 3, 2, |_| Links::ALL);
        assert_eq!(seen.len(), 7 * 7 * 5);
        let unique: HashSet<_> = seen.iter().collect();
        assert_eq!(unique.len(), seen.len());
    }

    #[test]
    fn real_terrain_hides_the_deep_ground() {
        use crate::world::ChunkStore;
        use crate::worldgen::WorldGen;
        let r = reg();
        let g = WorldGen::new(1337, &r);
        let mut store = ChunkStore::new();
        let spawn = g.find_spawn();
        let (sx, sz) = ((spawn[0] as i32).div_euclid(CHUNK_SIZE), (spawn[2] as i32).div_euclid(CHUNK_SIZE));
        let cy = (spawn[1] as i32).div_euclid(CHUNK_SIZE);
        for dz in -3..=3 {
            for dx in -3..=3 {
                let col = g.column(sx + dx, sz + dz);
                for dy in -5..=2 {
                    let p = ChunkPos::new(sx + dx, cy + dy, sz + dz);
                    store.insert(p, g.generate_chunk(p, &col));
                }
            }
        }
        let mut links = HashMap::new();
        for p in store.positions().collect::<Vec<_>>() {
            links.insert(p, links_of(&r, &PaddedChunk::capture(&store, p)));
        }
        // Standing on the ground at the spawn.
        let cam = ChunkPos::new(sx, cy, sz);
        let seen = visible(cam, 3, 6, |p| links.get(&p).copied().unwrap_or(Links::ALL));
        let loaded_seen = seen.iter().filter(|p| store.contains(**p)).count();
        assert!(loaded_seen < store.len() * 3 / 4, "{loaded_seen} of {}", store.len());
        assert!(seen.contains(&ChunkPos::new(sx, cy - 1, sz)), "the ground under the camera");
    }
}
