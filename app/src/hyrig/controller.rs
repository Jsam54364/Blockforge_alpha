//! The HyRig state machine: a port of the kit's `hyrig_controller.gd` / `.js`, function for
//! function (the names follow the original so the two can be read side by side). It has no
//! rendering and no physics of its own: input and three world queries go in; position, facing,
//! which clips to show, head turn, face and camera come out.
//!
//! `tests/hyrig_parity.rs` replays the kit's recorded reference run (13 008 frames covering
//! every state: on foot, flying, wading, swimming and diving) and checks this gives the same
//! result on every frame.
//!
//! Additions for the game, off unless set: `snap_down` and `fly_follow_pitch` (see the
//! fields), and the "interact_both" press (both hands reach out together, even with them
//! full: putting down a carried block).

use std::collections::{HashMap, HashSet};

use super::config::{self, Config};
use super::world::{LedgeQuery, World, NO_WATER};

/// The blended modes: flight, swimming at the surface and underwater. Each plays as one
/// pseudo clip on the body (its name below), expanded into real clips in the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Medium {
    Fly,
    Swim,
    Dive,
}

impl Medium {
    pub fn clip(self) -> &'static str {
        match self {
            Medium::Fly => "__fly",
            Medium::Swim => "__swim",
            Medium::Dive => "__dive",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    First,
    Follow,
}

/// One clip in the body's cross-fade.
#[derive(Clone, Debug)]
struct Track {
    clip: String,
    time: f64,
    w: f64,
    rate: f64,
    looped: bool,
    speed: f64,
    finished: bool,
    /// The clip whose stride this one keeps step with while fading out.
    sync: Option<String>,
    /// Below zero: none.
    warp: f64,
}

#[derive(Clone, Copy, Debug)]
struct Climb {
    h: f64,
    y0: f64,
    t0: f64,
    tg: f64,
    top: f64,
    sx: f64,
    sz: f64,
    hx: f64,
    hz: f64,
    tx: f64,
    tz: f64,
    yaw: f64,
}

#[derive(Clone, Debug)]
struct FaceState {
    blink_t: f64,
    bp: f64,
    dbl: bool,
    gx: f64,
    gy: f64,
    tx: f64,
    ty: f64,
    sac_t: f64,
    sq: f64,
    br: f64,
    br_r: f64,
    air: f64,
}

/// A clip to show: where in it, and how strongly.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub clip: String,
    pub time: f64,
    pub weight: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// 0 open .. 1 closed (never quite 0: it scales the eyelid).
    pub lid: f64,
    pub pupil: [f64; 2],
    pub brow_lift: f64,
    pub brow_tilt: f64,
    pub mouth: &'static str,
    pub mouth_uv: [f64; 2],
}

/// One frame's input.
#[derive(Clone, Debug, Default)]
pub struct Input {
    /// Stick: x = right, y = forward, length 0..1.
    pub move_x: f64,
    pub move_y: f64,
    /// Absolute view direction.
    pub look_yaw: f64,
    pub look_pitch: f64,
    /// Held: holding repeats.
    pub jump: bool,
    pub roll: bool,
    /// Held: descend while flying, dive from the surface, go down underwater.
    pub down: bool,
    /// Held: keep throwing jabs, alternating hands.
    pub punch: bool,
    /// Held: break blocks (a repeating strike; a "mineHit" event as each one lands).
    pub mine: bool,
    /// One-shot presses this frame (same names as `Controller::press`).
    pub actions: Vec<String>,
}

/// One frame's result.
#[derive(Clone, Debug)]
pub struct Output {
    pub position: [f64; 3],
    pub yaw: f64,
    /// One to three clips for the whole body; weights sum to 1.
    pub body: Vec<Layer>,
    /// Over the upper-body bones only.
    pub upper: Option<Layer>,
    /// The carry pose: last, over the listed arm bones only.
    pub hold: Option<(Layer, Vec<String>)>,
    /// Wading: over the upper-body bones, after `body` and before `upper`.
    pub wade: Option<Layer>,
    /// With "interact_both": the other hand's half of the action, over the left arm.
    pub upper_left: Option<Layer>,
    /// Whole-body tilt (forward, right) in radians about a point `Config::fly` `pivot` above
    /// the feet: top toward its front, and toward its right. Zero on foot.
    pub lean: [f64; 2],
    /// Extra head turn (yaw, pitch) toward the view, after the animation.
    pub head_look: Option<[f64; 2]>,
    pub face: Face,
    /// "footstep", "takeoff", "land", "rollStart", "climbStart", ... in order.
    pub events: Vec<&'static str>,
    pub view: View,
    /// "dead", "climb", "fly", "swim", "dive" or the body's current clip.
    pub state: String,
    /// The clip shown most (in a blended mode, its strongest clip).
    pub clip: String,
    pub flying: bool,
    pub swimming: bool,
    pub underwater: bool,
    pub wading: bool,
    pub velocity: [f64; 3],
    pub clip_time: f64,
    pub grounded: bool,
    pub speed: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub position: [f64; 3],
    pub target: [f64; 3],
    /// Vertical, degrees.
    pub fov: f64,
    pub near: f64,
    pub hide_head: bool,
}

pub struct Controller {
    pub cfg: Config,
    rand: Box<dyn FnMut() -> f64 + Send>,
    pub view: View,
    pub time_scale: f64,
    /// For block worlds (0 = the kit's behaviour): walking off an edge no higher than this
    /// keeps the feet on the ground instead of starting a fall, so going down half slabs is
    /// walking, not a string of little drops.
    pub snap_down: f64,
    /// For touch screens with no buttons to rise and sink (false = the kit's behaviour): in
    /// flight, forward follows the look pitch, as it does underwater.
    pub fly_follow_pitch: bool,
    dur: HashMap<String, f64>,
    gait_f: HashSet<String>,
    gait_b: HashSet<String>,
    upper: HashSet<String>,
    intr: HashSet<String>,
    attack: HashSet<String>,
    grav: f64,
    jump_v: f64,

    pub pos: [f64; 3],
    pub yaw: f64,
    vel_x: f64,
    vel_z: f64,
    vy: f64,
    grounded: bool,
    last_ground_y: f64,
    jump_fired: bool,
    falling: bool,
    pub crouching: bool,
    pub prone: bool,
    pub sitting: bool,
    pub dead: bool,
    one_shot: String,
    climbing: Option<Climb>,
    last_tier: i32,
    last_back: bool,
    stick_x: f64,
    stick_z: f64,
    stick_mag: f64,
    stick_back: bool,
    roll_x: f64,
    roll_z: f64,
    roll_back: bool,
    roll_yaw: f64,
    jump_buf: f64,
    gt: f64,
    held_jump: bool,
    held_roll: bool,
    look_yaw: f64,
    look_pitch: f64,
    cam_dist: f64,
    target: [f64; 3],
    eye: [f64; 3],
    eye_init: bool,

    tracks: Vec<Track>,
    cur: Option<String>,
    overlay: Option<(String, f64)>,
    ov_w: f64,
    hold_want: String,
    hold_cur: String,
    hold_w: f64,
    hold_t: f64,
    events: Vec<&'static str>,
    queue: Vec<String>,
    fidget_t: f64,
    head_look: Option<[f64; 2]>,
    f: FaceState,

    mode: Option<Medium>,
    flying: bool,
    /// The last mode active (its settings drive the limb spring and the lean).
    mode_cfg: Option<Medium>,
    wading: bool,
    wade_w: f64,
    wade_t: f64,
    stick_vy: f64,
    stick_in: bool,
    /// The limb momentum spring (world space): position, velocity, and last frame's velocity.
    mom: [f64; 9],
    jump_prev: bool,
    jump_latch: bool,
    last_jump_tap: f64,
    punch_alt: bool,
    held_down: bool,
    held_mine: bool,
    held_punch: bool,
    /// "interact_both" is playing: the left arm takes its half from the left-hand clip.
    both_hands: bool,
}

fn ang(a: f64, b: f64) -> f64 {
    (a - b).sin().atan2((a - b).cos())
}

fn smooth(u: f64) -> f64 {
    u * u * (3.0 - 2.0 * u)
}

fn smoother(u: f64) -> f64 {
    u * u * u * (u * (u * 6.0 - 15.0) + 10.0)
}

/// A small random generator for blinks, glances and fidgets (the kit lets the game supply one).
fn default_rand() -> Box<dyn FnMut() -> f64 + Send> {
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    Box::new(move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f64 / (1u64 << 53) as f64
    })
}

impl Controller {
    pub fn new(cfg: Config, view: View) -> Controller {
        Controller::with_random(cfg, view, default_rand())
    }

    pub fn with_random(cfg: Config, view: View, rand: Box<dyn FnMut() -> f64 + Send>) -> Controller {
        let mut dur: HashMap<String, f64> = cfg.clips.iter().map(|(k, c)| (k.clone(), c.duration)).collect();
        if let Some(f) = &cfg.fly {
            dur.insert(Medium::Fly.clip().into(), f.cycle);
        }
        if let Some(w) = &cfg.water {
            dur.insert(Medium::Swim.clip().into(), w.swim.cycle);
            dur.insert(Medium::Dive.clip().into(), w.dive.cycle);
        }
        let mode_cfg = if cfg.fly.is_some() {
            Some(Medium::Fly)
        } else if cfg.water.is_some() {
            Some(Medium::Swim)
        } else {
            None
        };
        let set = |names: &[&str]| names.iter().filter(|n| !n.is_empty()).map(|n| n.to_string()).collect::<HashSet<String>>();
        let gait_f = set(&[cfg.role("walk"), cfg.role("jog"), cfg.role("sprint"), cfg.role("crouchWalk"), "run"]);
        let gait_b = set(&[cfg.role("walkBack"), cfg.role("runBack"), cfg.role("crouchBack")]);
        let grav = 8.0 * cfg.jump_height / (cfg.air_time * cfg.air_time);
        let mut c = Controller {
            rand,
            view,
            time_scale: 1.0,
            snap_down: 0.0,
            fly_follow_pitch: false,
            dur,
            gait_f,
            gait_b,
            upper: cfg.upper_actions.iter().cloned().collect(),
            intr: cfg.interruptible.iter().cloned().collect(),
            attack: cfg.attack_clips.iter().cloned().collect(),
            grav,
            jump_v: grav * cfg.air_time / 2.0,
            pos: [0.0; 3],
            yaw: 0.0,
            vel_x: 0.0,
            vel_z: 0.0,
            vy: 0.0,
            grounded: true,
            last_ground_y: 0.0,
            jump_fired: false,
            falling: false,
            crouching: false,
            prone: false,
            sitting: false,
            dead: false,
            one_shot: String::new(),
            climbing: None,
            last_tier: -1,
            last_back: false,
            stick_x: 0.0,
            stick_z: 1.0,
            stick_mag: 0.0,
            stick_back: false,
            roll_x: 0.0,
            roll_z: 1.0,
            roll_back: false,
            roll_yaw: 0.0,
            jump_buf: -9.0,
            gt: 0.0,
            held_jump: false,
            held_roll: false,
            look_yaw: 0.0,
            look_pitch: 0.0,
            cam_dist: cfg.follow_distance,
            target: [0.0, cfg.target_height, 0.0],
            eye: [0.0, 1.7, 0.0],
            eye_init: false,
            tracks: Vec::new(),
            cur: None,
            overlay: None,
            ov_w: 0.0,
            hold_want: "none".into(),
            hold_cur: "none".into(),
            hold_w: 0.0,
            hold_t: 0.0,
            events: Vec::new(),
            queue: Vec::new(),
            fidget_t: 0.0,
            head_look: None,
            f: FaceState { blink_t: 0.0, bp: -1.0, dbl: false, gx: 0.0, gy: 0.0, tx: 0.0, ty: 0.0, sac_t: 0.5, sq: 0.0, br: 0.0, br_r: 0.0, air: 0.0 },
            mode: None,
            flying: false,
            mode_cfg,
            wading: false,
            wade_w: 0.0,
            wade_t: 0.0,
            stick_vy: 0.0,
            stick_in: false,
            mom: [0.0; 9],
            jump_prev: false,
            jump_latch: false,
            last_jump_tap: -9.0,
            punch_alt: false,
            held_down: false,
            held_mine: false,
            held_punch: false,
            both_hands: false,
            cfg,
        };
        c.fidget_t = c.fidget_wait();
        c.f.blink_t = 1.0 + (c.rand)() * 2.0;
        let idle = c.n("idle");
        c.play(&idle, true, false);
        c.events.clear();
        c
    }

