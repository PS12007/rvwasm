//! virtio-mmio (version 2, "modern") block device with one split virtqueue.
//!
//! Requests are completed synchronously when the driver writes QueueNotify;
//! the used-ring interrupt is raised immediately.

use crate::bus::RAM_BASE;

pub const BASE: u64 = 0x1000_1000;
pub const SIZE: u64 = 0x1000;
pub const IRQ: u32 = 1;

const MAGIC: u32 = 0x7472_6976; // "virt"
const VENDOR: u32 = 0x554d_4551; // "QEMU"
const QUEUE_MAX: u32 = 1024;
const SECTOR: u64 = 512;

const VIRTIO_F_VERSION_1: u64 = 1 << 32;
const VIRTIO_BLK_F_FLUSH: u64 = 1 << 9;

const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

const BLK_T_IN: u32 = 0;
const BLK_T_OUT: u32 = 1;
const BLK_T_FLUSH: u32 = 4;
const BLK_T_GET_ID: u32 = 8;

#[derive(Clone, Default)]
pub struct VirtioBlk {
    pub disk: Vec<u8>,
    device_features_sel: u32,
    driver_features: u64,
    driver_features_sel: u32,
    queue_sel: u32,
    queue_num: u32,
    queue_ready: bool,
    desc: u64,
    avail: u64,
    used: u64,
    last_avail: u16,
    interrupt_status: u32,
    status: u32,
    /// Number of requests serviced, exposed for stats.
    pub requests: u64,
    /// Number of write requests serviced (lets the host tell when to save the disk).
    pub writes: u64,
}

/// Bounds-checked guest physical memory accessors over the RAM slice.
struct Mem<'a>(&'a mut [u8]);

impl Mem<'_> {
    fn range(&self, pa: u64, len: usize) -> Option<std::ops::Range<usize>> {
        let off = pa.checked_sub(RAM_BASE)? as usize;
        let end = off.checked_add(len)?;
        (end <= self.0.len()).then_some(off..end)
    }
    fn rd<const N: usize>(&self, pa: u64) -> [u8; N] {
        self.range(pa, N).map(|r| self.0[r].try_into().unwrap()).unwrap_or([0; N])
    }
    fn u16(&self, pa: u64) -> u16 {
        u16::from_le_bytes(self.rd(pa))
    }
    fn u32(&self, pa: u64) -> u32 {
        u32::from_le_bytes(self.rd(pa))
    }
    fn u64(&self, pa: u64) -> u64 {
        u64::from_le_bytes(self.rd(pa))
    }
    fn write(&mut self, pa: u64, data: &[u8]) {
        if let Some(r) = self.range(pa, data.len()) {
            self.0[r].copy_from_slice(data);
        }
    }
}

