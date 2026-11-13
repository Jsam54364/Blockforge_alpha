//! Top-down map of the generated world: biome colours shaded by height. Writes a PPM.
//! cargo run --release --example worldmap -- <seed> <blocks per pixel> <out.ppm>
use blockforge::registry::BlockRegistry;
use blockforge::worldgen::WorldGen;
use std::io::Write;

fn colour(biome: &str) -> [f32; 3] {
    match biome {
        "sea" => [0.16, 0.32, 0.62],
        "snowy peaks" | "glacier" => [0.95, 0.96, 0.98],
        "bare rock" => [0.55, 0.53, 0.5],
        "scree" => [0.62, 0.6, 0.56],
        "tundra" => [0.55, 0.6, 0.45],
        "boreal forest" => [0.22, 0.42, 0.3],
        "cold steppe" => [0.62, 0.6, 0.4],
        "temperate forest" => [0.2, 0.5, 0.18],
        "grassland" => [0.45, 0.66, 0.3],
        "steppe" => [0.72, 0.68, 0.38],
        "desert" => [0.9, 0.8, 0.52],
        "savanna" => [0.72, 0.62, 0.3],
        "rainforest" => [0.08, 0.42, 0.12],
        _ => [1.0, 0.0, 1.0],
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let seed: u64 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(1234);
    let step: i32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(32);
    let out = a.get(3).cloned().unwrap_or("map.ppm".into());
    let (w, h) = (768i32, 768i32);
    let reg = BlockRegistry::builtin();
    let g = WorldGen::new(seed, &reg);
    let mut img = Vec::with_capacity((w * h * 3) as usize);
    let mut counts = std::collections::BTreeMap::new();
    for py in 0..h {
        for px in 0..w {
            let (x, z) = ((px - w / 2) * step, (py - h / 2) * step);
            let info = g.describe(x, z);
            *counts.entry(info.biome).or_insert(0) += 1;
            let e = g.describe(x + step, z - step).height;
            let shade = (1.0 + (info.height - e) * 0.04 * 32.0 / step as f32).clamp(0.55, 1.35);
            let c = colour(info.biome);
            let alt = if info.biome == "sea" { 1.0 + info.height / 120.0 } else { 0.9 + info.height / 400.0 };
            for k in 0..3 {
                let v = if info.biome == "sea" { c[k] * alt } else { c[k] * shade * alt };
                img.push((v.clamp(0.0, 1.0) * 255.0) as u8);
            }
        }
    }
    let mut f = std::fs::File::create(&out).unwrap();
    write!(f, "P6 {w} {h} 255\n").unwrap();
    f.write_all(&img).unwrap();
    let total: i32 = counts.values().sum();
    for (b, n) in counts {
        println!("{b:18} {:5.1}%", 100.0 * n as f32 / total as f32);
    }
}
