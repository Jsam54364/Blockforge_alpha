//! The authoritative game server. Singleplayer runs it on its own thread and talks to
//! the client through the same message protocol as a remote player would.
//!
//! Tick order (all deterministic given the same inputs):
//!   1. accept connections, read client messages
//!   2. integrate finished chunk-generation jobs
//!   3. apply player edits, then queued behaviour commands
//!   4. fire due scheduled ticks, run behaviours (budgeted), apply their commands
//!   5. stream chunks to clients (budgeted, nearest first), unload far ones
//!
//! Simulation distance (`sim_distance_chunks`) is independent of how far clients can
//! see: events outside it are dropped for now (TODO: re-scan a chunk when it enters
//! simulation range).
//!
//! Not yet implemented: reach/anti-cheat validation, per-player undo history, a real
//! network transport, saving on a background thread (autosave blocks the tick).

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::behavior::{BehaviorCtx, BehaviorTable, BlockEvent, Command};
use crate::block::{BlockState, AIR};
use crate::chunk::Chunk;
use crate::coords::{BlockPos, ChunkPos, Face};
use crate::jobs::{default_worker_threads, JobPool};
use crate::protocol::{local_pair, ClientMsg, Endpoint, ServerMsg};
use crate::registry::BlockRegistry;
use crate::rng::Rng;
use crate::save;
use crate::tick::FixedTimestep;
use crate::world::{Edit, EditJournal, World};
use crate::worldgen::{Column, WorldGen};

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub seed: u64,
    pub tick_hz: u32,
    /// Chunks around a player (all axes) in which blocks are simulated.
    pub sim_distance_chunks: i32,
    pub max_chunk_sends_per_tick: usize,
    pub max_events_per_tick: usize,
    pub worker_threads: usize,
    /// Vertical streaming radius cap; terrain is mostly horizontal so this stays small.
    pub vertical_view_chunks: i32,
    pub save_path: Option<PathBuf>,
    /// 0 disables autosave.
    pub autosave_ticks: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            seed: 1,
            tick_hz: 20,
            sim_distance_chunks: 4,
            max_chunk_sends_per_tick: 24,
            max_events_per_tick: 4096,
            worker_threads: default_worker_threads(),
            vertical_view_chunks: 2,
            save_path: None,
            autosave_ticks: 1200,
        }
    }
}

#[derive(Default, Clone, Debug)]
pub struct ServerStats {
    pub ticks: u64,
    pub chunks_generated: u64,
    pub events_processed: u64,
    pub block_edits: u64,
    pub loaded_chunks: usize,
}

struct GenJob {
    pos: ChunkPos,
    column: Arc<Column>,
}

struct GenResult {
    pos: ChunkPos,
    chunk: Chunk,
}

struct Session {
    link: Endpoint<ServerMsg, ClientMsg>,
    view_chunks: i32,
    center: ChunkPos,
    sent: HashSet<ChunkPos>,
    alive: bool,
}

pub struct Server {
    cfg: ServerConfig,
    world: World,
    behaviors: BehaviorTable,
    pool: JobPool<GenJob, GenResult>,
    sessions: Vec<Session>,
    incoming: mpsc::Receiver<Endpoint<ServerMsg, ClientMsg>>,
    in_flight: HashSet<ChunkPos>,
    tick: u64,
    rng: Rng,
    player_edits: Vec<(BlockPos, BlockState)>,
    commands: Vec<Command>,
    events: VecDeque<(BlockPos, BlockEvent)>,
    scheduled: BinaryHeap<Reverse<(u64, u64, BlockPos)>>,
    sched_seq: u64,
    stats: ServerStats,
    shared_stats: Arc<Mutex<ServerStats>>,
}

fn load_saved(path: &Option<PathBuf>) -> Option<(u64, EditJournal)> {
    let path = path.as_ref()?;
    let bytes = std::fs::read(path).ok()?;
    match save::decode_journal(&bytes) {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("server: could not load {}: {e}", path.display());
            None
        }
    }
}

