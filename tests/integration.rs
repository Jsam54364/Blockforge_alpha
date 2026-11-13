//! End-to-end: real server thread, real client, real worker pools.

use std::sync::Arc;
use std::time::{Duration, Instant};

use blockforge::client::Client;
use blockforge::coords::{BlockPos, ChunkPos};
use blockforge::mesh::{mesh_chunk_with, Mesher, PaddedChunk};
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

#[test]
fn greedy_mesher_works_through_the_client() {
    let (_h, mut c, reg) = start(7);
    assert!(pump_until(&mut c, Duration::from_secs(60), loaded));
    // Same chunks, both meshers: greedy must use fewer quads.
    let positions: Vec<_> = c.store.positions().collect();
    let (mut simple, mut greedy) = (0usize, 0usize);
    for &p in &positions {
        let pc = PaddedChunk::capture(&c.store, p);
        simple += mesh_chunk_with(Mesher::Simple, &reg, &pc).quad_count();
        greedy += mesh_chunk_with(Mesher::Greedy, &reg, &pc).quad_count();
    }
    assert!(simple > 0 && greedy < simple, "greedy {greedy} vs simple {simple}");

    // Switching the client's mesher re-meshes everything it has loaded.
    let before = c.stats.meshes_built;
    c.set_mesher(Mesher::Greedy);
    assert!(pump_until(&mut c, Duration::from_secs(60), |c| c.is_idle()));
    assert!(c.stats.meshes_built > before, "switching mesher should rebuild meshes");
}

/// Pumps until chunks have stopped arriving and every mesh job has finished.
fn settle(c: &mut Client) {
    let mut last = (0u64, 0u64, 0u64);
    let mut last_change = Instant::now();
    let ok = pump_until(c, Duration::from_secs(60), |c| {
        let now = (c.stats.chunks_received, c.stats.meshes_built, c.stats.meshes_skipped);
        if now != last {
            last = now;
            last_change = Instant::now();
        }
        now.0 >= 100 && c.is_idle() && last_change.elapsed() > Duration::from_millis(500)
    });
    assert!(ok, "never settled: {:?}", c.stats);
}

#[test]
fn streamed_meshes_are_final_and_cost_about_one_job_each() {
    let (_h, mut c, reg) = start(7);
    settle(&mut c);

    // Whatever order chunks arrived in, every mesh must show exactly the faces a fresh mesh
    // of the final world shows: nothing skipped or deferred may leave a stale or missing mesh.
    // Ambient occlusion bits are left out of the comparison: chunks at the edge of the
    // streamed area are not redone when only a diagonal neighbour arrives (see README).
    fn faces(layer: &blockforge::mesh::MeshLayer) -> Vec<[(u32, u32); 4]> {
        const AO: u32 = 3 << 27;
        let mut quads: Vec<[(u32, u32); 4]> = layer
            .vertices
            .chunks(4)
            .map(|q| {
                let mut v = [(q[0].a & !AO, q[0].b), (q[1].a & !AO, q[1].b), (q[2].a & !AO, q[2].b), (q[3].a & !AO, q[3].b)];
                v.sort_unstable();
                v
            })
            .collect();
        quads.sort_unstable();
        quads
    }
    let positions: Vec<_> = c.store.positions().collect();
    let mut with_geometry = 0u64;
    for &p in &positions {
        let fresh = mesh_chunk_with(Mesher::Simple, &reg, &PaddedChunk::capture(&c.store, p));
        match c.meshes.get(&p) {
            None => assert!(fresh.is_empty(), "missing mesh at {p:?}"),
            Some(m) => {
                with_geometry += 1;
                assert!(faces(&m.opaque) == faces(&fresh.opaque), "stale opaque mesh at {p:?}");
                assert!(faces(&m.cutout) == faces(&fresh.cutout), "stale cutout mesh at {p:?}");
                assert!(faces(&m.translucent) == faces(&fresh.translucent), "stale water mesh at {p:?}");
            }
        }
    }
    assert!(with_geometry > 0);

    // Sky and buried chunks never became jobs, and the rest were not rebuilt once per
    // arriving neighbour (that used to be about thirteen jobs per visible chunk).
    assert!(c.stats.meshes_skipped > 0);
    let jobs = c.stats.meshes_built + c.stats.stale_meshes;
    assert!(jobs <= with_geometry * 6, "{jobs} mesh jobs for {with_geometry} visible chunks: {:?}", c.stats);
}

