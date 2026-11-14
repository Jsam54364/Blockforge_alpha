//! The three questions the controller asks about the world, answered for a list of boxes (the
//! kit's own test world) and for the game's blocks.

/// What the ledge search is given (the controller's climb settings).
#[derive(Clone, Copy, Debug)]
pub struct LedgeQuery {
    pub min_height: f64,
    pub max_height: f64,
    pub reach: f64,
    pub facing: f64,
    pub stand_in: f64,
    pub hang_out: f64,
    pub radius: f64,
}

/// A climbable edge: its top, the direction into the wall, where to end up standing and where
/// the body hangs while climbing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ledge {
    pub top_y: f64,
    pub nx: f64,
    pub nz: f64,
    pub stand_x: f64,
    pub stand_z: f64,
    pub hang_x: f64,
    pub hang_z: f64,
}

pub trait World {
    /// Height of the highest surface at (x, z) that is no more than a step above y.
    fn ground_height(&self, x: f64, z: f64, y: f64) -> f64;
    /// A circle of radius `r` at (x, z), feet at y, pushed out of anything too tall to step onto.
    fn collide(&self, x: f64, z: f64, y: f64, r: f64) -> (f64, f64);
    /// A climbable edge ahead (direction dx, dz) whose top is `min_height..max_height` above
    /// `base_y` (the ground last stood on).
    #[allow(clippy::too_many_arguments)]
    fn find_ledge(&self, px: f64, pz: f64, py: f64, base_y: f64, dx: f64, dz: f64, q: &LedgeQuery) -> Option<Ledge>;
}

/// An upright box standing on the ground: x0..x1, z0..z1, top at h.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxShape {
    pub x0: f64,
    pub x1: f64,
    pub z0: f64,
    pub z1: f64,
    pub h: f64,
}

/// Pushes the circle (x, z, r) out of the box's footprint.
fn push_out(x: &mut f64, z: &mut f64, r: f64, b: &BoxShape) {
    let (cx, cz) = (x.clamp(b.x0, b.x1), z.clamp(b.z0, b.z1));
    let (dx, dz) = (*x - cx, *z - cz);
    let d2 = dx * dx + dz * dz;
    if d2 >= r * r {
        return;
    }
    if d2 > 1e-8 {
        let d = d2.sqrt();
        *x = cx + dx / d * r;
        *z = cz + dz / d * r;
    } else {
        // Inside the box: out through the nearest side.
        let (l, rr, n, f) = (*x - b.x0, b.x1 - *x, *z - b.z0, b.z1 - *z);
        let m = l.min(rr).min(n.min(f));
        if m == l {
            *x = b.x0 - r;
        } else if m == rr {
            *x = b.x1 + r;
        } else if m == n {
            *z = b.z0 - r;
        } else {
            *z = b.z1 + r;
        }
    }
}

/// The ledge a box offers to someone at (px, pz) heading (dx, dz), before checking that there
/// is room to stand on it: (distance, ledge).
#[allow(clippy::too_many_arguments)]
fn box_ledge(b: &BoxShape, px: f64, pz: f64, py: f64, base_y: f64, dx: f64, dz: f64, q: &LedgeQuery) -> Option<(f64, Ledge)> {
    let rel = b.h - base_y;
    if rel < q.min_height || rel > q.max_height || b.h - py < 0.3 || b.h - py > q.max_height {
        return None;
    }
    let (cx, cz) = (px.clamp(b.x0, b.x1), pz.clamp(b.z0, b.z1));
    let (ex, ez) = (cx - px, cz - pz);
    let d = (ex * ex + ez * ez).sqrt();
    if d > q.reach || d < 1e-4 || (ex * dx + ez * dz) / d < q.facing {
        return None;
    }
    let (tx, tz) = (cx + ex / d * q.stand_in, cz + ez / d * q.stand_in);
    if !(tx > b.x0 && tx < b.x1 && tz > b.z0 && tz < b.z1) {
        return None;
    }
    Some((
        d,
        Ledge {
            top_y: b.h,
            nx: ex / d,
            nz: ez / d,
            stand_x: tx,
            stand_z: tz,
            hang_x: cx - ex / d * q.hang_out,
            hang_z: cz - ez / d * q.hang_out,
        },
    ))
}

