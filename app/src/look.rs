//! Customisable shaders: the shader look's options (Settings > Shaders), like a shader pack's
//! options screen. Each is a percentage of the default look; presets set them all at once.
//! They only act with Shaders on (Lite or Fancy); `gfx` hands them to the shader.

/// The options. Percentages: 100 is the look as designed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShaderLook {
    /// Which preset these came from (`PRESETS`), or `CUSTOM` once one was changed.
    pub preset: u8,
    /// How dark shadows are: 0 (none) .. 100 (full).
    pub shadows: u8,
    /// Warm sunlight against cool shade: 0 .. 200.
    pub warmth: u8,
    /// Light bounced off sunlit ground into shade and onto walls: 0 .. 200.
    pub bounce: u8,
    /// Extra colour: 0 (as textured) .. 200.
    pub colour: u8,
    /// Overall brightness: 60 .. 140.
    pub exposure: u8,
    /// 60 (flat) .. 140 (punchy).
    pub contrast: u8,
    /// How much leaves move in the wind: 0 (still) .. 200.
    pub sway: u8,
    /// Torches and glowing blocks give warm orange light (else white).
    pub warm_torches: bool,
    /// Where fog starts: 50 (nearer) .. 150 (further).
    pub fog: u8,
}

/// `ShaderLook::preset` after the player changed an option by hand.
pub const CUSTOM: u8 = 255;

/// One option: its name, its range and step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Shadows,
    Warmth,
    Bounce,
    Colour,
    Exposure,
    Contrast,
    Sway,
    Fog,
}

impl Field {
    pub const ALL: [Field; 8] = [Field::Shadows, Field::Warmth, Field::Bounce, Field::Colour, Field::Exposure, Field::Contrast, Field::Sway, Field::Fog];

    pub fn label(self) -> &'static str {
        match self {
            Field::Shadows => "Shadow Darkness",
            Field::Warmth => "Sunlight Warmth",
            Field::Bounce => "Bounce Light",
            Field::Colour => "Colour",
            Field::Exposure => "Brightness",
            Field::Contrast => "Contrast",
            Field::Sway => "Leaf Sway",
            Field::Fog => "Fog Distance",
        }
    }

    /// (lowest, highest, step)
    fn range(self) -> (u8, u8, u8) {
        match self {
            Field::Shadows => (0, 100, 10),
            Field::Exposure | Field::Contrast => (60, 140, 10),
            Field::Fog => (50, 150, 10),
            _ => (0, 200, 25),
        }
    }
}

/// Presets: name and options. The first is the default look.
pub const PRESETS: [(&str, ShaderLook); 5] = [
    ("Default", ShaderLook::DEFAULT),
    (
        "Vibrant",
        ShaderLook { preset: 1, shadows: 90, warmth: 100, bounce: 125, colour: 175, exposure: 110, contrast: 110, sway: 100, warm_torches: true, fog: 110 },
    ),
    (
        "Cinematic",
        ShaderLook { preset: 2, shadows: 100, warmth: 175, bounce: 75, colour: 125, exposure: 90, contrast: 130, sway: 75, warm_torches: true, fog: 80 },
    ),
    (
        "Soft",
        ShaderLook { preset: 3, shadows: 60, warmth: 75, bounce: 150, colour: 100, exposure: 110, contrast: 80, sway: 100, warm_torches: true, fog: 120 },
    ),
    (
        "Natural",
        ShaderLook { preset: 4, shadows: 80, warmth: 50, bounce: 75, colour: 50, exposure: 100, contrast: 100, sway: 50, warm_torches: false, fog: 100 },
    ),
];

impl Default for ShaderLook {
    fn default() -> Self {
        ShaderLook::DEFAULT
    }
}

impl ShaderLook {
    pub const DEFAULT: ShaderLook =
        ShaderLook { preset: 0, shadows: 100, warmth: 100, bounce: 100, colour: 100, exposure: 100, contrast: 100, sway: 100, warm_torches: true, fog: 100 };

    pub fn get(&self, f: Field) -> u8 {
        match f {
            Field::Shadows => self.shadows,
            Field::Warmth => self.warmth,
            Field::Bounce => self.bounce,
            Field::Colour => self.colour,
            Field::Exposure => self.exposure,
            Field::Contrast => self.contrast,
            Field::Sway => self.sway,
            Field::Fog => self.fog,
        }
    }

    fn slot(&mut self, f: Field) -> &mut u8 {
        match f {
            Field::Shadows => &mut self.shadows,
            Field::Warmth => &mut self.warmth,
            Field::Bounce => &mut self.bounce,
            Field::Colour => &mut self.colour,
            Field::Exposure => &mut self.exposure,
            Field::Contrast => &mut self.contrast,
            Field::Sway => &mut self.sway,
            Field::Fog => &mut self.fog,
        }
    }

    /// One step of `f` up (`d` > 0) or down, within its range; the look becomes Custom.
    pub fn step(&mut self, f: Field, d: i8) {
        let (lo, hi, st) = f.range();
        let v = self.slot(f);
        let n = (*v as i32 + d.signum() as i32 * st as i32).clamp(lo as i32, hi as i32) as u8;
        if n != *v {
            *v = n;
            self.preset = CUSTOM;
        }
    }

