//! Times chunk generation (terrain, ores, trees and other features) in a few biomes.
//! cargo run --release --example chunk_bench
use blockforge::coords::{ChunkPos, CHUNK_SIZE};
use blockforge::registry::BlockRegistry;
use blockforge::worldgen::WorldGen;
use std::time::Instant;

fn lod_bench(g: &WorldGen, reg: &BlockRegistry) {
    use blockforge::lod::{build_section, SectionKey};
    for level in [1u8, 2, 3, 4, 6] {
        let t = Instant::now();
        let n = 6;
        for k in 0..n {
            std::hint::black_box(build_section(g, reg, SectionKey::new(level, k, -3)));
        }
        println!("LOD level {level}: {:6.2} ms per section", t.elapsed().as_secs_f64() * 1000.0 / n as f64);
    }
}

fn main() {
    let reg = BlockRegistry::builtin();
    let seed = std::env::var("SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1234);
    let g = WorldGen::new(seed, &reg);
    lod_bench(&g, &reg);
    for biome in ["Crystal Glacier", "Whitewind Tundra", "Frostpeak Highlands", "Stormbreak Coast", "river", "Ancient Wildwood", "Amberwood", "Winterpine Taiga", "Giants' Grasslands", "Stonefang Mountains", "Murkwater Swamp", "Emerald Oasis", "Petrified Kingdom", "Blightlands", "Sunscorched Expanse"] {
        // The nearest to the origin, in 4-chunk steps out to 12 km.
        let Some((cx, cz)) = (0..200)
            .flat_map(|r: i32| (-r..=r).flat_map(move |i| [(i * 4, -r * 4), (i * 4, r * 4), (-r * 4, i * 4), (r * 4, i * 4)]))
            .find(|&(cx, cz)| { let d = g.describe(cx * CHUNK_SIZE + 16, cz * CHUNK_SIZE + 16); d.biome == biome && d.height > 2.0 })
        else {
            println!("{biome}: not found");
            continue;
        };
        let (x, z) = (cx * CHUNK_SIZE + 16, cz * CHUNK_SIZE + 16);
        println!("{biome}: at {x},{z}, ground {:.0}", g.describe(x, z).height);
        let t = Instant::now();
        let mut n = 0;
        for dz in 0..4 {
            for dx in 0..4 {
                let col = g.column(cx + dx, cz + dz);
                let cy0 = col.min_top.div_euclid(CHUNK_SIZE) - 1;
                for cy in cy0..cy0 + 5 {
                    std::hint::black_box(g.generate_chunk(ChunkPos::new(cx + dx, cy, cz + dz), &col));
                    n += 1;
                }
            }
        }
        println!("{biome:22} {:6.2} ms per chunk ({n} chunks near the surface, cold cache)", t.elapsed().as_secs_f64() * 1000.0 / n as f64);
    }
}
