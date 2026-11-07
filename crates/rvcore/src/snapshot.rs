//! Saving and restoring the whole machine.
//!
//! `save_state`/`load_state` cover everything except RAM, the disk and the
//! boot ROM: harts registers and CSRs, device registers, the TLB and the
//! timing counters. That is a few tens of KiB, cheap enough to take often;
//! `timetravel` pairs it with journaled pages. A snapshot file adds the
//! memory contents, each page left out (all zeros), stored as one repeated
//! byte, run-length encoded (such as xv6's free pages: a pointer, then 1s)
//! or as is, whichever is smallest.
//!
//! Both directions walk one field list (`visit`), so they cannot disagree on
//! the layout.

use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::devices::virtio::VirtioBlk;
use crate::journal::PAGE;

const STATE_MAGIC: u32 = 0x5257_5354; // "RWST"
const STATE_VERSION: u32 = 2;
const FILE_MAGIC: &[u8; 16] = b"rvwasm-snapshot\0";
const FILE_VERSION: u32 = 1;

trait Io {
    fn u64(&mut self, v: &mut u64);
    fn bytes(&mut self, v: &mut Vec<u8>);
    fn u32(&mut self, v: &mut u32) {
        let mut x = *v as u64;
        self.u64(&mut x);
        *v = x as u32;
    }
    fn u16(&mut self, v: &mut u16) {
        let mut x = *v as u64;
        self.u64(&mut x);
        *v = x as u16;
    }
    fn u8(&mut self, v: &mut u8) {
        let mut x = *v as u64;
        self.u64(&mut x);
        *v = x as u8;
    }
    fn usize(&mut self, v: &mut usize) {
        let mut x = *v as u64;
        self.u64(&mut x);
        *v = x as usize;
    }
    fn bool(&mut self, v: &mut bool) {
        let mut x = *v as u64;
        self.u64(&mut x);
        *v = x != 0;
    }
    fn opt(&mut self, v: &mut Option<u64>) {
        let mut some = v.is_some();
        let mut x = v.unwrap_or(0);
        self.bool(&mut some);
        self.u64(&mut x);
        *v = some.then_some(x);
    }
    fn u64s(&mut self, v: &mut [u64]) {
        v.iter_mut().for_each(|x| self.u64(x));
    }
}

struct Writer(Vec<u8>);

impl Io for Writer {
    fn u64(&mut self, v: &mut u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, v: &mut Vec<u8>) {
        self.0.extend_from_slice(&(v.len() as u64).to_le_bytes());
        self.0.extend_from_slice(v);
    }
}

struct Reader<'a> {
    b: &'a [u8],
    bad: bool,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> &[u8] {
        if self.bad || self.b.len() < n {
            self.bad = true;
            return &[];
        }
        let (head, rest) = self.b.split_at(n);
        self.b = rest;
        head
    }
}

impl Io for Reader<'_> {
    fn u64(&mut self, v: &mut u64) {
        let b = self.take(8);
        *v = if b.len() == 8 { u64::from_le_bytes(b.try_into().unwrap()) } else { 0 };
    }
    fn bytes(&mut self, v: &mut Vec<u8>) {
        let mut n = 0;
        self.u64(&mut n);
        *v = self.take(n as usize).to_vec();
    }
}

fn visit(cpu: &mut Cpu, io: &mut impl Io) {
    let c = cpu;
    let mut magic = STATE_MAGIC;
    let mut version = STATE_VERSION;
    io.u32(&mut magic);
    io.u32(&mut version);
    let mut ram_len = c.bus.ram.len() as u64;
    io.u64(&mut ram_len);

    io.u64s(&mut c.x);
    io.u64(&mut c.pc);
    io.u8(&mut c.prv);
    io.u64s(&mut c.f);
    io.u8(&mut c.fflags);
    io.u8(&mut c.frm);
    for f in [
        &mut c.mstatus,
        &mut c.medeleg,
        &mut c.mideleg,
        &mut c.mie,
        &mut c.mip_sw,
        &mut c.mip_hw,
        &mut c.mtvec,
        &mut c.mcounteren,
        &mut c.menvcfg,
        &mut c.mscratch,
        &mut c.mepc,
        &mut c.mcause,
        &mut c.mtval,
        &mut c.stvec,
        &mut c.scounteren,
        &mut c.senvcfg,
        &mut c.sscratch,
        &mut c.sepc,
        &mut c.scause,
        &mut c.stval,
        &mut c.satp,
        &mut c.stimecmp,
        &mut c.instret,
        &mut c.idle_cycles,
        &mut c.mcycle_off,
        &mut c.minstret_off,
        &mut c.time_div,
        &mut c.time_frac,
        &mut c.last_sync,
        &mut c.chunk_left,
        &mut c.traps_taken,
    ] {
        io.u64(f);
    }
    io.u64s(&mut c.pmpcfg);
    io.u64s(&mut c.pmpaddr);
    io.u64s(&mut c.prv_instret);
    io.opt(&mut c.reservation);
    io.opt(&mut c.last_trap_instret);
    io.bool(&mut c.wfi);
    io.bool(&mut c.break_loop);
    io.bool(&mut c.halted);
    let mut reason = c.halt_reason.take().map(String::into_bytes);
    let mut has_reason = reason.is_some();
    io.bool(&mut has_reason);
    let mut r = reason.take().unwrap_or_default();
    io.bytes(&mut r);
    c.halt_reason = has_reason.then(|| String::from_utf8_lossy(&r).into_owned());
    let mut sbi = c.sbi.is_some();
    io.bool(&mut sbi);
    c.sbi = sbi.then_some(crate::linux::sbi_call as crate::cpu::SbiHandler);

    // The TLB is part of the state: a guest that edits a page table without
    // sfence.vma sees the stale entry, and a replay must see the same.
    for table in c.tlb.tables.iter_mut() {
        for e in table.iter_mut() {
            io.u64(&mut e.vpn);
            io.u64(&mut e.gen);
            io.u64(&mut e.ppage);
        }
    }
    io.u64(&mut c.tlb.gen);
    io.u64(&mut c.tlb.fetch_vpage);
    io.usize(&mut c.tlb.fetch_ram_off);
    io.bool(&mut c.vm_fetch);
    io.bool(&mut c.vm_data);
    io.u8(&mut c.data_prv);

    visit_bus(&mut c.bus, io);
}

