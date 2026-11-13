//! Just enough matrix math for a camera. Matrices are column-major `[f32; 16]`,
//! matching how WGSL reads a `mat4x4<f32>` from a buffer.

pub type Mat4 = [f32; 16];

pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l == 0.0 {
        [0.0, 0.0, -1.0]
    } else {
        [a[0] / l, a[1] / l, a[2] / l]
    }
}

pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [0.0f32; 16];
    for c in 0..4 {
        for row in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k * 4 + row] * b[c * 4 + k];
            }
            r[c * 4 + row] = s;
        }
    }
    r
}

pub fn transform(m: &Mat4, v: [f32; 4]) -> [f32; 4] {
    let mut r = [0.0f32; 4];
    for row in 0..4 {
        for k in 0..4 {
            r[row] += m[k * 4 + row] * v[k];
        }
    }
    r
}

/// Right-handed, depth range 0..1 (wgpu clip space).
pub fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fovy * 0.5).tan();
    let mut m = [0.0f32; 16];
    m[0] = f / aspect;
    m[5] = f;
    m[10] = far / (near - far);
    m[11] = -1.0;
    m[14] = near * far / (near - far);
    m
}

pub fn look_dir(eye: [f32; 3], forward: [f32; 3]) -> Mat4 {
    let f = normalize(forward);
    let s = normalize(cross(f, [0.0, 1.0, 0.0]));
    let u = cross(s, f);
    [
        s[0], u[0], -f[0], 0.0,
        s[1], u[1], -f[1], 0.0,
        s[2], u[2], -f[2], 0.0,
        -dot(s, eye), -dot(u, eye), dot(f, eye), 1.0,
    ]
}

/// Reverse-Z perspective with no far plane: depth is 1 at `near` and falls towards 0 with
/// distance (`near / distance`). With a 32-bit float depth buffer this spreads precision
/// evenly over the whole range, so terrain 30 km away still sorts correctly, which a
/// standard projection with a far plane that far would not manage.
pub fn perspective_reverse_z(fovy: f32, aspect: f32, near: f32) -> Mat4 {
    let f = 1.0 / (fovy * 0.5).tan();
    let mut m = [0.0f32; 16];
    m[0] = f / aspect;
    m[5] = f;
    m[11] = -1.0;
    m[14] = near;
    m
}

/// Clear value and comparison that go with `perspective_reverse_z`.
pub const DEPTH_CLEAR: f32 = 0.0;

pub fn view_proj(eye: [f32; 3], forward: [f32; 3], aspect: f32) -> Mat4 {
    mul(&perspective_reverse_z(1.1, aspect, 0.1), &look_dir(eye, forward))
}

/// Six clip planes extracted from a view-projection matrix (Gribb-Hartmann), pointing
/// inward and normalised so distances are in world units.
#[derive(Clone, Copy, Debug)]
pub struct Frustum {
    planes: [[f32; 4]; 6],
}

fn add4(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}

fn sub4(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]]
}

impl Frustum {
    /// `m` is column-major with wgpu clip conventions (x, y in -w..w, z in 0..w).
    pub fn from_view_proj(m: &Mat4) -> Frustum {
        let row = |r: usize| [m[r], m[4 + r], m[8 + r], m[12 + r]];
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
        let mut planes = [
            add4(r3, r0), // left:   x >= -w
            sub4(r3, r0), // right:  x <= w
            add4(r3, r1), // bottom: y >= -w
            sub4(r3, r1), // top:    y <= w
            r2,           // near:   z >= 0
            sub4(r3, r2), // far:    z <= w
        ];
        for p in planes.iter_mut() {
            let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            if len > 0.0 {
                for v in p.iter_mut() {
                    *v /= len;
                }
            }
        }
        Frustum { planes }
    }

