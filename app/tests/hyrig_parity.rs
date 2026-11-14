//! Replays the HyRig kit's recorded reference run (tests/data/hyrig_reference_trace.json:
//! 7 194 frames, about two minutes of scripted play covering every state, recorded from the
//! kit's JavaScript controller) and checks the Rust port gives the same result on every frame.
//! The same check the kit runs on its GDScript port (godot/tests/parity.gd).

use blockforge_app::hyrig::world::BoxShape;
use blockforge_app::hyrig::{BoxWorld, Config, Controller, Input, View};
use serde_json::Value;

#[test]
fn the_port_matches_the_kits_reference_run_frame_for_frame() {
    let trace: Value = serde_json::from_str(include_str!("data/hyrig_reference_trace.json")).unwrap();
    let f = |v: &Value| v.as_f64().unwrap();
    let boxes = trace["boxes"].as_array().unwrap().iter().map(|b| BoxShape { x0: f(&b["x0"]), x1: f(&b["x1"]), z0: f(&b["z0"]), z1: f(&b["z1"]), h: f(&b["h"]) }).collect();
    let world = BoxWorld::new(boxes, 100.0);
    // The run's random numbers: a linear congruential generator from its seed.
    let mut s = trace["seed"].as_u64().unwrap();
    let rand = Box::new(move || {
        s = (s * 1664525 + 1013904223) % 4294967296;
        s as f64 / 4294967296.0
    });
    let view = if trace["view"] == "follow" { View::Follow } else { View::First };
    let mut c = Controller::with_random(Config::builtin(), view, rand);
    let dt = 1.0 / f(&trace["fps"]);
    let (head_height, aspect) = (f(&trace["headHeight"]), f(&trace["aspect"]));
    let (mut worst, mut bad) = (0.0f64, 0);
    let frames = trace["frames"].as_array().unwrap();
    for (n, fr) in frames.iter().enumerate() {
        if let Some(tp) = fr.get("tp") {
            c.teleport(&world, f(&tp[0]), Some(f(&tp[1])), f(&tp[2]), Some(f(&tp[3])));
        }
        let input = Input {
            move_x: f(&fr["mx"]),
            move_y: f(&fr["my"]),
            look_yaw: f(&fr["yaw"]),
            look_pitch: f(&fr["pitch"]),
            jump: fr["jump"].as_bool().unwrap(),
            roll: fr["roll"].as_bool().unwrap(),
            actions: fr.get("press").and_then(Value::as_array).map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect()).unwrap_or_default(),
        };
        let o = c.update(&world, dt, &input);
        let cam = c.camera(dt, [c.pos[0], c.pos[1] + head_height, c.pos[2]], aspect);
        let num = Value::from;
        let got: Vec<Value> = vec![
            num(c.pos[0]),
            num(c.pos[1]),
            num(c.pos[2]),
            num(o.yaw),
            Value::from(o.state.clone()),
            num(o.clip_time),
            num(o.body.len() as f64),
            num(o.body[0].weight),
            Value::from(o.upper.as_ref().map_or(String::new(), |u| u.clip.clone())),
            num(o.upper.as_ref().map_or(0.0, |u| u.weight)),
            num(o.head_look.map_or(0.0, |h| h[0])),
            num(o.face.lid),
            num(o.face.pupil[0] * 1000.0),
            Value::from(o.face.mouth),
            num(cam.position[0]),
            num(cam.position[1]),
            num(cam.position[2]),
            Value::from(o.events.join(",")),
            Value::from(o.hold.as_ref().map_or(String::new(), |h| h.0.clip.clone())),
            num(o.hold.as_ref().map_or(0.0, |h| h.0.weight)),
            num(o.hold.as_ref().map_or(0.0, |h| h.0.time)),
        ];
        let want = fr["o"].as_array().unwrap();
        let mut ok = want.len() == got.len();
        for (w, g) in want.iter().zip(&got) {
            match (w.as_str(), g.as_str()) {
                (Some(a), Some(b)) => ok &= a == b,
                (None, None) => {
                    let d = (f(w) - f(g)).abs();
                    worst = worst.max(d);
                    ok &= d <= 1e-4;
                }
                _ => ok = false,
            }
        }
        if !ok {
            bad += 1;
            if bad <= 4 {
                eprintln!("frame {n} differs\n  expected {want:?}\n  got      {got:?}");
            }
        }
    }
    assert_eq!(bad, 0, "{bad} of {} frames differ (largest numeric difference {worst})", frames.len());
    assert_eq!(frames.len(), 7194);
}
