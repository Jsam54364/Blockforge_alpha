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

pub fn view_proj(eye: [f32; 3], forward: [f32; 3], aspect: f32) -> Mat4 {
    mul(&perspective(1.1, aspect, 0.1, 1000.0), &look_dir(eye, forward))
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