/// The kit's simple world: boxes on flat ground at y = 0 (its hyrig_box_world).
pub struct BoxWorld {
    pub boxes: Vec<BoxShape>,
    /// The character is kept within +-limit on x and z.
    pub limit: f64,
    pub step_height: f64,
}

impl BoxWorld {
    pub fn new(boxes: Vec<BoxShape>, limit: f64) -> BoxWorld {
        BoxWorld { boxes, limit, step_height: 0.3 }
    }
}

impl World for BoxWorld {
    fn ground_height(&self, x: f64, z: f64, y: f64) -> f64 {
        let mut g = 0.0;
        for b in &self.boxes {
            if x > b.x0 && x < b.x1 && z > b.z0 && z < b.z1 && b.h <= y + self.step_height && b.h > g {
                g = b.h;
            }
        }
        g
    }

    fn collide(&self, mut x: f64, mut z: f64, y: f64, r: f64) -> (f64, f64) {
        for b in &self.boxes {
            if b.h > y + self.step_height {
                push_out(&mut x, &mut z, r, b);
            }
        }
        (x.clamp(-self.limit, self.limit), z.clamp(-self.limit, self.limit))
    }

    fn find_ledge(&self, px: f64, pz: f64, py: f64, base_y: f64, dx: f64, dz: f64, q: &LedgeQuery) -> Option<Ledge> {
        let mut best: Option<(f64, Ledge)> = None;
        for (i, b) in self.boxes.iter().enumerate() {
            let Some((d, l)) = box_ledge(b, px, pz, py, base_y, dx, dz, q) else { continue };
            let r = q.radius;
            let blocked = self.boxes.iter().enumerate().any(|(j, o)| {
                j != i && o.h > b.h + 0.3 && l.stand_x > o.x0 - r && l.stand_x < o.x1 + r && l.stand_z > o.z0 - r && l.stand_z < o.z1 + r
            });
            if !blocked && best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, l));
            }
        }
        best.map(|(_, l)| l)
    }
}

/// What the block world needs to know about a block: `None` if that part of the world is not
/// loaded; otherwise whether there is something solid in the cell and, if so, the heights of
/// its bottom and top inside the cell (0..1; a half slab is (0.0, 0.5)).
pub trait Blocks {
    fn solid(&self, x: i32, y: i32, z: i32) -> Option<Option<(f32, f32)>>;
}

/// The game's blocks as the controller's world. Minecraft's rules where the kit has none: a
/// step is 0.6 blocks (half slabs are walked up and down, full blocks need a jump), and the
/// body is 1.8 tall for walls (it fits under a 2-block overhang).
pub struct BlockWorld<'a, B: Blocks> {
    pub blocks: &'a B,
    pub step_height: f64,
    pub height: f64,
}

impl<'a, B: Blocks> BlockWorld<'a, B> {
    pub fn new(blocks: &'a B) -> BlockWorld<'a, B> {
        BlockWorld { blocks, step_height: 0.6, height: 1.8 }
    }

    /// (bottom, top) of the solid part of a cell in world heights, if any. Unloaded = empty.
    fn span(&self, x: i32, y: i32, z: i32) -> Option<(f64, f64)> {
        self.blocks.solid(x, y, z).flatten().map(|(a, b)| (y as f64 + a as f64, y as f64 + b as f64))
    }

    /// Whether the cell column at (x, z) is loaded around height y.
    pub fn loaded(&self, x: f64, y: f64, z: f64) -> bool {
        self.blocks.solid(x.floor() as i32, y.floor() as i32, z.floor() as i32).is_some()
    }

    /// Nothing solid in the column at (x, z) between heights y0 and y1.
    fn clear(&self, x: i32, z: i32, y0: f64, y1: f64) -> bool {
        ((y0.floor() as i32)..=(y1.floor() as i32)).all(|y| self.span(x, y, z).map_or(true, |(a, b)| b <= y0 || a >= y1))
    }
}

impl<B: Blocks> World for BlockWorld<'_, B> {
    fn ground_height(&self, x: f64, z: f64, y: f64) -> f64 {
        let (bx, bz) = (x.floor() as i32, z.floor() as i32);
        let limit = y + self.step_height;
        let top = limit.floor() as i32;
        for by in (top - 384..=top).rev() {
            if let Some((_, t)) = self.span(bx, by, bz) {
                if t <= limit + 1e-9 {
                    return t;
                }
            }
        }
        -1.0e6
    }

