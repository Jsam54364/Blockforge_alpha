//! Pooled vertex buffers: chunk and LOD meshes live in a few large GPU buffers ("pages")
//! instead of one buffer each. Creating and freeing a buffer per mesh is slow on some
//! drivers (OpenGL ES especially) and scatters memory; with pages a new mesh is one
//! `write_buffer` into space that is already there, and a mesh that goes away just returns
//! its range.
//!
//! Each page keeps its free ranges sorted by offset (first fit, merged with their neighbours
//! when freed). A mesh larger than a page gets a page of its own. An empty page is given back
//! to the driver once the other pages have a page's worth of room, so the pool shrinks after
//! the view does, without creating and dropping a page over and over at the boundary.
//!
//! Reusing a range is safe while an earlier frame may still read it: wgpu runs a
//! `write_buffer` after the work submitted before it.

use std::collections::BTreeMap;

/// Ranges start on whole quads (4 vertices of 8 bytes).
pub const ALIGN: u64 = 32;
/// Bytes per ordinary page: about 130k quads, a few dozen chunk meshes.
pub const PAGE_BYTES: u64 = 4 << 20;

/// Where a mesh lives: page, byte offset and length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub page: usize,
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug)]
struct Page {
    size: u64,
    /// offset -> length, sorted, never touching each other.
    free: BTreeMap<u64, u64>,
    used: u64,
}

impl Page {
    fn new(size: u64) -> Page {
        Page { size, free: BTreeMap::from([(0, size)]), used: 0 }
    }

    fn alloc(&mut self, size: u64) -> Option<u64> {
        let (&at, &len) = self.free.iter().find(|(_, &len)| len >= size)?;
        self.free.remove(&at);
        if len > size {
            self.free.insert(at + size, len - size);
        }
        self.used += size;
        Some(at)
    }

    fn free(&mut self, at: u64, size: u64) {
        let (mut start, mut len) = (at, size);
        // Merge with the free range just before and just after.
        if let Some((&p, &plen)) = self.free.range(..at).next_back() {
            debug_assert!(p + plen <= at, "double free");
            if p + plen == at {
                self.free.remove(&p);
                start = p;
                len += plen;
            }
        }
        if let Some(&nlen) = self.free.get(&(at + size)) {
            self.free.remove(&(at + size));
            len += nlen;
        }
        self.free.insert(start, len);
        self.used -= size;
    }
}

/// The bookkeeping half of the pool (no GPU), so it can be tested on its own.
#[derive(Debug, Default)]
pub struct Allocator {
    /// `None`: a page given back, its index free for the next one.
    pages: Vec<Option<Page>>,
}

/// What `Allocator::alloc` needs from the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alloc {
    /// The range is ready.
    Ready(Slot),
    /// Create a GPU buffer of `size` bytes for page `page`, then the range is ready.
    NewPage { slot: Slot, size: u64 },
}

impl Allocator {
    pub fn alloc(&mut self, bytes: u64) -> Alloc {
        let size = bytes.div_ceil(ALIGN).max(1) * ALIGN;
        for (i, p) in self.pages.iter_mut().enumerate() {
            if let Some(at) = p.as_mut().and_then(|p| p.alloc(size)) {
                return Alloc::Ready(Slot { page: i, offset: at, size });
            }
        }
        let page_size = size.max(PAGE_BYTES);
        let i = match self.pages.iter().position(|p| p.is_none()) {
            Some(i) => i,
            None => {
                self.pages.push(None);
                self.pages.len() - 1
            }
        };
        let mut p = Page::new(page_size);
        let at = p.alloc(size).expect("fits a new page");
        self.pages[i] = Some(p);
        Alloc::NewPage { slot: Slot { page: i, offset: at, size }, size: page_size }
    }

    /// Returns the range. Returns `Some(page)` when that page should now be given back.
    pub fn free(&mut self, s: Slot) -> Option<usize> {
        let p = self.pages[s.page].as_mut().expect("freed into a live page");
        p.free(s.offset, s.size);
        if p.used > 0 {
            return None;
        }
        let room_elsewhere: u64 = self
            .pages
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != s.page)
            .filter_map(|(_, p)| p.as_ref())
            .map(|p| p.size - p.used)
            .sum();
        let big = self.pages[s.page].as_ref().is_some_and(|p| p.size > PAGE_BYTES);
        if big || room_elsewhere >= PAGE_BYTES {
            self.pages[s.page] = None;
            return Some(s.page);
        }
        None
    }

    /// Live pages and bytes: (pages, reserved, used).
    pub fn usage(&self) -> (usize, u64, u64) {
        let live = self.pages.iter().flatten();
        let (mut n, mut size, mut used) = (0, 0, 0);
        for p in live {
            n += 1;
            size += p.size;
            used += p.used;
        }
        (n, size, used)
    }
}