#[test]
fn renderer_can_ask_for_every_mesh_again() {
    let (_h, mut c, _reg) = start(7);
    settle(&mut c);
    let _ = c.take_mesh_changes(usize::MAX);
    assert!(c.take_mesh_changes(usize::MAX).is_empty());
    // What the app does after Android hands it a new surface.
    c.requeue_all_meshes();
    let mut again = c.take_mesh_changes(usize::MAX);
    let mut have: Vec<_> = c.meshes.keys().copied().collect();
    again.sort_unstable();
    have.sort_unstable();
    assert_eq!(again, have);
}

#[test]
fn save_now_writes_edits_that_a_new_server_loads() {
    let dir = std::env::temp_dir().join(format!("blockforge-save-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("world.bfwj");
    let _ = std::fs::remove_file(&path);
    let reg = Arc::new(BlockRegistry::builtin());
    let cfg = ServerConfig {
        seed: 7,
        tick_hz: 50,
        worker_threads: 2,
        save_path: Some(path.clone()),
        ..ServerConfig::default()
    };
    let open = |cfg: &ServerConfig| {
        let handle = server::spawn(cfg.clone(), reg.clone());
        let mut client = Client::connect(handle.connect(), 2);
        client.set_view_distance(3);
        client.set_position([0.0, 10.0, 0.0]);
        (handle, client)
    };

    let (h, mut c) = open(&cfg);
    settle(&mut c);
    // Nothing changed yet, so asking for a save must not create a file.
    assert!(h.save_now(Duration::from_secs(5)));
    assert!(!path.exists());

    let ground = (-64..=95)
        .rev()
        .find(|&y| c.store.block(BlockPos::new(0, y, 0)).map_or(false, |s| reg.get(s).solid))
        .expect("ground at origin");
    let pos = BlockPos::new(0, ground, 0);
    c.break_block(pos);
    assert!(pump_until(&mut c, Duration::from_secs(10), |c| c.store.block(pos).map_or(false, |s| s.is_air())));
    assert!(h.save_now(Duration::from_secs(5)));
    assert!(path.exists(), "save_now should have written the world");
    // Simulate the OS killing the app: the file on disk is all that survives.
    let saved = std::fs::read(&path).unwrap();
    drop(c);
    drop(h);
    std::fs::write(&path, &saved).unwrap();

    let (_h2, mut c2) = open(&cfg);
    settle(&mut c2);
    assert!(c2.store.block(pos).map_or(false, |s| s.is_air()), "the broken block came back");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lod_terrain_streams_in_and_stops_exactly_where_real_chunks_are_drawn() {
    use blockforge::lod::LodSettings;
    let (_h, mut c, _reg) = start(7);
    c.lod.set_settings(LodSettings::new(4096.0, 64.0));
    settle(&mut c);
    let start_t = Instant::now();
    while !c.lod.is_idle() {
        c.update();
        assert!(start_t.elapsed() < Duration::from_secs(60), "LODs never finished");
        std::thread::sleep(Duration::from_millis(2));
    }
    c.update();
    assert!(c.lod.render_set().len() > 50, "{:?}", c.lod.stats);
    assert!(c.lod.stats.drawn_quads > 10_000);

    // The renderer uploads everything first; then coverage is asked for.
    let _ = c.take_mesh_changes(usize::MAX);
    let draw = 3.0 * 32.0 + 16.0;
    let cov = c.lod_coverage(draw);
    let cam = BlockPos::from_f32(c.camera).chunk();
    assert!(cov.covered(cam.x, cam.z), "the camera's own column shows real terrain");
    assert!(cov.count() >= 9);
    assert!(!cov.covered(cam.x + 10, cam.z), "beyond the loaded chunks LODs must show");
    // Every covered column really has its surface meshed in the client.
    for dz in -5..=5 {
        for dx in -5..=5 {
            let (cx, cz) = (cam.x + dx, cam.z + dz);
            if !cov.covered(cx, cz) {
                continue;
            }
            let surface = (-8..8)
                .rev()
                .map(|cy| ChunkPos::new(cx, cy, cz))
                .find(|p| c.store.get(*p).map_or(false, |ch| ch.uniform_state() != Some(blockforge::block::AIR)))
                .expect("a covered column has ground loaded");
            let has_mesh = c.meshes.contains_key(&surface);
            let fresh = mesh_chunk_with(Mesher::Simple, &_reg, &PaddedChunk::capture(&c.store, surface));
            assert_eq!(has_mesh, !fresh.is_empty(), "column {cx},{cz}: surface chunk not drawn");
        }
    }
    // Not uploaded yet means not covered: the hole would show for a frame otherwise.
    c.requeue_all_meshes();
    assert_eq!(c.lod_coverage(draw).count(), 0);
}
