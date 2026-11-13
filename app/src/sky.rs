//! The sky and the light it gives.
//!
//! The sky is a photograph (`assets/sky`, baked from an HDRI by `tools/bake_sky.py`) with its
//! sun taken out. The game moves its own sun through the day and puts the sun's glow back
//! around wherever it is: a clear sky is close to "base sky" times "glow by angle from the
//! sun", and the bake measured that glow from the photo (`Glow`). On top come sunset colours,
//! a night sky with stars that turn with it, and a moon opposite the sun.
//!
//! The same sky lights the terrain (`face_light`: one colour per block-face direction, from
//! the whole sky plus the sun and the moon), colours the fog, and is what water reflects.
//! `radiance` here and `sky_radiance` in shader.wgsl are the same formula: keep them in step.

use std::f32::consts::PI;

/// One day passes in this many real seconds (Minecraft's day is 20 minutes too).
pub const DAY_SECONDS: f64 = 1200.0;
/// A new game starts mid-morning.
pub const START_HOUR: f64 = 9.0;
/// The world's latitude and the sun's declination (time of year), in degrees: the sun climbs
/// to 90 - 35 + 10 = 65 degrees at noon, and days are a little longer than nights.
const LATITUDE: f32 = 35.0;
const DECLINATION: f32 = 10.0;

/// Direct sunlight on a surface facing the sun, at its whitest.
const SUN_LIGHT: f32 = 0.65;
/// Direct moonlight.
const MOON_LIGHT: [f32; 3] = [0.06, 0.075, 0.11];
/// Sky light on terrain: scale and how much of the sky's blue it keeps (0 = grey light).
const AMBIENT_GAIN: f32 = 1.45;
const AMBIENT_SATURATION: f32 = 0.35;
/// Sky light counted per sample at most (the glow right around the sun is sunlight's job).
const AMBIENT_MAX: f32 = 1.2;
/// So nights stay playable: extra light at night (top faces; sides and bottoms get less).
const NIGHT_AMBIENT: [f32; 3] = [0.06, 0.07, 0.11];
const NIGHT_FACE: [f32; 6] = [0.8, 0.8, 1.0, 0.5, 0.8, 0.8];
/// Sunset colouring: low toward the sun, and everywhere else.
const DUSK_WARM: [f32; 3] = [1.55, 0.80, 0.42];
const DUSK_COOL: [f32; 3] = [0.80, 0.82, 1.0];
/// Night sky, straight up and at the horizon.
const NIGHT_ZENITH: [f32; 3] = [0.0012, 0.0020, 0.0055];
const NIGHT_HORIZON: [f32; 3] = [0.0045, 0.0065, 0.0130];
/// How much light the ground reflects back up (onto sides and bottoms).
const GROUND_ALBEDO: f32 = 0.4;
/// How much of what a face sees is ground (cosine-weighted): none for tops, half for sides.
const GROUND_VIEW: [f32; 6] = [0.5, 0.5, 0.0, 1.0, 0.5, 0.5];

/// Block-face directions in the shader's order (`face_normal`).
pub const FACE_NORMALS: [[f32; 3]; 6] =
    [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]];

const SKY_PNG: &[u8] = include_bytes!("../../assets/sky/sky.png");
const GLOW_TXT: &str = include_str!("../../assets/sky/glow.txt");

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

// ---- Time of day ----

/// The time of day, in hours (0..24).
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    hours: f64,
}

impl Default for Clock {
    fn default() -> Self {
        Clock::new()
    }
}

impl Clock {
    pub fn new() -> Clock {
        Clock { hours: START_HOUR }
    }

    pub fn hours(&self) -> f64 {
        self.hours
    }

    /// Lets `seconds` of real time pass.
    pub fn advance(&mut self, seconds: f64) {
        self.hours = (self.hours + seconds * 24.0 / DAY_SECONDS).rem_euclid(24.0);
    }

    /// Jumps forwards or backwards (the menu's time buttons).
    pub fn shift(&mut self, hours: f64) {
        self.hours = (self.hours + hours).rem_euclid(24.0);
    }
}

impl Clock {
    /// `time=<hours>` for the state file kept between launches.
    pub fn to_text(self) -> String {
        format!("time={:.4}\n", self.hours)
    }

