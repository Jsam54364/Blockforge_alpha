//! Versioned, checksummed binary formats for chunks and the edit journal.
//!
//! Every blob is `[magic][version u16][payload][crc32 of everything before]`.
//! * A flipped bit anywhere is caught by the CRC, so corruption is detected instead
//!   of silently loading garbage.
//! * The version lets the format change later: add a `decode_*_vN` and a match arm
//!   (migrating old data forward) instead of breaking existing worlds.
//! * Block ids are stored numerically. They are append-only in `blocks.toml`; the next
//!   step for robustness is a name->id table in the world header, remapped on load.
//!
//! The chunk format doubles as the network wire format.

use std::fmt;

use crate::block::BlockState;
use crate::chunk::{Chunk, Storage};
use crate::coords::{ChunkPos, CHUNK_VOLUME};
use crate::world::EditJournal;

const CHUNK_MAGIC: [u8; 4] = *b"BFCH";
const JOURNAL_MAGIC: [u8; 4] = *b"BFWJ";
pub const CHUNK_FORMAT_VERSION: u16 = 1;
pub const JOURNAL_FORMAT_VERSION: u16 = 1;

#[derive(Debug, PartialEq, Eq)]
pub enum SaveError {
    BadMagic,
    UnsupportedVersion(u16),
    Truncated,
    BadChecksum,
    Corrupt(&'static str),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::BadMagic => write!(f, "not a blockforge file"),
            SaveError::UnsupportedVersion(v) => write!(f, "unsupported format version {v}"),
            SaveError::Truncated => write!(f, "data is truncated"),
            SaveError::BadChecksum => write!(f, "checksum mismatch (corrupt data)"),
            SaveError::Corrupt(why) => write!(f, "corrupt data: {why}"),
        }
    }
}

impl std::error::Error for SaveError {}

/// Standard CRC-32 (IEEE), bitwise. Small and fast enough for chunk-sized blobs.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (!(crc & 1)).wrapping_add(1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], SaveError> {
        let end = self.pos.checked_add(n).ok_or(SaveError::Truncated)?;
        if end > self.buf.len() {
            return Err(SaveError::Truncated);
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, SaveError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, SaveError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, SaveError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i32(&mut self) -> Result<i32, SaveError> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, SaveError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn is_empty(&self) -> bool {
        self.pos == self.buf.len()
    }
}

fn seal(mut body: Vec<u8>) -> Vec<u8> {
    let c = crc32(&body);
    body.extend_from_slice(&c.to_le_bytes());
    body
}

fn unseal(bytes: &[u8]) -> Result<&[u8], SaveError> {
    if bytes.len() < 4 {
        return Err(SaveError::Truncated);
    }
    let (body, tail) = bytes.split_at(bytes.len() - 4);
    let stored = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
    if crc32(body) != stored {
        return Err(SaveError::BadChecksum);
    }
    Ok(body)
}

pub fn encode_chunk(chunk: &Chunk) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&CHUNK_MAGIC);
    b.extend_from_slice(&CHUNK_FORMAT_VERSION.to_le_bytes());
    match chunk.storage() {
        Storage::Uniform(s) => {
            b.push(0);
            b.extend_from_slice(&s.0.to_le_bytes());
        }
        Storage::Paletted(p) => {
            b.push(1);
            b.push(p.bits);
            b.extend_from_slice(&(p.palette.len() as u16).to_le_bytes());
            for s in &p.palette {
                b.extend_from_slice(&s.0.to_le_bytes());
            }
            b.extend_from_slice(&(p.data.len() as u32).to_le_bytes());
            for w in &p.data {
                b.extend_from_slice(&w.to_le_bytes());
            }
        }
    }
    seal(b)
}

pub fn decode_chunk(bytes: &[u8]) -> Result<Chunk, SaveError> {
    let body = unseal(bytes)?;
    let mut r = Reader::new(body);
    if r.take(4)? != &CHUNK_MAGIC[..] {
        return Err(SaveError::BadMagic);
    }
    match r.u16()? {
        1 => decode_chunk_v1(&mut r),
        // 2 => migrate from v2 layout ...
        v => Err(SaveError::UnsupportedVersion(v)),
    }
}

fn decode_chunk_v1(r: &mut Reader<'_>) -> Result<Chunk, SaveError> {
    let chunk = match r.u8()? {
        0 => Chunk::uniform(BlockState(r.u16()?)),
        1 => {
            let bits = r.u8()?;
            let n = r.u16()? as usize;
            let mut palette = Vec::with_capacity(n);
            for _ in 0..n {
                palette.push(BlockState(r.u16()?));
            }
            let words = r.u32()? as usize;
            if words > (1 << 16) {
                return Err(SaveError::Corrupt("implausible data length"));
            }
            let mut data = Vec::with_capacity(words);
            for _ in 0..words {
                data.push(r.u64()?);
            }
            Chunk::from_paletted(palette, bits, data).ok_or(SaveError::Corrupt("invalid palette data"))?
        }
        _ => return Err(SaveError::Corrupt("unknown storage kind")),
    };
    if !r.is_empty() {
        return Err(SaveError::Corrupt("trailing bytes"));
    }
    Ok(chunk)
}

