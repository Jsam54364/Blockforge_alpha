//! The player's character in the block world: the HyRig controller fed from the touch
//! controls, standing on the client's blocks, posed for the renderer, with its camera.

use blockforge::client::Client;
use blockforge::coords::BlockPos;

use crate::hyrig::rig::Vertex;
use crate::hyrig::{BlockWorld, Blocks, Config, Controller, Input, Rig, View, World};

/// Half slabs are walked up and down; a full block needs a jump (Minecraft's step is 0.6).
const STEP: f64 = 0.6;
/// How far the eyes are above the feet, for placing the camera before the character exists.
const EYE: f64 = 1.6;
/// Falling out of the world (through unloaded or dug-out ground) puts him back on top.
const VOID_Y: f64 = -300.0;

struct ClientBlocks<'a>(&'a Client);

impl Blocks for ClientBlocks<'_> {
    fn solid(&self, x: i32, y: i32, z: i32) -> Option<Option<(f32, f32)>> {
        self.0.solid_span(BlockPos { x, y, z })
    }
}

pub struct Player {
    pub ctl: Controller,
    rig: Rig,
    /// This frame's triangles, in world space.
    verts: Vec<Vertex>,
    /// Where he is to be put down once the world there has loaded (None = he is in the world).
    waiting: Option<[f64; 3]>,
    /// Buttons held down, and presses since the last frame.
    pub jump: bool,
    pub roll: bool,
    presses: Vec<String>,
    /// The camera's place as of the last frame.
    eye: [f32; 3],
}

impl Player {
    /// A character to be put down at `at` (feet) when the world there is ready.
    pub fn new(at: [f64; 3], view: View) -> Player {
        let cfg = Config::builtin();
        let rig = Rig::builtin(&cfg);
        let mut ctl = Controller::new(cfg, view);
        ctl.snap_down = STEP;
        Player {
            ctl,
            rig,
            verts: Vec::new(),
            waiting: Some(at),
            jump: false,
            roll: false,
            presses: Vec::new(),
            eye: [at[0] as f32, (at[1] + EYE) as f32, at[2] as f32],
        }
    }

    /// Put him down at `at` (he drops to the ground from there).
    pub fn place(&mut self, at: [f64; 3]) {
        self.waiting = Some(at);
    }

    /// A one-shot action (see `Controller::press`).
    pub fn press(&mut self, name: &str) {
        self.presses.push(name.to_string());
    }

    pub fn position(&self) -> [f64; 3] {
        self.waiting.unwrap_or(self.ctl.pos)
    }

    /// The posed model's triangles (empty until he is in the world).
    pub fn triangles(&self) -> &[Vertex] {
        &self.verts
    }

    /// Whether a block cell overlaps his body (so a block is not placed inside him).
    pub fn occupies(&self, cell: BlockPos) -> bool {
        let p = self.position();
        let r = self.ctl.cfg.radius;
        let near = |v: f64, c: i32| v + r > c as f64 && v - r < c as f64 + 1.0;
        near(p[0], cell.x) && near(p[2], cell.z) && p[1] + 1.8 > cell.y as f64 && p[1] < cell.y as f64 + 1.0
    }