    /// False only if the box is entirely outside at least one plane. Conservative: it may
    /// keep a box that is just outside a frustum corner, but never drops a visible one.
    pub fn contains_aabb(&self, min: [f32; 3], max: [f32; 3]) -> bool {
        for p in &self.planes {
            // The box corner furthest along the plane normal.
            let x = if p[0] >= 0.0 { max[0] } else { min[0] };
            let y = if p[1] >= 0.0 { max[1] } else { min[1] };
            let z = if p[2] >= 0.0 { max[2] } else { min[2] };
            if p[0] * x + p[1] * y + p[2] * z + p[3] < 0.0 {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(m: &Mat4, p: [f32; 3]) -> [f32; 4] {
        transform(m, [p[0], p[1], p[2], 1.0])
    }

    #[test]
    fn point_ahead_is_centered_and_inside_depth_range() {
        let m = view_proj([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], 2.0);
        let c = project(&m, [0.0, 0.0, -5.0]);
        assert!(c[3] > 0.0);
        assert!((c[0] / c[3]).abs() < 1e-5 && (c[1] / c[3]).abs() < 1e-5);
        let z = c[2] / c[3];
        assert!((0.0..=1.0).contains(&z), "z {z}");
    }

    #[test]
    fn depth_is_reversed_and_never_clipped_far_away() {
        let m = view_proj([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], 1.0);
        let depth = |d: f32| {
            let c = project(&m, [0.0, 0.0, -d]);
            c[2] / c[3]
        };
        assert!((depth(0.1) - 1.0).abs() < 1e-5, "near plane maps to 1");
        let mut last = 1.0;
        for d in [1.0, 10.0, 300.0, 5000.0, 40_000.0] {
            let z = depth(d);
            assert!(z > 0.0 && z < last, "depth {z} at {d}");
            last = z;
        }
        // Neighbouring surfaces 30 km out still get distinct depths in an f32 buffer.
        assert!(depth(30_000.0) != depth(30_001.0));
        // The far "plane" of the frustum never rejects anything in front of the camera.
        let f = Frustum::from_view_proj(&m);
        assert!(f.contains_aabb([-1.0, -1.0, -60_001.0], [1.0, 1.0, -60_000.0]));
        assert!(!f.contains_aabb([-1.0, -1.0, 5.0], [1.0, 1.0, 6.0]));
    }

    #[test]
    fn point_behind_has_negative_w() {
        let m = view_proj([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], 1.0);
        assert!(project(&m, [0.0, 0.0, 5.0])[3] < 0.0);
    }

    #[test]
    fn right_is_right_and_up_is_up() {
        let m = view_proj([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], 1.0);
        let r = project(&m, [1.0, 0.0, -5.0]);
        assert!(r[0] / r[3] > 0.0, "+X should be on the right");
        let u = project(&m, [0.0, 1.0, -5.0]);
        assert!(u[1] / u[3] > 0.0, "+Y should be up");
    }

    #[test]
    fn counter_clockwise_triangles_facing_the_camera_have_positive_ndc_area() {
        // wgpu's `FrontFace::Ccw` means positive signed area in y-up NDC.
        let m = view_proj([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], 1.0);
        let p = |x: f32, y: f32| {
            let c = transform(&m, [x, y, -5.0, 1.0]);
            [c[0] / c[3], c[1] / c[3]]
        };
        // A triangle in the plane z = -5 with its normal toward +Z (toward the camera),
        // counter-clockwise as the camera sees it.
        let (a, b, c) = (p(-1.0, -1.0), p(1.0, -1.0), p(0.0, 1.0));
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        assert!(area > 0.0, "area {area}");
    }

    fn cam_at(eye: [f32; 3], yaw: f32, pitch: f32) -> (Mat4, Frustum) {
        let fwd = [-yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos()];
        let vp = view_proj(eye, fwd, 16.0 / 9.0);
        (vp, Frustum::from_view_proj(&vp))
    }

    #[test]
    fn frustum_accepts_what_is_ahead_and_rejects_what_is_not() {
        let (_, f) = cam_at([0.0, 0.0, 0.0], 0.0, 0.0); // looks down -Z
        let check = |c: [f32; 3], h: f32| {
            f.contains_aabb([c[0] - h, c[1] - h, c[2] - h], [c[0] + h, c[1] + h, c[2] + h])
        };
        assert!(check([0.0, 0.0, -20.0], 2.0), "straight ahead");
        assert!(!check([0.0, 0.0, 20.0], 2.0), "behind");
        assert!(!check([-80.0, 0.0, -20.0], 2.0), "far left");
        assert!(!check([80.0, 0.0, -20.0], 2.0), "far right");
        assert!(!check([0.0, 80.0, -20.0], 2.0), "far above");
        assert!(!check([0.0, -80.0, -20.0], 2.0), "far below");
        // No far plane (reverse-Z): level-of-detail terrain is drawn tens of km out.
        assert!(check([0.0, 0.0, -30_000.0], 2.0), "far ahead stays visible");
        assert!(check([-14.0, 0.0, -20.0], 6.0), "straddling the left edge stays visible");
        assert!(check([0.0, 0.0, 0.0], 16.0), "a box containing the camera stays visible");
    }

    #[test]
    fn frustum_never_rejects_a_box_with_a_visible_corner() {
        let eye = [3.0, 4.0, -2.0];
        for &(yaw, pitch) in &[(0.0f32, 0.0f32), (0.7, 0.3), (-2.0, -0.5), (3.0, 1.0)] {
            let (vp, f) = cam_at(eye, yaw, pitch);
            let mut state = 12345u32;
            let mut next = || {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 8) as f32 / 16_777_216.0
            };
            let mut rejected = 0;
            for _ in 0..4000 {
                let min = [
                    eye[0] + (next() - 0.5) * 200.0,
                    eye[1] + (next() - 0.5) * 200.0,
                    eye[2] + (next() - 0.5) * 200.0,
                ];
                let size = 1.0 + next() * 40.0;
                let max = [min[0] + size, min[1] + size, min[2] + size];
                let corner_visible = (0..8).any(|i| {
                    let p = [
                        if i & 1 == 0 { min[0] } else { max[0] },
                        if i & 2 == 0 { min[1] } else { max[1] },
                        if i & 4 == 0 { min[2] } else { max[2] },
                    ];
                    let c = transform(&vp, [p[0], p[1], p[2], 1.0]);
                    // Strictly inside by a small margin, so float noise exactly on a plane
                    // can't make this check flaky.
                    c[3] > 0.0
                        && c[0].abs() <= c[3] * 0.999
                        && c[1].abs() <= c[3] * 0.999
                        && c[2] >= c[3] * 0.001
                        && c[2] <= c[3] * 0.999
                });
                let kept = f.contains_aabb(min, max);
                if corner_visible {
                    assert!(kept, "visible box rejected: yaw {yaw} pitch {pitch} min {min:?} size {size}");
                }
                if !kept {
                    rejected += 1;
                }
            }
            assert!(rejected > 1600, "culling should drop most random boxes, dropped {rejected}/4000");
        }
    }

    #[test]
    fn most_of_a_chunk_ring_is_culled_but_the_camera_chunk_never_is() {
        // Eye at the centre of chunk (0, 0, 0).
        let (_, f) = cam_at([16.0, 16.0, 16.0], 0.3, -0.2);
        let (mut visible, mut total) = (0, 0);
        for cx in -6..=6 {
            for cy in -2..=2 {
                for cz in -6..=6 {
                    let lo = [cx as f32 * 32.0, cy as f32 * 32.0, cz as f32 * 32.0];
                    let hi = [lo[0] + 32.0, lo[1] + 32.0, lo[2] + 32.0];
                    let kept = f.contains_aabb(lo, hi);
                    total += 1;
                    if kept {
                        visible += 1;
                    }
                    if (cx, cy, cz) == (0, 0, 0) {
                        assert!(kept, "the chunk the camera is in must be drawn");
                    }
                }
            }
        }
        assert!(visible > 0 && visible * 100 < total * 45, "{visible}/{total} chunks kept");
    }

    #[test]
    fn near_plane_maps_to_zero_and_far_to_one() {
        let p = perspective(1.0, 1.0, 0.1, 100.0);
        let n = transform(&p, [0.0, 0.0, -0.1, 1.0]);
        let f = transform(&p, [0.0, 0.0, -100.0, 1.0]);
        assert!((n[2] / n[3]).abs() < 1e-4);
        assert!((f[2] / f[3] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn translation_moves_the_world_opposite_to_the_camera() {
        let m = view_proj([10.0, 0.0, 0.0], [0.0, 0.0, -1.0], 1.0);
        // A point at the camera's x is dead centre.
        let c = project(&m, [10.0, 0.0, -5.0]);
        assert!((c[0] / c[3]).abs() < 1e-5);
    }
}
