//! Deterministic randomness. No `rand` crate: results must be identical on every
//! device and every run so replays, desync checks and world regeneration work.

/// SplitMix64: tiny, fast, good enough for gameplay RNG.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / 16_777_216.0)
    }

    /// Uniform in [0, n).
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        (((self.next_u64() >> 32) * (n as u64)) >> 32) as u32
    }
}

/// Stateless coordinate hash, used for noise lattices.
pub fn hash3(seed: u64, x: i32, y: i32, z: i32) -> u64 {
    let mut h = seed ^ 0x9E37_79B9_7F4A_7C15;
    h = (h ^ (x as u32 as u64)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 29) ^ (y as u32 as u64)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h = (h ^ (h >> 32) ^ (z as u32 as u64)).wrapping_mul(0xD6E8_FEB8_6659_FD93);
    h ^ (h >> 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let mut a = Rng::new(99);
        let mut b = Rng::new(99);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn floats_in_range() {
        let mut r = Rng::new(1);
        for _ in 0..10_000 {
            let f = r.next_f32();
            assert!((0.0..1.0).contains(&f));
            assert!(r.below(7) < 7);
        }
    }

    #[test]
    fn hash_is_deterministic_and_spread() {
        assert_eq!(hash3(1, 2, 3, 4), hash3(1, 2, 3, 4));
        assert_ne!(hash3(1, 2, 3, 4), hash3(1, 3, 2, 4));
        let mean: f32 = (0..2000)
            .map(|i| (hash3(5, i, 0, 0) >> 40) as f32 / 16_777_216.0)
            .sum::<f32>()
            / 2000.0;
        assert!((mean - 0.5).abs() < 0.05, "mean {mean}");
    }
}