    pub fn toggle_torches(&mut self) {
        self.warm_torches = !self.warm_torches;
        self.preset = CUSTOM;
    }

    /// The next (`d` > 0) or previous preset (from Custom: the first or last).
    pub fn cycle_preset(&mut self, d: i8) {
        let n = PRESETS.len() as i32;
        let i = if self.preset == CUSTOM {
            if d > 0 { 0 } else { n - 1 }
        } else {
            (self.preset as i32 + d.signum() as i32).rem_euclid(n)
        };
        *self = PRESETS[i as usize].1;
    }

    pub fn preset_name(&self) -> &'static str {
        PRESETS.get(self.preset as usize).map_or("Custom", |p| p.0)
    }

    /// The value as shown: "100%", or for Leaf Sway at 0 "Off".
    pub fn value_label(&self, f: Field) -> String {
        let v = self.get(f);
        if f == Field::Sway && v == 0 {
            "Off".into()
        } else {
            format!("{v}%")
        }
    }

    /// For the settings file: the options as a comma-separated list.
    pub fn save(&self) -> String {
        format!(
            "{},{},{},{},{},{},{},{},{},{}",
            self.preset, self.shadows, self.warmth, self.bounce, self.colour, self.exposure, self.contrast, self.sway, self.warm_torches as u8, self.fog
        )
    }

    /// From `save`'s list; out-of-range values are brought into range, a bad list gives None.
    pub fn load(s: &str) -> Option<ShaderLook> {
        let v: Vec<u8> = s.split(',').map(|x| x.trim().parse().ok()).collect::<Option<_>>()?;
        if v.len() != 10 {
            return None;
        }
        let mut l = ShaderLook {
            preset: v[0],
            shadows: v[1],
            warmth: v[2],
            bounce: v[3],
            colour: v[4],
            exposure: v[5],
            contrast: v[6],
            sway: v[7],
            warm_torches: v[8] != 0,
            fog: v[9],
        };
        for f in Field::ALL {
            let (lo, hi, _) = f.range();
            let s = l.slot(f);
            *s = (*s).clamp(lo, hi);
        }
        if l.preset as usize >= PRESETS.len() {
            l.preset = CUSTOM;
        }
        Some(l)
    }

    /// For the shader: `look` = (warmth, colour, warm torches, bounce) and
    /// `look2` = (exposure, contrast, sway, 0), as factors of the designed look.
    pub fn uniforms(&self) -> ([f32; 4], [f32; 4]) {
        let f = |v: u8| v as f32 / 100.0;
        (
            [f(self.warmth), f(self.colour), if self.warm_torches { 1.0 } else { 0.0 }, f(self.bounce)],
            [f(self.exposure), f(self.contrast), f(self.sway), 0.0],
        )
    }

    /// How dark shadows are, 0 .. 1.
    pub fn shadow_strength(&self) -> f32 {
        self.shadows as f32 / 100.0
    }

    /// Where fog starts, as a factor of the usual distance.
    pub fn fog_factor(&self) -> f32 {
        self.fog as f32 / 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_stay_in_range_and_make_the_look_custom() {
        let mut l = ShaderLook::DEFAULT;
        l.step(Field::Shadows, 1);
        assert_eq!((l.shadows, l.preset), (100, 0), "already at the top: still the preset");
        l.step(Field::Shadows, -1);
        assert_eq!((l.shadows, l.preset), (90, CUSTOM));
        for _ in 0..20 {
            l.step(Field::Contrast, -1);
        }
        assert_eq!(l.contrast, 60);
        assert_eq!(l.preset_name(), "Custom");
    }

    #[test]
    fn presets_cycle_both_ways_and_match_their_index() {
        for (i, (_, p)) in PRESETS.iter().enumerate() {
            assert_eq!(p.preset as usize, i);
        }
        let mut l = ShaderLook::DEFAULT;
        l.cycle_preset(-1);
        assert_eq!(l.preset_name(), "Natural");
        l.cycle_preset(1);
        assert_eq!(l, ShaderLook::DEFAULT);
        l.step(Field::Sway, -1);
        l.cycle_preset(1);
        assert_eq!(l, ShaderLook::DEFAULT, "from Custom, forward goes to the first");
    }

    #[test]
    fn saves_and_loads() {
        let mut l = PRESETS[2].1;
        l.step(Field::Fog, 1);
        l.toggle_torches();
        assert_eq!(ShaderLook::load(&l.save()), Some(l));
        assert_eq!(ShaderLook::load("1,2,3"), None);
        let wild = ShaderLook::load("9,250,0,0,0,0,250,0,1,0").unwrap();
        assert_eq!((wild.preset, wild.shadows, wild.exposure, wild.contrast, wild.fog), (CUSTOM, 100, 60, 140, 50));
    }

    #[test]
    fn the_default_look_is_the_designed_one() {
        let (a, b) = ShaderLook::DEFAULT.uniforms();
        assert_eq!(a, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(b, [1.0, 1.0, 1.0, 0.0]);
        assert_eq!(ShaderLook::DEFAULT.shadow_strength(), 1.0);
    }
}
