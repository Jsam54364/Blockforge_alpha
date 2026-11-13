use blockforge::climate::Climate;
use blockforge::noise::Fbm;
use blockforge::registry::BlockRegistry;
use blockforge::terrain::Terrain;
use blockforge::worldgen::WorldGen;
use std::time::Instant;

fn time<F: FnMut(f32, f32) -> f32>(name: &str, mut f: F) {
    let n = 400_000;
    let t = Instant::now();
    let mut acc = 0.0;
    for i in 0..n {
        acc += f((i % 997) as f32 * 13.7 - 5000.0, (i / 997) as f32 * 11.3 + 200.0);
    }
    println!("{name:28} {:7.0} ns/call  ({acc:.0})", t.elapsed().as_nanos() as f64 / n as f64);
}

fn main() {
    let t = Terrain::new(1234);
    let c = Climate::new(1234);
    let reg = BlockRegistry::builtin();
    let g = WorldGen::new(1234, &reg);
    let f = Fbm::new(5, 500.0, 1, 0.5);
    time("perlin (1 octave)", |x, z| f.at(x, z, 1.0));
    time("continent cell 1", |x, z| t.continent(x, z, 1.0));
    time("terrain.sample cell 1", |x, z| t.sample(x, z, 1.0).height);
    time("terrain.sample cell 64", |x, z| t.sample(x * 8.0, z * 8.0, 64.0).height);
    time("climate cell 1", |x, z| c.at(x, z, 10.0, 0.5, 1.0).temperature);
    time("climate cell 64", |x, z| c.at(x, z, 10.0, 0.5, 64.0).temperature);
    time("lod_surface cell 4", |x, z| g.lod_surface(x as i32, z as i32, 4).0 as f32);
    time("lod_height cell 4", |x, z| g.lod_height(x as i32, z as i32, 4) as f32);
}
