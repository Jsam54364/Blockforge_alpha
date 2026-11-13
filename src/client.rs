//! Client-side replica of the world plus the meshing pipeline.
//!
//! The client never owns game state. It mirrors what the server streams, snapshots
//! dirty chunks into padded copies, meshes them on worker threads, and throws away
//! results that were superseded by a newer edit (version check).
//!
//! The renderer reads `meshes`, uploads each changed chunk's vertex data (see
//! `take_mesh_changes`) and draws opaque -> cutout -> translucent.
//!
//! Two things keep meshing work proportional to what is actually visible:
//!   * chunks that cannot have geometry (open sky, solid rock between solid rock) never
//!     become a job at all (`mesh::has_no_faces`);
//!   * a newly arrived chunk waits briefly for the neighbours the server is still sending,
//!     so it is meshed once with its borders known instead of again per neighbour.
//!
//! Past the loaded chunks, `lod` keeps level-of-detail terrain built from the world seed
//! (see `lod.rs`), and `lod_coverage` tells the renderer where the real terrain already
//! shows the surface, so LOD terrain is hidden exactly there.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::block::{BlockState, AIR};
use crate::chunk::Chunk;
use crate::coords::{BlockPos, ChunkPos, Face, LocalPos, CHUNK_SIZE};
use crate::jobs::JobPool;
use crate::lod::LodManager;
use crate::mesh::{has_no_faces, mesh_chunk_with, ChunkMesh, Mesher, PaddedChunk};
use crate::protocol::{ClientMsg, Endpoint, ServerMsg};
use crate::registry::BlockRegistry;
use crate::world::ChunkStore;
use crate::worldgen::WorldGen;

struct MeshJob {
    pos: ChunkPos,
    version: u64,
    snapshot: PaddedChunk,
    mesher: Mesher,
}

struct MeshResult {
    pos: ChunkPos,
    version: u64,
    mesh: ChunkMesh,
}

#[derive(Default, Debug, Clone)]
pub struct ClientStats {
    pub chunks_received: u64,
    pub meshes_built: u64,
    /// Dirty chunks resolved without a job because they cannot have geometry.
    pub meshes_skipped: u64,
    pub stale_meshes: u64,
    pub block_updates: u64,
}

pub struct Client {
    link: Endpoint<ClientMsg, ServerMsg>,
    registry: Option<Arc<BlockRegistry>>,
    pub store: ChunkStore,
    pub meshes: HashMap<ChunkPos, ChunkMesh>,
    /// Chunks whose entry in `meshes` was inserted or removed since the renderer last looked.
    mesh_changes: Vec<ChunkPos>,
    versions: HashMap<ChunkPos, u64>,
    dirty: HashSet<ChunkPos>,
    /// Chunks that arrived and have not been meshed yet, with their arrival time.
    awaiting_neighbors: HashMap<ChunkPos, Instant>,
    /// Streaming footprint as the server sees it (horizontal radius, vertical cap).
    view_chunks: i32,
    vertical_view: i32,
    /// How long a new chunk may wait for neighbours that are still expected before it is
    /// meshed anyway. Zero meshes every chunk the moment it arrives (the old behaviour).
    pub neighbor_wait: Duration,
    pool: Option<JobPool<MeshJob, MeshResult>>,
    pub camera: [f32; 3],
    pub stats: ClientStats,
    pub connected: bool,
    pub seed: u64,
    pub tick_hz: u32,
    workers: usize,
    /// Max mesh jobs submitted per `update()`, to spread snapshot cost over frames.
    pub mesh_budget: usize,
    /// Which mesher new jobs use. Change it with `set_mesher` so existing chunks are redone.
    pub mesher: Mesher,
    /// Chunks whose current geometry is known: meshed (empty or not) at least once since they
    /// arrived. Their mesh may be an older version while an edit is re-meshed.
    resolved: HashSet<ChunkPos>,
    /// Resolved chunks that block the view from above all over (see `bottom_is_closed` and
    /// `lod_coverage`).
    closed_bottom: HashSet<ChunkPos>,
    /// Level-of-detail terrain past the loaded chunks.
    pub lod: LodManager,
}

