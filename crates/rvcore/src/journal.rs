//! Copy-on-write journal of RAM and disk pages, for rewinding.
//!
//! While it is on, the first write to each 4 KiB page of RAM or of the disk
//! after a checkpoint saves the page's old contents. Writing those back
//! returns memory to how it was at the checkpoint. A page written many times
//! is saved once, so the cost is one bit test per store.
//!
//! Every write to RAM passes through here, so the same hook also watches
//! the pages that hold compiled code (see `jit`): the first write to a
//! watched page records it in `code_hits` and stops watching it.

pub const PAGE: usize = 4096;

/// Old contents of one page: (page number, bytes). The last page of a disk
/// whose size is not a multiple of the page size is saved short.
pub type SavedPage = (u32, Box<[u8]>);

#[derive(Default)]
pub struct Journal {
    pub on: bool,
    ram_bits: Vec<u64>,
    disk_bits: Vec<u64>,
    pub ram: Vec<SavedPage>,
    pub disk: Vec<SavedPage>,
    /// True while any RAM page is watched for code changes.
    watch: bool,
    code_bits: Vec<u64>,
    /// Watched pages written since the JIT last looked.
    pub code_hits: Vec<u32>,
}

#[inline(always)]
fn test_and_set(bits: &mut Vec<u64>, page: usize) -> bool {
    let w = page >> 6;
    if w >= bits.len() {
        bits.resize(w + 1, 0);
    }
    let m = 1u64 << (page & 63);
    let was = bits[w] & m != 0;
    bits[w] |= m;
    was
}

fn save(list: &mut Vec<SavedPage>, mem: &[u8], page: usize) {
    let start = page * PAGE;
    let end = (start + PAGE).min(mem.len());
    if start < end {
        list.push((page as u32, mem[start..end].into()));
    }
}

impl Journal {
    /// About to write RAM at byte offset `off` (the write stays in one page).
    #[inline(always)]
    pub fn ram_write(&mut self, ram: &[u8], off: usize) {
        if self.on | self.watch {
            self.ram_write_slow(ram, off / PAGE);
        }
    }

    #[cold]
    #[inline(never)]
    fn ram_write_slow(&mut self, ram: &[u8], page: usize) {
        if self.on && !test_and_set(&mut self.ram_bits, page) {
            save(&mut self.ram, ram, page);
        }
        if self.watch && self.watched(page) {
            self.code_bits[page >> 6] &= !(1 << (page & 63));
            self.code_hits.push(page as u32);
        }
    }

    /// About to write `len` bytes of RAM from byte offset `off`.
    pub fn ram_range(&mut self, ram: &[u8], off: usize, len: usize) {
        if (self.on | self.watch) && len > 0 {
            for page in off / PAGE..=(off + len - 1) / PAGE {
                self.ram_write_slow(ram, page);
            }
        }
    }

    /// Start watching RAM page `page` for writes.
    pub fn watch_page(&mut self, page: usize) {
        let w = page >> 6;
        if w >= self.code_bits.len() {
            self.code_bits.resize(w + 1, 0);
        }
        self.code_bits[w] |= 1 << (page & 63);
        self.watch = true;
    }

    /// True if RAM page `page` is watched.
    #[inline]
    pub fn watched(&self, page: usize) -> bool {
        self.code_bits.get(page >> 6).is_some_and(|w| w & (1 << (page & 63)) != 0)
    }

    /// Stop watching every page.
    pub fn unwatch_all(&mut self) {
        self.code_bits.iter_mut().for_each(|w| *w = 0);
        self.code_hits.clear();
        self.watch = false;
    }

    /// Size the page bitmaps for `ram_len` bytes of RAM, once, so that
    /// compiled code can test them in place: a store may skip the journal
    /// when its page's saved bit is set (always, while the journal is off)
    /// and its code bit is clear. Returns the addresses of the two bitmaps.
    pub fn prepare(&mut self, ram_len: usize) -> (usize, usize) {
        let words = ram_len.div_ceil(PAGE * 64);
        if self.ram_bits.len() < words {
            let fill = if self.on { 0 } else { !0 };
            self.ram_bits.resize(words, fill);
        }
        if self.code_bits.len() < words {
            self.code_bits.resize(words, 0);
        }
        (self.ram_bits.as_ptr() as usize, self.code_bits.as_ptr() as usize)
    }

    /// About to write `len` bytes of the disk from byte offset `off`.
    pub fn disk_range(&mut self, disk: &[u8], off: usize, len: usize) {
        if self.on && len > 0 {
            for page in off / PAGE..=(off + len - 1) / PAGE {
                if !test_and_set(&mut self.disk_bits, page) {
                    save(&mut self.disk, disk, page);
                }
            }
        }
    }

    /// Start a new interval: forget which pages were saved and hand back
    /// the saved pages of the interval that just ended.
    pub fn take(&mut self) -> (Vec<SavedPage>, Vec<SavedPage>) {
        // While off, every page counts as saved (see `prepare`).
        let fill = if self.on { 0 } else { !0 };
        self.ram_bits.iter_mut().for_each(|w| *w = fill);
        self.disk_bits.iter_mut().for_each(|w| *w = 0);
        (std::mem::take(&mut self.ram), std::mem::take(&mut self.disk))
    }

    /// Bytes held in saved pages.
    pub fn bytes(&self) -> usize {
        self.ram.iter().chain(&self.disk).map(|(_, b)| b.len()).sum()
    }
}

/// Write saved pages back into memory.
pub fn restore(mem: &mut [u8], pages: &[SavedPage]) {
    for (page, bytes) in pages {
        let start = *page as usize * PAGE;
        mem[start..start + bytes.len()].copy_from_slice(bytes);
    }
}