impl VirtioBlk {
    pub fn new(disk: Vec<u8>) -> Self {
        VirtioBlk { disk, ..Default::default() }
    }

    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_BLK_F_FLUSH
    }

    pub fn irq(&self) -> bool {
        self.interrupt_status != 0
    }

    fn reset(&mut self) {
        let disk = std::mem::take(&mut self.disk);
        let (requests, writes) = (self.requests, self.writes);
        *self = VirtioBlk { disk, requests, writes, ..Default::default() };
    }

    pub fn read(&self, off: u64) -> u32 {
        match off {
            0x000 => MAGIC,
            0x004 => 2,
            0x008 => 2, // block device
            0x00c => VENDOR,
            0x010 => (self.device_features() >> (32 * (self.device_features_sel & 1))) as u32,
            0x034 => {
                if self.queue_sel == 0 {
                    QUEUE_MAX
                } else {
                    0
                }
            }
            0x038 => self.queue_num,
            0x044 => self.queue_ready as u32,
            0x060 => self.interrupt_status,
            0x070 => self.status,
            0x080 => self.desc as u32,
            0x084 => (self.desc >> 32) as u32,
            0x090 => self.avail as u32,
            0x094 => (self.avail >> 32) as u32,
            0x0a0 => self.used as u32,
            0x0a4 => (self.used >> 32) as u32,
            0x0fc => 0, // config generation
            // Config space: capacity in 512-byte sectors (u64), then
            // size_max/seg_max etc. which we leave zero.
            0x100 => (self.disk.len() as u64 / SECTOR) as u32,
            0x104 => ((self.disk.len() as u64 / SECTOR) >> 32) as u32,
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u64, val: u32, ram: &mut [u8]) {
        let set_lo = |r: &mut u64, v: u32| *r = (*r & !0xffff_ffff) | v as u64;
        let set_hi = |r: &mut u64, v: u32| *r = (*r & 0xffff_ffff) | ((v as u64) << 32);
        match off {
            0x014 => self.device_features_sel = val,
            0x020 => {
                let shift = 32 * (self.driver_features_sel & 1);
                self.driver_features = (self.driver_features & !(0xffff_ffffu64 << shift)) | ((val as u64) << shift);
            }
            0x024 => self.driver_features_sel = val,
            0x030 => self.queue_sel = val,
            0x038 => {
                if self.queue_sel == 0 {
                    self.queue_num = val.min(QUEUE_MAX)
                }
            }
            0x044 => {
                if self.queue_sel == 0 {
                    self.queue_ready = val & 1 != 0
                }
            }
            0x050 => {
                if val == 0 {
                    self.process_queue(ram)
                }
            }
            0x064 => self.interrupt_status &= !val,
            0x070 => {
                if val == 0 {
                    self.reset();
                } else {
                    self.status = val;
                }
            }
            0x080 => set_lo(&mut self.desc, val),
            0x084 => set_hi(&mut self.desc, val),
            0x090 => set_lo(&mut self.avail, val),
            0x094 => set_hi(&mut self.avail, val),
            0x0a0 => set_lo(&mut self.used, val),
            0x0a4 => set_hi(&mut self.used, val),
            _ => {}
        }
    }

    fn process_queue(&mut self, ram: &mut [u8]) {
        if !self.queue_ready || self.queue_num == 0 {
            return;
        }
        let mut mem = Mem(ram);
        let qn = self.queue_num as u64;
        let avail_idx = mem.u16(self.avail + 2);
        let mut did_work = false;
        while self.last_avail != avail_idx {
            let slot = self.last_avail as u64 % qn;
            let head = mem.u16(self.avail + 4 + slot * 2);
            let written = self.handle_request(&mut mem, head);
            // Append to the used ring.
            let used_idx = mem.u16(self.used + 2);
            let entry = self.used + 4 + (used_idx as u64 % qn) * 8;
            mem.write(entry, &(head as u32).to_le_bytes());
            mem.write(entry + 4, &written.to_le_bytes());
            mem.write(self.used + 2, &used_idx.wrapping_add(1).to_le_bytes());
            self.last_avail = self.last_avail.wrapping_add(1);
            self.requests += 1;
            did_work = true;
        }
        if did_work {
            self.interrupt_status |= 1;
        }
    }

    /// Execute one request chain; returns bytes written into guest memory.
    fn handle_request(&mut self, mem: &mut Mem, head: u16) -> u32 {
        // Gather the descriptor chain.
        let mut chain: Vec<(u64, u32, u16)> = Vec::new();
        let mut idx = head;
        for _ in 0..self.queue_num {
            let d = self.desc + (idx as u64 % self.queue_num as u64) * 16;
            let (addr, len, flags, next) = (mem.u64(d), mem.u32(d + 8), mem.u16(d + 12), mem.u16(d + 14));
            chain.push((addr, len, flags));
            if flags & DESC_F_NEXT == 0 {
                break;
            }
            idx = next;
        }
        if chain.len() < 2 {
            return 0;
        }
        let (hdr, _, _) = chain[0];
        let req_type = mem.u32(hdr);
        let sector = mem.u64(hdr + 8);
        let (status_addr, _, _) = *chain.last().unwrap();
        let data = &chain[1..chain.len() - 1];

        let mut status = 0u8;
        let mut written = 0u32;
        let mut pos = sector * SECTOR;
        match req_type {
            BLK_T_IN => {
                for &(addr, len, _) in data {
                    let (start, end) = (pos as usize, (pos + len as u64) as usize);
                    if end > self.disk.len() {
                        status = 1;
                        break;
                    }
                    mem.write(addr, &self.disk[start..end]);
                    written += len;
                    pos += len as u64;
                }
            }
            BLK_T_OUT => {
                self.writes += 1;
                for &(addr, len, _) in data {
                    let (start, end) = (pos as usize, (pos + len as u64) as usize);
                    if end > self.disk.len() {
                        status = 1;
                        break;
                    }
                    if let Some(r) = mem.range(addr, len as usize) {
                        self.disk[start..end].copy_from_slice(&mem.0[r]);
                    }
                    pos += len as u64;
                }
            }
            BLK_T_FLUSH => {}
            BLK_T_GET_ID => {
                if let Some(&(addr, len, flags)) = data.first() {
                    if flags & DESC_F_WRITE != 0 {
                        let id = *b"rvwasm-virtio-blk\0\0\0";
                        let n = (len as usize).min(id.len());
                        mem.write(addr, &id[..n]);
                        written += n as u32;
                    }
                }
            }
            _ => status = 2, // unsupported
        }
        mem.write(status_addr, &[status]);
        written + 1
    }
}
