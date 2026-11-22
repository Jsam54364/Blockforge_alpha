//! How long the generator takes: land samples, chunk columns, chunks, LOD columns.
use blockforge::coords::ChunkPos;
use blockforge::registry::BlockRegistry;
use blockforge::worldgen::WorldGen;
use std::time::Instant;

fn main() {
    let reg = BlockRegistry::builtin();
    let g = WorldGen::new(1337, &reg);
    let l = g.land();
    let n = 100_000;
    for cell in [1.0f32, 16.0, 256.0] {
        let t = Instant::now();
        let mut acc = 0.0;
        for i in 0..n {
            acc += l.sample((i % 997) as f32 * 13.7 - 5000.0, (i / 997) as f32 * 11.3 + 200.0, cell).height;
        }
        println!("land.sample cell {cell:5}: {:6.0} ns ({acc:.0})", t.elapsed().as_nanos() as f64 / n as f64);
    }
    let [sx, _, sz] = g.find_spawn();
    let (cx, cz) = ((sx as i32) >> 5, (sz as i32) >> 5);
    let t = Instant::now();
    let mut feats = 0;
    let cols: Vec<_> = (0..16).map(|i| { let c = g.column(cx + i % 4 * 7, cz + i / 4 * 7); feats += c.features.len(); c }).collect();
    println!("column: {:.2} ms each ({} features each)", t.elapsed().as_secs_f64() * 1000.0 / 16.0, feats / 16);
    let t = Instant::now();
    let mut k = 0;
    for c in &cols {
        let base = c.min_top.div_euclid(32);
        for cy in base - 1..=c.max_top.div_euclid(32) {
            let _ = g.generate_chunk(ChunkPos::new(c.cx, cy, c.cz), c);
            k += 1;
        }
    }
    println!("generate_chunk: {:.2} ms each ({k} chunks)", t.elapsed().as_secs_f64() * 1000.0 / k as f64);
}