    /// Reads what `to_text` wrote (a new day's morning if it's missing or bad).
    pub fn from_text(text: &str) -> Clock {
        let hours = text
            .lines()
            .find_map(|l| l.trim().strip_prefix("time=")?.parse::<f64>().ok())
            .filter(|h| h.is_finite())
            .unwrap_or(START_HOUR);
        Clock { hours: hours.rem_euclid(24.0) }
    }
}

/// "HH:MM".
pub fn time_label(hours: f64) -> String {
    let minutes = (hours.rem_euclid(24.0) * 60.0).floor() as i64 % (24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// The sun's hour angle: 0 at noon, growing by a full turn per day.
pub fn hour_angle(hours: f64) -> f32 {
    ((hours - 12.0) / 24.0 * std::f64::consts::TAU) as f32
}

/// Towards the sun at a time of day: it rises in the east (+x), crosses the south (+z) at
/// noon and sets in the west (-x).
pub fn sun_dir(hours: f64) -> [f32; 3] {
    let h = hour_angle(hours);
    let (lat, dec) = (LATITUDE.to_radians(), DECLINATION.to_radians());
    let east = -dec.cos() * h.sin();
    let north = dec.sin() * lat.cos() - dec.cos() * lat.sin() * h.cos();
    let up = dec.sin() * lat.sin() + dec.cos() * lat.cos() * h.cos();
    [east, up, -north]
}

/// The axis the sky turns around: towards the celestial pole, low in the north (-z).
pub fn pole() -> [f32; 3] {
    let lat = LATITUDE.to_radians();
    [0.0, lat.sin(), -lat.cos()]
}

/// `v` turned by `angle` radians around the unit vector `axis` (right-handed).
pub fn rotate(v: [f32; 3], axis: [f32; 3], angle: f32) -> [f32; 3] {
    let (s, c) = angle.sin_cos();
    let k = axis;
    let kxv = [k[1] * v[2] - k[2] * v[1], k[2] * v[0] - k[0] * v[2], k[0] * v[1] - k[1] * v[0]];
    let kd = dot(k, v) * (1.0 - c);
    [
        v[0] * c + kxv[0] * s + k[0] * kd,
        v[1] * c + kxv[1] * s + k[1] * kd,
        v[2] * c + kxv[2] * s + k[2] * kd,
    ]
}

// ---- The photographed sky ----

/// One mip level of the sky: rgb, linear, row 0 straight up, the last row at the horizon.
pub struct Level {
    pub width: u32,
    pub height: u32,
    pub texels: Vec<[f32; 3]>,
}

/// The sky with its mip levels (each half the size of the one before, averaged).
pub struct SkyMap {
    pub levels: Vec<Level>,
}

/// RGBE (Radiance's encoding: rgb mantissas and a shared exponent) to linear rgb.
fn from_rgbe(p: &[u8]) -> [f32; 3] {
    if p[3] == 0 {
        return [0.0; 3];
    }
    let f = (p[3] as f32 - 136.0).exp2();
    [(p[0] as f32 + 0.5) * f, (p[1] as f32 + 0.5) * f, (p[2] as f32 + 0.5) * f]
}

/// Linear rgb to the GPU's RGB9E5 (9-bit mantissas and a shared 5-bit exponent).
pub fn to_rgb9e5(c: [f32; 3]) -> u32 {
    const MAX: f32 = 65408.0; // (511 / 512) * 2^16
    let r = c[0].clamp(0.0, MAX);
    let g = c[1].clamp(0.0, MAX);
    let b = c[2].clamp(0.0, MAX);
    let m = r.max(g).max(b);
    if m <= 0.0 {
        return 0;
    }
    let mut e = (m.log2().floor() as i32).max(-16) + 1 + 15;
    let mut step = ((e - 15 - 9) as f32).exp2();
    if (m / step + 0.5).floor() >= 512.0 {
        e += 1;
        step *= 2.0;
    }
    let q = |v: f32| ((v / step + 0.5).floor() as u32).min(511);
    q(r) | (q(g) << 9) | (q(b) << 18) | ((e as u32) << 27)
}

impl SkyMap {
    /// The game's sky (falls back to a plain gradient if the file can't be read).
    pub fn load() -> SkyMap {
        SkyMap::decode(SKY_PNG).unwrap_or_else(|e| {
            eprintln!("blockforge: sky.png: {e}; using a plain sky");
            SkyMap::plain()
        })
    }

    /// Reads the baked sky: an RGBA PNG holding RGBE.
    pub fn decode(bytes: &[u8]) -> Result<SkyMap, String> {
        let dec = png::Decoder::new(bytes);
        let mut reader = dec.read_info().map_err(|e| e.to_string())?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
        if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
            return Err(format!("{:?} {:?}, not 8-bit RGBA (RGBE)", info.color_type, info.bit_depth));
        }
        if info.width < 4 || info.height < 2 || info.width > 2048 {
            return Err(format!("{}x{} is not a usable sky size", info.width, info.height));
        }
        let texels = buf[..info.buffer_size()].chunks(4).map(from_rgbe).collect();
        Ok(SkyMap::with_mips(Level { width: info.width, height: info.height, texels }))
    }

    /// A light blue gradient, if the photograph is missing.
    fn plain() -> SkyMap {
        let (w, h) = (64u32, 16u32);
        let mut texels = Vec::new();
        for y in 0..h {
            let t = (y as f32 + 0.5) / h as f32;
            for _ in 0..w {
                texels.push(lerp3([0.05, 0.09, 0.20], [0.70, 0.78, 0.88], t * t));
            }
        }
        SkyMap::with_mips(Level { width: w, height: h, texels })
    }

    fn with_mips(top: Level) -> SkyMap {
        let mut levels = vec![top];
        loop {
            let src = levels.last().unwrap();
            if src.width == 1 && src.height == 1 {
                break;
            }
            let (w, h) = ((src.width / 2).max(1), (src.height / 2).max(1));
            let mut texels = Vec::with_capacity((w * h) as usize);
            for y in 0..h {
                for x in 0..w {
                    let mut sum = [0.0f32; 3];
                    let mut n = 0.0;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let sx = (x * 2 + dx).min(src.width - 1);
                        let sy = (y * 2 + dy).min(src.height - 1);
                        let t = src.texels[(sy * src.width + sx) as usize];
                        for c in 0..3 {
                            sum[c] += t[c];
                        }
                        n += 1.0;
                    }
                    texels.push(scale3(sum, 1.0 / n));
                }
            }
            levels.push(Level { width: w, height: h, texels });
        }
        SkyMap { levels }
    }

    /// Drops the levels wider than `width` (once they are on the GPU, only the small ones are
    /// needed here, for the light).
    pub fn keep_levels_up_to(&mut self, width: u32) {
        let first = self.level_for_width(width);
        self.levels.drain(..first);
    }

    /// The first level at most `width` texels wide.
    pub fn level_for_width(&self, width: u32) -> usize {
        self.levels.iter().position(|l| l.width <= width).unwrap_or(self.levels.len() - 1)
    }

    /// The sky along a direction, bilinear, from one level. Below the horizon: the horizon.
    /// Same mapping as the shader's `sky_base` and the bake.
    pub fn sample(&self, level: usize, d: [f32; 3]) -> [f32; 3] {
        let l = &self.levels[level.min(self.levels.len() - 1)];
        let u = d[0].atan2(d[2]) / (2.0 * PI);
        let v = 1.0 - d[1].clamp(0.0, 1.0).asin() * (2.0 / PI);
        let x = u * l.width as f32 - 0.5;
        let y = (v * l.height as f32 - 0.5).clamp(0.0, l.height as f32 - 1.0);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |xi: f32, yi: f32| {
            let xi = (xi as i64).rem_euclid(l.width as i64) as u32;
            let yi = (yi as u32).min(l.height - 1);
            l.texels[(yi * l.width + xi) as usize]
        };
        let top = lerp3(at(x0, y0), at(x0 + 1.0, y0), fx);
        let bottom = lerp3(at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0), fx);
        lerp3(top, bottom, fy)
    }
}

