//! The title screen's moving background: a panorama of a sunset over the sea, photographed
//! in the game itself (twelve views from one spot, stitched by `tools/stitch_panorama.py`),
//! turning slowly behind the menu like Minecraft's. Drawn by `title.wgsl`.

const PANORAMA_JPG: &[u8] = include_bytes!("../../assets/title/panorama.jpg");

/// One turn every three minutes.
pub const TURN_SECONDS: f32 = 180.0;
/// Vertical field of view of the title camera (radians). The panorama covers 30 degrees
/// above and below the horizon, so at most 60 degrees fit.
pub const FOVY: f32 = 0.95;
/// Where the turning starts: facing the sun, which sets over the sea (west, yaw pi/2).
const START_YAW: f32 = std::f32::consts::FRAC_PI_2 + 0.35;

/// The camera's yaw after `seconds` on the title screen.
pub fn yaw(seconds: f32) -> f32 {
    START_YAW + seconds / TURN_SECONDS * std::f32::consts::TAU
}

/// The panorama as RGBA8 (sRGB), with its width and height, at most `max_width` wide (halved
/// as often as needed: some phones cannot hold a 4096-pixel texture).
pub fn panorama(max_width: u32) -> Result<(Vec<u8>, u32, u32), String> {
    let mut d = jpeg_decoder::Decoder::new(std::io::Cursor::new(PANORAMA_JPG));
    let rgb = d.decode().map_err(|e| format!("panorama: {e}"))?;
    let info = d.info().ok_or("panorama: no image info")?;
    if info.pixel_format != jpeg_decoder::PixelFormat::RGB24 {
        return Err(format!("panorama: unexpected pixel format {:?}", info.pixel_format));
    }
    let (mut w, mut h) = (info.width as u32, info.height as u32);
    let mut rgba: Vec<u8> = rgb.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    while w > max_width.max(1) && w >= 2 && h >= 2 {
        let (nw, nh) = (w / 2, h / 2);
        let mut half = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let at = |xx: u32, yy: u32| rgba[((yy * w + xx) * 4 + c) as usize] as u32;
                    let s = at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1);
                    half[((y * nw + x) * 4 + c) as usize] = ((s + 2) / 4) as u8;
                }
            }
        }
        (rgba, w, h) = (half, nw, nh);
    }
    Ok((rgba, w, h))
}

/// Yellow lines that pulse beside the logo, one picked per launch.
pub const SPLASHES: [&str; 12] = [
    "NOW WITH SUNSETS!",
    "ALSO TRY VINTAGE STORY!",
    "WATER, NOW WETTER!",
    "BUILT ON A PHONE!",
    "CUBIC CHUNKS!",
    "16 KM VIEW!",
    "GOD RAYS!",
    "51 ANIMATIONS!",
    "HALF SLABS!",
    "LOD TO THE HORIZON!",
    "FOAM ON EVERY SHORE!",
    "WRITTEN IN RUST!",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panorama_decodes_and_wraps_around() {
        let (px, w, h) = panorama(8192).unwrap();
        assert_eq!((w, h), (4096, 683));
        assert_eq!(px.len(), (w * h * 4) as usize);
        // The left and right edges meet (it is a full turn): their columns look alike.
        let col = |x: u32| -> f32 {
            (0..h).map(|y| px[((y * w + x) * 4) as usize] as f32 + px[((y * w + x) * 4 + 2) as usize] as f32).sum::<f32>() / h as f32
        };
        assert!((col(0) - col(w - 1)).abs() < 6.0, "{} vs {}", col(0), col(w - 1));
        // Sky above, darker sea below.
        let row = |y: u32| -> f32 { (0..w).map(|x| px[((y * w + x) * 4 + 1) as usize] as f32).sum::<f32>() / w as f32 };
        assert!(row(h / 3) > row(h - 5));
    }

    #[test]
    fn small_devices_get_a_smaller_panorama() {
        let (px, w, h) = panorama(2048).unwrap();
        assert_eq!((w, h), (2048, 341));
        assert_eq!(px.len(), (w * h * 4) as usize);
    }

    #[test]
    fn the_title_camera_turns_once_in_three_minutes_from_the_sunset() {
        assert!((yaw(0.0) - START_YAW).abs() < 1e-6);
        assert!((yaw(TURN_SECONDS) - yaw(0.0) - std::f32::consts::TAU).abs() < 1e-4);
        // Its view fits inside the panorama's 30 degrees above and below the horizon.
        assert!(FOVY * 0.5 < 30f32.to_radians());
        for s in SPLASHES {
            assert!(s.chars().all(crate::font::has_art), "{s}");
        }
    }
}