    fn collide(&self, mut x: f64, mut z: f64, y: f64, r: f64) -> (f64, f64) {
        let (lo, hi) = (y + self.step_height, y + self.height);
        // Twice: being pushed out of one block can push into its neighbour.
        for _ in 0..2 {
            let (x0, x1) = ((x - r).floor() as i32, (x + r).floor() as i32);
            let (z0, z1) = ((z - r).floor() as i32, (z + r).floor() as i32);
            for bz in z0..=z1 {
                for bx in x0..=x1 {
                    let wall = ((y.floor() as i32)..=(hi.floor() as i32)).any(|by| self.span(bx, by, bz).is_some_and(|(a, t)| t > lo + 1e-9 && a < hi));
                    if wall {
                        let b = BoxShape { x0: bx as f64, x1: bx as f64 + 1.0, z0: bz as f64, z1: bz as f64 + 1.0, h: 0.0 };
                        push_out(&mut x, &mut z, r, &b);
                    }
                }
            }
        }
        (x, z)
    }

    fn find_ledge(&self, px: f64, pz: f64, py: f64, base_y: f64, dx: f64, dz: f64, q: &LedgeQuery) -> Option<Ledge> {
        let (cx, cz) = (px.floor() as i32, pz.floor() as i32);
        let mut best: Option<(f64, Ledge)> = None;
        for bz in cz - 1..=cz + 1 {
            for bx in cx - 1..=cx + 1 {
                if bx == cx && bz == cz {
                    continue;
                }
                // Every top in this column at a climbable height with room to stand on it.
                let (y0, y1) = ((base_y + q.min_height).floor() as i32 - 1, (base_y + q.max_height).floor() as i32);
                for by in y0..=y1 {
                    let Some((_, top)) = self.span(bx, by, bz) else { continue };
                    if !self.clear(bx, bz, top + 0.01, top + self.height) {
                        continue;
                    }
                    let b = BoxShape { x0: bx as f64, x1: bx as f64 + 1.0, z0: bz as f64, z1: bz as f64 + 1.0, h: top };
                    let Some((d, l)) = box_ledge(&b, px, pz, py, base_y, dx, dz, q) else { continue };
                    // The way up has to be open too: above the head, and in front of the edge.
                    if !self.clear(cx, cz, py + self.height, top + self.height) {
                        continue;
                    }
                    if best.map_or(true, |(bd, _)| d < bd) {
                        best = Some((d, l));
                    }
                }
            }
        }
        best.map(|(_, l)| l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Blocks from a map; everything else is air, and the world is loaded everywhere.
    struct Grid(HashMap<(i32, i32, i32), (f32, f32)>);

    impl Blocks for Grid {
        fn solid(&self, x: i32, y: i32, z: i32) -> Option<Option<(f32, f32)>> {
            Some(self.0.get(&(x, y, z)).copied())
        }
    }

    /// Flat ground with its top at y = 10, plus the given blocks.
    fn grid(extra: &[((i32, i32, i32), (f32, f32))]) -> Grid {
        let mut m = HashMap::new();
        for z in -8..8 {
            for x in -8..8 {
                m.insert((x, 9, z), (0.0, 1.0));
            }
        }
        m.extend(extra.iter().copied());
        Grid(m)
    }

    const Q: LedgeQuery = LedgeQuery { min_height: 1.4, max_height: 2.55, reach: 0.5, facing: 0.55, stand_in: 0.42, hang_out: 0.5, radius: 0.3 };

    #[test]
    fn half_slabs_are_stepped_onto_and_full_blocks_are_walls() {
        let g = grid(&[((2, 10, 0), (0.0, 0.5)), ((4, 10, 0), (0.0, 1.0))]);
        let w = BlockWorld::new(&g);
        assert_eq!(w.ground_height(0.5, 0.5, 10.0), 10.0);
        assert_eq!(w.ground_height(2.5, 0.5, 10.0), 10.5, "a slab is a step up");
        assert_eq!(w.ground_height(4.5, 0.5, 10.0), 10.0, "a full block is not: the ground under it");
        assert_eq!(w.ground_height(4.5, 0.5, 11.0), 11.0, "from on top it is ground");
        assert_eq!(w.ground_height(0.5, 0.5, 30.0), 10.0, "from high above, the first ground below");
        // The slab does not push; the block does, back to a radius from its side.
        assert_eq!(w.collide(1.9, 0.5, 10.0, 0.3), (1.9, 0.5));
        let (x, z) = w.collide(3.9, 0.5, 10.0, 0.3);
        assert!((x - 3.7).abs() < 1e-9 && z == 0.5, "{x} {z}");
        // Standing on the slab, the block next to it is only half a block up: walkable.
        assert_eq!(w.collide(3.9, 0.5, 10.5, 0.3), (3.9, 0.5));
    }

    #[test]
    fn a_two_block_overhang_can_be_walked_under_but_not_a_one_block_gap() {
        let high = grid(&[((2, 12, 0), (0.0, 1.0))]);
        assert_eq!(BlockWorld::new(&high).collide(2.5, 0.5, 10.0, 0.3), (2.5, 0.5));
        let low = grid(&[((2, 11, 0), (0.0, 1.0))]);
        let (x, _) = BlockWorld::new(&low).collide(1.9, 0.5, 10.0, 0.3);
        assert!((x - 1.7).abs() < 1e-9, "{x}");
    }

    #[test]
    fn a_two_block_wall_is_a_ledge_and_a_one_or_three_block_wall_is_not() {
        for (height, climbable) in [(1, false), (2, true), (3, false)] {
            let wall: Vec<_> = (0..height).map(|k| ((1, 10 + k, 0), (0.0f32, 1.0f32))).collect();
            let g = grid(&wall);
            let w = BlockWorld::new(&g);
            let l = w.find_ledge(0.6, 0.5, 10.0, 10.0, 1.0, 0.0, &Q);
            assert_eq!(l.is_some(), climbable, "{height} blocks");
            if let Some(l) = l {
                assert_eq!((l.top_y, l.nx, l.nz), (12.0, 1.0, 0.0));
                assert!((l.stand_x - 1.42).abs() < 1e-9 && (l.hang_x - 0.5).abs() < 1e-9);
            }
            // Facing away from it: no.
            assert!(w.find_ledge(0.6, 0.5, 10.0, 10.0, -1.0, 0.0, &Q).is_none());
        }
        // No room to stand on top: no.
        let g = grid(&[((1, 10, 0), (0.0, 1.0)), ((1, 11, 0), (0.0, 1.0)), ((1, 13, 0), (0.0, 1.0))]);
        assert!(BlockWorld::new(&g).find_ledge(0.6, 0.5, 10.0, 10.0, 1.0, 0.0, &Q).is_none());
    }

    #[test]
    fn unloaded_world_has_no_ground() {
        struct Nothing;
        impl Blocks for Nothing {
            fn solid(&self, _: i32, _: i32, _: i32) -> Option<Option<(f32, f32)>> {
                None
            }
        }
        let w = BlockWorld::new(&Nothing);
        assert!(!w.loaded(0.0, 0.0, 0.0));
        assert!(w.ground_height(0.0, 0.0, 50.0) < -1.0e5);
    }
}
