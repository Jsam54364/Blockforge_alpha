use blockforge::lod::{build_section, SectionKey};
use blockforge::registry::BlockRegistry;
use blockforge::worldgen::WorldGen;
use std::time::Instant;
fn main() {
    let reg = BlockRegistry::builtin();
    let g = WorldGen::new(1337, &reg);
    for level in 1..=7u8 {
        let t = Instant::now();
        for k in 0..4 { let _ = build_section(&g, &reg, SectionKey::new(level, k - 2, 1)); }
        let tr = Instant::now();
        let key = SectionKey::new(level, 0, 1);
        let (ox, oz) = key.origin();
        let n = if level <= 2 { g.lod_trees(ox, oz, ox + key.width(), oz + key.width(), key.cell()).len() } else { 0 };
        println!("level {level}: {:.1} ms per section; trees {n} in {:.1} ms", t.elapsed().as_secs_f64() * 250.0, tr.elapsed().as_secs_f64() * 1000.0);
    }
}
