//! The world's 15 biomes (see the README), chosen from the climate, the altitude, the
//! distance to the coast and a few large "region" noises for the biomes that are places to
//! discover rather than climates (the Blightlands, the Petrified Kingdom, oases).
//!
//! The Hollow Earth is underground (see `WorldGen`), so it is not chosen here; its entry
//! names the blocks of its caverns.
//!
//! **Adding a biome** = an entry in `BIOMES` and a rule in `choose`. Blocks are named as in
//! `assets/blocks.toml`; "stone" in a biome means its own stone (`stone`).

/// One kind of land.
#[derive(Clone, Copy, Debug)]
pub struct Biome {
    pub name: &'static str,
    /// The biome's key in blocks.toml (its blocks' `biome`).
    pub key: &'static str,
    /// The top block, the one that shows in patches through it, and the few blocks under it.
    pub surface: &'static str,
    pub patches: &'static str,
    pub subsurface: &'static str,
    /// Its two stones: the rock just under the ground, on cliffs and in boulders.
    pub stone: &'static str,
    pub stone_b: &'static str,
    /// Grass colour (sRGB, 0xRRGGBB): greyscale grass textures are multiplied by it, and it
    /// blends smoothly between neighbouring biomes.
    pub grass: u32,
}

macro_rules! biome {
    ($name:expr, $key:expr, $surf:expr, $patch:expr, $sub:expr, $s1:expr, $s2:expr, $grass:expr) => {
        Biome { name: $name, key: $key, surface: $surf, patches: $patch, subsurface: $sub, stone: $s1, stone_b: $s2, grass: $grass }
    };
}

pub const WILDWOOD: usize = 0;
pub const AMBERWOOD: usize = 1;
pub const FROSTPEAK: usize = 2;
pub const STONEFANG: usize = 3;
pub const SUNSCORCHED: usize = 4;
pub const OASIS: usize = 5;
pub const MURKWATER: usize = 6;
pub const TUNDRA: usize = 7;
pub const GLACIER: usize = 8;
pub const TAIGA: usize = 9;
pub const BLIGHT: usize = 10;
pub const PETRIFIED: usize = 11;
pub const HOLLOW: usize = 12;
pub const STORMBREAK: usize = 13;
pub const GRASSLANDS: usize = 14;

pub const BIOMES: &[Biome] = &[
    biome!("Ancient Wildwood", "wildwood", "lush_grass", "forest_floor", "dirt", "mossrock", "elderstone", 0x4E9A3A),
    biome!("Amberwood", "amberwood", "leaf_litter", "grass", "dirt", "ochrestone", "honeystone", 0xA9AE4A),
    biome!("Frostpeak Highlands", "frostpeak", "cold_grass", "frostpeak_stony_soil", "dirt", "frostgranite", "rimeslate", 0x7E9C72),
    biome!("Stonefang Mountains", "stonefang", "cold_grass", "stonefang_pebbles", "stonefang_coarse_soil", "fangbasalt", "ironstone", 0x8AA06A),
    biome!("Sunscorched Expanse", "sunscorched", "sunscorched_sand", "red_sand", "sand", "sunstone", "dunerock", 0xBFB755),
    biome!("Emerald Oasis", "oasis", "lush_grass", "rich_soil", "rich_soil", "jadestone", "travertine", 0x3FBF4A),
    biome!("Murkwater Swamp", "murkwater", "grass", "mud", "mud", "bogstone", "mirestone", 0x5E7A34),
    biome!("Whitewind Tundra", "tundra", "tundra_moss", "snowy_grass", "permafrost", "windstone", "palestone", 0x8FA676),
    biome!("Crystal Glacier", "glacier", "snow", "packed_ice", "packed_ice", "glacierstone", "bluemarble", 0x86A07A),
    biome!("Winterpine Taiga", "taiga", "snowy_grass", "needle_floor", "dirt", "taigarock", "greystone", 0x5F8A5A),
    biome!("Blightlands", "blight", "blighted_turf", "blighted_soil", "blighted_soil", "rotstone", "ashstone", 0x6E6A50),
    biome!("Petrified Kingdom", "petrified", "petrified_sand", "cracked_earth", "petrified_coarse_soil", "petrastone", "dustrock", 0xA8A070),
    biome!("Hollow Earth", "hollow", "cave_moss", "hollow_coarse_soil", "gloomstone", "gloomstone", "cavernstone", 0x4E8A6A),
    biome!("Stormbreak Coast", "stormbreak", "grass", "shingle", "dirt", "tidestone", "seaslate", 0x7FA85E),
    biome!("Giants' Grasslands", "grasslands", "prairie_grass", "meadow", "dirt", "fieldstone", "meadowstone", 0x9CBF52),
];

