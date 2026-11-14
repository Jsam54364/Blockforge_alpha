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