// ---- Glow around the sun ----

/// The sky's brightening around the sun, per colour channel, as measured from the photo:
/// `a + c1 exp(-g / w1) + c2 exp(-g / w2) + e cos(g)^2`, g = angle from the sun in radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glow {
    pub a: [f32; 3],
    pub c1: [f32; 3],
    pub w1: f32,
    pub c2: [f32; 3],
    pub w2: f32,
    pub e: [f32; 3],
}

/// The glow was measured outside this angle (4 degrees); inside it the game draws the sun.
pub const GLOW_FROM: f32 = 0.0698;

impl Glow {
    /// No glow at all (if glow.txt is unreadable).
    pub const NONE: Glow = Glow { a: [1.0; 3], c1: [0.0; 3], w1: 1.0, c2: [0.0; 3], w2: 1.0, e: [0.0; 3] };

    pub fn load() -> Glow {
        Glow::parse(GLOW_TXT).unwrap_or_else(|e| {
            eprintln!("blockforge: glow.txt: {e}; no glow around the sun");
            Glow::NONE
        })
    }

    /// Lines `r|g|b a c1 w1 c2 w2 e`; `#` starts a comment.
    pub fn parse(text: &str) -> Result<Glow, String> {
        let mut rows: [Option<[f32; 6]>; 3] = [None; 3];
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let ch = match parts.next() {
                Some("r") => 0,
                Some("g") => 1,
                Some("b") => 2,
                other => return Err(format!("unknown channel {other:?}")),
            };
            let vals: Vec<f32> = parts.map(|p| p.parse::<f32>().map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
            if vals.len() != 6 || vals.iter().any(|v| !v.is_finite()) || vals[2] <= 0.0 || vals[4] <= 0.0 {
                return Err(format!("bad line: {line}"));
            }
            rows[ch] = Some([vals[0], vals[1], vals[2], vals[3], vals[4], vals[5]]);
        }
        let [Some(r), Some(g), Some(b)] = rows else { return Err("missing a channel".into()) };
        // One pair of widths for all channels (the bake fits them that way).
        Ok(Glow {
            a: [r[0], g[0], b[0]],
            c1: [r[1], g[1], b[1]],
            w1: r[2],
            c2: [r[3], g[3], b[3]],
            w2: r[4],
            e: [r[5], g[5], b[5]],
        })
    }