/// What a point is like, besides its climate: altitude above the sea, how far inland
/// (0 at the coast .. 1), and the region noises (each about -1..1).
#[derive(Clone, Copy, Debug, Default)]
pub struct Place {
    pub altitude: f32,
    pub inland: f32,
    /// Corruption: the Blightlands where this is high.
    pub blight: f32,
    /// The Petrified Kingdom where this is high.
    pub ancient: f32,
    /// Oases in the desert where this is high.
    pub oasis: f32,
}

fn hi(v: f32, at: f32, margin: f32) -> f32 {
    crate::noise::smoothstep(at - margin, at + margin, v)
}

fn lo(v: f32, at: f32, margin: f32) -> f32 {
    1.0 - hi(v, at, margin)
}

/// How much each biome (indexed as `BIOMES`) a climate at a place belongs to, summing to 1.
/// The rules of `choose`, each a soft step across a margin (1.5 degrees, 0.04 rainfall,
/// some blocks of altitude...), so terrain shaped per biome blends from one biome's style
/// to the next instead of jumping at the border.
pub fn weights(temperature: f32, rainfall: f32, p: Place) -> [f32; 15] {
    let (t, r, alt) = (temperature, rainfall, p.altitude);
    let mut w = [0.0f32; 15];
    let mut rest = 1.0f32;
    // Each rule takes its share of what earlier rules left.
    let take = |w: &mut [f32; 15], rest: &mut f32, share: f32, split: &[(usize, f32)]| {
        let g = share.clamp(0.0, 1.0) * *rest;
        for &(b, k) in split {
            w[b] += g * k;
        }
        *rest -= g;
    };
    take(&mut w, &mut rest, lo(t, -13.0, 1.5), &[(GLACIER, 1.0)]);
    let cold = lo(t, 2.0, 1.5);
    take(&mut w, &mut rest, hi(alt, 62.0, 8.0), &[(FROSTPEAK, cold), (STONEFANG, 1.0 - cold)]);
    take(&mut w, &mut rest, hi(p.blight, 0.36, 0.05) * hi(t, -6.0, 1.5), &[(BLIGHT, 1.0)]);
    take(&mut w, &mut rest, hi(p.ancient, 0.33, 0.05) * lo(r, 0.55, 0.04) * hi(t, 4.0, 1.5), &[(PETRIFIED, 1.0)]);
    take(&mut w, &mut rest, lo(p.inland, 0.1, 0.03) * lo(alt, 24.0, 6.0) * hi(t, -3.0, 1.5) * lo(t, 21.0, 1.5), &[(STORMBREAK, 1.0)]);
    let dry = lo(r, 0.45, 0.04);
    take(&mut w, &mut rest, lo(t, -2.0, 1.5), &[(TUNDRA, dry), (TAIGA, 1.0 - dry)]);
    let high = hi(alt, 42.0, 6.0);
    take(&mut w, &mut rest, lo(t, 5.0, 1.5), &[(FROSTPEAK, high), (TAIGA, 1.0 - high)]);
    take(&mut w, &mut rest, hi(r, 0.7, 0.04) * lo(alt, 16.0, 5.0) * hi(t, 8.0, 1.5), &[(MURKWATER, 1.0)]);
    let (desert, oasis, wet) = (lo(r, 0.34, 0.04), hi(p.oasis, 0.55, 0.08), hi(r, 0.52, 0.04));
    take(&mut w, &mut rest, hi(t, 19.0, 1.5), &[(OASIS, desert * oasis + (1.0 - desert) * wet), (SUNSCORCHED, desert * (1.0 - oasis)), (GRASSLANDS, (1.0 - desert) * (1.0 - wet))]);
    take(&mut w, &mut rest, hi(alt, 46.0, 6.0), &[(STONEFANG, 1.0)]);
    take(&mut w, &mut rest, lo(r, 0.33, 0.04), &[(GRASSLANDS, 1.0)]);
    take(&mut w, &mut rest, lo(r, 0.48, 0.04), &[(AMBERWOOD, 1.0)]);
    w[WILDWOOD] += rest;
    w
}

