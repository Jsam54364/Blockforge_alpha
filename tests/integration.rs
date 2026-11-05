//! End-to-end: real server thread, real client, real worker pools.

use std::sync::Arc;
use std::time::{Duration, Instant};

use blockforge::client::Client;
use blockforge::coords::BlockPos;
use blockforge::registry::BlockRegistry;
use blockforge::server::{self, ServerConfig};

fn pump_until<F: FnMut(&mut Client) -> bool>(c: &mut Client, timeout: Duration, mut done: F) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        c.update();
        if done(c) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

fn start(seed: u64) -> (server::ServerHandle, Client, Arc<BlockRegistry>) {
    let reg = Arc::new(BlockRegistry::builtin());
    let cfg = ServerConfig { seed, tick_hz: 50, worker_threads: 2, ..ServerConfig::default() };
    let handle = server::spawn(cfg, reg.clone());
    let mut client = Client::connect(handle.connect(), 2);
    client.set_view_distance(3);
    client.set_position([0.0, 10.0, 0.0]);
    (handle, client, reg)
}

fn loaded(c: &mut Client) -> bool {
    c.stats.chunks_received >= 100 && c.is_idle() && !c.meshes.is_empty()
}

#[test]
fn streams_generates_and_meshes() {
    let (_h, mut c, _reg) = start(7);
    assert!(
        pump_until(&mut c, Duration::from_secs(60), loaded),
        "timed out: {:?}",
        c.stats
    );
    let (mesh_chunks, quads) = c.mesh_totals();
    assert!(mesh_chunks > 0 && quads > 0);
    // The renderer is told about every mesh that appeared.
    assert!(!c.take_mesh_changes(usize::MAX).is_empty());
    // Cubic chunks: sky and deep underground are uniform, so far fewer meshes than chunks.
    assert!(mesh_chunks < c.store.len());
}

#[test]
fn break_and_place_roundtrip_through_server() {
    let (_h, mut c, reg) = start(7);
    assert!(pump_until(&mut c, Duration::from_secs(60), loaded));
    let ground = (-64..=95)
        .rev()
        .find(|&y| c.store.block(BlockPos::new(0, y, 0)).map_or(false, |s| reg.get(s).solid))
        .expect("ground at origin");
    let pos = BlockPos::new(0, ground, 0);
    c.break_block(pos);
    assert!(pump_until(&mut c, Duration::from_secs(10), |c| c
        .store
        .block(pos)
        .map_or(false, |s| s.is_air())));
    // Undo restores it.
    c.undo();
    assert!(pump_until(&mut c, Duration::from_secs(10), |c| c
        .store
        .block(pos)
        .map_or(false, |s| !s.is_air())));
}

#[test]
fn sand_falls_and_lands_on_ground() {
    let (_h, mut c, reg) = start(7);
    assert!(pump_until(&mut c, Duration::from_secs(60), loaded));
    let sand = reg.state("sand").unwrap();
    let ground = (-64..=95)
        .rev()
        .find(|&y| c.store.block(BlockPos::new(0, y, 0)).map_or(false, |s| reg.get(s).solid))
        .expect("ground at origin");
    let start_pos = BlockPos::new(0, ground + 8, 0);
    c.place_block(start_pos, sand);
    let landed_at = BlockPos::new(0, ground + 1, 0);
    assert!(
        pump_until(&mut c, Duration::from_secs(15), |c| c.store.block(landed_at) == Some(sand)),
        "sand never landed"
    );
    // ...and it left its starting cell.
    assert!(c.store.block(start_pos).map_or(false, |s| s.is_air() || !reg.get(s).solid));
}