    // ---------------------------------------------------------------- public controls

    /// Queue a one-shot press: "crouch", "crawl", "punch", "swing", "hit", "die", "revive",
    /// "chop", "interact", "interact_left", "eat", "wave", "sit", and the carry toggles
    /// "one_handed", "one_handed_left", "two_handed", "two_handed_item".
    pub fn press(&mut self, name: &str) {
        self.queue.push(name.to_string());
    }

    pub fn set_view(&mut self, v: View) {
        self.view = v;
        self.eye_init = false;
    }

    pub fn zoom(&mut self, factor: f64) {
        self.cam_dist = (self.cam_dist * factor).clamp(self.cfg.follow_min, self.cfg.follow_max);
    }

    /// Move the character. `y` None drops it onto the ground at (x, z).
    pub fn teleport(&mut self, world: &dyn World, x: f64, y: Option<f64>, z: f64, new_yaw: Option<f64>) {
        self.pos = [x, y.unwrap_or_else(|| world.ground_height(x, z, 1e6)), z];
        if let Some(a) = new_yaw {
            self.yaw = a;
            self.look_yaw = a;
        }
        self.vy = 0.0;
        self.vel_x = 0.0;
        self.vel_z = 0.0;
        self.grounded = self.mode.is_none();
        self.last_ground_y = self.pos[1];
        self.climbing = None;
        self.falling = false;
        self.target = [x, self.pos[1] + self.cfg.target_height, z];
        self.eye_init = false;
    }

    /// Carry pose layered over every animation: "none" or one of the hold states.
    pub fn set_hold(&mut self, state: &str) {
        if state != "none" && !self.cfg.hold_states.contains_key(state) {
            return;
        }
        if self.hold_want == state {
            return;
        }
        self.hold_want = state.to_string();
        self.events.push("hold");
    }

    pub fn hold(&self) -> &str {
        &self.hold_want
    }

    /// Creative flight on (true) or off (false). Returns true if the change went ahead.
    /// Taking off from the ground gives a small lift; switching off in the air falls.
    pub fn set_fly(&mut self, world: &dyn World, v: bool) -> bool {
        self.set_fly_q(world, v, false)
    }

    fn set_fly_q(&mut self, world: &dyn World, v: bool, quiet: bool) -> bool {
        if v == self.flying {
            return true;
        }
        if v {
            let Some(f) = self.cfg.fly.as_ref() else { return false };
            let (takeoff, vertical) = (f.takeoff, f.vertical);
            if self.dead || self.is_n("death", &self.one_shot) || self.climbing.is_some() || self.prone || self.in_prone_move() {
                return false;
            }
            self.crouching = false;
            self.sitting = false;
            self.falling = false;
            self.one_shot.clear();
            // From the ground or the water: a small lift.
            self.vy = if self.grounded || self.mode.is_some() { takeoff } else { self.vy.max(-vertical) };
            self.grounded = false;
            self.set_mode(Some(Medium::Fly));
            self.play(Medium::Fly.clip(), true, false);
            self.events.push("flyStart");
            return true;
        }
        self.set_mode(None);
        self.events.push("flyEnd");
        let fall = self.n("fall");
        if !quiet && !fall.is_empty() && self.pos[1] - world.ground_height(self.pos[0], self.pos[2], self.pos[1]) > 0.3 {
            self.falling = true;
            self.one_shot = fall.clone();
            self.play(&fall, true, false);
        }
        true
    }

    fn set_mode(&mut self, m: Option<Medium>) {
        self.mode = m;
        self.flying = m == Some(Medium::Fly);
        if m.is_some() {
            self.mode_cfg = m;
        }
    }

    pub fn is_flying(&self) -> bool {
        self.flying
    }

    /// Flying, swimming or diving.
    pub fn medium(&self) -> Option<Medium> {
        self.mode
    }

    pub fn revive(&mut self) {
        if !self.dead && self.one_shot != self.n("death") {
            return;
        }
        self.dead = false;
        self.one_shot.clear();
        self.events.push("revived");
    }

    /// Lie down (true) or stand up (false). Returns true if the change went ahead.
    pub fn set_prone(&mut self, v: bool) -> bool {
        if self.dead {
            return false;
        }
        if v == self.prone {
            return true;
        }
        if self.climbing.is_some() || !self.grounded || !self.one_shot.is_empty() || self.mode.is_some() || (v && self.wading) {
            return false;
        }
        self.stop_overlay();
        self.crouching = false;
        self.sitting = false;
        self.vel_x = 0.0;
        self.vel_z = 0.0;
        self.prone = v;
        self.one_shot = self.n(if v { "proneIn" } else { "proneOut" });
        let clip = self.one_shot.clone();
        self.play(&clip, false, true);
        self.events.push(if v { "proneStart" } else { "proneEnd" });
        true
    }

    pub fn is_grounded(&self) -> bool {
        self.grounded
    }

    pub fn is_climbing(&self) -> bool {
        self.climbing.is_some()
    }

    // ---------------------------------------------------------------- animation bookkeeping

    /// The clip a role plays ("" if none).
    fn n(&self, role: &str) -> String {
        self.cfg.role(role).to_string()
    }

    fn is_n(&self, role: &str, clip: &str) -> bool {
        !clip.is_empty() && self.cfg.role(role) == clip
    }

    fn d(&self, clip: &str) -> f64 {
        self.dur.get(clip).copied().unwrap_or(0.0)
    }

    fn track(&self, clip: &str) -> Option<&Track> {
        self.tracks.iter().find(|t| t.clip == clip)
    }

    fn cur_track(&self) -> Option<&Track> {
        self.cur.as_deref().and_then(|c| self.track(c))
    }

    fn cur_mut(&mut self) -> Option<&mut Track> {
        let c = self.cur.clone()?;
        self.tracks.iter_mut().find(|t| t.clip == c)
    }

    fn cur_time(&self) -> Option<f64> {
        self.cur_track().map(|t| t.time)
    }

    fn fidget_wait(&mut self) -> f64 {
        match self.cfg.fidget {
            None => 1e9,
            Some([min, max]) => min + (self.rand)() * (max - min),
        }
    }

    fn is_roll(&self, n: &str) -> bool {
        self.is_n("roll", n) || self.is_n("rollBack", n)
    }

    fn is_jump(&self, n: &str) -> bool {
        !n.is_empty() && self.cfg.windows.contains_key(n)
    }

    fn is_air(&self, n: &str) -> bool {
        self.is_jump(n) || self.is_n("fall", n) || self.is_n("land", n)
    }

    fn is_upper(&self, n: &str) -> bool {
        !n.is_empty() && self.upper.contains(n)
    }

    fn is_crawl(&self, n: &str) -> bool {
        self.is_n("crawl", n) || self.is_n("crawlBack", n) || self.is_n("crawlLeft", n) || self.is_n("crawlRight", n)
    }

    fn in_prone_move(&self) -> bool {
        self.is_n("proneIn", &self.one_shot) || self.is_n("proneOut", &self.one_shot)
    }

    fn hold_suspended(&self) -> bool {
        self.cfg.hold_suspend.iter().any(|k| match k.as_str() {
            "climb" => self.climbing.is_some(),
            "prone" => self.prone || self.in_prone_move(),
            "dead" => self.dead || self.is_n("death", &self.one_shot),
            "roll" => self.is_roll(&self.one_shot),
            "air" => !self.grounded,
            "crouch" => self.crouching,
            "sit" => self.sitting,
            "swim" => matches!(self.mode, Some(Medium::Swim | Medium::Dive)),
            _ => false,
        })
    }

    fn play(&mut self, name: &str, looped: bool, restart: bool) {
        let Some(&d) = self.dur.get(name) else { return };
        let prev = self.cur.clone();
        if prev.as_deref() == Some(name) && !restart {
            return;
        }
        let mut ph = -1.0;
        let mut fade = self.cfg.fade("default");
        let mut sync = false;
        if let Some(pt) = prev.as_deref().filter(|p| *p != name).and_then(|p| self.track(p)) {
            let pn = pt.clip.as_str();
            let pd = self.d(pn);
            let c = &self.cfg;
            let window_end = |clip: &str| c.windows.get(clip).map_or(0.0, |w| w[1]);
            if let Some(m) = self.mode_named(name).or(self.mode_named(pn)) {
                fade = self.mode_settings(m).fade;
            } else if (self.gait_f.contains(pn) && self.gait_f.contains(name)) || (self.gait_b.contains(pn) && self.gait_b.contains(name)) {
                ph = (pt.time / pd).rem_euclid(1.0);
                fade = c.fade("gait");
                sync = true;
            } else if c.land_phase_back.contains_key(pn) && self.gait_b.contains(name) {
                ph = (1.0 - (c.land_phase_back[pn] - (pt.time - window_end(pn)).max(0.0) / d)).rem_euclid(1.0);
                fade = c.fade("jumpToGait");
            } else if self.gait_b.contains(pn) && c.land_phase_back.contains_key(name) {
                fade = c.fade("jumpStart");
            } else if c.land_phase.contains_key(pn) && self.gait_f.contains(name) {
                ph = (c.land_phase[pn] + (pt.time - window_end(pn)).max(0.0) / d).rem_euclid(1.0);
                fade = c.fade("jumpToGait");
            } else if self.gait_f.contains(pn) && c.land_phase.contains_key(name) {
                fade = c.fade("jumpStart");
            } else if self.is_roll(name) {
                fade = c.fade("rollStart");
            } else if self.is_roll(pn) {
                fade = c.fade(if looped { "rollToLoop" } else { "rollToAction" });
            } else if !looped {
                fade = c.fade("action");
            } else if !pt.looped {
                fade = c.fade("actionToLoop");
            }
        }
        let start = if ph >= 0.0 { ph * d } else { 0.0 };
        if prev.as_deref() == Some(name) {
            // restart in place
            if let Some(t) = self.cur_mut() {
                t.time = start;
                t.w = 1.0;
                t.rate = 0.0;
                t.finished = false;
                t.looped = looped;
                t.speed = 1.0;
                t.sync = None;
                t.warp = -1.0;
            }
            self.events.push("seek");
        } else {
            let prev_w = prev.as_deref().and_then(|p| self.track(p)).map(|t| t.w);
            let prev_d = prev.as_deref().map(|p| self.d(p));
            let i = match self.tracks.iter().position(|t| t.clip == name) {
                Some(i) => i,
                None => {
                    self.tracks.push(Track { clip: name.to_string(), time: 0.0, w: 0.0, rate: 0.0, looped, speed: 1.0, finished: false, sync: None, warp: -1.0 });
                    self.tracks.len() - 1
                }
            };
            {
                let t = &mut self.tracks[i];
                t.time = start;
                t.looped = looped;
                t.speed = 1.0;
                t.finished = false;
                t.sync = None;
                t.warp = -1.0;
            }
            if let (Some(pn), Some(pw)) = (prev.as_deref(), prev_w) {
                self.tracks[i].w = 0.0;
                self.tracks[i].rate = 1.0 / fade;
                for o in self.tracks.iter_mut() {
                    if o.clip == pn {
                        o.rate = -pw.max(1e-3) / fade;
                    }
                }
                if sync {
                    // Gait change: every stride still fading keeps step with the new one, and
                    // the stride rate eases across.
                    for o in self.tracks.iter_mut() {
                        if o.clip != name && (o.clip == pn || o.sync.is_some()) {
                            o.sync = Some(name.to_string());
                        }
                    }
                    self.tracks[i].warp = d / prev_d.unwrap_or(d);
                } else {
                    for o in self.tracks.iter_mut() {
                        if o.clip == pn {
                            o.sync = None;
                        }
                    }
                }
            } else {
                self.tracks[i].w = 1.0;
                self.tracks[i].rate = 0.0;
            }
            self.events.push("play");
        }
        self.cur = Some(name.to_string());
    }