/// The biome (index into `BIOMES`) for a climate at a place: the one it belongs to most
/// (`weights`).
pub fn choose(temperature: f32, rainfall: f32, p: Place) -> usize {
    let w = weights(temperature, rainfall, p);
    (0..w.len()).max_by(|&a, &b| w[a].partial_cmp(&w[b]).unwrap()).unwrap_or(GRASSLANDS)
}

/// A biome's grass colour as sRGB bytes.
pub fn grass_rgb(b: &Biome) -> [u8; 3] {
    [(b.grass >> 16) as u8, (b.grass >> 8) as u8, b.grass as u8]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::BlockRegistry;

    #[test]
    fn every_biome_names_real_blocks_and_matches_its_blocks() {
        let reg = BlockRegistry::builtin();
        for (i, b) in BIOMES.iter().enumerate() {
            for n in [b.surface, b.patches, b.subsurface, b.stone, b.stone_b] {
                assert!(reg.id(n).is_some(), "{}: no block '{n}'", b.name);
            }
            assert_eq!(reg.def(reg.id(b.stone).unwrap()).biome.as_deref(), Some(b.key), "{}", b.name);
            assert_eq!(
                [WILDWOOD, AMBERWOOD, FROSTPEAK, STONEFANG, SUNSCORCHED, OASIS, MURKWATER, TUNDRA, GLACIER, TAIGA, BLIGHT, PETRIFIED, HOLLOW, STORMBREAK, GRASSLANDS][i],
                i
            );
        }
    }

    #[test]
    fn weights_sum_to_one_and_blend_across_borders() {
        let land = Place { altitude: 10.0, inland: 0.8, ..Default::default() };
        for t in [-20.0, -5.0, 3.0, 12.0, 25.0] {
            for r in [0.1, 0.3, 0.5, 0.8] {
                let w = weights(t, r, land);
                assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-4, "{t} {r}");
            }
        }
        // Right at the grasslands / Amberwood border, both share the place.
        let w = weights(12.0, 0.33, land);
        assert!(w[GRASSLANDS] > 0.3 && w[AMBERWOOD] > 0.3, "{w:?}");
    }

    #[test]
    fn climates_and_places_pick_sensible_biomes() {
        let land = Place { altitude: 10.0, inland: 0.8, ..Default::default() };
        let name = |t, r, p| BIOMES[choose(t, r, p)].name;
        assert_eq!(name(30.0, 0.05, land), "Sunscorched Expanse");
        assert_eq!(name(30.0, 0.05, Place { oasis: 0.8, ..land }), "Emerald Oasis");
        assert_eq!(name(12.0, 0.6, land), "Ancient Wildwood");
        assert_eq!(name(12.0, 0.4, land), "Amberwood");
        assert_eq!(name(12.0, 0.25, land), "Giants' Grasslands");
        assert_eq!(name(14.0, 0.8, Place { altitude: 4.0, ..land }), "Murkwater Swamp");
        assert_eq!(name(2.0, 0.5, land), "Winterpine Taiga");
        assert_eq!(name(-5.0, 0.3, land), "Whitewind Tundra");
        assert_eq!(name(-15.0, 0.3, land), "Crystal Glacier");
        assert_eq!(name(-3.0, 0.5, Place { altitude: 100.0, ..land }), "Frostpeak Highlands");
        assert_eq!(name(6.0, 0.5, Place { altitude: 80.0, ..land }), "Stonefang Mountains");
        assert_eq!(name(12.0, 0.5, Place { blight: 0.6, ..land }), "Blightlands");
        assert_eq!(name(14.0, 0.3, Place { ancient: 0.6, ..land }), "Petrified Kingdom");
        assert_eq!(name(12.0, 0.5, Place { inland: 0.02, ..land }), "Stormbreak Coast");
    }
}
