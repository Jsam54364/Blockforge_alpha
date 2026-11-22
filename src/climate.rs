//! Climate, Vintage Story style: temperature follows latitude (the z axis) and falls with
//! altitude; rainfall comes from its own map, shaped by latitude and the coast. Biomes are
//! not placed directly; they emerge from the climate (see `biome`).

use crate::noise::{smoothstep, Fbm};

/// Blocks from the equator to a pole. Walking along z crosses climate bands: z = 0 (the
/// spawn) is temperate, -z is towards the equator, +z towards a pole. Past a pole or the
/// equator the bands repeat in mirror image, so the world stays endless.
pub const POLE_DISTANCE: f32 = 25_000.0;
/// The spawn's latitude: 0 is the equator, 1 a pole.
const SPAWN_LATITUDE: f32 = 0.42;
/// Average yearly temperature drop per block of altitude above the sea (1.5 C per 10 blocks).
pub const LAPSE_RATE: f32 = 0.15;

/// 0 at the equator, 1 at a pole.
pub fn latitude(z: f32) -> f32 {
    let p = (z / POLE_DISTANCE + SPAWN_LATITUDE).rem_euclid(2.0);
    if p <= 1.0 {
        p
    } else {
        2.0 - p
    }
}

/// Average yearly temperature at sea level for a latitude, in degrees C: 30 at the
/// equator, about 12 at the spawn, -22 at the poles.
pub fn sea_level_temperature(latitude: f32) -> f32 {
    30.0 - 52.0 * latitude.powf(1.25)
}

/// Rainfall (0 dry .. 1 wet) by latitude, like Earth's belts: a wet equator, dry
/// subtropics, wet mid-latitudes and dry poles.
fn latitude_rain(latitude: f32) -> f32 {
    const BELTS: [(f32, f32); 5] = [(0.0, 0.8), (0.27, 0.1), (0.55, 0.6), (0.8, 0.45), (1.0, 0.25)];
    for w in BELTS.windows(2) {
        let ((l0, r0), (l1, r1)) = (w[0], w[1]);
        if latitude <= l1 {
            return r0 + (r1 - r0) * smoothstep(l0, l1, latitude);
        }
    }
    BELTS[BELTS.len() - 1].1
}

/// The climate at a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClimateSample {
    /// Average yearly temperature at the surface, degrees C.
    pub temperature: f32,
    /// 0 (desert) .. 1 (rainforest).
    pub rainfall: f32,
}

pub struct Climate {
    temperature: Fbm,
    rainfall: Fbm,
    /// Fine variation that makes the borders between biomes irregular.
    jitter_t: Fbm,
    jitter_r: Fbm,
}

impl Climate {
    pub fn new(seed: u64) -> Self {
        let s = |k: u64| seed.wrapping_mul(0x9FB2_1C65_1E98_DF25).wrapping_add(k);
        Climate {
            temperature: Fbm::new(s(1), 3000.0, 3, 0.5),
            rainfall: Fbm::new(s(2), 3200.0, 4, 0.5),
            jitter_t: Fbm::new(s(3), 48.0, 3, 0.6),
            jitter_r: Fbm::new(s(4), 48.0, 3, 0.6),
        }
    }

    /// The climate at `(x, z)` for ground `height` blocks above the sea, `inland` 0 at the
    /// coast .. 1 far inland (coasts are wetter), band-limited for cells `cell` blocks wide.
    pub fn at(&self, x: f32, z: f32, height: f32, inland: f32, cell: f32) -> ClimateSample {
        let lat = latitude(z);
        let mut t = sea_level_temperature(lat) + 6.0 * self.temperature.at(x, z, cell);
        t -= LAPSE_RATE * height.max(0.0);
        let mut r = latitude_rain(lat) + 0.42 * self.rainfall.at(x, z, cell);
        r += 0.12 * (1.0 - inland);
        if cell < 64.0 {
            t += 1.8 * self.jitter_t.at(x, z, cell);
            r += 0.06 * self.jitter_r.at(x, z, cell);
        }
        ClimateSample { temperature: t, rainfall: r.clamp(0.0, 1.0) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latitude_bands_repeat_and_the_spawn_is_temperate() {
        assert!((latitude(0.0) - SPAWN_LATITUDE).abs() < 1e-6);
        assert!(latitude(-SPAWN_LATITUDE * POLE_DISTANCE).abs() < 1e-3, "equator");
        assert!((latitude((1.0 - SPAWN_LATITUDE) * POLE_DISTANCE) - 1.0).abs() < 1e-3, "pole");
        assert!((latitude(3.0 * POLE_DISTANCE) - latitude(POLE_DISTANCE)).abs() < 1e-3);
        let t = sea_level_temperature(SPAWN_LATITUDE);
        assert!((8.0..16.0).contains(&t), "spawn temperature {t}");
    }

    #[test]
    fn mountains_are_colder() {
        let c = Climate::new(3);
        let low = c.at(100.0, 200.0, 5.0, 1.0, 1.0);
        let high = c.at(100.0, 200.0, 150.0, 1.0, 1.0);
        assert!((low.temperature - high.temperature - 145.0 * LAPSE_RATE).abs() < 1e-3);
        assert!((0.0..=1.0).contains(&low.rainfall));
    }
}
