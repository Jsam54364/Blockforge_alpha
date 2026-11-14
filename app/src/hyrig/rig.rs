//! The HyRig model and its clips, and posing it: what the kit's `hyrig_character.gd` does.
//!
//! The model is a tree of rigid parts (pelvis, chest, arms, head, eyelids...), each with its
//! own few triangles; clips turn and move the parts. `Rig::pose` blends the clips the
//! controller asks for into one pose (legs from the body clips; upper body from the body
//! clips faded against the upper-body action; the carry pose over its arm bones; then the
//! head turn and the face), and `Rig::triangles` gives the posed model as world-space
//! triangles, ready to draw.
//!
//! The data is `assets/hyrig/hyrig.bin`, written by `tools/convert_hyrig.py` (format there).

use std::collections::HashMap;

use super::config::Config;
use super::controller::{Layer, Output, View};

const BIN: &[u8] = include_bytes!("../../../assets/hyrig/hyrig.bin");
/// The model's texture (RGBA PNG, 256 x 256; drawn unfiltered, with alpha as a cut-out).
pub const TEXTURE_PNG: &[u8] = include_bytes!("../../../assets/hyrig/hyrig.png");
/// Texels more see-through than this are not drawn (the model's own cut-off).
pub const ALPHA_CUTOFF: f32 = 0.64;

type Quat = [f32; 4]; // x y z w
type Vec3 = [f32; 3];
/// Rotation and scale (columns) and translation.
#[derive(Clone, Copy, Debug)]
struct Mat {
    c: [Vec3; 3],
    t: Vec3,
}