impl Server {
    pub fn new(
        mut cfg: ServerConfig,
        registry: Arc<BlockRegistry>,
        incoming: mpsc::Receiver<Endpoint<ServerMsg, ClientMsg>>,
        shared_stats: Arc<Mutex<ServerStats>>,
    ) -> Self {
        // A saved world keeps its own seed.
        let saved = load_saved(&cfg.save_path);
        if let Some((seed, _)) = &saved {
            cfg.seed = *seed;
        }

        let generator = Arc::new(WorldGen::new(cfg.seed, &registry));
        let mut world = World::new(registry.clone(), generator.clone());
        if let Some((_, journal)) = saved {
            world.set_journal(journal);
        }
        let behaviors = BehaviorTable::builtin(&registry).expect("block behaviors must resolve");

        let for_workers = generator.clone();
        let pool = JobPool::new(cfg.worker_threads, move |job: GenJob| GenResult {
            pos: job.pos,
            chunk: for_workers.generate_chunk(job.pos, &job.column),
        });

        let seed = cfg.seed;
        Server {
            cfg,
            world,
            behaviors,
            pool,
            sessions: Vec::new(),
            incoming,
            in_flight: HashSet::new(),
            tick: 0,
            rng: Rng::new(seed),
            player_edits: Vec::new(),
            commands: Vec::new(),
            events: VecDeque::new(),
            scheduled: BinaryHeap::new(),
            sched_seq: 0,
            stats: ServerStats::default(),
            shared_stats,
        }
    }

    pub fn run(&mut self, shutdown: &AtomicBool) {
        let mut ts = FixedTimestep::new(self.cfg.tick_hz);
        while !shutdown.load(AtomicOrdering::Relaxed) {
            let n = ts.advance(Instant::now());
            for _ in 0..n {
                self.step();
            }
            let nap = ts
                .time_until_next()
                .min(Duration::from_millis(4))
                .max(Duration::from_micros(200));
            thread::sleep(nap);
        }
        self.save_world();
    }

    pub fn step(&mut self) {
        self.accept_connections();
        self.read_clients();
        self.poll_generation();
        self.apply_player_edits();
        self.apply_commands();
        self.fire_scheduled();
        self.process_events();
        self.apply_commands();
        self.stream_chunks();

        if self.tick % 100 == 99 {
            self.unload_unused();
        }
        if self.cfg.autosave_ticks > 0 && self.tick > 0 && self.tick % self.cfg.autosave_ticks == 0 {
            self.save_world();
        }
        self.tick += 1;
        self.stats.ticks = self.tick;
        self.stats.loaded_chunks = self.world.loaded_chunks();
        if let Ok(mut s) = self.shared_stats.lock() {
            *s = self.stats.clone();
        }
    }

    // ---- connections & messages ----

    fn accept_connections(&mut self) {
        while let Ok(link) = self.incoming.try_recv() {
            self.sessions.push(Session {
                link,
                view_chunks: 4,
                center: ChunkPos::default(),
                sent: HashSet::new(),
                alive: true,
            });
        }
    }

