//! Authoritative world state: loaded chunks, per-column data, and the edit journal.
//!
//! Saves store ONLY the journal (edits on top of procedural terrain), which keeps
//! world files tiny, makes multiplayer sync delta-only, and gives undo/redo for free.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::block::BlockState;
use crate::chunk::Chunk;
use crate::coords::{BlockPos, ChunkPos, LocalPos, CHUNK_SIZE};
use crate::lod::ColumnTop;
use crate::registry::BlockRegistry;
use crate::worldgen::{Column, WorldGen, BEDROCK_Y, SEA_LEVEL};

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

    /// The state recorded at `pos`, if it was edited.
    pub fn get(&self, pos: BlockPos) -> Option<BlockState> {
        self.edits.get(&pos.chunk()).and_then(|m| m.get(&(pos.local().index() as u16))).copied()
    }

    /// Every block column `(x, z)` with an edit somewhere in it, and its highest edited y.
    pub fn edited_columns(&self) -> HashMap<(i32, i32), i32> {
        let mut out: HashMap<(i32, i32), i32> = HashMap::new();
        for (cp, m) in &self.edits {
            for &i in m.keys() {
                let l = LocalPos::from_index(i as usize);
                let y = cp.y * CHUNK_SIZE + l.y as i32;
                let e = out.entry((cp.x * CHUNK_SIZE + l.x as i32, cp.z * CHUNK_SIZE + l.z as i32)).or_insert(y);
                *e = (*e).max(y);
            }
        }
        out
    }

    /// The highest edited y in block column `(x, z)`.
    pub fn top_edit_y(&self, x: i32, z: i32) -> Option<i32> {
        let (cx, cz) = (x.div_euclid(CHUNK_SIZE), z.div_euclid(CHUNK_SIZE));
        let (lx, lz) = (x.rem_euclid(CHUNK_SIZE) as u8, z.rem_euclid(CHUNK_SIZE) as u8);
        let mut top = None;
        for (cp, m) in &self.edits {
            if cp.x != cx || cp.z != cz {
                continue;
            }
            for &i in m.keys() {
                let l = LocalPos::from_index(i as usize);
                if l.x == lx && l.z == lz {
                    let y = cp.y * CHUNK_SIZE + l.y as i32;
                    top = Some(top.map_or(y, |t: i32| t.max(y)));
                }
            }
        }
        top
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

    /// The top of block column `(x, z)` as it is now (generated terrain plus edits, whether
    /// or not its chunks are loaded): what LOD terrain should show there.
    pub fn column_top(&mut self, x: i32, z: i32) -> ColumnTop {
        let e = self.journal.top_edit_y(x, z);
        self.column_top_below(x, z, e)
    }

    /// The top of every block column with edits (what a client joining needs).
    pub fn edited_column_tops(&mut self) -> Vec<ColumnTop> {
        let mut cols: Vec<((i32, i32), i32)> = self.journal.edited_columns().into_iter().collect();
        cols.sort_unstable();
        cols.into_iter().map(|((x, z), e)| self.column_top_below(x, z, Some(e))).collect()
    }

    /// `column_top`, given the highest edited y in the column.
    fn column_top_below(&mut self, x: i32, z: i32, top_edit: Option<i32>) -> ColumnTop {
        let col = self.column(x.div_euclid(CHUNK_SIZE), z.div_euclid(CHUNK_SIZE));
        let hq = col.height_q_at(x.rem_euclid(CHUNK_SIZE) as usize, z.rem_euclid(CHUNK_SIZE) as usize);
        // The terrain's top block or the sea, or an edit above them (trees and other
        // features are left out: distant terrain doesn't draw them).
        let mut y = (hq.div_euclid(4) + 1).max(SEA_LEVEL);
        if let Some(e) = top_edit {
            y = y.max(e);
        }
        while y > BEDROCK_Y {
            let pos = BlockPos::new(x, y, z);
            let s = self.journal.get(pos).unwrap_or_else(|| self.generator.seed_block(&col, x, y, z));
            if !s.is_air() {
                return ColumnTop { x, z, top_q: 4 * y + s.shape().y_range_q().1, block: s.id() };
            }
            y -= 1;
        }
        ColumnTop { x, z, top_q: 4 * (BEDROCK_Y + 1), block: self.generator.seed_block(&col, x, BEDROCK_Y, z).id() }
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
        let p = ChunkPos::new(0, 10, 0); // y 320..351: always sky (peaks stay under ~200)
        load(&mut w, p);
        let pos = BlockPos::new(3, 330, 3);
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
    fn column_tops_follow_edits_whether_or_not_chunks_are_loaded() {
        let mut w = world();
        let stone = w.registry.state("stone").unwrap();
        // The real top of the column, read from generated chunks.
        let real_top_at = |w: &mut World, x: i32, z: i32| -> (i32, u16) {
            for y in (-128..400).rev() {
                let p = BlockPos::new(x, y, z);
                if !w.has_chunk(p.chunk()) {
                    load(w, p.chunk());
                }
                let s = w.get_block(p).unwrap();
                if !s.is_air() {
                    return (4 * y + s.shape().y_range_q().1, s.id());
                }
            }
            unreachable!()
        };
        // A column with no tree over it (column tops leave trees out).
        let (x, z) = (0..64)
            .map(|i| (7 + i % 8, 9 + i / 8))
            .find(|&(x, z)| {
                let t = w.column_top(x, z);
                (t.top_q, t.block) == real_top_at(&mut w, x, z)
            })
            .expect("open ground");
        let real_top = |w: &mut World| -> (i32, u16) {
            for y in (-128..400).rev() {
                let p = BlockPos::new(x, y, z);
                if !w.has_chunk(p.chunk()) {
                    load(w, p.chunk());
                }
                let s = w.get_block(p).unwrap();
                if !s.is_air() {
                    return (4 * y + s.shape().y_range_q().1, s.id());
                }
            }
            unreachable!()
        };
        let t = w.column_top(x, z);
        assert_eq!((t.top_q, t.block), real_top(&mut w), "untouched");
        // A pillar on top, then a pit dug into the ground beside it.
        let (top_q, _) = real_top(&mut w);
        let ground_y = (top_q - 1).div_euclid(4);
        for dy in 1..=6 {
            w.set_block(BlockPos::new(x, ground_y + dy, z), stone, true);
        }
        let t = w.column_top(x, z);
        assert_eq!((t.top_q, t.block), (4 * (ground_y + 7), stone.id()));
        assert_eq!((t.top_q, t.block), real_top(&mut w));
        for dy in 0..3 {
            w.set_block(BlockPos::new(x + 1, ground_y - dy, z), crate::block::AIR, true);
        }
        let pit = w.column_top(x + 1, z);
        // Unloaded now: the journal still knows.
        let positions = w.loaded_positions();
        for p in positions {
            w.unload(p);
        }
        assert_eq!(w.column_top(x + 1, z), pit);
        assert!(pit.top_q <= 4 * (ground_y - 2), "{pit:?} vs ground {ground_y}");
        let cols = w.journal().edited_columns();
        assert_eq!(cols.get(&(x, z)), Some(&(ground_y + 6)));
        assert!(cols.get(&(x + 1, z)).is_some_and(|&y| y <= ground_y), "(the neighbour's ground may be lower)");
        assert_eq!(cols.len(), 2);
        assert_eq!(w.edited_column_tops(), vec![w.column_top(x, z), pit]);
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
