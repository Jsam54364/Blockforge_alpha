//! Voxel raycast (Amanatides & Woo DDA). One implementation shared by mining,
//! placing, AI line-of-sight and projectiles.
//!
//! Works at block granularity. Sub-block shapes (slabs) would need a second pass
//! testing the ray against the occupied height range inside the hit cell.

use crate::coords::{BlockPos, Face};

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RayHit {
    pub pos: BlockPos,
    /// The face of the hit block that the ray entered through.
    pub face: Face,
    pub distance: f32,
}

/// If the ray starts inside a solid cell, it hits at distance 0 with face `PosY`.
pub fn raycast(
    origin: [f32; 3],
    dir: [f32; 3],
    max_dist: f32,
    mut is_solid: impl FnMut(BlockPos) -> bool,
) -> Option<RayHit> {
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len == 0.0 || !len.is_finite() {
        return None;
    }
    let d = [dir[0] / len, dir[1] / len, dir[2] / len];
    let mut pos = BlockPos::from_f32(origin);
    let cell = [pos.x, pos.y, pos.z];

    let mut step = [0i32; 3];
    let mut t_delta = [f32::INFINITY; 3];
    let mut t_max = [f32::INFINITY; 3];
    for i in 0..3 {
        if d[i] > 0.0 {
            step[i] = 1;
            t_delta[i] = 1.0 / d[i];
            t_max[i] = ((cell[i] + 1) as f32 - origin[i]) / d[i];
        } else if d[i] < 0.0 {
            step[i] = -1;
            t_delta[i] = -1.0 / d[i];
            t_max[i] = (origin[i] - cell[i] as f32) / -d[i];
        }
    }

    let mut dist = 0.0f32;
    let mut face = Face::PosY;
    loop {
        if is_solid(pos) {
            return Some(RayHit { pos, face, distance: dist });
        }
        let axis = if t_max[0] < t_max[1] {
            if t_max[0] < t_max[2] {
                0
            } else {
                2
            }
        } else if t_max[1] < t_max[2] {
            1
        } else {
            2
        };
        dist = t_max[axis];
        if dist > max_dist {
            return None;
        }
        t_max[axis] += t_delta[axis];
        match axis {
            0 => {
                pos.x += step[0];
                face = if step[0] > 0 { Face::NegX } else { Face::PosX };
            }
            1 => {
                pos.y += step[1];
                face = if step[1] > 0 { Face::NegY } else { Face::PosY };
            }
            _ => {
                pos.z += step[2];
                face = if step[2] > 0 { Face::NegZ } else { Face::PosZ };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_wall_along_positive_x() {
        let wall = BlockPos::new(3, 0, 0);
        let hit = raycast([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 10.0, |p| p == wall).unwrap();
        assert_eq!(hit.pos, wall);
        assert_eq!(hit.face, Face::NegX);
        assert!((hit.distance - 2.5).abs() < 1e-4);
    }

    #[test]
    fn hits_wall_along_negative_x() {
        let wall = BlockPos::new(0, 0, 0);
        let hit = raycast([3.5, 0.5, 0.5], [-1.0, 0.0, 0.0], 10.0, |p| p == wall).unwrap();
        assert_eq!(hit.face, Face::PosX);
        assert!((hit.distance - 2.5).abs() < 1e-4);
    }

    #[test]
    fn misses_beyond_range() {
        assert!(raycast([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 2.0, |p| p.x == 5).is_none());
    }

    #[test]
    fn diagonal_ray_finds_floor() {
        let hit = raycast([0.5, 5.5, 0.5], [1.0, -1.0, 0.0], 20.0, |p| p.y <= 0).unwrap();
        assert_eq!(hit.pos.y, 0);
        assert_eq!(hit.face, Face::PosY);
    }

    #[test]
    fn zero_direction_is_none() {
        assert!(raycast([0.0; 3], [0.0; 3], 5.0, |_| true).is_none());
    }
}