    /// The glow at an angle from the sun (given as its cosine). `glare` scales the narrow
    /// part right around the sun and `broad` the wide one: both weaken as the sun gets low (at
    /// sunset the sun's own disc is the brightest thing, in a wide golden glow). Inside
    /// `GLOW_FROM` the angle is eased off rather than cut, so no ring shows there.
    pub fn eval(&self, cos_g: f32, glare: f32, broad: f32) -> [f32; 3] {
        let g0 = cos_g.clamp(-1.0, 1.0).acos();
        let g = (g0 * g0 + GLOW_FROM * GLOW_FROM).sqrt();
        let (k1, k2, k3) = ((-g / self.w1).exp() * glare, (-g / self.w2).exp() * broad, cos_g * cos_g);
        let ch = |i: usize| self.a[i] + self.c1[i] * k1 + self.c2[i] * k2 + self.e[i] * k3;
        [ch(0), ch(1), ch(2)]
    }

    /// For the shader: (a, w1), (c1, w2), (c2, -), (e, -).
    pub fn uniforms(&self) -> [[f32; 4]; 4] {
        [
            [self.a[0], self.a[1], self.a[2], self.w1],
            [self.c1[0], self.c1[1], self.c1[2], self.w2],
            [self.c2[0], self.c2[1], self.c2[2], 0.0],
            [self.e[0], self.e[1], self.e[2], 0.0],
        ]
    }
}

// ---- Per-frame state ----

/// Everything about the sky that depends on the time of day (cheap: computed every frame).
#[derive(Clone, Copy, Debug)]
pub struct SkyParams {
    pub hours: f64,
    /// Towards the sun, and how much of its disc is above the horizon.
    pub sun: [f32; 3],
    pub sun_disc: f32,
    /// Direct sunlight (colour times strength), white high up, orange low down, 0 once set.
    pub sun_light: [f32; 3],
    /// 0 at night, 1 in daylight.
    pub day: f32,
    pub moon: [f32; 3],
    pub moon_disc: f32,
    pub moon_light: [f32; 3],
    /// The photographed sky's brightness, how strong the glare and the wide glow around the
    /// sun are (`Glow::eval`), and how much sunset colouring it gets.
    pub brightness: f32,
    pub glare: f32,
    pub glow: f32,
    pub dusk: f32,
    /// Star visibility, and how far the starry sky has turned (radians around `pole`).
    pub stars: f32,
    pub star_angle: f32,
}