/// Whether nothing below `chunk` can be seen from above through it: each of its 32 x 32
/// columns holds an opaque block somewhere (every block shape covers its whole footprint).
/// Not "its bottom layer is solid": a cave crossing the bottom layer far under the ground
/// would then leave the column uncovered for good, and LOD terrain drawn over the real one.
fn bottom_is_closed(reg: &BlockRegistry, chunk: &Chunk) -> bool {
    let closes = |s: BlockState| reg.get(s).opaque;
    if let Some(s) = chunk.uniform_state() {
        return closes(s);
    }
    let n = CHUNK_SIZE as u8;
    // Bottom-up: in ground chunks the first block already settles most columns.
    (0..n).all(|z| (0..n).all(|x| (0..n).any(|y| closes(chunk.get(LocalPos { x, y, z }.index())))))
}

/// Which chunk columns around the camera show their real surface (meshed and within the
/// renderer's draw distance), one bit per column in a 64 x 64 window. LOD terrain is hidden
/// in these columns and drawn everywhere else, so there is neither overlap nor a hole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LodCoverage {
    /// Chunk column at bit 0.
    pub origin: (i32, i32),
    /// Row-major (z then x), 64 bits per row.
    pub bits: [u32; 128],
}

impl LodCoverage {
    pub const SIZE: i32 = 64;

    pub fn empty(origin: (i32, i32)) -> LodCoverage {
        LodCoverage { origin, bits: [0; 128] }
    }

    pub fn covered(&self, cx: i32, cz: i32) -> bool {
        let (x, z) = (cx - self.origin.0, cz - self.origin.1);
        if x < 0 || z < 0 || x >= Self::SIZE || z >= Self::SIZE {
            return false;
        }
        let bit = (z * Self::SIZE + x) as usize;
        self.bits[bit / 32] >> (bit % 32) & 1 == 1
    }

    fn set(&mut self, cx: i32, cz: i32) {
        let (x, z) = (cx - self.origin.0, cz - self.origin.1);
        if x >= 0 && z >= 0 && x < Self::SIZE && z < Self::SIZE {
            let bit = (z * Self::SIZE + x) as usize;
            self.bits[bit / 32] |= 1 << (bit % 32);
        }
    }

    pub fn count(&self) -> u32 {
        self.bits.iter().map(|w| w.count_ones()).sum()
    }
}

impl Client {
    pub fn connect(link: Endpoint<ClientMsg, ServerMsg>, workers: usize) -> Client {
        link.send(ClientMsg::Hello { name: "player".to_string() });
        Client {
            link,
            registry: None,
            store: ChunkStore::new(),
            meshes: HashMap::new(),
            mesh_changes: Vec::new(),
            versions: HashMap::new(),
            dirty: HashSet::new(),
            awaiting_neighbors: HashMap::new(),
            view_chunks: 4, // the server's default until `set_view_distance` is called
            vertical_view: i32::MAX,
            neighbor_wait: Duration::from_millis(400),
            pool: None,
            camera: [0.0; 3],
            stats: ClientStats::default(),
            connected: true,
            seed: 0,
            tick_hz: 20,
            workers: workers.max(1),
            mesh_budget: 8,
            mesher: Mesher::Simple,
            resolved: HashSet::new(),
            closed_bottom: HashSet::new(),
            // One worker: a section is a few hundred microseconds of noise sampling, and
            // chunk meshing near the player matters more.
            lod: LodManager::new(1),
        }
    }

    pub fn registry(&self) -> Option<&Arc<BlockRegistry>> {
        self.registry.as_ref()
    }

    // ---- outgoing ----

    pub fn set_position(&mut self, pos: [f32; 3]) {
        self.camera = pos;
        self.link.send(ClientMsg::Position { pos });
    }

    pub fn set_view_distance(&mut self, chunks: u8) {
        self.view_chunks = (chunks as i32).clamp(2, 32); // same clamp as the server
        self.link.send(ClientMsg::SetViewDistance { chunks });
    }

    pub fn break_block(&mut self, pos: BlockPos) {
        self.link.send(ClientMsg::BreakBlock { pos });
    }

    pub fn place_block(&mut self, pos: BlockPos, state: BlockState) {
        self.link.send(ClientMsg::PlaceBlock { pos, state });
    }