    fn read_clients(&mut self) {
        for si in 0..self.sessions.len() {
            loop {
                match self.sessions[si].link.try_recv() {
                    Ok(msg) => self.handle_msg(si, msg),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.sessions[si].alive = false;
                        break;
                    }
                }
            }
        }
        self.sessions.retain(|s| s.alive);
    }

    fn handle_msg(&mut self, si: usize, msg: ClientMsg) {
        match msg {
            ClientMsg::Hello { name: _ } => {
                let welcome = ServerMsg::Welcome {
                    seed: self.cfg.seed,
                    tick_hz: self.cfg.tick_hz,
                    blocks_toml: self.world.registry.source().to_string(),
                };
                self.sessions[si].link.send(welcome);
            }
            ClientMsg::Position { pos } => {
                self.sessions[si].center = BlockPos::from_f32(pos).chunk();
            }
            ClientMsg::SetViewDistance { chunks } => {
                self.sessions[si].view_chunks = (chunks as i32).clamp(2, 32);
            }
            ClientMsg::BreakBlock { pos } => {
                if let Some(s) = self.world.get_block(pos) {
                    let def = self.world.registry.get(s);
                    if !s.is_air() && def.solid && def.hardness >= 0.0 {
                        self.player_edits.push((pos, AIR));
                    }
                }
            }
            ClientMsg::PlaceBlock { pos, state } => {
                let reg = &self.world.registry;
                let valid = !state.is_air() && (state.id() as usize) < reg.len();
                if valid {
                    if let Some(cur) = self.world.get_block(pos) {
                        if cur.is_air() || !reg.get(cur).solid {
                            self.player_edits.push((pos, state));
                        }
                    }
                }
            }
            ClientMsg::Undo => {
                if let Some(e) = self.world.undo() {
                    self.after_edit(e);
                }
            }
            ClientMsg::Redo => {
                if let Some(e) = self.world.redo() {
                    self.after_edit(e);
                }
            }
            ClientMsg::Disconnect => {
                self.sessions[si].alive = false;
            }
        }
    }

    // ---- world changes ----

    fn apply_player_edits(&mut self) {
        let edits = std::mem::take(&mut self.player_edits);
        for (pos, state) in edits {
            self.apply_set(pos, state, true);
        }
    }

    fn apply_commands(&mut self) {
        let cmds = std::mem::take(&mut self.commands);
        for c in cmds {
            match c {
                Command::SetBlock { pos, state } => self.apply_set(pos, state, false),
                Command::ScheduleTick { pos, delay } => {
                    self.sched_seq += 1;
                    let due = self.tick + delay.max(1) as u64;
                    self.scheduled.push(Reverse((due, self.sched_seq, pos)));
                }
            }
        }
    }

    fn apply_set(&mut self, pos: BlockPos, state: BlockState, history: bool) {
        if let Some(edit) = self.world.set_block(pos, state, history) {
            self.after_edit(edit);
        }
    }

    /// Broadcast the change and queue the events it causes.
    fn after_edit(&mut self, e: Edit) {
        self.stats.block_edits += 1;
        self.broadcast_block(e.pos, e.new);
        if !e.old.is_air() {
            self.events.push_back((e.pos, BlockEvent::Broken(e.old)));
        }
        if !e.new.is_air() {
            self.events.push_back((e.pos, BlockEvent::Placed(e.new)));
        }
        for f in Face::ALL {
            let n = e.pos.neighbor(f);
            if let Some(s) = self.world.get_block(n) {
                if self.behaviors.has(s.id()) {
                    self.events.push_back((n, BlockEvent::NeighborChanged(s)));
                }
            }
        }
    }

    fn broadcast_block(&self, pos: BlockPos, state: BlockState) {
        let cp = pos.chunk();
        for s in &self.sessions {
            if s.sent.contains(&cp) {
                s.link.send(ServerMsg::BlockUpdate { pos, state });
            }
        }
    }

    fn fire_scheduled(&mut self) {
        while let Some(Reverse((due, _, pos))) = self.scheduled.peek().copied() {
            if due > self.tick {
                break;
            }
            self.scheduled.pop();
            if let Some(s) = self.world.get_block(pos) {
                self.events.push_back((pos, BlockEvent::Tick(s)));
            }
        }
    }

    fn is_simulated(&self, pos: BlockPos) -> bool {
        let c = pos.chunk();
        if !self.world.has_chunk(c) {
            return false;
        }
        let d = self.cfg.sim_distance_chunks;
        self.sessions.iter().any(|s| {
            s.alive
                && (c.x - s.center.x).abs() <= d
                && (c.y - s.center.y).abs() <= d
                && (c.z - s.center.z).abs() <= d
        })
    }

    /// Budgeted: at most `max_events_per_tick` behaviour calls; the rest wait a tick.
    fn process_events(&mut self) {
        let mut budget = self.cfg.max_events_per_tick;
        let mut out: Vec<Command> = Vec::new();
        while budget > 0 {
            let Some((pos, ev)) = self.events.pop_front() else { break };
            budget -= 1;
            let Some(b) = self.behaviors.get(ev.state().id()) else { continue };
            if !self.is_simulated(pos) {
                continue;
            }
            {
                let mut ctx = BehaviorCtx::new(&self.world, &mut self.rng, self.tick, &mut out);
                match ev {
                    BlockEvent::Placed(s) => b.on_placed(&mut ctx, pos, s),
                    BlockEvent::Broken(s) => b.on_broken(&mut ctx, pos, s),
                    BlockEvent::NeighborChanged(s) => b.on_neighbor_changed(&mut ctx, pos, s),
                    BlockEvent::Tick(s) => b.on_tick(&mut ctx, pos, s),
                }
            }
            self.stats.events_processed += 1;
        }
        self.commands.extend(out);
    }

    // ---- generation & streaming ----

    fn poll_generation(&mut self) {
        for r in self.pool.poll() {
            self.in_flight.remove(&r.pos);
            self.world.insert_generated(r.pos, r.chunk);
            self.stats.chunks_generated += 1;
        }
    }

    fn stream_chunks(&mut self) {
        let mut sends_left = self.cfg.max_chunk_sends_per_tick;
        for si in 0..self.sessions.len() {
            let center = self.sessions[si].center;
            let r = self.sessions[si].view_chunks;
            let rv = r.min(self.cfg.vertical_view_chunks);

            let mut ready: Vec<(i32, ChunkPos)> = Vec::new();
            for dy in -rv..=rv {
                for dz in -r..=r {
                    for dx in -r..=r {
                        let h = dx * dx + dz * dz;
                        if h > r * r + r {
                            continue; // roughly circular footprint
                        }
                        let p = center.offset(dx, dy, dz);
                        if self.sessions[si].sent.contains(&p) {
                            continue;
                        }
                        let prio = h + dy * dy * 2;
                        if self.world.has_chunk(p) {
                            ready.push((prio, p));
                        } else if !self.in_flight.contains(&p) {
                            let column = self.world.column(p.x, p.z);
                            self.pool.submit(GenJob { pos: p, column }, prio as u32);
                            self.in_flight.insert(p);
                        }
                    }
                }
            }

            ready.sort_unstable_by_key(|&(d, _)| d);
            for (_, p) in ready {
                if sends_left == 0 {
                    break;
                }
                if let Some(chunk) = self.world.chunk(p) {
                    let msg = ServerMsg::ChunkData { pos: p, chunk: Box::new(chunk.clone()) };
                    if self.sessions[si].link.send(msg) {
                        self.sessions[si].sent.insert(p);
                    }
                    sends_left -= 1;
                }
            }

            // Hysteresis: request inside `r`, release outside `r + 1`.
            let limit = r + 1;
            let far: Vec<ChunkPos> = self.sessions[si]
                .sent
                .iter()
                .copied()
                .filter(|p| {
                    (p.x - center.x).abs() > limit
                        || (p.z - center.z).abs() > limit
                        || (p.y - center.y).abs() > rv + 1
                })
                .collect();
            for p in far {
                self.sessions[si].sent.remove(&p);
                self.sessions[si].link.send(ServerMsg::ChunkUnload { pos: p });
            }
        }
    }

    /// Server keeps chunks a bit longer than clients do, so walking back is instant.
    /// Edits are in the journal, so unloading loses nothing.
    fn unload_unused(&mut self) {
        let vcap = self.cfg.vertical_view_chunks;
        let sessions = &self.sessions;
        let to_unload: Vec<ChunkPos> = self
            .world
            .loaded_positions()
            .into_iter()
            .filter(|p| {
                !sessions.iter().any(|s| {
                    s.alive
                        && (p.x - s.center.x).abs() <= s.view_chunks + 3
                        && (p.z - s.center.z).abs() <= s.view_chunks + 3
                        && (p.y - s.center.y).abs() <= s.view_chunks.min(vcap) + 3
                })
            })
            .collect();
        for p in to_unload {
            self.world.unload(p);
        }
        self.world.prune_columns();
    }

    // ---- persistence ----

    fn save_world(&self) {
        let Some(path) = &self.cfg.save_path else { return };
        let bytes = save::encode_journal(self.cfg.seed, self.world.journal());
        let tmp = path.with_extension("tmp");
        // Write-then-rename so a crash never leaves a half-written save.
        if std::fs::write(&tmp, &bytes).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

pub struct ServerHandle {
    shutdown: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    incoming: mpsc::Sender<Endpoint<ServerMsg, ClientMsg>>,
    stats: Arc<Mutex<ServerStats>>,
}

impl ServerHandle {
    /// Open a new in-process connection to the server.
    pub fn connect(&self) -> Endpoint<ClientMsg, ServerMsg> {
        let (client_end, server_end) = local_pair();
        let _ = self.incoming.send(server_end);
        client_end
    }

    pub fn stats(&self) -> ServerStats {
        self.stats.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, AtomicOrdering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

pub fn spawn(cfg: ServerConfig, registry: Arc<BlockRegistry>) -> ServerHandle {
    let (in_tx, in_rx) = mpsc::channel();
    let shutdown = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(Mutex::new(ServerStats::default()));
    let (flag, shared) = (shutdown.clone(), stats.clone());
    let join = thread::Builder::new()
        .name("server".to_string())
        .spawn(move || {
            let mut server = Server::new(cfg, registry, in_rx, shared);
            server.run(&flag);
        })
        .expect("failed to spawn server thread");
    ServerHandle { shutdown, join: Some(join), incoming: in_tx, stats }
}