    fn seek(&mut self, time: f64) {
        if let Some(t) = self.cur_mut() {
            t.time = time;
            self.events.push("seek");
        }
    }

    fn start_overlay(&mut self, name: &str, t: f64) {
        if !self.dur.contains_key(name) {
            return;
        }
        self.overlay = Some((name.to_string(), t));
        self.both_hands = false;
        self.events.push("upper");
    }

    fn stop_overlay(&mut self) {
        if self.overlay.is_some() {
            self.events.push("upperStop");
        }
        self.overlay = None;
        self.ov_w = 0.0;
        self.both_hands = false;
    }

    fn advance(&mut self, dt: f64) {
        let mut fin = false;
        for pass in 0..2 {
            for i in 0..self.tracks.len() {
                let d = self.d(&self.tracks[i].clip);
                let leader = self.tracks[i]
                    .sync
                    .as_deref()
                    .filter(|s| *s != self.tracks[i].clip)
                    .and_then(|s| self.track(s))
                    .map(|l| l.time / self.d(&l.clip));
                if leader.is_some() != (pass == 1) {
                    continue;
                }
                let is_cur = self.cur.as_deref() == Some(self.tracks[i].clip.as_str());
                if let Some(phase) = leader {
                    self.tracks[i].time = phase * d; // keep the outgoing gait in step
                } else {
                    let gait = self.gait_f.contains(&self.tracks[i].clip) || self.gait_b.contains(&self.tracks[i].clip);
                    let t = &mut self.tracks[i];
                    let before = t.time;
                    t.time += dt * t.speed * if t.warp >= 0.0 { t.warp + (1.0 - t.warp) * t.w } else { 1.0 };
                    if t.looped {
                        if is_cur && gait {
                            let (a, b) = (before / d, t.time / d);
                            if b >= 1.0 || (a < 0.5 && b >= 0.5) {
                                self.events.push("footstep");
                            }
                        }
                        t.time %= d;
                    } else if t.time >= d {
                        t.time = d;
                        if !t.finished {
                            t.finished = true;
                            if is_cur {
                                fin = true;
                            }
                        }
                    }
                }
                let t = &mut self.tracks[i];
                t.w = (t.w + t.rate * dt).clamp(0.0, 1.0);
                if t.w >= 1.0 || t.w <= 0.0 {
                    t.rate = 0.0;
                }
                if t.w >= 1.0 {
                    t.warp = -1.0;
                }
            }
        }
        let cur = self.cur.clone();
        self.tracks.retain(|t| cur.as_deref() == Some(t.clip.as_str()) || t.w > 0.0);
        if let Some((clip, ot)) = self.overlay.clone() {
            let odu = self.d(&clip);
            let mut nt = ot + dt;
            if let Some(&hit) = self.cfg.mine_hit_at.get(&clip) {
                // Mining loops while held; each strike reports when it lands.
                let ha = hit * odu;
                if ot < ha && nt >= ha {
                    self.events.push("mineHit");
                }
                if self.held_mine && nt >= odu {
                    nt -= odu;
                    if nt >= ha {
                        self.events.push("mineHit");
                    }
                }
            }
            if let Some((_, t)) = self.overlay.as_mut() {
                *t = nt.min(odu);
            }
        }
        if fin {
            if self.is_n("death", &self.one_shot) {
                self.dead = true;
                self.events.push("died");
            } else {
                self.one_shot.clear();
            }
        }
    }

    // ---------------------------------------------------------------- world helpers

    fn collide(&mut self, world: &dyn World) {
        if self.climbing.is_some() {
            return;
        }
        let radius = self.cfg.radius;
        let (x, z) = world.collide(self.pos[0], self.pos[2], self.pos[1], radius);
        self.pos[0] = x;
        self.pos[2] = z;
        if self.prone {
            // Lying down he is 1.8 m long: keep the head and feet out of walls too.
            let (fx, fz) = (self.yaw.sin(), self.yaw.cos());
            for i in 0..self.cfg.prone_probes.len() {
                let o = self.cfg.prone_probes[i];
                let (qx, qz) = (self.pos[0] + fx * o, self.pos[2] + fz * o);
                let (rx, rz) = world.collide(qx, qz, self.pos[1], radius);
                self.pos[0] += rx - qx;
                self.pos[2] += rz - qz;
            }
        }
    }

    fn hand_at(&self, t: f64) -> f64 {
        let h = &self.cfg.climb_hand;
        let x = (t / self.cfg.climb_duration).clamp(0.0, 1.0) * (h.len() - 1) as f64;
        let i = (x.floor() as usize).min(h.len() - 2);
        h[i] + (h[i + 1] - h[i]) * (x - i as f64)
    }

    fn grab(&self, cl: &Climb, t: f64) -> f64 {
        (cl.h - self.hand_at(t)).clamp(0.0, cl.h)
    }

    fn try_climb(&mut self, world: &dyn World) -> bool {
        let climb = self.n("climb");
        if climb.is_empty() || self.climbing.is_some() || self.dead || self.prone || self.in_prone_move() || matches!(self.mode, Some(Medium::Fly | Medium::Dive)) {
            return false;
        }
        let c = &self.cfg;
        let moving = self.stick_mag > c.deadzone;
        let dx = if moving { self.stick_x } else { self.yaw.sin() };
        let dz = if moving { self.stick_z } else { self.yaw.cos() };
        let query = LedgeQuery {
            min_height: c.climb_min,
            max_height: c.climb_max,
            reach: c.reach,
            facing: c.facing,
            stand_in: c.stand_in,
            hang_out: c.hang_out,
            radius: c.radius,
        };
        let base = if self.grounded || self.mode == Some(Medium::Swim) { self.pos[1] } else { self.last_ground_y };
        let Some(l) = world.find_ledge(self.pos[0], self.pos[2], self.pos[1], base, dx, dz, &query) else { return false };
        let big_h = l.top_y - self.pos[1];
        let h = &c.climb_hand;
        let mut t0 = 0.0;
        let mut pk = 0;
        for i in 1..h.len() {
            if h[i] > h[pk] {
                pk = i; // highest reach
            }
        }
        let tg = c.climb_duration * pk as f64 / (h.len() - 1) as f64;
        if big_h < h[pk] {
            for i in pk..h.len() - 1 {
                if h[i + 1] <= big_h {
                    // Low ledge: start where the hands match.
                    t0 = c.climb_duration * (i as f64 + (h[i] - big_h) / (h[i] - h[i + 1])) / (h.len() - 1) as f64;
                    break;
                }
            }
        }
        let rate = c.climb_rate;
        self.climbing = Some(Climb {
            h: big_h,
            y0: self.pos[1],
            t0,
            tg,
            top: h[pk],
            sx: self.pos[0],
            sz: self.pos[2],
            hx: l.hang_x,
            hz: l.hang_z,
            tx: l.stand_x,
            tz: l.stand_z,
            yaw: l.nx.atan2(l.nz),
        });
        if self.mode.is_some() {
            // Climbing out of the water.
            self.set_mode(None);
            self.events.push("exitWater");
        }
        self.stop_overlay();
        self.crouching = false;
        self.sitting = false;
        self.vy = 0.0;
        self.vel_x = 0.0;
        self.vel_z = 0.0;
        self.falling = false;
        self.jump_fired = true;
        self.one_shot = climb.clone();
        self.play(&climb, false, true);
        self.seek(t0);
        if let Some(t) = self.cur_mut() {
            t.speed = rate;
        }
        self.events.push("climbStart");
        true
    }

    // ---------------------------------------------------------------- actions

    fn chain_jump(&mut self) {
        let prev_n = self.one_shot.clone();
        let mut nm = self.n("jump");
        let jump_back = self.n("jumpBack");
        if self.last_back && !jump_back.is_empty() {
            nm = jump_back.clone();
            if prev_n == jump_back && self.dur.contains_key(&format!("{nm}_r")) {
                nm = format!("{nm}_r");
            }
        } else if self.last_tier >= 0 {
            let mv = self.n(if self.last_tier >= 1 { "jumpRun" } else { "jumpWalk" });
            if !mv.is_empty() {
                nm = mv.clone();
                if !prev_n.ends_with("_r") && self.cfg.land_phase.contains_key(&prev_n) && self.dur.contains_key(&format!("{mv}_r")) {
                    nm = format!("{mv}_r");
                }
            }
        }
        if nm.is_empty() {
            return;
        }
        self.one_shot = nm.clone();
        self.jump_fired = false;
        self.play(&nm, false, true);
        if let Some(w) = self.cfg.windows.get(&nm).copied() {
            self.seek((w[0] - self.cfg.chain_lead).max(0.0));
        }
    }

