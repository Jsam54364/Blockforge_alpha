//! Biomes emerge from the climate: each one claims a range of temperature, rainfall and
//! altitude, and the first entry in `BIOMES` whose ranges contain a point's climate wins.
//!
//! **Adding a biome** = adding an entry. Put more specific biomes (narrower ranges) before
//! broader ones, and name existing blocks (see `assets/blocks.toml`) for its ground.

/// One kind of ground cover.
#[derive(Clone, Copy, Debug)]
pub struct Biome {
    pub name: &'static str,
    /// Average yearly temperature at the surface, degrees C: `min <= t < max`.
    pub temperature: (f32, f32),
    /// Rainfall, 0 (desert) .. 1 (rainforest): `min <= r < max`.
    pub rainfall: (f32, f32),
    /// Lowest altitude it appears at, blocks above the sea.
    pub min_altitude: f32,
    /// The top block.
    pub surface: &'static str,
    /// The few blocks under the top.
    pub subsurface: &'static str,
    /// Grass colour (sRGB, 0xRRGGBB): the greyscale grass textures are multiplied by it, and
    /// it blends smoothly between neighbouring biomes.
    pub grass: u32,
}

const ANY: (f32, f32) = (f32::MIN, f32::MAX);
const ANY_RAIN: (f32, f32) = (0.0, 2.0);

/// Checked in order; the first match wins (the last entry matches everything).
pub const BIOMES: &[Biome] = &[
    Biome { name: "snowy peaks", temperature: (f32::MIN, -7.0), rainfall: ANY_RAIN, min_altitude: 70.0, surface: "snow", subsurface: "stone", grass: 0x86A07A },
    Biome { name: "glacier", temperature: (f32::MIN, -7.0), rainfall: ANY_RAIN, min_altitude: f32::MIN, surface: "snow", subsurface: "dirt", grass: 0x86A07A },
    Biome { name: "bare rock", temperature: (-7.0, -1.0), rainfall: ANY_RAIN, min_altitude: 75.0, surface: "stone", subsurface: "stone", grass: 0x8AA67A },
    Biome { name: "scree", temperature: (-7.0, 2.0), rainfall: ANY_RAIN, min_altitude: 60.0, surface: "gravel", subsurface: "stone", grass: 0x8AA67A },
    Biome { name: "tundra", temperature: (-7.0, 0.0), rainfall: ANY_RAIN, min_altitude: f32::MIN, surface: "cold_grass", subsurface: "dirt", grass: 0x8FA676 },
    Biome { name: "boreal forest", temperature: (0.0, 7.0), rainfall: (0.3, 2.0), min_altitude: f32::MIN, surface: "cold_grass", subsurface: "dirt", grass: 0x6F9A5A },
    Biome { name: "cold steppe", temperature: (0.0, 7.0), rainfall: (0.0, 0.3), min_altitude: f32::MIN, surface: "dry_grass", subsurface: "dirt", grass: 0xA6A766 },
    Biome { name: "temperate forest", temperature: (7.0, 19.0), rainfall: (0.5, 2.0), min_altitude: f32::MIN, surface: "grass", subsurface: "dirt", grass: 0x79C05A },
    Biome { name: "grassland", temperature: (7.0, 19.0), rainfall: (0.28, 0.5), min_altitude: f32::MIN, surface: "grass", subsurface: "dirt", grass: 0x91BD59 },
    Biome { name: "steppe", temperature: (7.0, 19.0), rainfall: (0.0, 0.28), min_altitude: f32::MIN, surface: "dry_grass", subsurface: "dirt", grass: 0xB3B460 },
    Biome { name: "desert", temperature: (19.0, f32::MAX), rainfall: (0.0, 0.2), min_altitude: f32::MIN, surface: "sand", subsurface: "sand", grass: 0xBFB755 },
    Biome { name: "savanna", temperature: (19.0, f32::MAX), rainfall: (0.2, 0.52), min_altitude: f32::MIN, surface: "dry_grass", subsurface: "dirt", grass: 0xBFB755 },
    Biome { name: "rainforest", temperature: (19.0, f32::MAX), rainfall: (0.52, 2.0), min_altitude: f32::MIN, surface: "lush_grass", subsurface: "dirt", grass: 0x59C93C },
    Biome { name: "grassland", temperature: ANY, rainfall: ANY_RAIN, min_altitude: f32::MIN, surface: "grass", subsurface: "dirt", grass: 0x91BD59 },
];

/// A biome's grass colour as sRGB bytes.
pub fn grass_rgb(b: &Biome) -> [u8; 3] {
    [(b.grass >> 16) as u8, (b.grass >> 8) as u8, b.grass as u8]
}

/// The biome for a climate and an altitude (blocks above the sea): its index in `BIOMES`.
pub fn choose(temperature: f32, rainfall: f32, altitude: f32) -> usize {
    BIOMES
        .iter()
        .position(|b| {
            (b.temperature.0..b.temperature.1).contains(&temperature)
                && (b.rainfall.0..b.rainfall.1).contains(&rainfall)
                && altitude >= b.min_altitude
        })
        .unwrap_or(BIOMES.len() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::BlockRegistry;

    #[test]
    fn every_biome_names_real_blocks() {
        let reg = BlockRegistry::builtin();
        for b in BIOMES {
            assert!(reg.id(b.surface).is_some(), "{}: no block '{}'", b.name, b.surface);
            assert!(reg.id(b.subsurface).is_some(), "{}: no block '{}'", b.name, b.subsurface);
        }
    }

    #[test]
    fn climates_pick_sensible_biomes() {
        let name = |t, r, a| BIOMES[choose(t, r, a)].name;
        assert_eq!(name(30.0, 0.05, 10.0), "desert");
        assert_eq!(name(26.0, 0.8, 10.0), "rainforest");
        assert_eq!(name(12.0, 0.6, 10.0), "temperate forest");
        assert_eq!(name(3.0, 0.5, 10.0), "boreal forest");
        assert_eq!(name(-12.0, 0.5, 120.0), "snowy peaks");
        assert_eq!(name(-3.0, 0.5, 100.0), "bare rock");
        // Every climate gets some biome.
        for t in [-40.0, -5.0, 0.0, 10.0, 40.0] {
            for r in [0.0, 0.5, 1.0] {
                assert!(choose(t, r, 0.0) < BIOMES.len());
            }
        }
    }
}
