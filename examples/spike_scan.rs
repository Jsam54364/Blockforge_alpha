//! Looks for isolated spikes in the terrain: columns far above all their neighbours a few
//! blocks away, per biome.
use blockforge::registry::BlockRegistry;
use blockforge::worldgen::WorldGen;
use std::collections::BTreeMap;
fn main() {
    let reg = BlockRegistry::builtin();
    let g = WorldGen::new(1337, &reg);
    let mut spikes: BTreeMap<&str, (u32, u32)> = BTreeMap::new();
    for i in 0..60_000 {
        let (x, z) = ((i % 300) * 13 - 2000, (i / 300) * 13 - 1300);
        let h = |x: i32, z: i32| g.sample(x as f32, z as f32, 1.0).height;
        let c = h(x, z);
        let n = [h(x + 4, z), h(x - 4, z), h(x, z + 4), h(x, z - 4)];
        let b = g.describe(x, z).biome;
        let e = spikes.entry(b).or_default();
        e.1 += 1;
        if n.iter().all(|v| c - v > 12.0) {
            e.0 += 1;
        }
    }
    for (b, (s, n)) in spikes {
        println!("{b:22} {s:5} spikes in {n} samples");
    }
}