    fn do_press(&mut self, world: &dyn World, name: &str) {
        if name == "revive" {
            self.revive();
            return;
        }
        if self.dead {
            return;
        }
        if self.cfg.hold_states.contains_key(name) {
            let next = if self.hold_want == name { "none" } else { name };
            self.set_hold(next);
            return;
        }
        let name = if name == "interact_left" { "interactLeft" } else { name };
        if name == "fly" {
            let v = !self.flying;
            self.set_fly(world, v);
            return;
        }
        if (self.mode.is_some() || self.wading) && matches!(name, "crouch" | "crawl" | "sit") {
            return;
        }
        if name == "interact_both" {
            self.interact_both();
            return;
        }
        match name {
            "crawl" => {
                self.set_prone(!self.prone);
            }
            "crouch" => {
                if self.prone {
                    if self.set_prone(false) {
                        self.crouching = true;
                    }
                    return;
                }
                if self.in_prone_move() {
                    return;
                }
                self.crouching = !self.crouching;
                if self.crouching {
                    self.sitting = false;
                }
            }
            "sit" => {
                if self.prone || !self.one_shot.is_empty() || !self.grounded || self.climbing.is_some() {
                    return;
                }
                self.sitting = !self.sitting;
                if self.sitting {
                    self.crouching = false;
                }
            }
            "die" => self.act(world, "death"),
            other => self.act(world, other),
        }
    }

    fn act(&mut self, world: &dyn World, role: &str) {
        let mut name = self.n(role);
        if name.is_empty() || !self.dur.contains_key(&name) {
            return;
        }
        if let Some(hs) = self.cfg.hold_states.get(&self.hold_want) {
            // What is being carried changes some actions (two-handed swing) and rules others
            // out (hands full).
            if hs.block.iter().any(|b| b == role) {
                return;
            }
            if let Some(alt) = hs.actions.get(role).filter(|a| self.dur.contains_key(*a)) {
                name = alt.clone();
            }
        }
        if self.climbing.is_some() || self.dead || self.is_n("death", &self.one_shot) {
            return;
        }
        if self.mode.is_some() {
            // Flying or swimming, Jump and Roll are not actions (Jump rises); dying ends the
            // mode.
            if role == "jump" || role == "roll" {
                return;
            }
            if role == "death" {
                if self.mode == Some(Medium::Fly) {
                    self.set_fly_q(world, false, true);
                } else {
                    self.leave_water(world);
                }
            }
        }
        if role == "roll" && self.wading {
            return;
        }
        if self.prone || self.in_prone_move() {
            // Lying down: Jump gets you up, Die still works, the rest wait until you stand.
            if role == "jump" {
                if self.prone {
                    self.set_prone(false);
                }
                return;
            }
            if role != "death" {
                return;
            }
            self.prone = false;
        }
        let mut link_roll = false;
        if role == "jump" {
            if self.try_climb(world) {
                return;
            }
            self.crouching = false;
            self.sitting = false; // jumping stands you up
            if !self.grounded {
                self.jump_buf = self.gt; // pressed in the air: remember it for touchdown
                return;
            }
            if self.is_jump(&self.one_shot) {
                let touchdown = self.cfg.windows[&self.one_shot][1];
                if self.cur_time().is_some_and(|t| t >= touchdown) {
                    self.chain_jump();
                }
                return;
            }
            if self.is_roll(&self.one_shot) {
                // Jumping out of a roll keeps its direction and speed.
                let bk = self.is_n("rollBack", &self.one_shot);
                let ksr = self.cfg.speed(if bk { "runBack" } else { "sprint" });
                self.vel_x = self.roll_x * ksr;
                self.vel_z = self.roll_z * ksr;
                self.last_back = bk;
                self.last_tier = if bk { -1 } else { 2 };
            }
            let jump_back = self.n("jumpBack");
            let cur = self.cur_track().map(|t| (t.clip.clone(), t.time));
            if self.last_back && !jump_back.is_empty() {
                let mut mb = jump_back;
                if let Some((clip, time)) = cur.as_ref().filter(|(c, _)| self.gait_b.contains(c)) {
                    let bp = 1.0 - (time / self.d(clip)).rem_euclid(1.0);
                    if bp >= 0.5 && self.dur.contains_key(&format!("{mb}_r")) {
                        mb = format!("{mb}_r");
                    }
                }
                name = mb;
            } else if self.last_tier >= 0 {
                let mut mv = self.n(if self.last_tier >= 1 { "jumpRun" } else { "jumpWalk" });
                // Take off from whichever foot is about to plant.
                if let Some((clip, time)) = cur.as_ref().filter(|(c, _)| !mv.is_empty() && self.gait_f.contains(c)) {
                    let gp = (time / self.d(clip)).rem_euclid(1.0);
                    if (0.25..0.75).contains(&gp) && self.dur.contains_key(&format!("{mv}_r")) {
                        mv = format!("{mv}_r");
                    }
                }
                if !mv.is_empty() {
                    name = mv;
                }
            }
            self.jump_fired = false;
        }
        if role == "roll" {
            if !self.grounded {
                return;
            }
            if self.is_roll(&self.one_shot) {
                // Already rolling: only link a new one as this one comes back onto its feet.
                let link_at = self.cfg.roll_link_at.get(&self.one_shot).copied().unwrap_or(f64::MAX);
                if !self.cur_time().is_some_and(|t| t >= link_at) {
                    return;
                }
                link_roll = true;
            }
            // Roll the way the stick points; pulling back gives the backward roll.
            if self.stick_mag > self.cfg.deadzone {
                let roll_back = self.n("rollBack");
                let use_back = self.stick_back && !roll_back.is_empty();
                self.roll_x = self.stick_x;
                self.roll_z = self.stick_z;
                self.roll_back = use_back;
                self.roll_yaw = if use_back { (-self.stick_x).atan2(-self.stick_z) } else { self.stick_x.atan2(self.stick_z) };
                if use_back {
                    name = roll_back;
                }
            } else {
                self.roll_x = self.yaw.sin();
                self.roll_z = self.yaw.cos();
                self.roll_back = false;
                self.roll_yaw = self.yaw;
            }
            self.sitting = false;
        }
        let punch_left = self.n("punchLeft");
        let tool_punch = self.cfg.hold_states.get(&self.hold_want).is_some_and(|hs| hs.actions.contains_key("punch"));
        if role == "punch" && !tool_punch && !punch_left.is_empty() && self.dur.contains_key(&punch_left) {
            if self.punch_alt {
                name = punch_left; // bare hands: each press throws one jab, alternating right and left
            }
            self.punch_alt = !self.punch_alt;
        }
        if self.is_upper(&name) {
            // Upper body only: the legs carry on with whatever they are doing, idle included.
            self.start_overlay(&name, 0.0);
            return;
        }
        if role == "roll" || role == "death" {
            self.stop_overlay();
        }
        self.sitting = false;
        self.one_shot = name.clone();
        self.play(&name, false, true);
        if role == "roll" {
            if link_roll {
                self.seek(self.cfg.roll_link_start); // linked rolls skip the stand-up and wind-up
            }
            self.events.push("rollStart");
        }
        if role == "death" {
            self.events.push("dying");
        }
    }

    /// Both hands reach out together: the main hand's interact on the upper body, the other
    /// hand's over the left arm. Unlike "interact" it goes ahead with the hands full (it is
    /// how a carried block is put down).
    fn interact_both(&mut self) {
        let (name, left) = (self.n("interact"), self.n("interactLeft"));
        if name.is_empty() || !self.dur.contains_key(&name) || self.climbing.is_some() || self.dead || self.is_n("death", &self.one_shot) || self.prone || self.in_prone_move() {
            return;
        }
        self.start_overlay(&name, 0.0);
        self.both_hands = !left.is_empty() && self.dur.contains_key(&left);
    }

    fn is_punch(&self, n: &str) -> bool {
        self.is_n("punch", n) || self.is_n("punchLeft", n)
    }

    /// Breaking blocks: a strike that repeats on the upper body while held.
    fn mine(&mut self) {
        let mut name = self.n("mine");
        if name.is_empty() || !self.dur.contains_key(&name) {
            return;
        }
        if let Some(hs) = self.cfg.hold_states.get(&self.hold_want) {
            // What is carried picks the strike (other hand, two-handed) or rules it out.
            if hs.block.iter().any(|b| b == "mine") {
                return;
            }
            if let Some(alt) = hs.actions.get("mine").filter(|a| self.dur.contains_key(*a)) {
                name = alt.clone();
            }
        }
        if self.climbing.is_some() || self.prone || self.in_prone_move() || self.is_n("death", &self.one_shot) {
            return;
        }
        if self.overlay.as_ref().map_or(true, |(c, _)| *c != name) {
            self.start_overlay(&name, 0.0);
            self.events.push("mineStart");
        }
    }

    // ---------------------------------------------------------------- flight and water

    fn mode_named(&self, clip: &str) -> Option<Medium> {
        [Medium::Fly, Medium::Swim, Medium::Dive].into_iter().find(|m| m.clip() == clip && self.has_mode(*m))
    }

    fn has_mode(&self, m: Medium) -> bool {
        match m {
            Medium::Fly => self.cfg.fly.is_some(),
            _ => self.cfg.water.is_some(),
        }
    }

    fn mode_settings(&self, m: Medium) -> &config::Mode {
        match m {
            Medium::Fly => self.cfg.fly.as_ref().unwrap(),
            Medium::Swim => &self.cfg.water.as_ref().unwrap().swim,
            Medium::Dive => &self.cfg.water.as_ref().unwrap().dive,
        }
    }

    /// Moving in a mode: the view sets the heading; past sprintAt the stick gives the fast
    /// speed.
    fn fly_move(&mut self, world: &dyn World, dt: f64, jx: f64, jy: f64, mag: f64, locked: bool) {
        let m = self.mode.unwrap();
        let f = self.mode_settings(m).clone();
        let (deadzone, sprint_at) = (self.cfg.deadzone, self.cfg.sprint_at);
        let ly = self.look_yaw;
        let (mut tx, mut tz) = (0.0, 0.0);
        self.stick_vy = 0.0;
        self.stick_in = !locked && mag > deadzone;
        if self.stick_in {
            let (fx, fz, rx, rz) = (ly.sin(), ly.cos(), -ly.cos(), ly.sin());
            let mut sp = if mag > sprint_at { f.fast_speed } else { f.speed * mag / sprint_at };
            sp *= 1.0 - (1.0 - f.back_scale) * (jy / mag).max(0.0); // some modes are slower going backward
            // Underwater (and, for the game, in flight), forward follows the look pitch.
            let follow = f.follow_pitch || (m == Medium::Fly && self.fly_follow_pitch);
            let cp = if follow { self.look_pitch.cos() } else { 1.0 };
            if follow {
                self.stick_vy = -jy * self.look_pitch.sin() / mag * sp;
            }
            tx = (fx * -jy * cp + rx * jx) / mag * sp;
            tz = (fz * -jy * cp + rz * jx) / mag * sp;
        }
        let kv = (dt * f.accel).min(1.0); // slow easing: flight carries momentum
        self.vel_x += (tx - self.vel_x) * kv;
        self.vel_z += (tz - self.vel_z) * kv;
        let v2 = self.vel_x * self.vel_x + self.vel_z * self.vel_z;
        if v2 > 1e-5 {
            // App extension: fast flight moves in steps of at most 0.4 m, so it can't pass
            // through a wall or a leaf in one frame (one step at the kit's speeds).
            let n = (v2.sqrt() * dt / 0.4).ceil().max(1.0) as usize;
            for _ in 0..n {
                self.pos[0] += self.vel_x * dt / n as f64;
                self.pos[2] += self.vel_z * dt / n as f64;
                self.collide(world);
            }
        }
        if !locked {
            self.yaw += ang(ly, self.yaw) * (dt * f.turn_rate).min(1.0); // the body faces the view; the limbs show the travel direction
        }
        self.last_tier = -1;
        self.last_back = false;
        self.sitting = false;
        self.fidget_t = self.fidget_wait();
        if self.one_shot.is_empty() {
            self.play(m.clip(), true, false);
        }
        if f.stroke_rate != 0.0 && self.cur.as_deref() == Some(m.clip()) {
            // Strokes quicken with speed.
            let hs = (self.vel_x * self.vel_x + self.vel_z * self.vel_z).sqrt();
            if let Some(t) = self.cur_mut() {
                t.speed = (hs / f.speed).clamp(1.0, f.stroke_rate);
            }
        }
    }