pub fn params(hours: f64) -> SkyParams {
    let sun = sun_dir(hours);
    let s = sun[1];
    let day = smoothstep(-0.14, 0.10, s);
    let warm = smoothstep(0.0, 0.5, s);
    let sun_light = scale3(lerp3([1.0, 0.40, 0.14], [1.0, 0.95, 0.88], warm), SUN_LIGHT * smoothstep(-0.02, 0.08, s));
    let moon = [-sun[0], -sun[1], -sun[2]];
    SkyParams {
        hours,
        sun,
        sun_disc: smoothstep(-0.03, 0.03, s),
        sun_light,
        day,
        moon,
        moon_disc: smoothstep(-0.03, 0.03, moon[1]),
        moon_light: scale3(MOON_LIGHT, smoothstep(-0.02, 0.12, moon[1])),
        brightness: (0.03 + 0.97 * day) * (0.9 + 0.25 * s.clamp(0.0, 1.0)),
        glare: smoothstep(0.05, 0.6, s),
        glow: 0.3 + 0.7 * smoothstep(0.0, 0.45, s),
        dusk: smoothstep(-0.18, 0.0, s) * (1.0 - smoothstep(0.02, 0.32, s)),
        stars: 1.0 - smoothstep(-0.22, -0.04, s),
        star_angle: hour_angle(hours),
    }
}

/// The sky's colour along a direction (no sun or moon disc, no stars): the photographed sky
/// times the glow around the sun, sunset colouring, plus the night sky. Below the horizon it
/// continues the horizon. Mirrors `sky_radiance` in shader.wgsl.
pub fn radiance(map: &SkyMap, glow: &Glow, p: &SkyParams, d: [f32; 3], level: usize) -> [f32; 3] {
    let base = map.sample(level, d);
    let gl = glow.eval(dot(d, p.sun), p.glare, p.glow);
    let hz = (1.0 - d[1].clamp(0.0, 1.0)).powi(3);
    let hd = d[0] * p.sun[0] + d[2] * p.sun[2];
    let hl = ((d[0] * d[0] + d[2] * d[2]) * (p.sun[0] * p.sun[0] + p.sun[2] * p.sun[2])).max(1e-8);
    let toward = (hd / hl.sqrt()).max(0.0);
    let tint = lerp3(DUSK_COOL, DUSK_WARM, hz * (0.25 + 0.75 * toward * toward));
    let night = lerp3(NIGHT_ZENITH, NIGHT_HORIZON, hz);
    let mut out = [0.0; 3];
    for c in 0..3 {
        let dusk = 1.0 + (tint[c] - 1.0) * p.dusk;
        out[c] = base[c] * gl[c] * dusk * p.brightness + night[c] * (1.0 - p.day);
    }
    out
}

/// Light from the sky (the half above the horizon, cosine-weighted) on a surface facing each
/// block-face direction. The slow part: a few hundred sky samples.
pub fn sky_ambient(map: &SkyMap, glow: &Glow, p: &SkyParams) -> [[f32; 3]; 6] {
    const N_AZ: usize = 32;
    const N_EL: usize = 8;
    let level = map.level_for_width(64);
    let (d_az, d_el) = (2.0 * PI / N_AZ as f32, PI / 2.0 / N_EL as f32);
    let mut acc = [[0.0f32; 3]; 6];
    for j in 0..N_EL {
        let el = (j as f32 + 0.5) * d_el;
        let (sin_el, cos_el) = el.sin_cos();
        for i in 0..N_AZ {
            let (sin_az, cos_az) = ((i as f32 + 0.5) * d_az).sin_cos();
            let d = [cos_el * sin_az, sin_el, cos_el * cos_az];
            let l = radiance(map, glow, p, d, level).map(|v| v.min(AMBIENT_MAX));
            let w = cos_el * d_az * d_el;
            for (f, n) in FACE_NORMALS.iter().enumerate() {
                let k = dot(*n, d).max(0.0) * w;
                for c in 0..3 {
                    acc[f][c] += l[c] * k;
                }
            }
        }
    }
    acc.map(|e| scale3(e, 1.0 / PI))
}

