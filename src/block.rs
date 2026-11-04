//! Block state = 12-bit block id + 4-bit shape, packed in a u16.
//! Shape lives in the state from day one so slabs/layers never need a chunk-format change.

pub const ID_BITS: u16 = 12;
pub const ID_MASK: u16 = (1 << ID_BITS) - 1;
pub const MAX_BLOCK_IDS: usize = 1 << ID_BITS;

/// Vertical occupancy of a block, in quarter-block units (a block is 4 tall).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Shape {
    Full = 0,
    SlabBottom = 1,
    SlabTop = 2,
    Quarter = 3,
    ThreeQuarter = 4,
}

impl Shape {
    pub fn from_bits(b: u8) -> Shape {
        match b {
            1 => Shape::SlabBottom,
            2 => Shape::SlabTop,
            3 => Shape::Quarter,
            4 => Shape::ThreeQuarter,
            _ => Shape::Full,
        }
    }

    /// (min, max) occupied height in quarter units, 0..=4.
    pub const fn y_range_q(self) -> (i32, i32) {
        match self {
            Shape::Full => (0, 4),
            Shape::SlabBottom => (0, 2),
            Shape::SlabTop => (2, 4),
            Shape::Quarter => (0, 1),
            Shape::ThreeQuarter => (0, 3),
        }
    }

    pub const fn is_full(self) -> bool {
        matches!(self, Shape::Full)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct BlockState(pub u16);

pub const AIR: BlockState = BlockState(0);
/// Sentinel for "chunk not loaded" in padded mesh snapshots. Never stored in a chunk.
pub const UNKNOWN: BlockState = BlockState(0xFFFF);

impl BlockState {
    pub const fn new(id: u16, shape: Shape) -> Self {
        BlockState((id & ID_MASK) | ((shape as u16) << ID_BITS))
    }

    pub const fn full(id: u16) -> Self {
        BlockState::new(id, Shape::Full)
    }

    #[inline]
    pub const fn id(self) -> u16 {
        self.0 & ID_MASK
    }

    #[inline]
    pub fn shape(self) -> Shape {
        Shape::from_bits((self.0 >> ID_BITS) as u8)
    }

    #[inline]
    pub const fn is_air(self) -> bool {
        self.id() == 0
    }

    pub fn with_shape(self, shape: Shape) -> BlockState {
        BlockState::new(self.id(), shape)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack() {
        let s = BlockState::new(1234, Shape::SlabTop);
        assert_eq!(s.id(), 1234);
        assert_eq!(s.shape(), Shape::SlabTop);
        assert!(!s.is_air());
        assert!(AIR.is_air());
        assert!(!UNKNOWN.is_air());
    }

    #[test]
    fn full_is_plain_id() {
        assert_eq!(BlockState::full(7).0, 7);
    }
}