    fn fly_vertical(&mut self, world: &dyn World, dt: f64) {
        let f = self.cfg.fly.as_ref().unwrap();
        let (vertical, accel, land, land_below) = (f.vertical, f.accel, f.land_on_ground, f.land_below);
        let mut tv = (if self.held_jump { vertical } else { 0.0 }) - (if self.held_down { vertical } else { 0.0 });
        if self.fly_follow_pitch {
            tv += self.stick_vy;
        }
        self.vy += (tv - self.vy) * (dt * accel).min(1.0);
        self.pos[1] += self.vy * dt;
        let gh = world.ground_height(self.pos[0], self.pos[2], self.pos[1]);
        if self.pos[1] <= gh {
            if land && self.vy < -land_below {
                self.fly_land(gh); // flying down onto the ground ends the flight
                return;
            }
            self.pos[1] = gh;
            if self.vy < 0.0 {
                self.vy = 0.0;
            }
        }
        self.grounded = false;
    }

    fn fly_land(&mut self, gh: f64) {
        let impact = -self.vy;
        let land_impact = self.cfg.fly.as_ref().map_or(f64::MAX, |f| f.land_impact);
        self.set_mode(None);
        self.pos[1] = gh;
        self.vy = 0.0;
        self.grounded = true;
        self.last_ground_y = gh;
        self.falling = false;
        self.events.push("flyEnd");
        self.events.push("land");
        let land = self.n("land");
        if impact > land_impact && !land.is_empty() {
            if self.stick_mag > self.cfg.deadzone {
                self.start_overlay(&land, 0.05);
            } else {
                self.vel_x *= self.cfg.land_keep;
                self.vel_z *= self.cfg.land_keep;
                self.one_shot = land.clone();
                self.play(&land, false, true);
            }
        }
    }

    /// Limb momentum: a spring chases the velocity plus a share of its acceleration, so the
    /// limbs whip back on take-off, trail at speed, and swing past when you stop or turn. Not
    /// in a mode, it settles back to rest.
    fn fly_momentum(&mut self, dt: f64) {
        let Some(mc) = self.mode_cfg else { return };
        if dt <= 0.0 {
            return;
        }
        let (inertia, max_accel, stiff, damp) = {
            let f = self.mode_settings(mc);
            (f.inertia, f.max_accel, f.limb_stiffness, f.limb_damping)
        };
        let v = match self.mode {
            Some(Medium::Fly | Medium::Dive) => [self.vel_x, self.vy, self.vel_z],
            Some(Medium::Swim) => [self.vel_x, 0.0, self.vel_z],
            None => [0.0; 3],
        };
        let m = &mut self.mom;
        for i in 0..3 {
            let a = ((v[i] - m[6 + i]) / dt).clamp(-max_accel, max_accel);
            let g = v[i] + inertia * a;
            m[3 + i] += (stiff * (g - m[i]) - damp * m[3 + i]) * dt;
            m[i] += m[3 + i] * dt;
            m[6 + i] = v[i];
        }
    }

    /// The momentum spring in body space, scaled so 1 = full speed: forward, toward the
    /// character's right, up.
    fn fly_mix(&self, m: Option<Medium>) -> [f64; 3] {
        let Some(m) = m else { return [0.0; 3] };
        let f = self.mode_settings(m);
        let (sy, cy) = (self.yaw.sin(), self.yaw.cos());
        let fv = self.mom[0] * sy + self.mom[2] * cy;
        [fv / if fv < 0.0 { f.speed * f.back_scale } else { f.speed }, (-self.mom[0] * cy + self.mom[2] * sy) / f.speed, self.mom[1] / f.vertical]
    }

    /// A blended mode's clips and weights: its still clip plus the directions the limbs are
    /// being carried.
    fn fly_weights(&self, mix: [f64; 3], m: Medium) -> Vec<(String, f64)> {
        let k = &self.mode_settings(m).clips;
        let [f, r, u] = mix;
        let c = [f.max(0.0), (-f).max(0.0), r.max(0.0), (-r).max(0.0), u.max(0.0), (-u).max(0.0)];
        let n = [&k.forward, &k.back, &k.right, &k.left, &k.up, &k.down];
        let sum: f64 = c.iter().sum();
        let tot = (f * f + r * r + u * u).sqrt().min(1.0);
        let mut out = vec![(k.idle.clone(), 1.0 - tot)];
        for i in 0..6 {
            let wi = if sum > 1e-9 { tot * c[i] / sum } else { 0.0 };
            if n[i].is_empty() {
                out[0].1 += wi; // a mode without that direction (no up or down at the surface) holds still instead
            } else {
                out.push((n[i].clone(), wi));
            }
        }
        out
    }

    fn water_level(&self, world: &dyn World) -> f64 {
        world.water_level(self.pos[0], self.pos[2])
    }

    /// On foot or in the air: wading in waist-deep water, or getting into deep water.
    fn water_step(&mut self, world: &dyn World) {
        self.wading = false;
        let Some(w) = self.cfg.water.as_ref() else { return };
        let (wade_depth, swim_depth, dive_entry) = (w.wade_depth, w.swim_depth, w.dive_entry);
        let s = self.water_level(world);
        if s <= NO_WATER {
            return;
        }
        let p = self.pos;
        let gh = world.ground_height(p[0], p[2], p[1]);
        let dg = s - gh;
        if dg >= swim_depth && p[1] <= s - swim_depth && !self.dead && !self.is_n("death", &self.one_shot) {
            // Falling in fast goes under.
            let m = if !self.grounded && self.vy < -dive_entry { Medium::Dive } else { Medium::Swim };
            self.enter_water(m);
            return;
        }
        // Feet well under the surface, on the ground or hopping.
        self.wading = dg >= wade_depth && p[1] < s - wade_depth * 0.6 && !self.dead;
        if self.wading {
            self.crouching = false;
            self.sitting = false;
            if self.prone && !self.in_prone_move() {
                self.set_prone(false);
            }
        }
    }

    fn enter_water(&mut self, m: Medium) {
        let keep = self.cfg.water.as_ref().map_or(0.0, |w| w.entry_keep);
        self.crouching = false;
        self.sitting = false;
        self.falling = false;
        self.prone = false;
        self.wading = false;
        if !self.is_n("death", &self.one_shot) {
            self.one_shot.clear();
        }
        self.vy = if m == Medium::Dive { self.vy * keep } else { 0.0 };
        self.grounded = false;
        self.set_mode(Some(m));
        self.play(m.clip(), true, false);
        self.events.push("enterWater");
    }

    fn leave_water(&mut self, world: &dyn World) {
        let gh = world.ground_height(self.pos[0], self.pos[2], self.pos[1]);
        self.set_mode(None);
        self.stick_vy = 0.0;
        if self.pos[1] <= gh + 1e-6 {
            self.pos[1] = gh;
            self.grounded = true;
            self.last_ground_y = gh;
            self.vy = 0.0;
        } else {
            self.grounded = false;
        }
        self.events.push("exitWater");
    }

    /// At the surface: float at swimming depth, riding a little higher at speed; Down dives;
    /// shallower water stands you up.
    fn swim_vertical(&mut self, world: &dyn World, dt: f64) {
        let w = self.cfg.water.as_ref().unwrap();
        let (swim_depth, exit_margin, dive_v) = (w.swim_depth, w.exit_margin, w.dive.vertical);
        let (speed, plane_lift, float_rate) = (w.swim.speed, w.swim.plane_lift, w.swim.float_rate);
        let s = self.water_level(world);
        if s <= NO_WATER {
            self.leave_water(world);
            return;
        }
        let gh = world.ground_height(self.pos[0], self.pos[2], self.pos[1]);
        if s - gh < swim_depth - exit_margin {
            self.leave_water(world);
            return;
        }
        if self.held_down {
            self.set_mode(Some(Medium::Dive));
            self.vy = -dive_v * 0.6;
            self.play(Medium::Dive.clip(), true, false);
            self.events.push("dive");
            self.grounded = false;
            return;
        }
        let hs = (self.vel_x * self.vel_x + self.vel_z * self.vel_z).sqrt();
        let tgt = s - swim_depth + plane_lift * (hs / speed).min(1.0);
        let mut ny = self.pos[1] + (tgt - self.pos[1]) * (dt * float_rate).min(1.0);
        if ny < gh {
            ny = gh;
        }
        self.vy = if dt > 0.0 { (ny - self.pos[1]) / dt } else { 0.0 };
        self.pos[1] = ny;
        self.grounded = false;
    }

    /// Underwater: free movement (forward follows the look pitch), Jump up, Down down,
    /// drifting up when idle; reaching the surface swims.
    fn dive_vertical(&mut self, world: &dyn World, dt: f64) {
        let w = self.cfg.water.as_ref().unwrap();
        let (swim_depth, exit_margin) = (w.swim_depth, w.exit_margin);
        let (vertical, buoyancy, accel) = (w.dive.vertical, w.dive.buoyancy, w.dive.accel);
        let vin = (if self.held_jump { vertical } else { 0.0 }) - (if self.held_down { vertical } else { 0.0 });
        let mut tv = self.stick_vy + vin;
        if vin == 0.0 && !self.stick_in {
            tv = buoyancy;
        }
        self.vy += (tv - self.vy) * (dt * accel).min(1.0);
        self.pos[1] += self.vy * dt;
        let s = self.water_level(world);
        let gh = world.ground_height(self.pos[0], self.pos[2], self.pos[1]);
        if s <= NO_WATER || s - gh < swim_depth - exit_margin {
            self.leave_water(world);
            return;
        }
        if self.pos[1] < gh {
            self.pos[1] = gh;
            if self.vy < 0.0 {
                self.vy = 0.0;
            }
        }
        if self.pos[1] >= s - swim_depth && self.vy >= 0.0 {
            self.pos[1] = s - swim_depth;
            self.vy = 0.0;
            self.set_mode(Some(Medium::Swim));
            self.play(Medium::Swim.clip(), true, false);
            self.events.push("surface");
        }
        self.grounded = false;
    }

    // ---------------------------------------------------------------- the frame