    /// One frame: moves and poses the character and returns where the camera goes.
    /// `stick` is (right, forward); `look` is the view's (yaw, pitch) in the controller's
    /// sense (yaw 0 faces +Z). With `control` off he stands by himself (the fly camera).
    pub fn update(&mut self, client: &Client, dt: f64, stick: (f32, f32), look: (f64, f64), aspect: f64, control: bool) -> [f32; 3] {
        let blocks = ClientBlocks(client);
        let world = BlockWorld { step_height: STEP, ..BlockWorld::new(&blocks) };
        if let Some(at) = self.waiting {
            if !world.loaded(at[0], at[1], at[2]) {
                self.eye = [at[0] as f32, (at[1] + EYE) as f32, at[2] as f32];
                return self.eye;
            }
            self.ctl.teleport(&world, at[0], Some(at[1]), at[2], Some(look.0));
            self.waiting = None;
        }
        let p = self.ctl.pos;
        if !world.loaded(p[0], p[1], p[2]) {
            return self.eye; // the ground under him is not there yet: wait for it
        }
        if p[1] < VOID_Y {
            let top = world.ground_height(p[0], p[2], 1.0e4);
            self.ctl.teleport(&world, p[0], Some(if top > VOID_Y { top } else { 200.0 }), p[2], None);
        }
        let input = Input {
            move_x: if control { stick.0 as f64 } else { 0.0 },
            move_y: if control { stick.1 as f64 } else { 0.0 },
            look_yaw: look.0,
            look_pitch: look.1,
            jump: control && self.jump,
            roll: control && self.roll,
            actions: std::mem::take(&mut self.presses),
        };
        let out = self.ctl.update(&world, dt, &input);
        let pose = self.rig.pose(&out, [0.0; 3]);
        self.verts.clear();
        self.rig.triangles(&pose, &mut self.verts);

        let head = pose.head_position().unwrap_or([out.position[0], out.position[1] + EYE, out.position[2]]);
        let cam = self.ctl.camera(dt, head, aspect);
        let mut at = cam.position;
        if out.view == View::Follow {
            // Keep the camera out of the ground and walls: walk from the shoulder back toward
            // where it wants to be and stop short of the first solid block.
            let from = self.ctl.camera_pivot();
            let d = [at[0] - from[0], at[1] - from[1], at[2] - from[2]];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let inside = |t: f64| {
                let q = [from[0] + d[0] * t, from[1] + d[1] * t, from[2] + d[2] * t];
                let cell = BlockPos { x: q[0].floor() as i32, y: q[1].floor() as i32, z: q[2].floor() as i32 };
                client.solid_span(cell).flatten().is_some_and(|(lo, hi)| {
                    let f = q[1] - q[1].floor();
                    f >= lo as f64 - 0.15 && f <= hi as f64 + 0.15
                })
            };
            let steps = (len / 0.15).ceil().max(1.0) as usize;
            let mut free = 1.0;
            for i in 1..=steps {
                let t = i as f64 / steps as f64;
                if inside(t) {
                    free = ((i as f64 - 1.5) / steps as f64).max(0.05);
                    break;
                }
            }
            at = [from[0] + d[0] * free, from[1] + d[1] * free, from[2] + d[2] * free];
        }
        self.eye = [at[0] as f32, at[1] as f32, at[2] as f32];
        self.eye
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blockforge::registry::BlockRegistry;
    use blockforge::server::{self, ServerConfig};
    use blockforge::worldgen::WorldGen;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn he_drops_onto_real_terrain_then_runs_and_jumps_across_it_without_entering_a_block() {
        let reg = Arc::new(BlockRegistry::builtin());
        let handle = server::spawn(ServerConfig { seed: 1337, worker_threads: 2, ..ServerConfig::default() }, reg.clone());
        let mut client = Client::connect(handle.connect(), 2);
        client.set_view_distance(4);
        let [x, y, z] = WorldGen::new(1337, &reg).find_spawn();
        let start = [x as f64 + 0.5, y as f64 + 3.0, z as f64 + 0.5];
        client.set_position([start[0] as f32, start[1] as f32, start[2] as f32]);
        let mut p = Player::new(start, View::Follow);
        assert!(p.triangles().is_empty(), "nothing to draw until the world is there");

        // The world arrives; he is put down and falls the last few blocks.
        for _ in 0..5000 {
            client.update();
            p.update(&client, 1.0 / 60.0, (0.0, 0.0), (0.0, 0.0), 1.8, true);
            if p.waiting.is_none() && p.ctl.is_grounded() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let at = p.position();
        assert!(p.waiting.is_none() && p.ctl.is_grounded(), "he never landed: {at:?}");
        assert!((at[1] - y as f64).abs() < 4.0 && (at[1] * 2.0 - (at[1] * 2.0).round()).abs() < 1e-6, "on a block or a half slab: {at:?}");
        assert_eq!(p.triangles().len(), 768);
        assert!(p.occupies(BlockPos { x: at[0].floor() as i32, y: at[1].floor() as i32 + 1, z: at[2].floor() as i32 }));
        assert!(!p.occupies(BlockPos { x: at[0].floor() as i32 + 2, y: at[1].floor() as i32, z: at[2].floor() as i32 }));

        // Ten seconds of sprinting with Jump held: over steps, slabs and whatever is there.
        let solid_at = |c: &Client, q: [f64; 3]| {
            let cell = BlockPos { x: q[0].floor() as i32, y: q[1].floor() as i32, z: q[2].floor() as i32 };
            c.solid_span(cell).flatten().is_some_and(|(lo, hi)| {
                let f = q[1] - q[1].floor();
                f > lo as f64 + 1e-6 && f < hi as f64 - 1e-6
            })
        };
        p.jump = true;
        let mut airborne = 0;
        for i in 0..600 {
            let q = p.position();
            if i % 20 == 0 {
                client.set_position([q[0] as f32, q[1] as f32, q[2] as f32]);
            }
            client.update();
            let eye = p.update(&client, 1.0 / 60.0, (0.0, 1.0), (0.0, -0.2), 1.8, true);
            let q = p.position();
            assert!(!solid_at(&client, [q[0], q[1] + 0.9, q[2]]), "his middle is inside a block at {q:?} (frame {i})");
            assert!(!solid_at(&client, [eye[0] as f64, eye[1] as f64, eye[2] as f64]), "the camera is inside a block at {eye:?}");
            airborne += !p.ctl.is_grounded() as u32;
            std::thread::sleep(Duration::from_millis(1));
        }
        let end = p.position();
        let run = ((end[0] - at[0]).powi(2) + (end[2] - at[2]).powi(2)).sqrt();
        assert!(run > 15.0, "he got {run:.1} blocks in ten seconds");
        assert!(airborne > 60, "he jumped ({airborne} frames in the air)");
        assert!(end[1] > VOID_Y);
    }
}
