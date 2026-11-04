//! Distance-ring LOD selection over cubic chunks, as an octree.
//!
//! Level L nodes cover 2^L chunks per side. A node subdivides while the camera is
//! closer than `near * 2^(L-1)` chunks, so detail rings double in size and the result
//! is watertight (no gaps or overlaps between rings, unlike picking nodes per ring).
//! Distances are integer Chebyshev distances in chunk units.
//!
//! Each selected node is meshed with the same 32^3 job as a normal chunk, sampled at
//! 2^L spacing; seam handling (skirts or dithered fades) is left to the renderer.

use crate::coords::ChunkPos;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LodNode {
    /// Position in units of `2^level` chunks.
    pub pos: ChunkPos,
    pub level: u8,
}

impl LodNode {
    pub fn size(&self) -> i32 {
        1 << self.level
    }

    pub fn min_chunk(&self) -> ChunkPos {
        ChunkPos::new(self.pos.x << self.level, self.pos.y << self.level, self.pos.z << self.level)
    }

    pub fn contains(&self, c: ChunkPos) -> bool {
        let m = self.min_chunk();
        let s = self.size();
        c.x >= m.x && c.x < m.x + s && c.y >= m.y && c.y < m.y + s && c.z >= m.z && c.z < m.z + s
    }
}

#[derive(Copy, Clone, Debug)]
pub struct LodConfig {
    /// Radius (in chunks) rendered at full detail.
    pub near: i32,
    pub max_level: u8,
    /// Nothing is selected beyond this Chebyshev distance.
    pub max_distance: i32,
}

fn node_distance(node: &LodNode, cam: ChunkPos) -> i32 {
    let m = node.min_chunk();
    let s = node.size();
    let axis = |lo: i32, c: i32| -> i32 {
        let hi = lo + s - 1;
        if c < lo {
            lo - c
        } else if c > hi {
            c - hi
        } else {
            0
        }
    };
    axis(m.x, cam.x).max(axis(m.y, cam.y)).max(axis(m.z, cam.z))
}

fn recurse(node: LodNode, cam: ChunkPos, cfg: &LodConfig, out: &mut Vec<LodNode>) {
    let d = node_distance(&node, cam);
    if d > cfg.max_distance {
        return;
    }
    let leaf = node.level == 0 || d >= (cfg.near << (node.level - 1));
    if leaf {
        out.push(node);
        return;
    }
    let child_level = node.level - 1;
    for dy in 0..2 {
        for dz in 0..2 {
            for dx in 0..2 {
                let child = LodNode {
                    pos: ChunkPos::new(node.pos.x * 2 + dx, node.pos.y * 2 + dy, node.pos.z * 2 + dz),
                    level: child_level,
                };
                recurse(child, cam, cfg, out);
            }
        }
    }
}

pub fn select(cam: ChunkPos, cfg: &LodConfig) -> Vec<LodNode> {
    let mut out = Vec::new();
    let s = 1i32 << cfg.max_level;
    let r = cfg.max_distance;
    let lo = |c: i32| (c - r).div_euclid(s);
    let hi = |c: i32| (c + r).div_euclid(s);
    for y in lo(cam.y)..=hi(cam.y) {
        for z in lo(cam.z)..=hi(cam.z) {
            for x in lo(cam.x)..=hi(cam.x) {
                recurse(LodNode { pos: ChunkPos::new(x, y, z), level: cfg.max_level }, cam, cfg, &mut out);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octree_is_disjoint_and_full_detail_near_camera() {
        let cfg = LodConfig { near: 2, max_level: 3, max_distance: 12 };
        let cam = ChunkPos::new(3, -2, 5);
        let nodes = select(cam, &cfg);
        let r = cfg.max_distance;
        for dy in -r..=r {
            for dz in -r..=r {
                for dx in -r..=r {
                    let p = cam.offset(dx, dy, dz);
                    let covering: Vec<&LodNode> = nodes.iter().filter(|n| n.contains(p)).collect();
                    assert!(covering.len() <= 1, "overlap at {p:?}");
                    let d = dx.abs().max(dy.abs()).max(dz.abs());
                    if d < cfg.near {
                        assert_eq!(covering.len(), 1, "gap near camera at {p:?}");
                        assert_eq!(covering[0].level, 0);
                    }
                }
            }
        }
    }

    #[test]
    fn far_nodes_are_coarser() {
        let cfg = LodConfig { near: 2, max_level: 3, max_distance: 16 };
        let cam = ChunkPos::new(0, 0, 0);
        let nodes = select(cam, &cfg);
        let max_level_seen = nodes.iter().map(|n| n.level).max().unwrap();
        assert!(max_level_seen >= 2);
        // Fewer nodes than chunks: that is the point.
        assert!(nodes.len() < (2 * 16 + 1usize).pow(3));
    }
}