fn visit_bus(b: &mut Bus, io: &mut impl Io) {
    io.u32(&mut b.clint.msip);
    io.u64(&mut b.clint.mtimecmp);
    io.u64(&mut b.clint.mtime);

    for p in b.plic.priority.iter_mut() {
        io.u32(p);
    }
    io.u64(&mut b.plic.pending);
    io.u64(&mut b.plic.claimed);
    io.u64s(&mut b.plic.enable);
    for t in b.plic.threshold.iter_mut() {
        io.u32(t);
    }

    let u = &mut b.uart;
    let mut rx: Vec<u8> = u.rx.iter().copied().collect();
    io.bytes(&mut rx);
    u.rx = rx.into();
    for f in [&mut u.ier, &mut u.lcr, &mut u.mcr, &mut u.scr, &mut u.dll, &mut u.dlm] {
        io.u8(f);
    }
    io.bool(&mut u.thre_ip);

    let mut has_disk = b.virtio.is_some();
    io.bool(&mut has_disk);
    if has_disk {
        let v = b.virtio.get_or_insert_with(|| VirtioBlk::new(Vec::new()));
        io.u32(&mut v.device_features_sel);
        io.u64(&mut v.driver_features);
        io.u32(&mut v.driver_features_sel);
        io.u32(&mut v.queue_sel);
        io.u32(&mut v.queue_num);
        io.bool(&mut v.queue_ready);
        io.u64(&mut v.desc);
        io.u64(&mut v.avail);
        io.u64(&mut v.used);
        io.u16(&mut v.last_avail);
        io.u32(&mut v.interrupt_status);
        io.u32(&mut v.status);
        io.u64(&mut v.requests);
        io.u64(&mut v.writes);
    } else {
        b.virtio = None;
    }
    io.opt(&mut b.tohost);
    io.opt(&mut b.fromhost);
    io.opt(&mut b.htif_exit);
    io.bool(&mut b.mmio_dirty);
}

/// Whether `bytes` look like a `save_snapshot` file.
pub fn is_snapshot(bytes: &[u8]) -> bool {
    bytes.starts_with(FILE_MAGIC)
}

impl Cpu {
    /// Everything but RAM, the disk and the ROM. Pending UART output is not
    /// included: it belongs to the host once produced.
    pub fn save_state(&mut self) -> Vec<u8> {
        let mut w = Writer(Vec::with_capacity(24 << 10));
        visit(self, &mut w);
        w.0
    }