fn q_mul(a: Quat, b: Quat) -> Quat {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

fn q_axis(axis: Vec3, angle: f32) -> Quat {
    let (s, c) = (angle * 0.5).sin_cos();
    [axis[0] * s, axis[1] * s, axis[2] * s, c]
}

/// From a to b by t, the short way round.
fn q_slerp(a: Quat, mut b: Quat, t: f32) -> Quat {
    let mut d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    if d < 0.0 {
        b = [-b[0], -b[1], -b[2], -b[3]];
        d = -d;
    }
    let (wa, wb) = if d > 0.9995 {
        (1.0 - t, t)
    } else {
        let th = d.clamp(-1.0, 1.0).acos();
        let s = th.sin();
        (((1.0 - t) * th).sin() / s, (t * th).sin() / s)
    };
    let q = [a[0] * wa + b[0] * wb, a[1] * wa + b[1] * wb, a[2] * wa + b[2] * wb, a[3] * wa + b[3] * wb];
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt().max(1e-12);
    [q[0] / n, q[1] / n, q[2] / n, q[3] / n]
}

fn lerp3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn q_rotate(q: Quat, v: Vec3) -> Vec3 {
    let u = [q[0], q[1], q[2]];
    let uv = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let uuv = [u[1] * uv[2] - u[2] * uv[1], u[2] * uv[0] - u[0] * uv[2], u[0] * uv[1] - u[1] * uv[0]];
    [v[0] + 2.0 * (q[3] * uv[0] + uuv[0]), v[1] + 2.0 * (q[3] * uv[1] + uuv[1]), v[2] + 2.0 * (q[3] * uv[2] + uuv[2])]
}

impl Mat {
    fn trs(t: Vec3, r: Quat, s: Vec3) -> Mat {
        let col = |axis: Vec3, k: f32| {
            let v = q_rotate(r, axis);
            [v[0] * k, v[1] * k, v[2] * k]
        };
        Mat { c: [col([1.0, 0.0, 0.0], s[0]), col([0.0, 1.0, 0.0], s[1]), col([0.0, 0.0, 1.0], s[2])], t }
    }

    fn dir(&self, v: Vec3) -> Vec3 {
        [
            self.c[0][0] * v[0] + self.c[1][0] * v[1] + self.c[2][0] * v[2],
            self.c[0][1] * v[0] + self.c[1][1] * v[1] + self.c[2][1] * v[2],
            self.c[0][2] * v[0] + self.c[1][2] * v[1] + self.c[2][2] * v[2],
        ]
    }

    fn point(&self, v: Vec3) -> Vec3 {
        let d = self.dir(v);
        [d[0] + self.t[0], d[1] + self.t[1], d[2] + self.t[2]]
    }

    /// self * other: `other` happens first.
    fn then(&self, other: &Mat) -> Mat {
        Mat { c: [self.dir(other.c[0]), self.dir(other.c[1]), self.dir(other.c[2])], t: self.point(other.t) }
    }
}

struct Node {
    name: String,
    parent: Option<usize>,
    t: Vec3,
    r: Quat,
    s: Vec3,
    /// This part's triangles: a range of `Rig::vertices`.
    verts: std::ops::Range<usize>,
}

/// A vertex in its part's own space.
#[derive(Clone, Copy)]
struct LocalVertex {
    pos: Vec3,
    normal: Vec3,
    uv: [f32; 2],
}

/// One animated property of one node: keys at `times`, 3 (position) or 4 (rotation) floats each.
struct Channel {
    times: Vec<f32>,
    values: Vec<f32>,
}

impl Channel {
    /// (key before, key after, how far between) at time t.
    fn at(&self, t: f32) -> (usize, usize, f32) {
        let n = self.times.len();
        let i = self.times.partition_point(|k| *k <= t);
        if i == 0 {
            return (0, 0, 0.0);
        }
        if i >= n {
            return (n - 1, n - 1, 0.0);
        }
        let (a, b) = (self.times[i - 1], self.times[i]);
        (i - 1, i, if b > a { (t - a) / (b - a) } else { 0.0 })
    }

    fn vec3(&self, t: f32) -> Vec3 {
        let (i, j, f) = self.at(t);
        let k = |n: usize| [self.values[n * 3], self.values[n * 3 + 1], self.values[n * 3 + 2]];
        lerp3(k(i), k(j), f)
    }

    fn quat(&self, t: f32) -> Quat {
        let (i, j, f) = self.at(t);
        let k = |n: usize| [self.values[n * 4], self.values[n * 4 + 1], self.values[n * 4 + 2], self.values[n * 4 + 3]];
        q_slerp(k(i), k(j), f)
    }
}

#[derive(Default)]
struct Clip {
    duration: f32,
    /// node -> channel
    pos: HashMap<usize, Channel>,
    rot: HashMap<usize, Channel>,
}

/// A vertex of the posed model, in the world: what the renderer draws (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// Where every part is for one frame.
pub struct Pose {
    world: Vec<Mat>,
    mouth_uv: [f32; 2],
    first_person: bool,
    head: Option<usize>,
}

impl Pose {
    /// The head's place in the world (for the first-person camera).
    pub fn head_position(&self) -> Option<[f64; 3]> {
        self.head.map(|h| {
            let t = self.world[h].t;
            [t[0] as f64, t[1] as f64, t[2] as f64]
        })
    }
}

pub struct Rig {
    nodes: Vec<Node>,
    vertices: Vec<LocalVertex>,
    clips: HashMap<String, Clip>,
    index: HashMap<String, usize>,
    lower: Vec<usize>,
    upper: Vec<usize>,
    pupils: Vec<usize>,
    lids: Vec<usize>,
    brows: Vec<usize>,
    mouth: Option<usize>,
    head: Option<usize>,
    /// Parts the first-person camera must not see: head, neck and the face.
    hidden_first: Vec<bool>,
    yaw_offset: f32,
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.data.len()).ok_or("hyrig.bin ends early")?;
        let s = &self.data[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    fn floats(&mut self, n: usize) -> Result<Vec<f32>, String> {
        let b = self.bytes(n.checked_mul(4).ok_or("hyrig.bin: bad count")?)?;
        Ok(b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect())
    }

    fn name(&mut self) -> Result<String, String> {
        let n = self.u8()? as usize;
        String::from_utf8(self.bytes(n)?.to_vec()).map_err(|e| e.to_string())
    }
}

impl Rig {
    /// The character shipped in assets/hyrig.
    pub fn builtin(cfg: &Config) -> Rig {
        Rig::parse(BIN, cfg).expect("assets/hyrig/hyrig.bin")
    }

    pub fn parse(data: &[u8], cfg: &Config) -> Result<Rig, String> {
        let mut r = Reader { data, at: 0 };
        if r.bytes(8)? != b"HYRIG1\0\0" {
            return Err("not a hyrig.bin".into());
        }
        let mut nodes = Vec::new();
        let mut vertices = Vec::new();
        for i in 0..r.u32()? as usize {
            let name = r.name()?;
            let parent = i32::from_le_bytes(r.bytes(4)?.try_into().unwrap());
            let f = r.floats(10)?;
            let start = vertices.len();
            let count = r.u32()? as usize;
            for v in r.floats(count.checked_mul(8).ok_or("hyrig.bin: bad count")?)?.chunks_exact(8) {
                vertices.push(LocalVertex { pos: [v[0], v[1], v[2]], normal: [v[3], v[4], v[5]], uv: [v[6], v[7]] });
            }
            if parent >= 0 && parent as usize == i {
                return Err("hyrig.bin: a node is its own parent".into());
            }
            nodes.push(Node {
                name,
                parent: (parent >= 0).then_some(parent as usize),
                t: [f[0], f[1], f[2]],
                r: [f[3], f[4], f[5], f[6]],
                s: [f[7], f[8], f[9]],
                verts: start..vertices.len(),
            });
        }
        if nodes.iter().any(|n| n.parent.is_some_and(|p| p >= nodes.len())) {
            return Err("hyrig.bin: bad parent".into());
        }
        let mut clips = HashMap::new();
        for _ in 0..r.u32()? {
            let name = r.name()?;
            let mut clip = Clip { duration: r.f32()?, ..Clip::default() };
            for _ in 0..r.u32()? {
                let node = r.u16()? as usize;
                let path = r.u8()?;
                let keys = r.u32()? as usize;
                let times = r.floats(keys)?;
                let values = r.floats(keys * if path == 0 { 3 } else { 4 })?;
                if node >= nodes.len() || keys == 0 {
                    return Err("hyrig.bin: bad channel".into());
                }
                let ch = Channel { times, values };
                if path == 0 { &mut clip.pos } else { &mut clip.rot }.insert(node, ch);
            }
            clips.insert(name, clip);
        }
        let index: HashMap<String, usize> = nodes.iter().enumerate().map(|(i, n)| (n.name.clone(), i)).collect();
        let find = |names: &[String]| names.iter().filter_map(|n| index.get(n).copied()).collect::<Vec<usize>>();
        let head = index.get(&cfg.first_bone).copied();
        // Hidden in first person: the listed parts, and everything attached to the head.
        let listed = find(&cfg.first_hide);
        let hidden_first = (0..nodes.len())
            .map(|i| {
                let mut at = Some(i);
                let mut under_head = false;
                while let Some(n) = at {
                    under_head |= Some(n) == head;
                    at = nodes[n].parent;
                }
                under_head || listed.contains(&i)
            })
            .collect();
        Ok(Rig {
            lower: find(&cfg.lower_bones),
            upper: find(&cfg.upper_bones),
            pupils: find(&cfg.pupils),
            lids: find(&cfg.lids),
            brows: find(&cfg.brows),
            mouth: index.get(&cfg.mouth).copied(),
            head,
            hidden_first,
            yaw_offset: cfg.yaw_offset as f32,
            nodes,
            vertices,
            clips,
            index,
        })
    }

    pub fn clip_names(&self) -> impl Iterator<Item = &str> {
        self.clips.keys().map(String::as_str)
    }

    pub fn clip_duration(&self, name: &str) -> Option<f32> {
        self.clips.get(name).map(|c| c.duration)
    }

    pub fn has_node(&self, name: &str) -> bool {
        self.index.contains_key(name)
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    /// A node's position and rotation in a clip at a time (its rest pose where the clip does
    /// not move it).
    fn sample(&self, clip: &str, node: usize, time: f64) -> (Vec3, Quat) {
        let n = &self.nodes[node];
        let Some(c) = self.clips.get(clip) else { return (n.t, n.r) };
        let t = (time as f32).min(c.duration);
        (c.pos.get(&node).map_or(n.t, |ch| ch.vec3(t)), c.rot.get(&node).map_or(n.r, |ch| ch.quat(t)))
    }

    fn blend(&self, node: usize, body: &[Layer], upper: Option<&Layer>, hold: Option<&Layer>, look: Option<[f64; 2]>) -> (Vec3, Quat) {
        let (mut p, mut q) = (self.nodes[node].t, self.nodes[node].r);
        let mut acc = 0.0f64;
        for b in body.iter().filter(|b| b.weight > 0.0) {
            let (sp, sq) = self.sample(&b.clip, node, b.time);
            acc += b.weight;
            if acc == b.weight {
                (p, q) = (sp, sq);
            } else {
                let k = (b.weight / acc) as f32;
                q = q_slerp(q, sq, k);
                p = lerp3(p, sp, k);
            }
        }
        if let Some(u) = upper.filter(|u| u.weight > 0.0) {
            // The upper-body action takes over from the movement clip.
            let (sp, sq) = self.sample(&u.clip, node, u.time);
            q = q_slerp(q, sq, u.weight as f32);
            p = lerp3(p, sp, u.weight as f32);
        }
        if let Some(h) = hold.filter(|h| h.weight > 0.0) {
            // Carry pose over whatever the arm was doing.
            q = q_slerp(q, self.sample(&h.clip, node, h.time).1, h.weight as f32);
        }
        if let Some([yaw, pitch]) = look {
            // Head turn toward the look direction, on top of the animation (yaw, then pitch).
            q = q_mul(q, q_mul(q_axis([0.0, 1.0, 0.0], yaw as f32), q_axis([1.0, 0.0, 0.0], pitch as f32)));
        }
        (p, q)
    }

    /// The pose for one frame of controller output, placed in the world. `origin` is taken
    /// off the position first (keep it near the character so 32-bit floats stay exact).
    pub fn pose(&self, out: &Output, origin: [f64; 3]) -> Pose {
        let n = self.nodes.len();
        let mut local: Vec<(Vec3, Quat, Vec3)> = self.nodes.iter().map(|n| (n.t, n.r, n.s)).collect();
        for &i in &self.lower {
            let (p, q) = self.blend(i, &out.body, None, None, None);
            (local[i].0, local[i].1) = (p, q);
        }
        for &i in &self.upper {
            let hold = out.hold.as_ref().filter(|(_, bones)| bones.iter().any(|b| *b == self.nodes[i].name)).map(|(l, _)| l);
            let look = out.head_look.filter(|_| Some(i) == self.head);
            let (p, q) = self.blend(i, &out.body, out.upper.as_ref(), hold, look);
            (local[i].0, local[i].1) = (p, q);
        }
        // Face.
        let f = &out.face;
        for &i in &self.pupils {
            local[i].0[0] += f.pupil[0] as f32;
            local[i].0[1] += f.pupil[1] as f32;
        }
        for &i in &self.lids {
            local[i].2[1] = f.lid as f32;
        }
        for &i in &self.brows {
            let side = if self.nodes[i].t[0] > 0.0 { 1.0 } else { -1.0 };
            local[i].0[1] += f.brow_lift as f32;
            local[i].1 = q_mul(local[i].1, q_axis([0.0, 0.0, 1.0], side * f.brow_tilt as f32));
        }
        // The model is authored facing -Z; turn it so yaw 0 faces +Z, then to the body's yaw.
        let at = [(out.position[0] - origin[0]) as f32, (out.position[1] - origin[1]) as f32, (out.position[2] - origin[2]) as f32];
        let root = Mat::trs(at, q_axis([0.0, 1.0, 0.0], out.yaw as f32 + self.yaw_offset), [1.0; 3]);
        let mut world: Vec<Option<Mat>> = vec![None; n];
        for i in 0..n {
            self.resolve(i, &local, &root, &mut world);
        }
        Pose {
            world: world.into_iter().map(|m| m.unwrap()).collect(),
            mouth_uv: [f.mouth_uv[0] as f32, f.mouth_uv[1] as f32],
            first_person: out.view == View::First,
            head: self.head,
        }
    }

    fn resolve(&self, i: usize, local: &[(Vec3, Quat, Vec3)], root: &Mat, world: &mut Vec<Option<Mat>>) -> Mat {
        if let Some(m) = world[i] {
            return m;
        }
        let (t, r, s) = local[i];
        let own = Mat::trs(t, r, s);
        let m = match self.nodes[i].parent {
            Some(p) => self.resolve(p, local, root, world).then(&own),
            None => root.then(&own),
        };
        world[i] = Some(m);
        m
    }

    /// The posed model's triangles (three vertices each), appended to `out`. In first person
    /// the head, neck and face are left out, so the camera inside the head sees the body.
    pub fn triangles(&self, pose: &Pose, out: &mut Vec<Vertex>) {
        for (i, node) in self.nodes.iter().enumerate() {
            if pose.first_person && self.hidden_first[i] {
                continue;
            }
            let m = &pose.world[i];
            let shift = if Some(i) == self.mouth { pose.mouth_uv } else { [0.0, 0.0] };
            for v in &self.vertices[node.verts.clone()] {
                let n = m.dir(v.normal);
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
                out.push(Vertex { pos: m.point(v.pos), normal: [n[0] / len, n[1] / len, n[2] / len], uv: [v.uv[0] + shift[0], v.uv[1] + shift[1]] });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hyrig::controller::Face;

    fn output(body: Vec<Layer>) -> Output {
        Output {
            position: [0.0; 3],
            yaw: 0.0,
            body,
            upper: None,
            hold: None,
            head_look: None,
            face: Face { lid: 0.001, pupil: [0.0; 2], brow_lift: 0.0, brow_tilt: 0.0, mouth: "closed", mouth_uv: [0.0; 2] },
            events: Vec::new(),
            view: View::Follow,
            state: "idle".into(),
            clip_time: 0.0,
            grounded: true,
            speed: 0.0,
        }
    }

    fn layer(clip: &str, time: f64, weight: f64) -> Layer {
        Layer { clip: clip.into(), time, weight }
    }

    fn bounds(v: &[Vertex]) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in v {
            for k in 0..3 {
                lo[k] = lo[k].min(p.pos[k]);
                hi[k] = hi[k].max(p.pos[k]);
            }
        }
        (lo, hi)
    }

    #[test]
    fn the_model_and_every_clip_the_state_machine_names_are_there() {
        let cfg = Config::builtin();
        let rig = Rig::builtin(&cfg);
        assert_eq!(rig.clip_names().count(), 51);
        for (name, clip) in &cfg.clips {
            let d = rig.clip_duration(name).unwrap_or_else(|| panic!("clip {name} is missing"));
            assert!((d as f64 - clip.duration).abs() < 0.02, "{name}: {d} vs {}", clip.duration);
        }
        for role in cfg.roles.values().filter(|c| !c.is_empty()) {
            assert!(rig.clip_duration(role).is_some(), "role clip {role}");
        }
        for bone in cfg.upper_bones.iter().chain(&cfg.lower_bones).chain(&cfg.pupils).chain(&cfg.lids).chain(&cfg.brows) {
            assert!(rig.has_node(bone), "bone {bone}");
        }
        assert!(rig.has_node(&cfg.mouth) && rig.has_node(&cfg.first_bone));
        assert!(rig.vertex_count() % 3 == 0 && rig.vertex_count() > 600);
    }

    #[test]
    fn standing_idle_he_is_1_8_tall_feet_on_the_ground_facing_plus_z() {
        let cfg = Config::builtin();
        let rig = Rig::builtin(&cfg);
        let pose = rig.pose(&output(vec![layer("idle", 0.0, 1.0)]), [0.0; 3]);
        let mut v = Vec::new();
        rig.triangles(&pose, &mut v);
        assert_eq!(v.len(), rig.vertex_count());
        let (lo, hi) = bounds(&v);
        assert!(lo[1].abs() < 0.03, "feet at {}", lo[1]);
        assert!((1.7..1.95).contains(&hi[1]), "height {}", hi[1]);
        assert!(lo[0] < -0.2 && hi[0] > 0.2 && (lo[0] + hi[0]).abs() < 0.1, "centred: {lo:?} {hi:?}");
        // The face is on the +Z side: yaw 0 faces +Z.
        let head = pose.head_position().unwrap();
        assert!(head[1] > 1.3);
        let face_z = rig.nodes[rig.mouth.unwrap()].verts.clone().map(|i| v[i].pos[2]).fold(f32::MIN, f32::max);
        assert!(face_z > head[2] as f32 + 0.1, "mouth at z {face_z}, head at {}", head[2]);
        for p in &v {
            let n = p.normal;
            assert!(((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() - 1.0).abs() < 1e-3);
        }
    }

    #[test]
    fn yaw_position_and_first_person_are_applied() {
        let cfg = Config::builtin();
        let rig = Rig::builtin(&cfg);
        let mut o = output(vec![layer("idle", 0.0, 1.0)]);
        o.position = [100_000.0, 64.0, -50_000.0];
        o.yaw = std::f64::consts::FRAC_PI_2; // facing +X
        let pose = rig.pose(&o, [100_000.0, 64.0, -50_000.0]);
        let mut v = Vec::new();
        rig.triangles(&pose, &mut v);
        let face_x = rig.nodes[rig.mouth.unwrap()].verts.clone().map(|i| v[i].pos[0]).fold(f32::MIN, f32::max);
        assert!(face_x > 0.15, "the face points along +X: {face_x}");
        let (lo, _) = bounds(&v);
        assert!(lo[1].abs() < 0.03, "relative to the origin given");
        o.view = View::First;
        let mut fp = Vec::new();
        rig.triangles(&rig.pose(&o, [100_000.0, 64.0, -50_000.0]), &mut fp);
        let (_, hi) = bounds(&fp);
        assert!(fp.len() < v.len() - 150 && hi[1] < 1.5, "no head or neck in first person: top at {}", hi[1]);
    }

    #[test]
    fn clips_layers_and_the_face_move_the_right_parts() {
        let cfg = Config::builtin();
        let rig = Rig::builtin(&cfg);
        let tri = |o: &Output| {
            let mut v = Vec::new();
            rig.triangles(&rig.pose(o, [0.0; 3]), &mut v);
            v
        };
        let part = |v: &[Vertex], name: &str| v[rig.nodes[rig.index[name]].verts.clone()].to_vec();
        let moved = |a: &[Vertex], b: &[Vertex]| a.iter().zip(b).map(|(p, q)| (0..3).map(|k| (p.pos[k] - q.pos[k]).abs()).fold(0.0, f32::max)).fold(0.0, f32::max);
        let idle = output(vec![layer("idle", 0.0, 1.0)]);
        let base = tri(&idle);
        // A stride moves the feet; half-and-half with idle moves them less.
        let walk = tri(&output(vec![layer("walk", 0.3, 1.0)]));
        let half = tri(&output(vec![layer("idle", 0.0, 0.5), layer("walk", 0.3, 0.5)]));
        let full = moved(&part(&base, "left_foot"), &part(&walk, "left_foot")).max(moved(&part(&base, "right_foot"), &part(&walk, "right_foot")));
        let mid = moved(&part(&base, "left_foot"), &part(&half, "left_foot")).max(moved(&part(&base, "right_foot"), &part(&half, "right_foot")));
        assert!(full > 0.1 && mid > 0.02 && mid < full, "{full} {mid}");
        // An upper-body action moves an arm and leaves the legs alone.
        let mut punch = idle.clone();
        punch.upper = Some(layer("punch", 0.3, 1.0));
        let p = tri(&punch);
        let arms = moved(&part(&base, "left_hand"), &part(&p, "left_hand")).max(moved(&part(&base, "right_hand"), &part(&p, "right_hand")));
        assert!(arms > 0.1, "{arms}");
        assert!(moved(&part(&base, "left_foot"), &part(&p, "left_foot")) < 1e-5);
        // The carry pose moves only the bones it lists.
        let mut hold = idle.clone();
        hold.hold = Some((layer("hold_1h", 0.0, 1.0), cfg.hold_states["one_handed"].bones.clone()));
        let h = tri(&hold);
        assert!(moved(&part(&base, "right_hand"), &part(&h, "right_hand")) > 0.05);
        assert!(moved(&part(&base, "left_hand"), &part(&h, "left_hand")) < 1e-5);
        // Looking left turns the head and what is attached to it, not the chest.
        let mut look = idle.clone();
        look.head_look = Some([0.8, 0.0]);
        let l = tri(&look);
        assert!(moved(&part(&base, "mouth"), &part(&l, "mouth")) > 0.05);
        assert!(moved(&part(&base, "chest"), &part(&l, "chest")) < 1e-5);
        // Closing the eyes grows the lids; a mouth frame shifts only the mouth's texture.
        let mut face = idle.clone();
        face.face.lid = 1.0;
        face.face.mouth_uv = [0.07421875, -0.03125];
        let f = tri(&face);
        let height = |v: &[Vertex]| bounds(v).1[1] - bounds(v).0[1];
        assert!(height(&part(&f, "left_lid")) > height(&part(&base, "left_lid")) * 50.0);
        assert!((part(&f, "mouth")[0].uv[0] - part(&base, "mouth")[0].uv[0] - 0.07421875).abs() < 1e-6);
        assert_eq!(part(&f, "head")[0].uv, part(&base, "head")[0].uv);
    }
}