/// The character shown on the title screen: the HyRig model idling on flat ground, posed
/// each frame and turned toward the viewer (or the way the arrows under him turned him).
pub struct Showcase {
    ctl: crate::hyrig::Controller,
    rig: crate::hyrig::Rig,
    world: crate::hyrig::BoxWorld,
    /// The turn he is drawn at (eases toward the one asked for).
    yaw: f32,
    verts: Vec<crate::hyrig::rig::Vertex>,
}

impl Showcase {
    pub fn new() -> Showcase {
        let cfg = crate::hyrig::Config::builtin();
        let rig = crate::hyrig::Rig::builtin(&cfg);
        let mut ctl = crate::hyrig::Controller::new(cfg, crate::hyrig::View::Follow);
        let world = crate::hyrig::BoxWorld::new(Vec::new(), 1.0e3);
        ctl.teleport(&world, 0.0, Some(0.0), 0.0, Some(0.0));
        Showcase { ctl, rig, world, yaw: 0.0, verts: Vec::new() }
    }

    /// Moves him on by `dt` seconds, turned to `yaw` (0: facing the viewer); returns his
    /// triangles with his feet at the origin, facing +Z when `yaw` is 0.
    pub fn update(&mut self, dt: f32, yaw: f32) -> &[crate::hyrig::rig::Vertex] {
        let input = crate::hyrig::Input {
            move_x: 0.0,
            move_y: 0.0,
            look_yaw: 0.0,
            look_pitch: 0.0,
            jump: false,
            roll: false,
            down: false,
            punch: false,
            mine: false,
            actions: Vec::new(),
        };
        let out = self.ctl.update(&self.world, dt.clamp(0.0, 0.1) as f64, &input);
        let pose = self.rig.pose(&out, [0.0; 3]);
        self.verts.clear();
        self.rig.triangles(&pose, &mut self.verts);
        // Ease toward the turn asked for, and turn the posed model about his feet.
        self.yaw += (yaw - self.yaw) * (1.0 - (-dt * 8.0).exp());
        let p = out.position;
        let (s, c) = self.yaw.sin_cos();
        for v in &mut self.verts {
            let (x, z) = (v.pos[0] - p[0] as f32, v.pos[2] - p[2] as f32);
            v.pos = [x * c + z * s, v.pos[1] - p[1] as f32, -x * s + z * c];
            let (nx, nz) = (v.normal[0], v.normal[2]);
            v.normal = [nx * c + nz * s, v.normal[1], -nx * s + nz * c];
        }
        &self.verts
    }
}

impl Default for Showcase {
    fn default() -> Self {
        Showcase::new()
    }
}

#[cfg(test)]
mod showcase_tests {
    use super::*;

    #[test]
    fn the_showcase_stands_him_on_the_origin_facing_the_viewer() {
        let mut s = Showcase::new();
        let mut verts = Vec::new();
        for _ in 0..30 {
            verts = s.update(1.0 / 30.0, 0.0).to_vec();
        }
        assert!(verts.len() > 100);
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in &verts {
            for i in 0..3 {
                lo[i] = lo[i].min(v.pos[i]);
                hi[i] = hi[i].max(v.pos[i]);
            }
        }
        // Feet on the ground, about 1.8 tall, centred on the origin.
        assert!(lo[1].abs() < 0.1 && (hi[1] - 1.8).abs() < 0.25, "{lo:?} {hi:?}");
        assert!((lo[0] + hi[0]).abs() < 0.3 && (lo[2] + hi[2]).abs() < 0.4, "{lo:?} {hi:?}");
        // Turned half way round, what was in front is behind.
        let front: f32 = verts.iter().map(|v| v.pos[2]).sum::<f32>() / verts.len() as f32;
        for _ in 0..60 {
            verts = s.update(1.0 / 30.0, std::f32::consts::PI).to_vec();
        }
        let back: f32 = verts.iter().map(|v| v.pos[2]).sum::<f32>() / verts.len() as f32;
        assert!((front + back).abs() < 0.05, "{front} {back}");
    }
}
