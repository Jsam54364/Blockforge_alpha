//! Coordinates, faces and the chunk-size constants.
//! `CHUNK_BITS` is the one knob to turn when benchmarking 16^3 vs 32^3 on a phone.

pub const CHUNK_BITS: i32 = 5;
pub const CHUNK_SIZE: i32 = 1 << CHUNK_BITS;
pub const CHUNK_MASK: i32 = CHUNK_SIZE - 1;
pub const CHUNK_VOLUME: usize = (CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize;

// The edit journal stores voxel indices as u16.
const _: () = assert!(CHUNK_VOLUME <= (1 << 16));

const BITS: usize = CHUNK_BITS as usize;

/// World-space block coordinate.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Chunk coordinate (cubic chunks: all three axes are equal citizens).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Position inside a chunk, each axis 0..CHUNK_SIZE.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LocalPos {
    pub x: u8,
    pub y: u8,
    pub z: u8,
}

impl LocalPos {
    /// y-major layout: whole horizontal layers are contiguous.
    #[inline]
    pub fn index(self) -> usize {
        ((self.y as usize) << (2 * BITS)) | ((self.z as usize) << BITS) | (self.x as usize)
    }

    #[inline]
    pub fn from_index(i: usize) -> LocalPos {
        let m = CHUNK_MASK as usize;
        LocalPos {
            x: (i & m) as u8,
            z: ((i >> BITS) & m) as u8,
            y: ((i >> (2 * BITS)) & m) as u8,
        }
    }
}

impl BlockPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        BlockPos { x, y, z }
    }

    /// Arithmetic shift == floor division, so negatives work.
    #[inline]
    pub fn chunk(self) -> ChunkPos {
        ChunkPos { x: self.x >> CHUNK_BITS, y: self.y >> CHUNK_BITS, z: self.z >> CHUNK_BITS }
    }

    #[inline]
    pub fn local(self) -> LocalPos {
        LocalPos {
            x: (self.x & CHUNK_MASK) as u8,
            y: (self.y & CHUNK_MASK) as u8,
            z: (self.z & CHUNK_MASK) as u8,
        }
    }

    pub fn offset(self, dx: i32, dy: i32, dz: i32) -> BlockPos {
        BlockPos { x: self.x + dx, y: self.y + dy, z: self.z + dz }
    }

    pub fn neighbor(self, face: Face) -> BlockPos {
        let (dx, dy, dz) = face.normal();
        self.offset(dx, dy, dz)
    }

    pub fn from_f32(p: [f32; 3]) -> BlockPos {
        BlockPos { x: p[0].floor() as i32, y: p[1].floor() as i32, z: p[2].floor() as i32 }
    }
}

impl ChunkPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        ChunkPos { x, y, z }
    }

    pub fn origin(self) -> BlockPos {
        BlockPos { x: self.x << CHUNK_BITS, y: self.y << CHUNK_BITS, z: self.z << CHUNK_BITS }
    }

    pub fn offset(self, dx: i32, dy: i32, dz: i32) -> ChunkPos {
        ChunkPos { x: self.x + dx, y: self.y + dy, z: self.z + dz }
    }

    pub fn block_at(self, l: LocalPos) -> BlockPos {
        self.origin().offset(l.x as i32, l.y as i32, l.z as i32)
    }

    pub fn dist_sq(self, o: ChunkPos) -> i64 {
        let dx = (self.x - o.x) as i64;
        let dy = (self.y - o.y) as i64;
        let dz = (self.z - o.z) as i64;
        dx * dx + dy * dy + dz * dz
    }
}

/// Cube faces. The discriminant order is relied on by mesh tables and texture slots.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Face {
    PosX = 0,
    NegX = 1,
    PosY = 2,
    NegY = 3,
    PosZ = 4,
    NegZ = 5,
}

impl Face {
    pub const ALL: [Face; 6] =
        [Face::PosX, Face::NegX, Face::PosY, Face::NegY, Face::PosZ, Face::NegZ];

    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn normal(self) -> (i32, i32, i32) {
        match self {
            Face::PosX => (1, 0, 0),
            Face::NegX => (-1, 0, 0),
            Face::PosY => (0, 1, 0),
            Face::NegY => (0, -1, 0),
            Face::PosZ => (0, 0, 1),
            Face::NegZ => (0, 0, -1),
        }
    }

    pub const fn opposite(self) -> Face {
        match self {
            Face::PosX => Face::NegX,
            Face::NegX => Face::PosX,
            Face::PosY => Face::NegY,
            Face::NegY => Face::PosY,
            Face::PosZ => Face::NegZ,
            Face::NegZ => Face::PosZ,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_coordinates_floor_correctly() {
        let p = BlockPos::new(-1, -32, 31);
        assert_eq!(p.chunk(), ChunkPos::new(-1, -1, 0));
        let l = p.local();
        assert_eq!((l.x, l.y, l.z), (31, 0, 31));
        assert_eq!(p.chunk().block_at(l), p);
    }

    #[test]
    fn local_index_roundtrip() {
        for i in [0usize, 1, 31, 32, 1023, 1024, CHUNK_VOLUME - 1] {
            assert_eq!(LocalPos::from_index(i).index(), i);
        }
    }

    #[test]
    fn opposite_faces_cancel() {
        for f in Face::ALL {
            let (a, b, c) = f.normal();
            let (d, e, g) = f.opposite().normal();
            assert_eq!((a + d, b + e, c + g), (0, 0, 0));
        }
    }
}