pub fn encode_journal(seed: u64, journal: &EditJournal) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&JOURNAL_MAGIC);
    b.extend_from_slice(&JOURNAL_FORMAT_VERSION.to_le_bytes());
    b.extend_from_slice(&seed.to_le_bytes());
    let chunks = journal.to_sorted();
    b.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
    for (cp, edits) in &chunks {
        b.extend_from_slice(&cp.x.to_le_bytes());
        b.extend_from_slice(&cp.y.to_le_bytes());
        b.extend_from_slice(&cp.z.to_le_bytes());
        b.extend_from_slice(&(edits.len() as u32).to_le_bytes());
        for (idx, state) in edits {
            b.extend_from_slice(&idx.to_le_bytes());
            b.extend_from_slice(&state.0.to_le_bytes());
        }
    }
    seal(b)
}

pub fn decode_journal(bytes: &[u8]) -> Result<(u64, EditJournal), SaveError> {
    let body = unseal(bytes)?;
    let mut r = Reader::new(body);
    if r.take(4)? != &JOURNAL_MAGIC[..] {
        return Err(SaveError::BadMagic);
    }
    match r.u16()? {
        1 => decode_journal_v1(&mut r),
        v => Err(SaveError::UnsupportedVersion(v)),
    }
}

fn decode_journal_v1(r: &mut Reader<'_>) -> Result<(u64, EditJournal), SaveError> {
    let seed = r.u64()?;
    let chunk_count = r.u32()? as usize;
    if chunk_count > r.remaining() / 16 {
        return Err(SaveError::Corrupt("implausible chunk count"));
    }
    let mut journal = EditJournal::default();
    for _ in 0..chunk_count {
        let cp = ChunkPos::new(r.i32()?, r.i32()?, r.i32()?);
        let n = r.u32()? as usize;
        if n > r.remaining() / 4 {
            return Err(SaveError::Corrupt("implausible edit count"));
        }
        for _ in 0..n {
            let idx = r.u16()?;
            let state = BlockState(r.u16()?);
            if idx as usize >= CHUNK_VOLUME {
                return Err(SaveError::Corrupt("edit index out of range"));
            }
            journal.record_local(cp, idx, state);
        }
    }
    if !r.is_empty() {
        return Err(SaveError::Corrupt("trailing bytes"));
    }
    Ok((seed, journal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::AIR;
    use crate::coords::BlockPos;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn uniform_chunk_roundtrip_is_tiny() {
        let c = Chunk::uniform(BlockState::full(5));
        let bytes = encode_chunk(&c);
        assert!(bytes.len() < 24);
        assert_eq!(decode_chunk(&bytes).unwrap(), c);
    }

    #[test]
    fn paletted_chunk_roundtrip() {
        let mut c = Chunk::air();
        for i in 0..300usize {
            c.set(i * 3, BlockState::full((i % 20) as u16 + 1));
        }
        let bytes = encode_chunk(&c);
        assert_eq!(decode_chunk(&bytes).unwrap(), c);
    }

    #[test]
    fn any_single_flipped_byte_is_detected() {
        let mut c = Chunk::air();
        c.set(10, BlockState::full(3));
        let bytes = encode_chunk(&c);
        for i in 0..bytes.len() {
            let mut bad = bytes.clone();
            bad[i] ^= 0x40;
            assert!(decode_chunk(&bad).is_err(), "byte {i} flip not detected");
        }
    }

    #[test]
    fn truncation_and_wrong_magic_are_errors() {
        let bytes = encode_chunk(&Chunk::air());
        assert!(decode_chunk(&bytes[..bytes.len() - 1]).is_err());
        assert_eq!(decode_chunk(&[1, 2]), Err(SaveError::Truncated));
        let journal_bytes = encode_journal(1, &EditJournal::default());
        assert_eq!(decode_chunk(&journal_bytes), Err(SaveError::BadMagic));
    }

    #[test]
    fn future_versions_are_rejected_cleanly() {
        let mut b = Vec::new();
        b.extend_from_slice(&CHUNK_MAGIC);
        b.extend_from_slice(&99u16.to_le_bytes());
        assert_eq!(decode_chunk(&seal(b)), Err(SaveError::UnsupportedVersion(99)));
    }

    #[test]
    fn journal_roundtrip_is_deterministic() {
        let mut j = EditJournal::default();
        j.record(BlockPos::new(1, 2, 3), BlockState::full(4));
        j.record(BlockPos::new(-40, 70, 9), AIR);
        j.record(BlockPos::new(1, 2, 4), BlockState::full(6));
        let a = encode_journal(777, &j);
        let (seed, back) = decode_journal(&a).unwrap();
        assert_eq!(seed, 777);
        assert_eq!(back.len(), 3);
        assert_eq!(encode_journal(777, &back), a); // stable bytes
    }
}