    /// Restore what `save_state` saved, from this same machine (same RAM size
    /// and disk presence). RAM and disk contents are left alone.
    pub fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
        let mut head = Reader { b: state, bad: false };
        let (mut magic, mut version, mut ram_len) = (0u32, 0u32, 0u64);
        head.u32(&mut magic);
        head.u32(&mut version);
        head.u64(&mut ram_len);
        if head.bad || magic != STATE_MAGIC || version != STATE_VERSION {
            return Err("not an rvwasm machine state (or from another version)".into());
        }
        if ram_len != self.bus.ram.len() as u64 {
            return Err(format!("state is for {} MiB of RAM, machine has {}", ram_len >> 20, self.bus.ram.len() >> 20));
        }
        let mut r = Reader { b: state, bad: false };
        visit(self, &mut r);
        if r.bad {
            return Err("machine state is truncated".into());
        }
        self.bus.uart.tx.clear();
        self.jit_resv = self.reservation.unwrap_or(u64::MAX);
        // RAM was rewritten behind the journal's back.
        self.jit_flush();
        Ok(())
    }

    /// A self-contained snapshot: state plus RAM and disk contents.
    pub fn save_snapshot(&mut self) -> Vec<u8> {
        let mut w = Writer(Vec::new());
        w.0.extend_from_slice(FILE_MAGIC);
        let mut version = FILE_VERSION;
        w.u32(&mut version);
        let mut state = self.save_state();
        w.bytes(&mut state);
        let mut rom = self.bus.rom.clone();
        w.bytes(&mut rom);
        write_pages(&mut w, &self.bus.ram);
        if let Some(v) = &self.bus.virtio {
            write_pages(&mut w, &v.disk);
        }
        w.0
    }

    /// Rebuild a machine from `save_snapshot` output.
    pub fn from_snapshot(bytes: &[u8]) -> Result<Cpu, String> {
        if bytes.len() < 16 || &bytes[..16] != FILE_MAGIC {
            return Err("not an rvwasm snapshot".into());
        }
        let mut r = Reader { b: &bytes[16..], bad: false };
        let mut version = 0u32;
        r.u32(&mut version);
        if version != FILE_VERSION {
            return Err(format!("snapshot format {version} is not supported (expected {FILE_VERSION})"));
        }
        let (mut state, mut rom) = (Vec::new(), Vec::new());
        r.bytes(&mut state);
        r.bytes(&mut rom);
        // RAM size and disk presence are in the state header and body.
        let mut peek = Reader { b: &state, bad: false };
        let (mut m, mut v, mut ram_len) = (0u32, 0u32, 0u64);
        peek.u32(&mut m);
        peek.u32(&mut v);
        peek.u64(&mut ram_len);
        if ram_len > 4 << 30 {
            return Err("snapshot RAM size is implausible".into());
        }
        let mut bus = Bus::new(ram_len as usize);
        bus.rom = rom;
        read_pages(&mut r, &mut bus.ram)?;
        let mut cpu = Cpu::new(bus);
        cpu.load_state(&state)?;
        if let Some(v) = cpu.bus.virtio.as_mut() {
            read_pages(&mut r, &mut v.disk)?;
        }
        if r.bad {
            return Err("snapshot is truncated".into());
        }
        Ok(cpu)
    }
}

// The top two bits of a page number say how the page is stored.
const KIND_SHIFT: u32 = 62;
const RAW: u64 = 0; // 4096 bytes follow
const FILL: u64 = 1; // one byte value repeated; the byte follows
const RLE: u64 = 2; // run-length encoded: length, then (run - 1, byte) pairs

/// (run - 1, byte) pairs; runs are at most 256 bytes.
fn rle(page: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < page.len() {
        let b = page[i];
        let mut n = 1;
        while i + n < page.len() && n < 256 && page[i + n] == b {
            n += 1;
        }
        out.push((n - 1) as u8);
        out.push(b);
        i += n;
    }
    out
}

/// Memory as its length, then each page that is not all zeros as (page
/// number | kind << 62, contents), ended by u64::MAX.
fn write_pages(w: &mut Writer, mem: &[u8]) {
    w.u64(&mut (mem.len() as u64));
    for (i, page) in mem.chunks(PAGE).enumerate() {
        let i = i as u64;
        let first = page[0];
        if page.iter().all(|&b| b == first) {
            if first != 0 {
                w.u64(&mut (i | FILL << KIND_SHIFT));
                w.u64(&mut (first as u64));
            }
            continue;
        }
        let mut packed = rle(page);
        if packed.len() + 8 < page.len() {
            w.u64(&mut (i | RLE << KIND_SHIFT));
            w.bytes(&mut packed);
        } else {
            w.u64(&mut (i | RAW << KIND_SHIFT));
            w.0.extend_from_slice(page);
        }
    }
    w.0.extend_from_slice(&u64::MAX.to_le_bytes());
}

fn read_pages(r: &mut Reader, mem: &mut Vec<u8>) -> Result<(), String> {
    let mut len = 0u64;
    r.u64(&mut len);
    if len > 4 << 30 {
        return Err("snapshot memory size is implausible".into());
    }
    *mem = vec![0; len as usize];
    loop {
        let mut page = 0u64;
        r.u64(&mut page);
        if r.bad {
            return Err("snapshot is truncated".into());
        }
        if page == u64::MAX {
            return Ok(());
        }
        let kind = page >> KIND_SHIFT;
        let page = page & ((1 << KIND_SHIFT) - 1);
        let start = (page as usize).checked_mul(PAGE).filter(|&s| s < mem.len()).ok_or("bad page in snapshot")?;
        let end = (start + PAGE).min(mem.len());
        let dst = &mut mem[start..end];
        match kind {
            FILL => {
                let mut b = 0u64;
                r.u64(&mut b);
                dst.fill(b as u8);
                continue;
            }
            RLE => {
                let mut packed = Vec::new();
                r.bytes(&mut packed);
                let mut at = 0;
                let (pairs, _) = packed.as_chunks::<2>();
                for &[len, byte] in pairs {
                    let n = len as usize + 1;
                    let run = dst.get_mut(at..at + n).ok_or("bad page in snapshot")?;
                    run.fill(byte);
                    at += n;
                }
                if at != dst.len() {
                    return Err("bad page in snapshot".into());
                }
                continue;
            }
            RAW => {}
            _ => return Err("bad page in snapshot".into()),
        }
        let data = r.take(end - start);
        if data.len() != end - start {
            return Err("snapshot is truncated".into());
        }
        dst.copy_from_slice(data);
    }
}
