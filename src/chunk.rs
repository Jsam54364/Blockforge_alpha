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

#[inline]
fn get_idx(data: &[u64], bits: u8, i: usize) -> usize {
    let per = 64 / bits as usize;
    let word = data[i / per];
    let shift = (i % per) * bits as usize;
    ((word >> shift) & ((1u64 << bits) - 1)) as usize
}

#[inline]
fn set_idx(data: &mut [u64], bits: u8, i: usize, v: usize) {
    let per = 64 / bits as usize;
    let w = i / per;
    let shift = (i % per) * bits as usize;
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