    /// Advance one frame.
    pub fn update(&mut self, world: &dyn World, dt_in: f64, input: &Input) -> Output {
        let dz = self.cfg.deadzone;
        let raw = dt_in.clamp(0.0, 0.05);
        let dt = raw * self.time_scale;
        self.gt += dt;
        self.events.clear();
        self.look_yaw = input.look_yaw;
        self.look_pitch = input.look_pitch.clamp(self.cfg.pitch_limits[0], self.cfg.pitch_limits[1]);
        let jx = input.move_x;
        let jy = -input.move_y;
        let mag = (jx * jx + jy * jy).sqrt().min(1.0);
        let ly = self.look_yaw;
        let (fx, fz) = (ly.sin(), ly.cos());
        let (rx, rz) = (-ly.cos(), ly.sin());

        if mag > dz && self.cfg.revive_on_move && (self.dead || self.is_n("death", &self.one_shot)) {
            self.dead = false;
            self.one_shot.clear();
        }
        self.stick_mag = mag;
        if mag > dz {
            self.stick_x = (fx * -jy + rx * jx) / mag;
            self.stick_z = (fz * -jy + rz * jx) / mag;
            self.stick_back = (-jy / mag) < self.cfg.back_at;
        }
        let jump_in = input.jump;
        if jump_in && !self.jump_prev {
            // Double-tap Jump toggles flight.
            let tap = self.cfg.fly.as_ref().map_or(0.0, |f| f.double_tap_jump);
            if tap > 0.0 && self.gt - self.last_jump_tap <= tap && self.set_fly(world, !self.flying) {
                self.last_jump_tap = -9.0;
                self.jump_latch = !self.flying;
            } else {
                self.last_jump_tap = self.gt;
            }
        }
        self.jump_prev = jump_in;
        if !jump_in {
            self.jump_latch = false;
        }
        self.held_jump = jump_in && !self.jump_latch;
        self.held_roll = input.roll;
        self.held_down = input.down;
        let mut q = std::mem::take(&mut self.queue);
        q.extend(input.actions.iter().cloned());
        for name in &q {
            self.do_press(world, name);
        }
        if !self.dead && self.mode.is_none() {
            // Held buttons keep firing.
            if self.held_roll {
                self.act(world, "roll");
            }
            if self.held_jump {
                self.act(world, "jump");
            }
        } else if !self.dead && self.mode == Some(Medium::Swim) && self.held_jump {
            self.try_climb(world); // Jump at the surface climbs out onto a ledge in reach
        }
        self.held_punch = input.punch;
        if self.held_punch && !self.dead {
            if let Some(repeat_at) = self.cfg.punch_repeat_at {
                // Holding punch keeps throwing jabs, alternating hands.
                let again = match &self.overlay {
                    None => true,
                    Some((clip, time)) => !self.is_punch(clip) || *time >= repeat_at,
                };
                if again {
                    self.act(world, "punch");
                }
            }
        }
        self.held_mine = input.mine;
        if self.held_mine && !self.dead {
            self.mine(); // breaking blocks: a strike that repeats while held
        }

        if self.is_roll(&self.one_shot) {
            if let Some(ct) = self.cur_time() {
                self.yaw += ang(self.roll_yaw, self.yaw) * (dt * self.cfg.roll_turn_rate).min(1.0); // turn into the roll
                if ct > self.cfg.roll_link_at.get(&self.one_shot).copied().unwrap_or(f64::MAX) && mag > dz {
                    // Still pushing the stick: come out of the roll moving.
                    let ks = self.cfg.speed(if self.roll_back { "runBack" } else { "sprint" });
                    self.one_shot.clear();
                    self.vel_x = self.roll_x * ks;
                    self.vel_z = self.roll_z * ks;
                }
            }
        }
        if !self.one_shot.is_empty() && self.grounded && mag > dz {
            if let Some(ct) = self.cur_time() {
                let landing = self.is_n("land", &self.one_shot)
                    || (self.is_jump(&self.one_shot) && self.jump_fired && ct >= self.cfg.windows[&self.one_shot][1]);
                if landing {
                    // Started moving mid-landing: legs go to the stride.
                    let clip = self.one_shot.clone();
                    self.start_overlay(&clip, ct);
                    self.ov_w = 1.0;
                    self.one_shot.clear();
                }
            }
        }
        if !self.one_shot.is_empty() && self.intr.contains(&self.one_shot) && mag > dz {
            self.one_shot.clear();
        }

        let locked = self.dead || (!self.one_shot.is_empty() && !self.is_air(&self.one_shot));
        let moving = mag > dz && !locked;
        if self.mode.is_some() {
            self.fly_move(world, dt, jx, jy, mag, locked);
        } else {
            let (mut dvx, mut dvz) = (0.0, 0.0);
            if moving {
                self.sitting = false;
                let c = &self.cfg;
                let mut tier: i32 = if mag > c.sprint_at { 2 } else if mag > c.jog_at { 1 } else { 0 };
                let mut sp = c.speed(match tier {
                    2 => "sprint",
                    1 => "jog",
                    _ => "walk",
                });
                let mut mx = (fx * -jy + rx * jx) / mag;
                let mut mz = (fz * -jy + rz * jx) / mag;
                // The view sets the heading; the body turns up to bodyTurnLimit under it toward
                // the travel direction.
                let fa = -jy / mag;
                let sa = jx / mag;
                let mut back = fa < c.back_at && !c.role("walkBack").is_empty();
                let mut back_clip = "";
                let off = (if back { (-sa).atan2(-fa) } else { sa.atan2(fa) }).clamp(-c.body_turn_limit, c.body_turn_limit);
                let mut want = (fx * off.cos() + rx * off.sin()).atan2(fz * off.cos() + rz * off.sin());
                if back {
                    let rb = mag > c.back_run_at && !c.role("runBack").is_empty();
                    back_clip = c.role(if rb { "runBack" } else { "walkBack" });
                    sp = c.speed(if rb { "runBack" } else { "walkBack" });
                    tier = -1;
                }
                let mut cclip = "";
                if self.crouching && !c.role("crouchWalk").is_empty() {
                    // Crouched: one gait each way.
                    let cb = back && !c.role("crouchBack").is_empty();
                    cclip = c.role(if cb { "crouchBack" } else { "crouchWalk" });
                    sp = c.speed(if back { "crouchBack" } else { "crouch" });
                    tier = -1;
                    back = false;
                }
                if self.prone && !c.role("crawl").is_empty() {
                    // Lying down: the body keeps facing where you look; the stick picks forward,
                    // back or sideways.
                    cclip = c.role("crawl");
                    sp = c.speed("crawl");
                    tier = -1;
                    back = false;
                    want = fx.atan2(fz);
                    if sa.abs() > fa.abs() && !c.role("crawlRight").is_empty() && !c.role("crawlLeft").is_empty() {
                        let sg = if sa > 0.0 { 1.0 } else { -1.0 };
                        cclip = c.role(if sg > 0.0 { "crawlRight" } else { "crawlLeft" });
                        sp = c.speed("crawlSide");
                        mx = rx * sg;
                        mz = rz * sg;
                    } else if fa < 0.0 && !c.role("crawlBack").is_empty() {
                        cclip = c.role("crawlBack");
                        sp = c.speed("crawlBack");
                        mx = -fx;
                        mz = -fz;
                    } else {
                        mx = fx;
                        mz = fz;
                    }
                }
                dvx = mx * sp;
                dvz = mz * sp;
                let rate = if self.prone { c.prone_turn_rate } else { c.turn_rate };
                let clip = if !cclip.is_empty() {
                    cclip
                } else if back {
                    back_clip
                } else {
                    c.role(match tier {
                        2 => "sprint",
                        1 => "jog",
                        _ => "walk",
                    })
                }
                .to_string();
                self.yaw += ang(want, self.yaw) * (dt * rate).min(1.0);
                self.last_tier = tier;
                self.last_back = back;
                if self.one_shot.is_empty() {
                    self.play(&clip, true, false);
                }
                self.fidget_t = self.fidget_wait();
            } else {
                self.last_tier = -1;
                if self.one_shot.is_empty() {
                    self.last_back = false;
                }
                if !locked {
                    // Standing: the body follows once the view turns past the limit.
                    let dd = ang(ly, self.yaw);
                    let limit = self.cfg.body_turn_limit;
                    if dd > limit {
                        self.yaw = ly - limit;
                    } else if dd < -limit {
                        self.yaw = ly + limit;
                    }
                }
            }
            if !moving && self.one_shot.is_empty() && !self.dead {
                let idle = self.n("idle");
                let pick = |role: &str, on: bool| Some(self.n(role)).filter(|n| on && !n.is_empty());
                let idle_clip = pick("proneIdle", self.prone).or(pick("crouch", self.crouching)).or(pick("sit", self.sitting)).unwrap_or_else(|| idle.clone());
                self.play(&idle_clip, true, false);
                let fidget = self.n("fidget");
                if idle_clip == idle && !fidget.is_empty() && self.grounded && self.overlay.is_none() {
                    self.fidget_t -= dt;
                    if self.fidget_t <= 0.0 {
                        self.fidget_t = self.fidget_wait();
                        self.one_shot = fidget.clone();
                        self.play(&fidget, false, true);
                    }
                } else {
                    self.fidget_t = self.fidget_wait();
                }
            }
            // Ease speed in and out so starts, stops and gear changes are not instant.
            if locked {
                self.vel_x = 0.0;
                self.vel_z = 0.0;
            } else {
                let on_ground = self.grounded && !self.is_jump(&self.one_shot);
                let kv = (dt * if on_ground { self.cfg.accel } else { self.cfg.air_accel }).min(1.0); // little steering in the air
                self.vel_x += (dvx - self.vel_x) * kv;
                self.vel_z += (dvz - self.vel_z) * kv;
                if self.vel_x * self.vel_x + self.vel_z * self.vel_z > 1e-5 {
                    self.pos[0] += self.vel_x * dt;
                    self.pos[2] += self.vel_z * dt;
                    self.collide(world);
                }
            }
        }
        // Upper-body action layer weight.
        if let Some((clip, time)) = self.overlay.clone() {
            let od = self.d(&clip);
            let (fade_in, fade_out) = (self.cfg.fade("upperIn"), self.cfg.fade("upperOut"));
            let mining = self.cfg.mine_hit_at.contains_key(&clip) && self.held_mine;
            let punching = self.held_punch && self.is_punch(&clip);
            let otgt = if time < od - fade_out || mining || punching { 1.0 } else { 0.0 };
            let step = (otgt - self.ov_w).abs().min(dt / if otgt > 0.0 { fade_in } else { fade_out });
            self.ov_w += if otgt > self.ov_w { step } else { -step };
            if otgt == 0.0 && self.ov_w <= 0.001 {
                self.stop_overlay();
            }
        }
        // Carry pose envelope: fades with state changes, and lets go while the hands are needed.
        {
            if self.hold_cur != self.hold_want && self.hold_w <= 0.0 {
                self.hold_cur = self.hold_want.clone();
                self.hold_t = 0.0;
            }
            let state_clip = self.cfg.hold_states.get(&self.hold_cur).map(|s| s.clip.clone());
            let htgt = if state_clip.is_some() && self.hold_cur == self.hold_want && !self.hold_suspended() { 1.0 } else { 0.0 };
            let step = (htgt - self.hold_w).abs().min(dt / self.cfg.hold_fade);
            self.hold_w += if htgt > self.hold_w { step } else { -step };
            if let Some(clip) = state_clip {
                self.hold_t = (self.hold_t + dt) % self.d(&clip);
            }
        }
        // Wading: the arms-out-over-the-water pose fades in over the upper body while the legs
        // keep their normal gait.
        let wade = self.n("wadeArms");
        if !wade.is_empty() && self.dur.contains_key(&wade) {
            let fade = self.cfg.water.as_ref().map_or(0.3, |w| w.wade_fade);
            let wtg = if self.wading { 1.0 } else { 0.0 };
            let step = (wtg - self.wade_w).abs().min(dt / fade);
            self.wade_w += if wtg > self.wade_w { step } else { -step };
            self.wade_t = (self.wade_t + dt) % self.d(&wade);
        }
        self.advance(dt);

        // ---- vertical: flying, swimming, diving; or climbing, ground, jumping, falling ----
        match self.mode {
            Some(Medium::Fly) => self.fly_vertical(world, dt),
            Some(Medium::Swim) => self.swim_vertical(world, dt),
            Some(Medium::Dive) => self.dive_vertical(world, dt),
            None => {
                let climb_clip = self.n("climb");
                if let (Some(cl), Some(ct), true) = (self.climbing, self.cur_time(), self.one_shot == climb_clip && !climb_clip.is_empty()) {
                    let c = &self.cfg;
                    let yr = if cl.t0 == 0.0 && ct < cl.tg {
                        (cl.h - cl.top) * smooth(ct / cl.tg) // small hop up to the edge
                    } else if ct < c.grab_until {
                        self.grab(&cl, ct) // fingertips stay on the edge while the body rises
                    } else {
                        let u2 = smoother(((ct - c.grab_until) / (c.top_at - c.grab_until)).min(1.0));
                        let g = self.grab(&cl, c.grab_until);
                        g + (cl.h - g) * u2 // ease up onto the top
                    };
                    // Hang out from the face so arms and knees stay clear of it; move in once the feet
                    // are nearly level with the top.
                    let uh = smooth(((ct - cl.t0) / 0.1).clamp(0.0, 1.0));
                    let uf = smoother(((ct - c.move_in[0]) / (c.move_in[1] - c.move_in[0])).clamp(0.0, 1.0));
                    let bx = cl.sx + (cl.hx - cl.sx) * uh;
                    let bz = cl.sz + (cl.hz - cl.sz) * uh;
                    self.pos[0] = bx + (cl.tx - bx) * uf;
                    self.pos[2] = bz + (cl.tz - bz) * uf;
                    self.pos[1] = cl.y0 + yr;
                    self.vy = 0.0;
                    self.grounded = true;
                    self.yaw += ang(cl.yaw, self.yaw) * (dt * 18.0).min(1.0);
                } else if let Some(cl) = self.climbing {
                    self.pos = [cl.tx, cl.y0 + cl.h, cl.tz];
                    self.climbing = None;
                    self.grounded = true;
                    self.vy = 0.0;
                    self.events.push("climbEnd");
                }
                let mut gh = world.ground_height(self.pos[0], self.pos[2], self.pos[1]);
                if self.grounded && self.climbing.is_none() {
                    if self.pos[1] > gh + 0.02 && self.pos[1] - gh > self.snap_down {
                        self.grounded = false;
                        if gh > self.pos[1] - 0.35 {
                            self.last_ground_y = gh;
                        }
                    } else {
                        self.pos[1] = gh;
                        self.last_ground_y = gh;
                    }
                }
                if !self.grounded && self.climbing.is_none() && self.vy < self.cfg.air_grab_max_rise && self.held_jump {
                    self.try_climb(world); // holding Jump in the air grabs a ledge in reach
                }
                let cn = self.cur.clone().unwrap_or_default();
                let window = self.cfg.windows.get(&cn).copied();
                let owns = window.is_some() && self.one_shot == cn;
                let (jw0, jw1) = window.map_or((0.0, 0.0), |w| (w[0], w[1]));
                if owns && self.grounded && !self.jump_fired && self.cur_time().is_some_and(|t| t >= jw0) {
                    self.last_ground_y = self.pos[1];
                    self.vy = self.jump_v;
                    self.grounded = false;
                    self.jump_fired = true;
                    self.events.push("takeoff");
                }
                if !self.grounded && self.climbing.is_none() {
                    let sinking = self.cfg.water.as_ref().filter(|w| (self.dead || self.is_n("death", &self.one_shot)) && self.water_level(world) > self.pos[1] + w.swim_depth * 0.5).map(|w| w.sink_speed);
                    if let Some(sink) = sinking {
                        // A body in deep water sinks slowly.
                        self.vy += (-sink - self.vy) * (dt * 3.0).min(1.0);
                        self.pos[1] += self.vy * dt;
                    } else {
                        let y0 = self.pos[1];
                        self.pos[1] += self.vy * dt - 0.5 * self.grav * dt * dt;
                        self.vy -= self.grav * dt;
                        // The head bumps into what is above instead of going into it.
                        if self.pos[1] > y0 {
                            if let Some(top) = world.head_room(self.pos[0], self.pos[2], y0, self.pos[1], self.cfg.radius) {
                                self.pos[1] = top.max(y0);
                                self.vy = self.vy.min(0.0);
                            }
                        }
                    }
                    gh = world.ground_height(self.pos[0], self.pos[2], self.pos[1]);
                    if self.vy <= 0.0 && self.pos[1] <= gh {
                        self.pos[1] = gh;
                        self.vy = 0.0;
                        self.grounded = true;
                        self.events.push("land");
                        let land = self.n("land");
                        if owns {
                            if self.cur_time().is_some_and(|t| t < jw1) {
                                self.seek(jw1); // touched down early: skip to the landing
                            }
                            if self.gt - self.jump_buf < self.cfg.buffer_time {
                                self.jump_buf = -9.0;
                                self.chain_jump(); // jump was pressed just before touchdown
                            } else if self.stick_mag > dz {
                                // Landing on the move: legs go straight back into the stride on the
                                // landing foot; the rest of the landing plays on the upper body.
                                self.start_overlay(&cn, jw1);
                                self.ov_w = 1.0;
                                self.one_shot.clear();
                            } else {
                                // Landing still: shed leftover drift so planted feet do not skate.
                                self.vel_x *= self.cfg.land_keep;
                                self.vel_z *= self.cfg.land_keep;
                            }
                        } else if self.falling {
                            self.falling = false;
                            if self.stick_mag > dz {
                                self.one_shot.clear();
                                if !land.is_empty() {
                                    self.start_overlay(&land, 0.05);
                                }
                            } else {
                                self.vel_x *= self.cfg.land_keep;
                                self.vel_z *= self.cfg.land_keep;
                                if !land.is_empty() {
                                    self.one_shot = land.clone();
                                    self.play(&land, false, true);
                                } else {
                                    self.one_shot.clear();
                                }
                            }
                        }
                    } else {
                        if owns && self.jump_fired {
                            if let Some(t) = self.cur_mut() {
                                if t.time > jw1 - 0.04 {
                                    t.time = jw1 - 0.04; // still in the air: hold the reach pose
                                }
                            }
                        }
                        let fall = self.n("fall");
                        if !self.is_jump(&self.one_shot) && !self.falling && !self.dead && !self.is_n("death", &self.one_shot) && !fall.is_empty() && self.pos[1] - gh > 0.3 {
                            self.falling = true;
                            self.prone = false;
                            self.sitting = false;
                            self.stop_overlay();
                            self.one_shot = fall.clone();
                            self.play(&fall, true, false);
                        }
                    }
                }
                if self.climbing.is_none() {
                    self.water_step(world);
                }
            }
        }
        self.fly_momentum(dt);
        if self.is_roll(&self.one_shot) && self.cur_time().is_some_and(|t| t > self.cfg.roll_travel[0] && t < self.cfg.roll_travel[1]) {
            let s = self.cfg.speed("roll");
            self.pos[0] += self.roll_x * s * dt;
            self.pos[2] += self.roll_z * s * dt;
            self.collide(world);
        }

        // ---- head turn toward the look direction (third-person follow) ----
        let hoff = ang(ly, self.yaw).clamp(-self.cfg.head_turn_limit, self.cfg.head_turn_limit);
        let head_free = !self.dead && !self.is_n("death", &self.one_shot) && !self.is_roll(&self.one_shot) && !self.prone && !self.in_prone_move();
        self.head_look = (head_free && self.view == View::Follow).then(|| [hoff, self.look_pitch * self.cfg.head_pitch_share]);

        let face = self.face(dt);

        // Camera target follows the body, smoothed in real time.
        let th = self.cfg.target_height;
        self.target[1] += (th + self.pos[1] - self.target[1]) * (raw * 6.0).min(1.0);
        self.target[0] += (self.pos[0] - self.target[0]) * (raw * 10.0).min(1.0);
        self.target[2] += (self.pos[2] - self.target[2]) * (raw * 10.0).min(1.0);

        // ---- output ----
        let sum: f64 = self.tracks.iter().map(|t| t.w).sum();
        let cur = self.cur.clone().unwrap_or_default();
        let mix = self.fly_mix(self.mode_cfg);
        let mut shown = cur.clone();
        let mut best = -1.0;
        let mut body = Vec::new();
        for t in &self.tracks {
            let tw = if sum > 0.0 { t.w / sum } else if t.clip == cur { 1.0 } else { 0.0 };
            let Some(tm) = self.mode_named(&t.clip) else {
                body.push(Layer { clip: t.clip.clone(), time: t.time.min(self.d(&t.clip)), weight: tw });
                continue;
            };
            // A blended mode: its still clip plus the directions the limbs are being carried.
            for (clip, w) in self.fly_weights(self.fly_mix(Some(tm)), tm) {
                if w > 0.0 {
                    if t.clip == cur && w > best {
                        best = w;
                        shown = clip.clone();
                    }
                    body.push(Layer { time: t.time.min(self.d(&clip)), clip, weight: tw * w });
                }
            }
        }
        // The whole body tilts into the direction of travel, around a pivot at the hips
        // (swimming lays it out flat).
        let mut lean = [0.0; 2];
        if let Some(m) = self.mode_cfg {
            let f = self.mode_settings(m);
            let lf = mix[0].clamp(-f.lean_max, f.lean_max);
            let lr = mix[1].clamp(-f.lean_max, f.lean_max);
            let ld = (-mix[2]).clamp(0.0, f.lean_max);
            lean = [(lf * if lf > 0.0 { f.lean_forward } else { f.lean_back } + ld * f.lean_down).clamp(-f.lean_limit, f.lean_limit), lr * f.lean_side];
        }
        let upper = self.overlay.as_ref().map(|(clip, time)| Layer { clip: clip.clone(), time: *time, weight: self.ov_w });
        // Carry pose: an upper-body action takes the arms back unless it is one the pose is
        // kept during.
        let hold = self.cfg.hold_states.get(&self.hold_cur).filter(|_| self.hold_w > 0.0).and_then(|hst| {
            let mut hw = self.hold_w;
            if let Some((clip, _)) = &self.overlay {
                if self.upper.contains(clip) && !hst.keep_during.contains(clip) {
                    hw *= 1.0 - self.ov_w;
                }
            }
            (hw > 0.0).then(|| (Layer { clip: hst.clip.clone(), time: self.hold_t, weight: hw }, hst.bones.clone()))
        });
        let wade = (self.wade_w > 0.0).then(|| Layer { clip: self.n("wadeArms"), time: self.wade_t, weight: self.wade_w });
        let upper_left = match (&self.overlay, self.both_hands) {
            (Some((_, time)), true) => Some(Layer { clip: self.n("interactLeft"), time: *time, weight: self.ov_w }),
            _ => None,
        };
        let state = if self.dead {
            "dead".to_string()
        } else if self.climbing.is_some() {
            "climb".to_string()
        } else {
            match self.mode {
                Some(Medium::Fly) => "fly".to_string(),
                Some(Medium::Swim) => "swim".to_string(),
                Some(Medium::Dive) => "dive".to_string(),
                None => shown.clone(),
            }
        };
        Output {
            position: self.pos,
            yaw: self.yaw,
            body,
            upper,
            hold,
            wade,
            upper_left,
            lean,
            clip: shown,
            flying: self.flying,
            swimming: matches!(self.mode, Some(Medium::Swim | Medium::Dive)),
            underwater: self.mode == Some(Medium::Dive),
            wading: self.wading,
            velocity: [self.vel_x, self.vy, self.vel_z],
            head_look: self.head_look,
            face,
            events: self.events.clone(),
            view: self.view,
            state,
            clip_time: self.cur_time().unwrap_or(0.0),
            grounded: self.grounded,
            speed: (self.vel_x * self.vel_x + self.vel_z * self.vel_z).sqrt(),
        }
    }

