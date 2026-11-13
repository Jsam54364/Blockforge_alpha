//! Palette-compressed chunk storage.
//!
//! Uniform chunks (all air, all stone) cost a single entry. Mixed chunks use a palette
//! plus packed indices with a power-of-two bit width (1/2/4/8/16) so entries never
//! straddle a u64 word.

use crate::block::{BlockState, AIR};
use crate::coords::{LocalPos, CHUNK_VOLUME};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paletted {
    pub palette: Vec<BlockState>,
    pub bits: u8,
    pub data: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Storage {
    Uniform(BlockState),
    Paletted(Paletted),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    storage: Storage,
}

fn words_for(bits: u8) -> usize {
    let per = 64 / bits as usize;
    (CHUNK_VOLUME + per - 1) / per
}

fn bits_for(palette_len: usize) -> u8 {
    match palette_len {
        0..=2 => 1,
        3..=4 => 2,
        5..=16 => 4,
        17..=256 => 8,
        _ => 16,
    }
}

/// `bits` is always a power of two (1/2/4/8/16), so entries-per-word is too and the
/// divide/modulo become shifts. This is the hottest function in the engine.
#[inline]
fn get_idx(data: &[u64], bits: u8, i: usize) -> usize {
    let lb = bits.trailing_zeros() as usize; // log2(bits)
    let per_shift = 6 - lb; // log2(64 / bits)
    let word = data[i >> per_shift];
    let shift = (i & ((1usize << per_shift) - 1)) << lb;
    ((word >> shift) & ((1u64 << bits) - 1)) as usize
}

#[inline]
fn set_idx(data: &mut [u64], bits: u8, i: usize, v: usize) {
    let lb = bits.trailing_zeros() as usize;
    let per_shift = 6 - lb;
    let w = i >> per_shift;
    let shift = (i & ((1usize << per_shift) - 1)) << lb;
    let mask = ((1u64 << bits) - 1) << shift;
    data[w] = (data[w] & !mask) | ((v as u64) << shift);
}

impl Paletted {
    fn new(first: BlockState) -> Self {
        Paletted { palette: vec![first], bits: 1, data: vec![0; words_for(1)] }
    }

    fn set(&mut self, i: usize, state: BlockState) {
        let pi = match self.palette.iter().position(|&s| s == state) {
            Some(p) => p,
            None => {
                self.palette.push(state);
                let need = bits_for(self.palette.len());
                if need > self.bits {
                    self.repack(need);
                }
                self.palette.len() - 1
            }
        };
        set_idx(&mut self.data, self.bits, i, pi);
    }

    fn repack(&mut self, new_bits: u8) {
        let mut new_data = vec![0u64; words_for(new_bits)];
        for i in 0..CHUNK_VOLUME {
            let v = get_idx(&self.data, self.bits, i);
            set_idx(&mut new_data, new_bits, i, v);
        }
        self.data = new_data;
        self.bits = new_bits;
    }
}

impl Chunk {
    pub fn uniform(state: BlockState) -> Chunk {
        Chunk { storage: Storage::Uniform(state) }
    }

    pub fn air() -> Chunk {
        Chunk::uniform(AIR)
    }

    /// Validating constructor for untrusted data (disk, network).
    pub fn from_paletted(palette: Vec<BlockState>, bits: u8, data: Vec<u64>) -> Option<Chunk> {
        if !matches!(bits, 1 | 2 | 4 | 8 | 16) {
            return None;
        }
        if palette.is_empty() || palette.len() > (1usize << bits) {
            return None;
        }
        if data.len() != words_for(bits) {
            return None;
        }
        for i in 0..CHUNK_VOLUME {
            if get_idx(&data, bits, i) >= palette.len() {
                return None;
            }
        }
        Some(Chunk { storage: Storage::Paletted(Paletted { palette, bits, data }) })
    }

    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    #[inline]
    pub fn get(&self, idx: usize) -> BlockState {
        debug_assert!(idx < CHUNK_VOLUME);
        match &self.storage {
            Storage::Uniform(s) => *s,
            Storage::Paletted(p) => p.palette[get_idx(&p.data, p.bits, idx)],
        }
    }

    /// Writes every block into `out` (length `CHUNK_VOLUME`, same order as `get`).
    /// Much faster than `CHUNK_VOLUME` calls to `get`: words are unpacked sequentially.
    pub fn decode_into(&self, out: &mut [BlockState]) {
        assert_eq!(out.len(), CHUNK_VOLUME);
        match &self.storage {
            Storage::Uniform(s) => out.fill(*s),
            Storage::Paletted(p) => {
                let bits = p.bits as usize;
                let per = 64 / bits;
                let mask = (1u64 << bits) - 1;
                for (cells, &word) in out.chunks_mut(per).zip(p.data.iter()) {
                    let mut w = word;
                    for c in cells {
                        *c = p.palette[(w & mask) as usize];
                        w >>= bits;
                    }
                }
            }
        }
    }

    pub fn get_local(&self, l: LocalPos) -> BlockState {
        self.get(l.index())
    }

    pub fn set(&mut self, idx: usize, state: BlockState) {
        debug_assert!(idx < CHUNK_VOLUME);
        if let Storage::Uniform(cur) = &self.storage {
            let cur = *cur;
            if cur == state {
                return;
            }
            let mut p = Paletted::new(cur);
            p.set(idx, state);
            self.storage = Storage::Paletted(p);
            return;
        }
        if let Storage::Paletted(p) = &mut self.storage {
            p.set(idx, state);
        }
    }

    pub fn set_local(&mut self, l: LocalPos, state: BlockState) {
        self.set(l.index(), state);
    }

    pub fn is_uniform(&self) -> bool {
        matches!(self.storage, Storage::Uniform(_))
    }

    pub fn uniform_state(&self) -> Option<BlockState> {
        match &self.storage {
            Storage::Uniform(s) => Some(*s),
            _ => None,
        }
    }

    /// Drops unused palette entries and collapses to `Uniform` when possible.
    /// Call after bulk edits (explosions) or before saving.
    pub fn compact(&mut self) {
        if self.is_uniform() {
            return;
        }
        let mut rebuilt = Chunk::uniform(self.get(0));
        for i in 1..CHUNK_VOLUME {
            rebuilt.set(i, self.get(i));
        }
        *self = rebuilt;
    }

    pub fn memory_bytes(&self) -> usize {
        let base = std::mem::size_of::<Chunk>();
        match &self.storage {
            Storage::Uniform(_) => base,
            Storage::Paletted(p) => base + p.palette.len() * 2 + p.data.len() * 8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_into_matches_get_for_every_bit_width() {
        // 1, 3, 9, 40 and 300 distinct states exercise 1/2/4/8/16-bit palettes.
        for kinds in [1usize, 2, 3, 9, 40, 300] {
            let mut c = Chunk::air();
            for i in 0..CHUNK_VOLUME {
                let id = ((i * 7 + i / 13) % kinds) as u16;
                c.set(i, BlockState::full(id));
            }
            let mut out = vec![AIR; CHUNK_VOLUME];
            c.decode_into(&mut out);
            for i in 0..CHUNK_VOLUME {
                assert_eq!(out[i], c.get(i), "kinds {kinds}, index {i}");
            }
        }
        let mut out = vec![AIR; CHUNK_VOLUME];
        Chunk::uniform(BlockState::full(5)).decode_into(&mut out);
        assert!(out.iter().all(|&s| s == BlockState::full(5)));
    }

    #[test]
    fn uniform_costs_almost_nothing() {
        let c = Chunk::air();
        assert!(c.is_uniform());
        assert!(c.memory_bytes() < 64);
        assert_eq!(c.get(12345), AIR);
    }

    #[test]
    fn set_get_and_palette_growth() {
        let mut c = Chunk::air();
        // 40 distinct states forces 1 -> 2 -> 4 -> 8 bit repacking.
        for i in 0..40usize {
            c.set(i * 7, BlockState::full(i as u16 + 1));
        }
        for i in 0..40usize {
            assert_eq!(c.get(i * 7), BlockState::full(i as u16 + 1));
        }
        assert_eq!(c.get(1), AIR);
        match c.storage() {
            Storage::Paletted(p) => assert_eq!(p.bits, 8),
            _ => panic!("expected paletted"),
        }
    }

    #[test]
    fn compact_collapses_back_to_uniform() {
        let mut c = Chunk::air();
        c.set(5, BlockState::full(3));
        assert!(!c.is_uniform());
        c.set(5, AIR);
        assert!(!c.is_uniform()); // palette still has the stale entry
        c.compact();
        assert!(c.is_uniform());
    }

    #[test]
    fn from_paletted_rejects_garbage() {
        assert!(Chunk::from_paletted(vec![], 1, vec![0; words_for(1)]).is_none());
        assert!(Chunk::from_paletted(vec![AIR], 3, vec![0; 10]).is_none());
        assert!(Chunk::from_paletted(vec![AIR], 1, vec![0; 3]).is_none());
        // index 1 with a one-entry palette
        let mut data = vec![0u64; words_for(1)];
        data[0] = 0b10;
        assert!(Chunk::from_paletted(vec![AIR], 1, data).is_none());
    }
}