    pub fn undo(&mut self) {
        self.link.send(ClientMsg::Undo);
    }

    pub fn redo(&mut self) {
        self.link.send(ClientMsg::Redo);
    }

    // ---- per-frame work ----

    /// Call once per frame: drain the network, submit mesh jobs, collect finished meshes.
    pub fn update(&mut self) {
        loop {
            match self.link.try_recv() {
                Ok(msg) => self.on_msg(msg),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.connected = false;
                    break;
                }
            }
        }
        self.submit_mesh_jobs();
        self.collect_mesh_results();
        self.lod.update(self.camera);
    }

    /// Switches mesher and re-meshes every loaded chunk with it (old meshes stay on screen
    /// until their replacements arrive).
    pub fn set_mesher(&mut self, m: Mesher) {
        if self.mesher == m {
            return;
        }
        self.mesher = m;
        let all: Vec<ChunkPos> = self.store.positions().collect();
        for p in all {
            self.mark_dirty(p);
        }
    }

    /// Nothing waiting to be meshed and nothing in flight.
    pub fn is_idle(&self) -> bool {
        self.dirty.is_empty() && self.pool.as_ref().map_or(true, |p| p.in_flight() == 0)
    }

    /// Queues every existing mesh for the renderer again. Call after the GPU resources were
    /// recreated (Android destroys the surface whenever the app goes to the background),
    /// otherwise only chunks that change afterwards would be drawn.
    pub fn requeue_all_meshes(&mut self) {
        self.mesh_changes.clear();
        let cam = BlockPos::from_f32(self.camera).chunk();
        let mut all: Vec<ChunkPos> = self.meshes.keys().copied().collect();
        all.sort_unstable_by_key(|p| p.dist_sq(cam)); // nearest first
        self.mesh_changes = all;
        self.lod.requeue_all();
    }

    /// See `LodCoverage`. Call after the renderer took this frame's mesh changes: chunks it
    /// has not uploaded yet don't count. `draw_distance` is the renderer's cut-off for real
    /// chunks (centre distance from the camera, in blocks).
    pub fn lod_coverage(&self, draw_distance: f32) -> LodCoverage {
        let cam = BlockPos::from_f32(self.camera).chunk();
        let half = LodCoverage::SIZE / 2;
        let mut cov = LodCoverage::empty((cam.x - half, cam.z - half));
        let not_uploaded: HashSet<ChunkPos> = self.mesh_changes.iter().copied().collect();
        let vv = self.view_chunks.min(self.vertical_view);
        let r = (self.view_chunks + 1).min(half - 1);
        let d2_max = draw_distance * draw_distance;
        for dz in -r..=r {
            for dx in -r..=r {
                let (cx, cz) = (cam.x + dx, cam.z + dz);
                // From the top of the streamed window down: skip the sky, then every chunk the
                // ground passes through must be drawn (known, uploaded, within the draw
                // distance), down to one that blocks the view all over. Hilly ground often
                // dips into the chunk below, which is further away when seen from above.
                let mut ground = false;
                for cy in (cam.y - vv..=cam.y + vv).rev() {
                    let p = ChunkPos::new(cx, cy, cz);
                    let Some(chunk) = self.store.get(p) else { break };
                    if !self.resolved.contains(&p) || not_uploaded.contains(&p) {
                        break;
                    }
                    if !ground && chunk.uniform_state() == Some(AIR) {
                        continue;
                    }
                    ground = true;
                    let o = p.origin();
                    let h = CHUNK_SIZE as f32 * 0.5;
                    let c = [o.x as f32 + h - self.camera[0], o.y as f32 + h - self.camera[1], o.z as f32 + h - self.camera[2]];
                    if c[0] * c[0] + c[1] * c[1] + c[2] * c[2] > d2_max {
                        break;
                    }
                    if self.closed_bottom.contains(&p) {
                        cov.set(cx, cz);
                        break;
                    }
                }
            }
        }
        cov
    }

    /// Up to `max` chunk positions whose mesh changed (or was removed) since the last call.
    /// The renderer re-uploads `meshes[pos]` if present, or frees its GPU buffers if not.
    pub fn take_mesh_changes(&mut self, max: usize) -> Vec<ChunkPos> {
        let n = max.min(self.mesh_changes.len());
        self.mesh_changes.drain(..n).collect()
    }

    /// Solid block at `pos` in the client's replica (false if unloaded). For raycasts.
    pub fn is_solid(&self, pos: BlockPos) -> bool {
        match (self.store.block(pos), self.registry.as_ref()) {
            (Some(s), Some(r)) => r.get(s).solid,
            _ => false,
        }
    }

    /// (chunks that have geometry, total quads)
    pub fn mesh_totals(&self) -> (usize, usize) {
        (self.meshes.len(), self.meshes.values().map(|m| m.quad_count()).sum())
    }

    // ---- internals ----

    fn on_msg(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::Welcome { seed, tick_hz, vertical_view_chunks, blocks_toml } => {
                self.seed = seed;
                self.vertical_view = vertical_view_chunks.max(0);
                self.tick_hz = tick_hz;
                match BlockRegistry::from_toml(&blocks_toml) {
                    Ok(reg) => {
                        let reg = Arc::new(reg);
                        self.lod.start(Arc::new(WorldGen::new(seed, &reg)), reg.clone());
                        let for_workers = reg.clone();
                        self.pool = Some(JobPool::new(self.workers, move |job: MeshJob| {
                            let mesh = mesh_chunk_with(job.mesher, &for_workers, &job.snapshot);
                            MeshResult { pos: job.pos, version: job.version, mesh }
                        }));
                        self.registry = Some(reg);
                    }
                    Err(e) => eprintln!("client: server sent an invalid block registry: {e}"),
                }
            }
            ServerMsg::ChunkData { pos, chunk } => {
                self.store.insert(pos, *chunk);
                self.stats.chunks_received += 1;
                self.awaiting_neighbors.entry(pos).or_insert_with(Instant::now);
                self.mark_dirty_with_neighbors(pos);
            }
            ServerMsg::ChunkUnload { pos } => {
                self.store.remove(pos);
                if self.meshes.remove(&pos).is_some() {
                    self.mesh_changes.push(pos);
                }
                self.dirty.remove(&pos);
                self.awaiting_neighbors.remove(&pos);
                self.versions.remove(&pos);
                self.resolved.remove(&pos);
                self.closed_bottom.remove(&pos);
            }
            ServerMsg::BlockUpdate { pos, state } => {
                if self.store.set_block(pos, state).is_some() {
                    self.stats.block_updates += 1;
                    self.mark_block_dirty(pos);
                }
            }
        }
    }

    fn mark_dirty(&mut self, p: ChunkPos) {
        if self.store.contains(p) {
            *self.versions.entry(p).or_insert(0) += 1;
            self.dirty.insert(p);
        }
    }

    /// Face neighbours need a remesh because their border faces depend on this chunk.
    /// Edge/corner neighbours (AO only) are intentionally not remeshed.
    fn mark_dirty_with_neighbors(&mut self, p: ChunkPos) {
        self.mark_dirty(p);
        for f in Face::ALL {
            let (dx, dy, dz) = f.normal();
            self.mark_dirty(p.offset(dx, dy, dz));
        }
    }

    fn mark_block_dirty(&mut self, pos: BlockPos) {
        let c = pos.chunk();
        self.mark_dirty(c);
        let l = pos.local();
        let edge = |v: u8| -> i32 {
            if v == 0 {
                -1
            } else if v as i32 == CHUNK_SIZE - 1 {
                1
            } else {
                0
            }
        };
        let (ex, ey, ez) = (edge(l.x), edge(l.y), edge(l.z));
        if ex != 0 {
            self.mark_dirty(c.offset(ex, 0, 0));
        }
        if ey != 0 {
            self.mark_dirty(c.offset(0, ey, 0));
        }
        if ez != 0 {
            self.mark_dirty(c.offset(0, 0, ez));
        }
    }

    /// Is `p` inside the area the server streams around `center`? Mirrors
    /// `Server::stream_chunks`; being slightly off only costs a short wait.
    fn is_expected(&self, p: ChunkPos, center: ChunkPos) -> bool {
        let r = self.view_chunks;
        let (dx, dy, dz) = (p.x - center.x, p.y - center.y, p.z - center.z);
        dy.abs() <= r.min(self.vertical_view) && dx * dx + dz * dz <= r * r + r
    }

    /// A chunk that has never been meshed holds off while any of its 26 neighbours is still
    /// on its way, up to `neighbor_wait`: face neighbours decide which border faces exist,
    /// edge and corner neighbours feed ambient occlusion along the border. Neighbours outside
    /// the streamed area are not waited for, so chunks at its edge mesh straight away (and
    /// are redone when the player moves and their face neighbours arrive).
    fn should_wait(&self, p: ChunkPos, center: ChunkPos) -> bool {
        let Some(arrived) = self.awaiting_neighbors.get(&p) else { return false };
        if arrived.elapsed() >= self.neighbor_wait {
            return false;
        }
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let n = p.offset(dx, dy, dz);
                    if !self.store.contains(n) && self.is_expected(n, center) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn submit_mesh_jobs(&mut self) {
        let (Some(pool), Some(reg)) = (self.pool.as_ref(), self.registry.as_ref()) else { return };
        if self.dirty.is_empty() {
            return;
        }
        // Keep the queue shallow so priorities stay fresh as the player moves.
        let max_in_flight = self.workers * 3;
        let mut free = max_in_flight.saturating_sub(pool.in_flight()).min(self.mesh_budget);
        let cam = BlockPos::from_f32(self.camera).chunk();
        let mut order: Vec<(i64, ChunkPos)> = self.dirty.iter().map(|&p| (p.dist_sq(cam), p)).collect();
        order.sort_unstable_by_key(|&(d, _)| d);
        for (d, p) in order {
            if !self.store.contains(p) {
                self.dirty.remove(&p);
                continue;
            }
            // Free: no snapshot, no job, and it does not use up this frame's budget.
            if has_no_faces(reg, &self.store, p) {
                self.dirty.remove(&p);
                self.awaiting_neighbors.remove(&p);
                if self.meshes.remove(&p).is_some() {
                    self.mesh_changes.push(p);
                }
                // (inline `mark_resolved`: `pool` and `reg` borrow other fields of self)
                self.resolved.insert(p);
                if self.store.get(p).map_or(false, |c| bottom_is_closed(reg, c)) {
                    self.closed_bottom.insert(p);
                } else {
                    self.closed_bottom.remove(&p);
                }
                self.stats.meshes_skipped += 1;
                continue;
            }
            if free == 0 || self.should_wait(p, cam) {
                continue;
            }
            free -= 1;
            self.dirty.remove(&p);
            self.awaiting_neighbors.remove(&p);
            let version = self.versions.get(&p).copied().unwrap_or(0);
            let snapshot = PaddedChunk::capture(&self.store, p);
            pool.submit(MeshJob { pos: p, version, snapshot, mesher: self.mesher }, d.min(u32::MAX as i64) as u32);
        }
    }

    /// `pos` now has its current geometry; note whether its bottom is closed.
    fn mark_resolved(&mut self, pos: ChunkPos) {
        self.resolved.insert(pos);
        let closed = match (self.store.get(pos), self.registry.as_ref()) {
            (Some(chunk), Some(reg)) => bottom_is_closed(reg, chunk),
            _ => false,
        };
        if closed {
            self.closed_bottom.insert(pos);
        } else {
            self.closed_bottom.remove(&pos);
        }
    }

    fn collect_mesh_results(&mut self) {
        let Some(pool) = self.pool.as_ref() else { return };
        for r in pool.poll() {
            let current = self.versions.get(&r.pos).copied();
            if current == Some(r.version) && self.store.contains(r.pos) {
                // Only tell the renderer when there is something to upload or free.
                if r.mesh.is_empty() {
                    if self.meshes.remove(&r.pos).is_some() {
                        self.mesh_changes.push(r.pos);
                    }
                } else {
                    self.meshes.insert(r.pos, r.mesh);
                    self.mesh_changes.push(r.pos);
                }
                self.mark_resolved(r.pos);
                self.stats.meshes_built += 1;
            } else {
                // A newer edit superseded this snapshot; its replacement is queued.
                self.stats.stale_meshes += 1;
            }
        }
    }
}