    // ---------------------------------------------------------------- face: blinks, glances and expressions

    fn face(&mut self, dt: f64) -> Face {
        let cn = self.cur.clone().unwrap_or_default();
        let (on, on_time) = self.overlay.clone().unwrap_or_default();
        let ct = self.cur_time().unwrap_or(0.0);
        self.f.air = if self.grounded { 0.0 } else { self.f.air + dt };
        let is = |role: &str| self.is_n(role, &cn) || self.is_n(role, &on);
        // Expression for what the body is doing: squint 0..1, brow lift -1..1, brow tilt
        // (+ = angry), mouth sprite.
        let (mut sq, mut br, mut br_r, mut m) = (0.0, 0.0, 0.0, "closed");
        let mut look: Option<(f64, f64)> = None;
        if self.dead || is("death") {
            sq = if self.dead || ct > 0.4 { 1.0 } else { 0.7 };
            m = if self.dead { "closed_alt" } else { "wide" };
            br = -0.2;
            br_r = -0.6;
        } else if is("hit") {
            (sq, br, br_r, m) = (0.9, -0.5, -0.8, "grin");
        } else if self.is_roll(&cn) {
            (sq, br, br_r, m) = (0.8, -0.6, 0.4, "grin");
        } else if self.attack.contains(&cn) || self.attack.contains(&on) {
            (sq, br, br_r, m) = (0.28, -0.8, 1.0, "grin");
        } else if self.is_n("climb", &cn) {
            (sq, br, br_r, m) = (0.35, -0.5, 0.7, "grin");
            look = Some((0.0, 0.45));
        } else if self.mode == Some(Medium::Dive) {
            (sq, br, br_r, m) = (0.45, -0.15, 0.2, "closed"); // holding breath, eyes narrowed
        } else if self.mode == Some(Medium::Swim) {
            let sm = self.fly_mix(self.mode_cfg);
            let shs = (sm[0] * sm[0] + sm[1] * sm[1]).sqrt().min(1.0);
            (sq, br) = (0.15 + 0.2 * shs, 0.1);
            m = if shs > 0.6 { "small" } else { "closed" };
        } else if self.flying {
            // Wind in the face at speed; brows up when dropping.
            let fm = self.fly_mix(self.mode_cfg);
            let hsp = (fm[0] * fm[0] + fm[1] * fm[1]).sqrt().min(1.0);
            (sq, br, br_r) = (0.06 + 0.3 * hsp, 0.2 + 0.5 * (-fm[2]).max(0.0), -0.1);
            m = if hsp > 0.75 || fm[2] < -0.6 { "small" } else { "closed" };
        } else if self.falling || self.is_n("fall", &cn) {
            (br, br_r) = (1.0, -0.5);
            m = if self.f.air > 0.45 { "wide" } else { "small" };
            look = Some((0.0, -0.4));
        } else if self.is_jump(&cn) && !self.grounded {
            (br, br_r, m) = (0.55, -0.2, "small");
        } else if is("land") {
            (sq, br, m) = (0.55, -0.3, "grin");
        } else if self.is_n("sprint", &cn) || self.is_n("runBack", &cn) {
            (sq, br, br_r, m) = (0.2, -0.25, 0.3, "small");
        } else if self.is_crawl(&cn) {
            (sq, br, br_r) = (0.18, -0.3, 0.4);
        } else if self.is_n("crouch", &cn) || self.is_n("crouchWalk", &cn) || self.is_n("crouchBack", &cn) {
            (sq, br, br_r) = (0.22, -0.35, 0.45);
        } else if is("eat") {
            let et = if self.is_n("eat", &on) { on_time } else { ct };
            let ep = et % 0.4;
            m = if et > 0.2 && et < 1.45 && ep > 0.1 && ep < 0.24 { "wide" } else { "closed" };
            look = Some((0.0, -0.35));
        } else if is("wave") {
            (br, m) = (0.6, "small");
        } else if self.is_n("sit", &cn) {
            sq = 0.15;
        }
        // Blinking.
        let c = &self.cfg;
        let (close, hold, open) = (c.blink_close, c.blink_hold, c.blink_open);
        let (every, double) = (c.blink_every, c.blink_double);
        let s = &mut self.f;
        s.blink_t -= dt;
        if s.bp < 0.0 && s.blink_t <= 0.0 {
            s.bp = 0.0;
            s.dbl = (self.rand)() < double;
        }
        let mut bl = 0.0;
        if s.bp >= 0.0 {
            s.bp += dt;
            let bp = s.bp;
            bl = if bp < close {
                bp / close
            } else if bp < close + hold {
                1.0
            } else {
                (1.0 - (bp - close - hold) / open).max(0.0)
            };
            bl = smooth(bl);
            if bp >= close + hold + open {
                s.bp = -1.0;
                if s.dbl {
                    s.dbl = false;
                    s.blink_t = 0.1;
                } else {
                    s.blink_t = every[0] + (self.rand)() * (every[1] - every[0]);
                }
            }
        }
        // Gaze: the eyes cover whatever part of the look direction the head has not turned to.
        if let Some((lx, ly)) = look {
            s.tx = lx;
            s.ty = ly;
        } else {
            let head_yaw = self.yaw + self.head_look.map_or(0.0, |h| h[0]);
            let head_pitch = self.head_look.map_or(0.0, |h| h[1]);
            let gx = -ang(self.look_yaw, head_yaw);
            let gy = self.look_pitch - head_pitch;
            let still = self.stick_mag <= c.deadzone && gx.abs() < 0.08;
            if still {
                // Nothing to track: idle glances.
                s.sac_t -= dt;
                if s.sac_t <= 0.0 {
                    s.sac_t = 0.7 + (self.rand)() * 2.1;
                    let (ox, oy) = (s.tx, s.ty);
                    s.tx = -0.75 + (self.rand)() * 1.5;
                    s.ty = -0.35 + (self.rand)() * 0.65;
                    if (s.tx - ox).abs() + (s.ty - oy).abs() > 0.9 && s.bp < 0.0 && (self.rand)() < 0.3 {
                        s.blink_t = 0.0; // big glances often come with a blink
                    }
                }
            } else if gx.abs() < 1.4 {
                s.tx = (gx / c.gaze_range[0]).clamp(-1.0, 1.0);
                s.ty = (gy / c.gaze_range[1]).clamp(-1.0, 1.0);
            }
        }
        let k = (dt * 24.0).min(1.0);
        s.gx += (s.tx - s.gx) * k;
        s.gy += (s.ty - s.gy) * k; // quick saccades
        let ke = (dt * 12.0).min(1.0);
        s.sq += (sq - s.sq) * ke;
        s.br += (br - s.br) * ke;
        s.br_r += (br_r - s.br_r) * ke;
        let lid = bl.max(s.sq);
        Face {
            lid: lid.max(0.001),
            pupil: [s.gx * c.pupil_x, if s.gy > 0.0 { s.gy * c.pupil_up } else { s.gy * c.pupil_down }],
            brow_lift: s.br * c.brow_lift - lid * 0.25 * c.brow_lift,
            brow_tilt: s.br_r * c.brow_tilt,
            mouth: m,
            mouth_uv: c.mouth_frames.get(m).copied().unwrap_or([0.0, 0.0]),
        }
    }

