//! Headless demo: runs the integrated server, connects a client over the in-process
//! protocol, streams + meshes the area around the origin, then edits the world.
//!
//!   cargo run --release --bin headless
//!
//! No window or GPU: this exercises everything except the renderer.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use blockforge::client::Client;
use blockforge::coords::{BlockPos, CHUNK_VOLUME};
use blockforge::jobs::default_worker_threads;
use blockforge::lod::{build_section, LodSettings};
use blockforge::mesh::{mesh_chunk_with, Mesher, PaddedChunk};
use blockforge::registry::BlockRegistry;
use blockforge::server::{self, ServerConfig};

fn pump(c: &mut Client, d: Duration) {
    let start = Instant::now();
    while start.elapsed() < d {
        c.update();
        thread::sleep(Duration::from_millis(2));
    }
}

/// Stops once data has been flowing and then stays quiet for a moment.
fn wait_until_settled(c: &mut Client, timeout: Duration) {
    let start = Instant::now();
    let mut last_change = Instant::now();
    let mut last = (0u64, 0u64);
    while start.elapsed() < timeout {
        c.update();
        let now = (c.stats.chunks_received, c.stats.meshes_built);
        if now != last {
            last = now;
            last_change = Instant::now();
        }
        if last.0 > 0 && c.is_idle() && last_change.elapsed() > Duration::from_millis(400) {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

/// Meshes every loaded chunk with both meshers and compares quad counts and meshing time.
fn compare_meshers(c: &Client, reg: &BlockRegistry) {
    let positions: Vec<_> = c.store.positions().collect();
    let mut totals = [(0usize, Duration::ZERO); 2];
    for (i, kind) in [Mesher::Simple, Mesher::Greedy].into_iter().enumerate() {
        for &p in &positions {
            let pc = PaddedChunk::capture(&c.store, p);
            let t = Instant::now();
            let mesh = mesh_chunk_with(kind, reg, &pc);
            totals[i].1 += t.elapsed();
            totals[i].0 += mesh.quad_count();
        }
    }
    let ((sq, st), (gq, gt)) = (totals[0], totals[1]);
    println!("\nMesher comparison over {} loaded chunks (this machine, not a phone):", positions.len());
    println!("  simple: {sq} quads in {st:.2?}");
    println!(
        "  greedy: {gq} quads in {gt:.2?}  ({:.0}% of the quads, {:.0}% of the vertex data)",
        100.0 * gq as f32 / sq.max(1) as f32,
        100.0 * gq as f32 / sq.max(1) as f32
    );
}

fn highest_solid(c: &Client, reg: &BlockRegistry, x: i32, z: i32) -> Option<i32> {
    for y in (-96..=127).rev() {
        if let Some(s) = c.store.block(BlockPos::new(x, y, z)) {
            if reg.get(s).solid {
                return Some(y);
            }
        }
    }
    None
}

fn top_block(c: &Client, x: i32, z: i32) -> Option<(i32, blockforge::block::BlockState)> {
    for y in (-96..=127).rev() {
        if let Some(s) = c.store.block(BlockPos::new(x, y, z)) {
            if !s.is_air() {
                return Some((y, s));
            }
        }
    }
    None
}

fn print_map(c: &Client, reg: &BlockRegistry) {
    let water = reg.id("water");
    let shades: Vec<char> = " .:-=+*#%@".chars().collect();
    println!("\nTop-down height map (x -64..64, z -48..48; '~' = water, ' ' = not loaded):");
    for z in (-48..48).step_by(3) {
        let mut row = String::new();
        for x in (-64..64).step_by(2) {
            row.push(match top_block(c, x, z) {
                None => ' ',
                Some((_, s)) if Some(s.id()) == water => '~',
                Some((y, _)) => {
                    let t = ((y + 20).clamp(0, 59) as usize * shades.len()) / 60;
                    shades[t.min(shades.len() - 1)]
                }
            });
        }
        println!("{row}");
    }
}

/// Level-of-detail terrain around the spawn: how much of it there is per quality and
/// distance, and how long it takes to build on one thread.
fn lod_figures(reg: &BlockRegistry, seed: u64) {
    let gen = blockforge::worldgen::WorldGen::new(seed, reg);
    println!("\nLOD terrain from the seed (whole view around the spawn, before culling):");
    for (name, unit) in LodSettings::QUALITY {
        for radius in [4096.0f32, 16384.0] {
            let keys = LodSettings::new(radius, unit).select([0.5, 40.0, 0.5]);
            let t = Instant::now();
            let quads: usize = keys.iter().map(|k| build_section(&gen, reg, *k).quads()).sum();
            println!(
                "  {name:4} {:>2} km: {:3} sections, {:4}k quads ({:4.1} MB), built in {:.0?} on one thread",
                radius as u32 / 1024,
                keys.len(),
                quads / 1000,
                quads as f32 * 32.0 / 1.0e6,
                t.elapsed()
            );
        }
    }
}

fn main() {
    let registry = Arc::new(BlockRegistry::builtin());
    let sand = registry.state("sand").expect("sand block");

    let cfg = ServerConfig { seed: 1337, ..ServerConfig::default() };
    let handle = server::spawn(cfg, registry.clone());
    let mut client = Client::connect(handle.connect(), default_worker_threads());
    client.set_view_distance(6);
    client.set_position([0.0, 20.0, 0.0]);

    println!("Streaming and meshing around the origin...");
    let t0 = Instant::now();
    wait_until_settled(&mut client, Duration::from_secs(60));
    let elapsed = t0.elapsed();

    let (mut uniform, mut mixed) = (0, 0);
    for (_, chunk) in client.store.iter() {
        if chunk.is_uniform() {
            uniform += 1;
        } else {
            mixed += 1;
        }
    }
    let raw = client.store.len() * CHUNK_VOLUME * 2;
    let (mesh_chunks, quads) = client.mesh_totals();
    let vbytes: usize = client.meshes.values().map(|m| m.vertex_bytes()).sum();
    println!("  settled in {:.2?}", elapsed);
    println!(
        "  chunks: {} ({} uniform, {} mixed), {} KiB palette-compressed vs {} KiB raw",
        client.store.len(),
        uniform,
        mixed,
        client.store.memory_bytes() / 1024,
        raw / 1024
    );
    println!(
        "  meshes: {} chunks with geometry, {} quads, {} KiB of 8-byte vertices ({} stale results dropped, {} chunks skipped as empty)",
        mesh_chunks,
        quads,
        vbytes / 1024,
        client.stats.stale_meshes,
        client.stats.meshes_skipped
    );

    print_map(&client, &registry);
    compare_meshers(&client, &registry);
    lod_figures(&registry, client.seed);

    if let Some(g) = highest_solid(&client, &registry, 0, 0) {
        let name = &registry.get(client.store.block(BlockPos::new(0, g, 0)).unwrap()).name;
        println!("\nGround at (0, {g}, 0) is {name}. Breaking it, then dropping sand from 12 blocks up...");
        client.break_block(BlockPos::new(0, g, 0));
        pump(&mut client, Duration::from_millis(600));
        client.place_block(BlockPos::new(0, g + 12, 0), sand);
        pump(&mut client, Duration::from_secs(3));
        match top_block(&client, 0, 0) {
            Some((y, s)) => println!("  top block at (0,0) is now {} at y={y}", registry.get(s).name),
            None => println!("  column not loaded"),
        }
        println!("  block updates received: {}", client.stats.block_updates);
    }

    let s = handle.stats();
    println!(
        "\nServer: {} ticks, {} chunks generated, {} behaviour events, {} block edits, {} chunks resident",
        s.ticks, s.chunks_generated, s.events_processed, s.block_edits, s.loaded_chunks
    );
}