/// The light on each block-face direction: sky light (greyed a little, so terrain isn't blue
/// all over), sunlight, moonlight, light bounced off the ground, and a little extra at night.
pub fn face_light(ambient: &[[f32; 3]; 6], p: &SkyParams) -> [[f32; 4]; 6] {
    let direct = |f: usize| {
        let a = ambient[f];
        let grey = 0.2126 * a[0] + 0.7152 * a[1] + 0.0722 * a[2];
        let (sun, moon) = (dot(FACE_NORMALS[f], p.sun).max(0.0), dot(FACE_NORMALS[f], p.moon).max(0.0));
        let mut out = [0.0f32; 3];
        for c in 0..3 {
            let sky = (grey + (a[c] - grey) * AMBIENT_SATURATION) * AMBIENT_GAIN;
            out[c] = sky + p.sun_light[c] * sun + p.moon_light[c] * moon;
        }
        out
    };
    // The ground is lit like a top face; what it reflects reaches whatever looks down at it.
    let ground = direct(2);
    let mut out = [[0.0f32; 4]; 6];
    for f in 0..6 {
        let d = direct(f);
        for c in 0..3 {
            let bounce = ground[c] * GROUND_ALBEDO * GROUND_VIEW[f];
            let night = NIGHT_AMBIENT[c] * NIGHT_FACE[f] * (1.0 - p.day);
            out[f][c] = (d[c] + bounce + night).min(1.3);
        }
        out[f][3] = 1.0;
    }
    out
}

/// The sky part of the shader's uniforms (laid out as the tail of `Globals` in shader.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SkyUniforms {
    /// xyz: towards the sun; w: how much of its disc shows.
    pub sun: [f32; 4],
    /// rgb: direct sunlight; w: daylight (0 night .. 1 day).
    pub sun_color: [f32; 4],
    /// xyz: towards the moon; w: how much of its disc shows.
    pub moon: [f32; 4],
    /// x: the photographed sky's brightness; y: wide glow, z: glare (`Glow::eval`); w: sunset
    /// colouring.
    pub sky_tint: [f32; 4],
    /// The glow formula (`Glow::uniforms`).
    pub glow: [[f32; 4]; 4],
    /// Light on each block-face direction (`face_light`).
    pub face_light: [[f32; 4]; 6],
    /// xyz: the axis the stars turn around; w: how far they have turned.
    pub stars: [f32; 4],
    /// x: star visibility.
    pub night: [f32; 4],
}

pub fn uniforms(glow: &Glow, p: &SkyParams, faces: [[f32; 4]; 6]) -> SkyUniforms {
    let k = pole();
    SkyUniforms {
        sun: [p.sun[0], p.sun[1], p.sun[2], p.sun_disc],
        sun_color: [p.sun_light[0], p.sun_light[1], p.sun_light[2], p.day],
        moon: [p.moon[0], p.moon[1], p.moon[2], p.moon_disc],
        sky_tint: [p.brightness, p.glow, p.glare, p.dusk],
        glow: glow.uniforms(),
        face_light: faces,
        stars: [k[0], k[1], k[2], p.star_angle],
        night: [p.stars, 0.0, 0.0, 0.0],
    }
}

/// The sky for the renderer: the map (uploaded once), the glow, and the per-face light, which
/// is only recomputed when the time of day has moved on a little (the sun moves 0.15 degrees
/// in 0.01 hours; the light changes far less than anyone could see).
pub struct Sky {
    pub map: SkyMap,
    pub glow: Glow,
    lit_at: f64,
    faces: [[f32; 4]; 6],
}

impl Sky {
    pub fn new() -> Sky {
        Sky { map: SkyMap::load(), glow: Glow::load(), lit_at: f64::NAN, faces: [[0.0; 4]; 6] }
    }

    pub fn uniforms(&mut self, hours: f64) -> SkyUniforms {
        let p = params(hours);
        let moved = (hours - self.lit_at).abs();
        if self.lit_at.is_nan() || (0.01..=23.99).contains(&moved) {
            self.faces = face_light(&sky_ambient(&self.map, &self.glow, &p), &p);
            self.lit_at = hours;
        }
        uniforms(&self.glow, &p, self.faces)
    }
}