    // ---------------------------------------------------------------- camera

    /// Call after the character has been posed. `head_pos` is the head's place in the world
    /// (needed for first person); `aspect` is width / height.
    pub fn camera(&mut self, raw_dt: f64, head_pos: [f64; 3], aspect: f64) -> Camera {
        let c = &self.cfg;
        let ly = self.look_yaw;
        let pd = -self.look_pitch;
        let (hx, hz) = (ly.sin(), ly.cos());
        let (cp, sp) = (pd.cos(), pd.sin());
        if self.view == View::First {
            let e = [head_pos[0] + hx * c.first_forward, head_pos[1] + c.first_up, head_pos[2] + hz * c.first_forward];
            if !self.eye_init {
                self.eye = e;
                self.eye_init = true;
            } else {
                let k = (raw_dt * c.first_smoothing).min(1.0);
                for i in 0..3 {
                    self.eye[i] += (e[i] - self.eye[i]) * k;
                }
            }
            let eye = self.eye;
            return Camera {
                position: eye,
                target: [eye[0] + hx * cp, eye[1] - sp, eye[2] + hz * cp],
                fov: if aspect > 1.0 { c.first_fov_landscape } else { c.first_fov_portrait },
                near: c.first_near,
                hide_head: true,
            };
        }
        let (rx, rz) = (-ly.cos(), ly.sin());
        let fd = self.cam_dist;
        let px = self.target[0] + rx * c.shoulder;
        let py = self.target[1] + (c.follow_height - c.target_height);
        let pz = self.target[2] + rz * c.shoulder;
        let floor_y = self.target[1] - c.target_height + c.min_above_ground;
        Camera {
            position: [px - hx * cp * fd, floor_y.max(py + sp * fd), pz - hz * cp * fd],
            target: [px + hx * cp * c.look_ahead, py - sp * c.look_ahead, pz + hz * cp * c.look_ahead],
            fov: c.follow_fov,
            near: c.follow_near,
            hide_head: false,
        }
    }

    /// The point the follow camera orbits (over the right shoulder), for keeping the camera
    /// out of walls.
    pub fn camera_pivot(&self) -> [f64; 3] {
        let c = &self.cfg;
        let (rx, rz) = (-self.look_yaw.cos(), self.look_yaw.sin());
        [self.target[0] + rx * c.shoulder, self.target[1] + (c.follow_height - c.target_height), self.target[2] + rz * c.shoulder]
    }
}
