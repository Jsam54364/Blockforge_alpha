//! Client-side replica of the world plus the meshing pipeline.
//!
//! The client never owns game state. It mirrors what the server streams, snapshots
//! dirty chunks into padded copies, meshes them on worker threads, and throws away
//! results that were superseded by a newer edit (version check).
//!
//! The renderer (wgpu) would read `meshes`, upload each chunk's vertex data into a
//! pooled buffer, and draw opaque -> cutout -> translucent.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::sync::Arc;

use crate::block::BlockState;
use crate::coords::{BlockPos, ChunkPos, Face, CHUNK_SIZE};
use crate::jobs::JobPool;
use crate::mesh::{mesh_chunk, ChunkMesh, PaddedChunk};
use crate::protocol::{ClientMsg, Endpoint, ServerMsg};
use crate::registry::BlockRegistry;
use crate::world::ChunkStore;

struct MeshJob {
    pos: ChunkPos,
    version: u64,
    snapshot: PaddedChunk,
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
    pub stale_meshes: u64,
    pub block_updates: u64,
}

pub struct Client {
    link: Endpoint<ClientMsg, ServerMsg>,
    registry: Option<Arc<BlockRegistry>>,
    pub store: ChunkStore,
    pub meshes: HashMap<ChunkPos, ChunkMesh>,
    versions: HashMap<ChunkPos, u64>,
    dirty: HashSet<ChunkPos>,
    pool: Option<JobPool<MeshJob, MeshResult>>,
    pub camera: [f32; 3],
    pub stats: ClientStats,
    pub connected: bool,
    pub seed: u64,
    pub tick_hz: u32,
    workers: usize,
    /// Max mesh jobs submitted per `update()`, to spread snapshot cost over frames.
    pub mesh_budget: usize,
}

impl Client {
    pub fn connect(link: Endpoint<ClientMsg, ServerMsg>, workers: usize) -> Client {
        link.send(ClientMsg::Hello { name: "player".to_string() });
        Client {
            link,
            registry: None,
            store: ChunkStore::new(),
            meshes: HashMap::new(),
            versions: HashMap::new(),
            dirty: HashSet::new(),
            pool: None,
            camera: [0.0; 3],
            stats: ClientStats::default(),
            connected: true,
            seed: 0,
            tick_hz: 20,
            workers: workers.max(1),
            mesh_budget: 8,
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
    }

    /// Nothing waiting to be meshed and nothing in flight.
    pub fn is_idle(&self) -> bool {
        self.dirty.is_empty() && self.pool.as_ref().map_or(true, |p| p.in_flight() == 0)
    }

    /// (chunks that have geometry, total quads)
    pub fn mesh_totals(&self) -> (usize, usize) {
        (self.meshes.len(), self.meshes.values().map(|m| m.quad_count()).sum())
    }

    // ---- internals ----

    fn on_msg(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::Welcome { seed, tick_hz, blocks_toml } => {
                self.seed = seed;
                self.tick_hz = tick_hz;
                match BlockRegistry::from_toml(&blocks_toml) {
                    Ok(reg) => {
                        let reg = Arc::new(reg);
                        let for_workers = reg.clone();
                        self.pool = Some(JobPool::new(self.workers, move |job: MeshJob| {
                            let mesh = mesh_chunk(&for_workers, &job.snapshot);
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
                self.mark_dirty_with_neighbors(pos);
            }
            ServerMsg::ChunkUnload { pos } => {
                self.store.remove(pos);
                self.meshes.remove(&pos);
                self.dirty.remove(&pos);
                self.versions.remove(&pos);
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

    fn submit_mesh_jobs(&mut self) {
        let Some(pool) = self.pool.as_ref() else { return };
        // Keep the queue shallow so priorities stay fresh as the player moves.
        let max_in_flight = self.workers * 3;
        let free = max_in_flight.saturating_sub(pool.in_flight());
        if free == 0 || self.dirty.is_empty() {
            return;
        }
        let cam = BlockPos::from_f32(self.camera).chunk();
        let mut order: Vec<(i64, ChunkPos)> = self.dirty.iter().map(|&p| (p.dist_sq(cam), p)).collect();
        order.sort_unstable_by_key(|&(d, _)| d);
        for (d, p) in order.into_iter().take(free.min(self.mesh_budget)) {
            self.dirty.remove(&p);
            if !self.store.contains(p) {
                continue;
            }
            let version = self.versions.get(&p).copied().unwrap_or(0);
            let snapshot = PaddedChunk::capture(&self.store, p);
            pool.submit(MeshJob { pos: p, version, snapshot }, d.min(u32::MAX as i64) as u32);
        }
    }

    fn collect_mesh_results(&mut self) {
        let Some(pool) = self.pool.as_ref() else { return };
        for r in pool.poll() {
            let current = self.versions.get(&r.pos).copied();
            if current == Some(r.version) && self.store.contains(r.pos) {
                if r.mesh.is_empty() {
                    self.meshes.remove(&r.pos);
                } else {
                    self.meshes.insert(r.pos, r.mesh);
                }
                self.stats.meshes_built += 1;
            } else {
                // A newer edit superseded this snapshot; its replacement is queued.
                self.stats.stale_meshes += 1;
            }
        }
    }
}