/// The GPU half: one vertex buffer per live page.
pub struct VertexPool {
    alloc: Allocator,
    bufs: Vec<Option<wgpu::Buffer>>,
}

impl VertexPool {
    pub fn new() -> VertexPool {
        VertexPool { alloc: Allocator::default(), bufs: Vec::new() }
    }

    /// Copies `parts` back to back into a new range. `None` if they are empty.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, parts: &[&[u8]]) -> Option<Slot> {
        let bytes: u64 = parts.iter().map(|p| p.len() as u64).sum();
        if bytes == 0 {
            return None;
        }
        let slot = match self.alloc.alloc(bytes) {
            Alloc::Ready(s) => s,
            Alloc::NewPage { slot, size } => {
                let buf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("vertex pool page"),
                    size,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                if self.bufs.len() <= slot.page {
                    self.bufs.resize_with(slot.page + 1, || None);
                }
                self.bufs[slot.page] = Some(buf);
                slot
            }
        };
        let buf = self.bufs[slot.page].as_ref().expect("live page");
        let mut at = slot.offset;
        for p in parts {
            if !p.is_empty() {
                // (lengths are whole vertices of 8 bytes: a multiple of wgpu's 4)
                queue.write_buffer(buf, at, p);
                at += p.len() as u64;
            }
        }
        Some(slot)
    }

    pub fn free(&mut self, s: Slot) {
        if let Some(page) = self.alloc.free(s) {
            self.bufs[page] = None;
        }
    }

    /// The vertices of `s` (only the bytes that were written are drawn).
    pub fn slice(&self, s: &Slot) -> wgpu::BufferSlice<'_> {
        self.bufs[s.page].as_ref().expect("live page").slice(s.offset..s.offset + s.size)
    }

    pub fn usage(&self) -> (usize, u64, u64) {
        self.alloc.usage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(a: Alloc) -> Slot {
        match a {
            Alloc::Ready(s) | Alloc::NewPage { slot: s, .. } => s,
        }
    }

    #[test]
    fn ranges_never_overlap_and_freed_room_is_reused() {
        let mut a = Allocator::default();
        let mut live: Vec<Slot> = Vec::new();
        let mut seed = 12345u64;
        let mut rnd = |n: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % n
        };
        for step in 0..20000 {
            if live.len() > 300 || (rnd(3) == 0 && !live.is_empty()) {
                let i = rnd(live.len() as u64) as usize;
                a.free(live.swap_remove(i));
            } else {
                // Mostly chunk-sized meshes, now and then one bigger than a page.
                let bytes = if rnd(500) == 0 { PAGE_BYTES + rnd(PAGE_BYTES) } else { 8 + rnd(120_000) };
                let s = slot(a.alloc(bytes));
                assert_eq!(s.offset % ALIGN, 0);
                assert!(s.size >= bytes);
                live.push(s);
            }
            if step % 997 == 0 {
                let mut by_page: Vec<&Slot> = live.iter().collect();
                by_page.sort_by_key(|s| (s.page, s.offset));
                for w in by_page.windows(2) {
                    if w[0].page == w[1].page {
                        assert!(w[0].offset + w[0].size <= w[1].offset, "{:?} overlaps {:?}", w[0], w[1]);
                    }
                }
                let used: u64 = live.iter().map(|s| s.size).sum();
                assert_eq!(a.usage().2, used);
            }
        }
        // ~300 meshes of ~60 KB (18 MB) should not need more than a few pages of slack.
        let (pages, reserved, used) = a.usage();
        assert!(reserved < used + 6 * PAGE_BYTES, "{pages} pages, {reserved} reserved for {used}");
        for s in live.drain(..) {
            a.free(s);
        }
        let (pages, _, used) = a.usage();
        assert_eq!(used, 0);
        assert!(pages <= 1, "empty pages are given back, all but one: {pages}");
    }

    #[test]
    fn freed_neighbours_merge_into_one_range() {
        let mut a = Allocator::default();
        let s: Vec<Slot> = (0..3).map(|_| slot(a.alloc(1000))).collect();
        a.free(s[0]);
        a.free(s[2]);
        a.free(s[1]);
        // Everything is one range again: a page-sized mesh fits without a second page.
        assert!(matches!(a.alloc(PAGE_BYTES), Alloc::Ready(_)));
    }

    #[test]
    fn an_empty_page_is_kept_while_it_is_the_only_room() {
        let mut a = Allocator::default();
        let s = slot(a.alloc(1000));
        assert_eq!(a.free(s), None, "the last page stays");
        assert!(matches!(a.alloc(1000), Alloc::Ready(_)), "and is used again");
        let big = slot(a.alloc(PAGE_BYTES * 2));
        assert_eq!(a.free(big), Some(big.page), "an oversized page goes as soon as it is empty");
    }
}