impl Default for Sky {
    fn default() -> Self {
        Sky::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < eps)
    }

    #[test]
    fn the_clock_wraps_and_labels_itself() {
        let mut c = Clock::new();
        assert_eq!(time_label(c.hours()), "09:00");
        c.advance(DAY_SECONDS / 2.0);
        assert_eq!(time_label(c.hours()), "21:00");
        c.shift(4.5);
        assert_eq!(time_label(c.hours()), "01:30");
        c.shift(-2.0);
        assert_eq!(time_label(c.hours()), "23:30");
        assert_eq!(time_label(Clock::from_text(&format!("cam=1\n{}", c.to_text())).hours()), "23:30");
        assert_eq!(Clock::from_text("time=banana").hours(), START_HOUR);
        assert_eq!(Clock::from_text("time=NaN").hours(), START_HOUR);
        assert_eq!(time_label(23.9999), "23:59");
    }

    #[test]
    fn the_sun_rises_in_the_east_peaks_in_the_south_and_sets_in_the_west() {
        let noon = sun_dir(12.0);
        assert!((noon[1].asin().to_degrees() - 65.0).abs() < 0.01, "{noon:?}");
        assert!(noon[0].abs() < 1e-6 && noon[2] > 0.0, "noon sun is due south: {noon:?}");
        assert!(sun_dir(0.0)[1] < -0.5, "midnight");
        let morning = sun_dir(7.0);
        assert!(morning[0] > 0.5 && morning[1] > 0.0, "{morning:?}");
        let evening = sun_dir(17.0);
        assert!(evening[0] < -0.5 && evening[1] > 0.0, "{evening:?}");
        // Days a little longer than nights: up before 6:00, still up after 18:00.
        assert!(sun_dir(5.9)[1] > 0.0 && sun_dir(18.1)[1] > 0.0);
        for h in [0.0, 3.3, 9.0, 15.7] {
            let d = sun_dir(h);
            assert!((dot(d, d) - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn the_sun_turns_with_the_starry_sky() {
        // The sun is fixed on the turning sky: turning noon's sun by minus the hour angle
        // around the pole gives the sun at any hour (the shader turns stars the same way).
        for h in [3.0, 7.5, 12.0, 16.25, 22.0] {
            let turned = rotate(sun_dir(12.0), pole(), -hour_angle(h));
            assert!(close(turned, sun_dir(h), 1e-5), "{h}: {turned:?} vs {:?}", sun_dir(h));
        }
    }

    #[test]
    fn rgb9e5_keeps_sky_colours_to_a_fraction_of_a_percent() {
        fn back(v: u32) -> [f32; 3] {
            let step = (((v >> 27) as i32 - 15 - 9) as f32).exp2();
            [(v & 511) as f32 * step, ((v >> 9) & 511) as f32 * step, ((v >> 18) & 511) as f32 * step]
        }
        for c in [[0.045, 0.081, 0.19], [0.72, 0.78, 0.87], [1.0, 1.0, 1.0], [3.5, 2.0, 0.5], [0.0002, 0.0004, 0.001]] {
            let b = back(to_rgb9e5(c));
            let m = c[0].max(c[1]).max(c[2]);
            for i in 0..3 {
                assert!((b[i] - c[i]).abs() <= m / 256.0, "{c:?} -> {b:?}");
            }
        }
        assert_eq!(to_rgb9e5([0.0; 3]), 0);
        assert_eq!(to_rgb9e5([-1.0, f32::NAN, 0.0]) & 0x7ff_ffff, 0);
    }

    #[test]
    fn the_baked_sky_loads_with_a_full_mip_chain() {
        let map = SkyMap::decode(SKY_PNG).expect("assets/sky/sky.png");
        let top = &map.levels[0];
        assert_eq!((top.width, top.height), (2048, 512));
        let last = map.levels.last().unwrap();
        assert_eq!((last.width, last.height), (1, 1));
        assert_eq!(map.levels.len(), 12, "matches the GPU's mip count for 2048 x 512");
        // A real sky: deeper blue straight up, paler at the horizon, and no sun left in it.
        let zenith = map.sample(0, [0.0, 1.0, 0.0]);
        let horizon = map.sample(0, [1.0, 0.0, 0.0]);
        assert!(zenith[2] > zenith[0] * 2.0 && horizon[0] > zenith[0] * 4.0, "{zenith:?} {horizon:?}");
        let brightest = top.texels.iter().map(|t| t[0].max(t[1]).max(t[2])).fold(0.0, f32::max);
        assert!(brightest < 3.0, "sun painted out: {brightest}");
    }

    #[test]
    fn the_glow_file_parses_and_peaks_toward_the_sun() {
        let g = Glow::parse(GLOW_TXT).expect("assets/sky/glow.txt");
        let near = g.eval(GLOW_FROM.cos(), 1.0, 1.0);
        let side = g.eval(0.0, 1.0, 1.0);
        assert!(g.eval(GLOW_FROM.cos(), 0.0, 0.3)[0] < near[0] * 0.5, "weaker glow");
        // No ring where the measured glow starts: smooth through GLOW_FROM.
        let at = |deg: f32| g.eval(deg.to_radians().cos(), 1.0, 1.0)[1];
        let (a, b, c) = (at(3.0), at(4.0), at(5.0));
        assert!(a > b && b > c && (a - b) < (b - c) * 1.5, "{a} {b} {c}");
        assert!(near.iter().zip(side.iter()).all(|(n, s)| n > &(s * 3.0)), "{near:?} {side:?}");
        // Whiter near the sun than the sky is (red rises most).
        assert!(near[0] / side[0] > near[2] / side[2]);
        assert!(Glow::parse("r 1 2 3 4 5 6\ng 1 2 3 4 5 6").is_err());
        assert!(Glow::parse("r 1 2 0 4 5 6\ng 1 2 3 4 5 6\nb 1 2 3 4 5 6").is_err());
    }

    #[test]
    fn daylight_beats_night_and_the_sunny_side_is_brighter() {
        let sky = SkyMap::decode(SKY_PNG).unwrap();
        let glow = Glow::parse(GLOW_TXT).unwrap();
        let lum = |c: [f32; 4]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let at = |h: f64| {
            let p = params(h);
            face_light(&sky_ambient(&sky, &glow, &p), &p)
        };
        let (noon, morning, night) = (at(12.0), at(8.0), at(0.5));
        // Noon: tops brightest, bottoms darkest; tops close to the old fixed lighting (~0.9-1.1).
        assert!(lum(noon[2]) > lum(noon[0]) && lum(noon[0]) > lum(noon[3]));
        assert!((0.85..1.25).contains(&lum(noon[2])), "noon top {:?}", noon[2]);
        // Morning: the east-facing (+x) side gets the sun.
        assert!(lum(morning[0]) > lum(morning[1]) + 0.2, "{:?} vs {:?}", morning[0], morning[1]);
        // Night: dark but not black, and bluish.
        let top = night[2];
        assert!((0.03..0.2).contains(&lum(top)) && top[2] > top[0], "night top {top:?}");
        for f in 0..6 {
            assert!(lum(noon[f]) > lum(night[f]) * 3.0, "face {f}");
        }
    }

    #[test]
    fn the_relit_sky_is_blue_brighter_near_the_sun_and_dark_at_night() {
        let sky = SkyMap::decode(SKY_PNG).unwrap();
        let glow = Glow::parse(GLOW_TXT).unwrap();
        let p = params(12.0);
        let up = radiance(&sky, &glow, &p, [0.0, 1.0, 0.0], 0);
        let away = radiance(&sky, &glow, &p, [0.0, 0.2, -1.0], 0);
        assert!(up[2] > up[0] && away[2] > away[0], "blue sky: {up:?} {away:?}");
        let (y, z) = (p.sun[1] - 0.2, p.sun[2]);
        let n = (y * y + z * z).sqrt();
        let toward = radiance(&sky, &glow, &p, [0.0, y / n, z / n], 0);
        assert!(toward[0] > away[0] * 1.5, "brighter near the sun");
        let night = radiance(&sky, &glow, &params(0.0), [0.0, 1.0, 0.0], 0);
        assert!(night[2] < 0.02, "night sky {night:?}");
    }
}
