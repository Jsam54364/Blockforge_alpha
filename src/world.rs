//! Authoritative world state: loaded chunks, per-column data, and the edit journal.
//!
//! Saves store ONLY the journal (edits on top of procedural terrain), which keeps
//! world files tiny, makes multiplayer sync delta-only, and gives undo/redo for free.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::block::BlockState;
use crate::chunk::Chunk;
use crate::coords::{BlockPos, ChunkPos};
use crate::registry::BlockRegistry;
use crate::worldgen::{Column, WorldGen};

const HISTORY_LIMIT: usize = 4096;

/// A bag of loaded chunks. Used by the server (authoritative) and the client (replica).
#[derive(Default)]
pub struct ChunkStore {
    chunks: HashMap<ChunkPos, Chunk>,
}

impl ChunkStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, p: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&p)
    }

    pub fn get_mut(&mut self, p: ChunkPos) -> Option<&mut Chunk> {
        self.chunks.get_mut(&p)
    }

    pub fn insert(&mut self, p: ChunkPos, c: Chunk) -> Option<Chunk> {
        self.chunks.insert(p, c)
    }

    pub fn remove(&mut self, p: ChunkPos) -> Option<Chunk> {
        self.chunks.remove(&p)
    }

    pub fn contains(&self, p: ChunkPos) -> bool {
        self.chunks.contains_key(&p)
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.chunks.keys().copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ChunkPos, &Chunk)> {
        self.chunks.iter()
    }

    /// `None` when the chunk is not loaded.
    pub fn block(&self, pos: BlockPos) -> Option<BlockState> {
        self.chunks.get(&pos.chunk()).map(|c| c.get(pos.local().index()))
    }

    /// Returns the previous state, or `None` if the chunk is not loaded.
    pub fn set_block(&mut self, pos: BlockPos, state: BlockState) -> Option<BlockState> {
        let c = self.chunks.get_mut(&pos.chunk())?;
        let idx = pos.local().index();
        let old = c.get(idx);
        c.set(idx, state);
        Some(old)
    }

    pub fn memory_bytes(&self) -> usize {
        self.chunks.values().map(|c| c.memory_bytes()).sum()
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub pos: BlockPos,
    pub old: BlockState,
    pub new: BlockState,
}

/// Edits layered over procedural terrain: chunk -> (local index -> final state).
#[derive(Default, Clone, Debug)]
pub struct EditJournal {
    edits: HashMap<ChunkPos, HashMap<u16, BlockState>>,
}

impl EditJournal {
    pub fn record(&mut self, pos: BlockPos, state: BlockState) {
        self.record_local(pos.chunk(), pos.local().index() as u16, state);
    }

    pub fn record_local(&mut self, cp: ChunkPos, idx: u16, state: BlockState) {
        self.edits.entry(cp).or_default().insert(idx, state);
    }

