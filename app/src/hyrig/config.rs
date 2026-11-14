//! The state machine's numbers (`hyrig_controller.json`), read once into plain structs.

use std::collections::HashMap;

use serde_json::Value;

const JSON: &str = include_str!("../../../assets/hyrig/hyrig_controller.json");

fn at<'a>(v: &'a Value, path: &str) -> &'a Value {
    let mut cur = v;
    for key in path.split('.') {
        cur = cur.get(key).unwrap_or_else(|| panic!("hyrig config: missing {path}"));
    }
    cur
}

fn num(v: &Value, path: &str) -> f64 {
    at(v, path).as_f64().unwrap_or_else(|| panic!("hyrig config: {path} is not a number"))
}

fn nums(v: &Value, path: &str) -> Vec<f64> {
    at(v, path).as_array().unwrap_or_else(|| panic!("hyrig config: {path} is not a list")).iter().filter_map(Value::as_f64).collect()
}

fn pair(v: &Value, path: &str) -> [f64; 2] {
    let n = nums(v, path);
    [n[0], n[1]]
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn text(v: &Value, path: &str) -> String {
    at(v, path).as_str().unwrap_or_else(|| panic!("hyrig config: {path} is not text")).to_string()
}

fn map<T>(v: &Value, path: &str, f: impl Fn(&Value) -> T) -> HashMap<String, T> {
    at(v, path).as_object().map(|o| o.iter().filter(|(k, _)| *k != "note").map(|(k, x)| (k.clone(), f(x))).collect()).unwrap_or_default()
}

#[derive(Clone, Debug)]
pub struct Clip {
    pub duration: f64,
    pub looped: bool,
}

#[derive(Clone, Debug)]
pub struct HoldState {
    pub clip: String,
    pub bones: Vec<String>,
    pub keep_during: Vec<String>,
    /// role -> the clip it plays while this is carried
    pub actions: HashMap<String, String>,
    /// roles ignored while this is carried
    pub block: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub yaw_offset: f64,
    pub roles: HashMap<String, String>,
    pub clips: HashMap<String, Clip>,
    pub speeds: HashMap<String, f64>,
    pub radius: f64,
    pub prone_probes: Vec<f64>,
    // locomotion
    pub deadzone: f64,
    pub jog_at: f64,
    pub sprint_at: f64,
    pub back_at: f64,
    pub back_run_at: f64,
    pub body_turn_limit: f64,
    pub turn_rate: f64,
    pub prone_turn_rate: f64,
    pub accel: f64,
    pub air_accel: f64,
    // jump
    pub jump_height: f64,
    pub air_time: f64,
    pub buffer_time: f64,
    pub chain_lead: f64,
    pub land_keep: f64,
    pub windows: HashMap<String, [f64; 2]>,
    pub land_phase: HashMap<String, f64>,
    pub land_phase_back: HashMap<String, f64>,
    // roll
    pub roll_travel: [f64; 2],
    pub roll_link_at: HashMap<String, f64>,
    pub roll_link_start: f64,
    pub roll_turn_rate: f64,
    // climb
    pub climb_min: f64,
    pub climb_max: f64,
    pub climb_rate: f64,
    pub climb_duration: f64,
    pub climb_hand: Vec<f64>,
    pub grab_until: f64,
    pub top_at: f64,
    pub move_in: [f64; 2],
    pub reach: f64,
    pub facing: f64,
    pub stand_in: f64,
    pub hang_out: f64,
    pub air_grab_max_rise: f64,
    pub fades: HashMap<String, f64>,
    pub upper_bones: Vec<String>,
    pub lower_bones: Vec<String>,
    pub upper_actions: Vec<String>,
    pub attack_clips: Vec<String>,
    pub interruptible: Vec<String>,
    pub fidget: Option<[f64; 2]>,
    pub revive_on_move: bool,
    pub hold_fade: f64,
    pub hold_states: HashMap<String, HoldState>,
    pub hold_suspend: Vec<String>,
    // face
    pub pupil_x: f64,
    pub pupil_up: f64,
    pub pupil_down: f64,
    pub brow_lift: f64,
    pub blink_every: [f64; 2],
    pub blink_double: f64,
    pub blink_close: f64,
    pub blink_hold: f64,
    pub blink_open: f64,
    pub gaze_range: [f64; 2],
    pub brow_tilt: f64,
    pub mouth_frames: HashMap<String, [f64; 2]>,
    pub pupils: Vec<String>,
    pub lids: Vec<String>,
    pub brows: Vec<String>,
    pub mouth: String,
    // cameras
    pub first_bone: String,
    pub first_forward: f64,
    pub first_up: f64,
    pub first_smoothing: f64,
    pub first_fov_portrait: f64,
    pub first_fov_landscape: f64,
    pub first_near: f64,
    pub first_hide: Vec<String>,
    pub follow_distance: f64,
    pub follow_min: f64,
    pub follow_max: f64,
    pub shoulder: f64,
    pub follow_height: f64,
    pub target_height: f64,
    pub look_ahead: f64,
    pub follow_fov: f64,
    pub follow_near: f64,
    pub min_above_ground: f64,
    pub head_turn_limit: f64,
    pub head_pitch_share: f64,
    pub pitch_limits: [f64; 2],
}

impl Config {
    /// The kit's settings, as shipped in assets/hyrig.
    pub fn builtin() -> Config {
        Config::parse(JSON).expect("assets/hyrig/hyrig_controller.json")
    }

    pub fn parse(json: &str) -> Result<Config, String> {
        let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let v = &v;
        let two = |x: &Value| {
            let a: Vec<f64> = x.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
            [a.first().copied().unwrap_or(0.0), a.get(1).copied().unwrap_or(0.0)]
        };
        let f = |x: &Value| x.as_f64().unwrap_or(0.0);
        Ok(Config {
            yaw_offset: num(v, "meta.model.yawOffset"),
            roles: map(v, "roles", |x| x.as_str().unwrap_or("").to_string()),
            clips: map(v, "clips", |x| Clip {
                duration: x["duration"].as_f64().unwrap_or(0.0),
                looped: x["loop"].as_bool().unwrap_or(false),
            }),
            speeds: map(v, "speeds", f).into_iter().filter(|(_, s)| *s > 0.0).collect(),
            radius: num(v, "body.radius"),
            prone_probes: nums(v, "body.proneProbes"),
            deadzone: num(v, "locomotion.deadzone"),
            jog_at: num(v, "locomotion.jogAt"),
            sprint_at: num(v, "locomotion.sprintAt"),
            back_at: num(v, "locomotion.backAt"),
            back_run_at: num(v, "locomotion.backRunAt"),
            body_turn_limit: num(v, "locomotion.bodyTurnLimit"),
            turn_rate: num(v, "locomotion.turnRate"),
            prone_turn_rate: num(v, "locomotion.proneTurnRate"),
            accel: num(v, "locomotion.accel"),
            air_accel: num(v, "locomotion.airAccel"),
            jump_height: num(v, "jump.height"),
            air_time: num(v, "jump.airTime"),
            buffer_time: num(v, "jump.bufferTime"),
            chain_lead: num(v, "jump.chainLead"),
            land_keep: num(v, "jump.landKeep"),
            windows: map(v, "jump.windows", two),
            land_phase: map(v, "jump.landPhase", f),
            land_phase_back: map(v, "jump.landPhaseBack", f),
            roll_travel: pair(v, "roll.travel"),
            roll_link_at: map(v, "roll.linkAt", f),
            roll_link_start: num(v, "roll.linkStart"),
            roll_turn_rate: num(v, "roll.turnRate"),
            climb_min: num(v, "climb.minHeight"),
            climb_max: num(v, "climb.maxHeight"),
            climb_rate: num(v, "climb.rate"),
            climb_duration: num(v, "climb.duration"),
            climb_hand: nums(v, "climb.hand"),
            grab_until: num(v, "climb.grabUntil"),
            top_at: num(v, "climb.topAt"),
            move_in: pair(v, "climb.moveIn"),
            reach: num(v, "climb.reach"),
            facing: num(v, "climb.facing"),
            stand_in: num(v, "climb.standIn"),
            hang_out: num(v, "climb.hangOut"),
            air_grab_max_rise: num(v, "climb.airGrabMaxRise"),
            fades: map(v, "fades", f),
            upper_bones: strings(at(v, "layers.upper")),
            lower_bones: strings(at(v, "layers.lower")),
            upper_actions: strings(at(v, "upperBodyActions")),
            attack_clips: strings(at(v, "attackClips")),
            interruptible: strings(at(v, "interruptible")),
            fidget: v.get("idleFidget").map(|x| [x["min"].as_f64().unwrap_or(9.0), x["max"].as_f64().unwrap_or(18.0)]),
            revive_on_move: v.get("reviveOnMove").and_then(Value::as_bool).unwrap_or(false),
            hold_fade: num(v, "hold.fade"),
            hold_states: map(v, "hold.states", |x| HoldState {
                clip: x["clip"].as_str().unwrap_or("").to_string(),
                bones: strings(&x["bones"]),
                keep_during: strings(&x["keepDuring"]),
                actions: x["actions"].as_object().map(|o| o.iter().map(|(k, a)| (k.clone(), a.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default(),
                block: strings(&x["block"]),
            }),
            hold_suspend: strings(at(v, "hold.suspend")),
            pupil_x: num(v, "face.limits.pupilX"),
            pupil_up: num(v, "face.limits.pupilUp"),
            pupil_down: num(v, "face.limits.pupilDown"),
            brow_lift: num(v, "face.limits.browLift"),
            blink_every: pair(v, "face.blink.every"),
            blink_double: num(v, "face.blink.doubleChance"),
            blink_close: num(v, "face.blink.close"),
            blink_hold: num(v, "face.blink.hold"),
            blink_open: num(v, "face.blink.open"),
            gaze_range: pair(v, "face.gazeRange"),
            brow_tilt: num(v, "face.browTilt"),
            mouth_frames: map(v, "face.mouthFrames", two),
            pupils: strings(at(v, "face.nodes.pupils")),
            lids: strings(at(v, "face.nodes.lids")),
            brows: strings(at(v, "face.nodes.brows")),
            mouth: text(v, "face.nodes.mouth"),
            first_bone: text(v, "camera.first.bone"),
            first_forward: num(v, "camera.first.forward"),
            first_up: num(v, "camera.first.up"),
            first_smoothing: num(v, "camera.first.smoothing"),
            first_fov_portrait: num(v, "camera.first.fovPortrait"),
            first_fov_landscape: num(v, "camera.first.fovLandscape"),
            first_near: num(v, "camera.first.near"),
            first_hide: strings(at(v, "camera.first.hide")),
            follow_distance: num(v, "camera.follow.distance"),
            follow_min: num(v, "camera.follow.minDistance"),
            follow_max: num(v, "camera.follow.maxDistance"),
            shoulder: num(v, "camera.follow.shoulder"),
            follow_height: num(v, "camera.follow.height"),
            target_height: num(v, "camera.follow.targetHeight"),
            look_ahead: num(v, "camera.follow.lookAhead"),
            follow_fov: num(v, "camera.follow.fov"),
            follow_near: num(v, "camera.follow.near"),
            min_above_ground: num(v, "camera.follow.minAboveGround"),
            head_turn_limit: num(v, "camera.follow.headTurnLimit"),
            head_pitch_share: num(v, "camera.follow.headPitchShare"),
            pitch_limits: pair(v, "camera.pitchLimits"),
        })
    }

    /// The clip a role plays ("" if the role has none).
    pub fn role(&self, name: &str) -> &str {
        self.roles.get(name).map(String::as_str).unwrap_or("")
    }

    pub fn speed(&self, name: &str) -> f64 {
        self.speeds.get(name).copied().unwrap_or(0.0)
    }

    pub fn fade(&self, name: &str) -> f64 {
        self.fades.get(name).copied().unwrap_or(0.25)
    }
}