    pub fn apply_to(&self, cp: ChunkPos, chunk: &mut Chunk) {
        if let Some(m) = self.edits.get(&cp) {
            for (&i, &s) in m {
                chunk.set(i as usize, s);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.edits.values().map(|m| m.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// Deterministic ordering, for stable save files.
    pub fn to_sorted(&self) -> Vec<(ChunkPos, Vec<(u16, BlockState)>)> {
        let mut v: Vec<(ChunkPos, Vec<(u16, BlockState)>)> = self
            .edits
            .iter()
            .map(|(&cp, m)| {
                let mut e: Vec<(u16, BlockState)> = m.iter().map(|(&i, &s)| (i, s)).collect();
                e.sort_unstable_by_key(|&(i, _)| i);
                (cp, e)
            })
            .collect();
        v.sort_unstable_by_key(|(cp, _)| *cp);
        v
    }
}

pub struct World {
    pub registry: Arc<BlockRegistry>,
    pub generator: Arc<WorldGen>,
    store: ChunkStore,
    columns: HashMap<(i32, i32), Arc<Column>>,
    journal: EditJournal,
    undo: VecDeque<Edit>,
    redo: Vec<Edit>,
}

impl World {
    pub fn new(registry: Arc<BlockRegistry>, generator: Arc<WorldGen>) -> Self {
        World {
            registry,
            generator,
            store: ChunkStore::new(),
            columns: HashMap::new(),
            journal: EditJournal::default(),
            undo: VecDeque::new(),
            redo: Vec::new(),
        }
    }

    pub fn chunk(&self, p: ChunkPos) -> Option<&Chunk> {
        self.store.get(p)
    }

    pub fn has_chunk(&self, p: ChunkPos) -> bool {
        self.store.contains(p)
    }

    pub fn loaded_chunks(&self) -> usize {
        self.store.len()
    }

    pub fn loaded_positions(&self) -> Vec<ChunkPos> {
        self.store.positions().collect()
    }

    pub fn journal(&self) -> &EditJournal {
        &self.journal
    }

    pub fn set_journal(&mut self, j: EditJournal) {
        self.journal = j;
    }

    /// Cached column data (heightmap etc.), generated on first request.
    pub fn column(&mut self, cx: i32, cz: i32) -> Arc<Column> {
        if let Some(c) = self.columns.get(&(cx, cz)) {
            return c.clone();
        }
        let c = Arc::new(self.generator.column(cx, cz));
        self.columns.insert((cx, cz), c.clone());
        c
    }

    /// Drop column data that no loaded chunk needs.
    pub fn prune_columns(&mut self) {
        let live: HashSet<(i32, i32)> = self.store.positions().map(|p| (p.x, p.z)).collect();
        self.columns.retain(|k, _| live.contains(k));
    }

    /// A freshly generated chunk arrives (from a worker): replay journal edits, then store.
    pub fn insert_generated(&mut self, pos: ChunkPos, mut chunk: Chunk) {
        self.journal.apply_to(pos, &mut chunk);
        self.store.insert(pos, chunk);
    }

    pub fn unload(&mut self, pos: ChunkPos) {
        self.store.remove(pos);
    }

    pub fn get_block(&self, pos: BlockPos) -> Option<BlockState> {
        self.store.block(pos)
    }

    /// Returns the edit if anything changed. `None` if unchanged or the chunk isn't loaded.
    pub fn set_block(&mut self, pos: BlockPos, state: BlockState, record_history: bool) -> Option<Edit> {
        let cp = pos.chunk();
        let idx = pos.local().index();
        let chunk = self.store.get_mut(cp)?;
        let old = chunk.get(idx);
        if old == state {
            return None;
        }
        chunk.set(idx, state);
        self.journal.record(pos, state);
        let edit = Edit { pos, old, new: state };
        if record_history {
            if self.undo.len() >= HISTORY_LIMIT {
                self.undo.pop_front();
            }
            self.undo.push_back(edit);
            self.redo.clear();
        }
        Some(edit)
    }

    /// Undo the most recent player edit. Returns the edit actually applied.
    pub fn undo(&mut self) -> Option<Edit> {
        while let Some(e) = self.undo.pop_back() {
            if let Some(applied) = self.set_block(e.pos, e.old, false) {
                self.redo.push(e);
                return Some(applied);
            }
        }
        None
    }

    pub fn redo(&mut self) -> Option<Edit> {
        while let Some(e) = self.redo.pop() {
            if let Some(applied) = self.set_block(e.pos, e.new, false) {
                self.undo.push_back(e);
                return Some(applied);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        let reg = Arc::new(BlockRegistry::builtin());
        let g = Arc::new(WorldGen::new(42, &reg));
        World::new(reg, g)
    }

    fn load(w: &mut World, p: ChunkPos) {
        let col = w.column(p.x, p.z);
        let c = w.generator.generate_chunk(p, &col);
        w.insert_generated(p, c);
    }

    #[test]
    fn edit_undo_redo() {
        let mut w = world();
        let p = ChunkPos::new(0, 2, 0); // y 64..95: always sky (terrain tops out near y=42)
        load(&mut w, p);
        let pos = BlockPos::new(3, 70, 3);
        let stone = w.registry.state("stone").unwrap();
        assert_eq!(w.get_block(pos).unwrap().is_air(), true);
        assert!(w.set_block(pos, stone, true).is_some());
        assert_eq!(w.get_block(pos), Some(stone));
        assert!(w.set_block(pos, stone, true).is_none()); // no-op
        let e = w.undo().unwrap();
        assert_eq!(e.new.is_air(), true);
        assert!(w.get_block(pos).unwrap().is_air());
        w.redo().unwrap();
        assert_eq!(w.get_block(pos), Some(stone));
    }

    #[test]
    fn journal_survives_unload_and_regeneration() {
        let mut w = world();
        let p = ChunkPos::new(0, 0, 0);
        load(&mut w, p);
        let pos = BlockPos::new(5, 3, 5);
        let sand = w.registry.state("sand").unwrap();
        w.set_block(pos, sand, true).unwrap();
        w.unload(p);
        assert!(w.get_block(pos).is_none());
        load(&mut w, p);
        assert_eq!(w.get_block(pos), Some(sand));
    }

    #[test]
    fn unloaded_chunk_edit_is_refused() {
        let mut w = world();
        assert!(w.set_block(BlockPos::new(0, 0, 0), BlockState::full(1), true).is_none());
    }
}
