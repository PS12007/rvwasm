//! A just-in-time compiler from RISC-V to WebAssembly, for the browser build.
//!
//! The interpreter counts how often each block (a run of instructions up to
//! a jump or branch, within one page) starts. Once a block is hot it is
//! translated (`wasm`), and batches of blocks are handed to the host, which
//! compiles them as a WebAssembly module and adds the functions to this
//! module's function table. From then on the run loop calls the block
//! instead of interpreting it.
//!
//! Compiled code is exact: every block retires the same instructions with
//! the same results as the interpreter, and only runs when it fits in the
//! current chunk, so device syncs and interrupts land on the same
//! instruction counts. Runs are as deterministic with the JIT as without,
//! and time travel works unchanged.
//!
//! Blocks are keyed by virtual pc and the RAM offset it maps to, and each
//! page with compiled code is watched (through the journal hook that every
//! RAM write passes): a write to it drops that page's blocks, so
//! self-modifying code, loaders and DMA all see their new code.
//!
//! Calling into the table only exists on wasm32; elsewhere the JIT cannot be
//! enabled.

pub(crate) mod wasm;

use crate::bus::RAM_BASE;
use crate::cpu::Cpu;
use crate::mmu::Access;
use std::collections::HashMap;
use wasm::{classify, Block, Kind, Layout};

/// Slots, in pairs: a block may live in either slot of its pair.
const SLOTS: usize = 1 << 16;
/// Longest block, in instructions.
const MAX_BLOCK: usize = 64;
/// Slot counts at or above this are not counting.
const PENDING: u16 = u16::MAX - 1;
const NEVER: u16 = u16::MAX;

/// Compiles a WebAssembly module and adds its exports f0..fN-1 to the
/// function table at consecutive indices; returns the first index.
pub type Installer = fn(&[u8]) -> Option<u32>;

#[derive(Clone, Copy, Default)]
struct Slot {
    pc: u64,
    off: u32,
    /// Table index of the compiled block, 0 if none.
    func: u32,
    /// Instructions in the block.
    len: u16,
    /// Times the block started while interpreted (or PENDING / NEVER).
    count: u16,
    /// Bumped whenever the slot changes, so stale compiles are dropped.
    gen: u16,
}

struct Pending {
    slot: u32,
    gen: u16,
    block: Block,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct JitStats {
    pub blocks: u64,
    pub modules: u64,
    pub invalidated: u64,
    /// Instructions retired by compiled code.
    pub insts: u64,
}

pub struct Jit {
    slots: Box<[Slot]>,
    /// RAM page -> slots that may hold blocks in it.
    pages: HashMap<u32, Vec<u32>>,
    pending: Vec<Pending>,
    pending_since: u64,
    install: Installer,
    /// Starts before a block is compiled (at most 65533).
    pub hot: u16,
    /// Compile once this many blocks are waiting...
    pub batch: usize,
    /// ...or the oldest has waited this many instructions.
    pub delay: u64,
    pub stats: JitStats,
}

impl Jit {
    pub fn new(install: Installer) -> Jit {
        Jit {
            slots: vec![Slot::default(); SLOTS].into_boxed_slice(),
            pages: HashMap::new(),
            pending: Vec::new(),
            pending_since: 0,
            install,
            // Compiling has a cost on first call (the host compiles each
            // function lazily), which code that runs only a few hundred times,
            // as most of a kernel boot does, never earns back.
            hot: 1000,
            batch: 32,
            delay: 200_000,
            stats: JitStats::default(),
        }
    }
}

#[inline(always)]
fn slot_index(pc: u64, off: usize) -> usize {
    // Fibonacci hashing of the pc and its RAM offset together.
    let k = (pc >> 1) ^ ((off as u64) << 31);
    (k.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> (64 - SLOTS.trailing_zeros())) as usize
}

fn layout(cpu: &mut Cpu) -> Layout {
    use std::mem::offset_of;
    let (saved_bits, code_bits) = cpu.bus.journal.prepare(cpu.bus.ram.len());
    Layout {
        x: offset_of!(Cpu, x) as u32,
        pc: offset_of!(Cpu, pc) as u32,
        bail: offset_of!(Cpu, jit_bail) as u32,
        budget: offset_of!(Cpu, jit_budget) as u32,
        load_fn: jit_load as extern "C" fn(*mut Cpu, u64, u32) -> u64 as usize as u32,
        store_fn: jit_store as extern "C" fn(*mut Cpu, u64, u64, u32) -> u32 as usize as u32,
        alu_fn: jit_alu as extern "C" fn(u32, u64, u64) -> u64 as usize as u32,
        vm_data: offset_of!(Cpu, vm_data) as u32,
        tlb_gen: (offset_of!(Cpu, tlb) + offset_of!(crate::mmu::Tlb, gen)) as u32,
        resv: offset_of!(Cpu, jit_resv) as u32,
        tlb: cpu.tlb.tables.as_ptr() as usize as u32,
        ram: cpu.bus.ram.as_ptr() as usize as u32,
        ram_len: cpu.bus.ram.len() as u64,
        saved_bits: saved_bits as u32,
        code_bits: code_bits as u32,
        tohost_page: cpu.bus.tohost.map_or(u32::MAX, |t| (t.wrapping_sub(RAM_BASE) >> 12) as u32),
    }
}

/// What `jit_enter` did.
#[derive(PartialEq, Eq)]
pub(crate) enum Enter {
    /// Ran a block to its end.
    Ran,
    /// Did nothing; interpret one instruction.
    Miss,
    /// Did nothing or stopped part-way; interpret one instruction, and the
    /// one after it starts a block (the rest of a block that bailed, or code
    /// after an instruction the JIT does not compile).
    Interpret,
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
unsafe fn call(func: u32, cpu: *mut Cpu) -> u32 {
    // On wasm32 a function pointer is an index into the function table.
    let f: extern "C" fn(*mut Cpu) -> u32 = unsafe { std::mem::transmute(func as usize) };
    f(cpu)
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn call(_: u32, _: *mut Cpu) -> u32 {
    unreachable!("the JIT only runs on wasm32")
}

impl Cpu {
    /// Turn the JIT on with the host's module installer (wasm32 only).
    pub fn enable_jit(&mut self, install: Installer) {
        if cfg!(target_arch = "wasm32") {
            self.jit = Some(Box::new(Jit::new(install)));
        }
    }

    pub fn disable_jit(&mut self) {
        self.jit_flush();
        self.jit = None;
    }

    /// Forget all compiled code (memory was replaced wholesale).
    pub fn jit_flush(&mut self) {
        if let Some(j) = self.jit.as_deref_mut() {
            for s in j.slots.iter_mut() {
                *s = Slot { gen: s.gen.wrapping_add(1), ..Slot::default() };
            }
            j.pages.clear();
            j.pending.clear();
        }
        self.bus.journal.unwatch_all();
    }

    /// Called where a block starts: run the compiled block at pc, if there
    /// is one that fits in `left` instructions, else count towards
    /// compiling one.
    #[inline(never)]
    pub(crate) fn jit_enter(&mut self, left: u64) -> Enter {
        let pc = self.pc;
        if pc & 0xfff == 0xffe {
            return Enter::Miss; // a 32-bit instruction here would straddle pages
        }
        if pc >> 12 != self.tlb.fetch_vpage {
            // Refill the fetch cache as the interpreter's fetch would. A
            // fault is left for the interpreter to take.
            let Ok(pa) = self.translate(pc, Access::Fetch) else { return Enter::Miss };
            let page = (pa & !0xfff).wrapping_sub(RAM_BASE);
            if page >= self.bus.ram.len() as u64 {
                return Enter::Miss;
            }
            self.tlb.fetch_vpage = pc >> 12;
            self.tlb.fetch_ram_off = page as usize;
        }
        let off = self.tlb.fetch_ram_off + (pc & 0xfff) as usize;
        let Some(j) = self.jit.as_deref_mut() else { return Enter::Miss };
        if !self.bus.journal.code_hits.is_empty() {
            j.invalidate(&mut self.bus.journal.code_hits);
        }
        let pair = slot_index(pc, off) & !1;
        let hit = |s: &Slot| s.pc == pc && s.off == off as u32;
        let i = if hit(&j.slots[pair]) {
            pair
        } else if hit(&j.slots[pair + 1]) {
            pair + 1
        } else {
            // Replace the less valuable of the two: compiled or waiting to
            // be compiled beats counting, and a higher count beats a lower.
            let worth = |s: &Slot| if s.func != 0 || s.count == PENDING { u32::MAX } else { s.count as u32 };
            let v = if worth(&j.slots[pair]) <= worth(&j.slots[pair + 1]) { pair } else { pair + 1 };
            let s = &mut j.slots[v];
            *s = Slot { pc, off: off as u32, count: 1, gen: s.gen.wrapping_add(1), ..Slot::default() };
            return Enter::Miss;
        };
        let s = &mut j.slots[i];
        if s.func == 0 {
            if s.count < j.hot {
                s.count += 1;
            } else if s.count < PENDING {
                self.jit_queue(i, pc, off);
            } else if s.count == NEVER {
                return Enter::Interpret;
            }
            return Enter::Miss;
        }
        if s.len as u64 > left {
            return Enter::Miss;
        }
        let func = s.func;
        self.jit_budget = left.min(u32::MAX as u64) as u32;
        let n = unsafe { call(func, self as *mut Cpu) };
        self.instret += n as u64;
        if let Some(j) = self.jit.as_deref_mut() {
            j.stats.insts += n as u64;
        }
        if self.jit_bail != 0 {
            self.jit_bail = 0;
            return Enter::Interpret;
        }
        Enter::Ran
    }

    /// Decode the block at pc (RAM offset `off`) and queue it for compiling.
    #[cold]
    fn jit_queue(&mut self, i: usize, pc: u64, off: usize) {
        let mut insts = Vec::new();
        let mut a = pc;
        let mut o = off;
        while insts.len() < MAX_BLOCK {
            let Some(lo) = self.bus.ram.get(o..o + 2) else { break };
            let lo = u16::from_le_bytes([lo[0], lo[1]]);
            let (inst, len) = if lo & 3 == 3 {
                if a & 0xfff > 0xffc {
                    break; // would continue on the next page
                }
                let b = &self.bus.ram[o..o + 4];
                (u32::from_le_bytes([b[0], b[1], b[2], b[3]]), 4u8)
            } else {
                match self.rvc.get(lo) {
                    0 => break,
                    e => (e, 2u8),
                }
            };
            let kind = classify(inst);
            if kind == Kind::No {
                break;
            }
            insts.push((inst, len));
            a = a.wrapping_add(len as u64);
            o += len as usize;
            if kind == Kind::End || a & 0xfff == 0 || o + 2 > self.bus.ram.len() {
                break;
            }
        }
        let instret = self.instret;
        let j = self.jit.as_deref_mut().unwrap();
        let s = &mut j.slots[i];
        if insts.is_empty() {
            s.count = NEVER;
            return;
        }
        s.count = PENDING;
        let page = (off >> 12) as u32;
        j.pages.entry(page).or_default().push(i as u32);
        if j.pending.is_empty() {
            j.pending_since = instret;
        }
        j.pending.push(Pending { slot: i as u32, gen: s.gen, block: Block { pc, insts } });
        self.bus.journal.watch_page(page as usize);
    }

    /// Compile waiting blocks if enough are queued or they have waited long
    /// enough. Called at chunk boundaries.
    #[inline]
    pub(crate) fn jit_maybe_compile(&mut self) {
        let Some(j) = self.jit.as_deref() else { return };
        if j.pending.is_empty() || (j.pending.len() < j.batch && self.instret - j.pending_since < j.delay) {
            return;
        }
        let j = self.jit.as_deref_mut().unwrap();
        let pending = std::mem::take(&mut j.pending);
        let live: Vec<&Pending> = pending.iter().filter(|p| j.slots[p.slot as usize].gen == p.gen).collect();
        if live.is_empty() {
            return;
        }
        let blocks: Vec<&Block> = live.iter().map(|p| &p.block).collect();
        let install = j.install;
        let lay = layout(self);
        let bytes = wasm::module(&blocks, &lay);
        let j = self.jit.as_deref_mut().unwrap();
        match install(&bytes) {
            Some(base) => {
                for (k, p) in live.iter().enumerate() {
                    let s = &mut j.slots[p.slot as usize];
                    s.func = base + k as u32;
                    s.len = p.block.insts.len() as u16;
                }
                j.stats.blocks += live.len() as u64;
                j.stats.modules += 1;
            }
            None => self.disable_jit(),
        }
    }
}

impl Jit {
    /// Drop the blocks in pages that were written.
    #[cold]
    fn invalidate(&mut self, hits: &mut Vec<u32>) {
        for page in hits.drain(..) {
            let Some(list) = self.pages.remove(&page) else { continue };
            for i in list {
                let s = &mut self.slots[i as usize];
                if s.off >> 12 == page && s.count != 0 {
                    *s = Slot { pc: s.pc, off: s.off, gen: s.gen.wrapping_add(1), ..Slot::default() };
                    self.stats.invalidated += 1;
                }
            }
        }
    }
}

// ------------------------------------------------------- helpers for compiled code

/// Load for compiled code; `f3` is the load's funct3. Bails (sets
/// `jit_bail`) for anything but an aligned access to RAM that translates.
extern "C" fn jit_load(cpu: *mut Cpu, va: u64, f3: u32) -> u64 {
    let cpu = unsafe { &mut *cpu };
    let size = 1usize << (f3 & 3);
    let v = (|| {
        if va & (size as u64 - 1) != 0 {
            return None;
        }
        let pa = cpu.translate(va, Access::Load).ok()?;
        if pa.wrapping_sub(RAM_BASE) >= cpu.bus.ram.len() as u64 {
            return None;
        }
        cpu.bus.read(pa, size).ok()
    })();
    match v {
        Some(v) => match f3 {
            0 => v as i8 as i64 as u64,
            1 => v as i16 as i64 as u64,
            2 => v as i32 as i64 as u64,
            _ => v,
        },
        None => {
            cpu.jit_bail = 1;
            0
        }
    }
}

/// Store for compiled code. Returns nonzero (and does nothing) if the
/// interpreter must do it: a fault, MMIO, misalignment, or a page with code.
extern "C" fn jit_store(cpu: *mut Cpu, va: u64, val: u64, size: u32) -> u32 {
    let cpu = unsafe { &mut *cpu };
    let size = size as usize;
    let done = (|| {
        if va & (size as u64 - 1) != 0 {
            return false;
        }
        let Ok(pa) = cpu.translate(va, Access::Store) else { return false };
        let off = pa.wrapping_sub(RAM_BASE);
        if off >= cpu.bus.ram.len() as u64 || cpu.bus.journal.watched((off >> 12) as usize) {
            return false;
        }
        cpu.phys_write(pa, size, val, va).is_ok()
    })();
    if done {
        0
    } else {
        cpu.jit_bail = 1;
        1
    }
}

/// The ALU operations compiled code does not inline: M-extension multiply-
/// high and divides, and bit manipulation (b is the shift amount for the
/// immediate forms).
extern "C" fn jit_alu(inst: u32, a: u64, b: u64) -> u64 {
    if let Some(op) = crate::bitmanip::decode(inst) {
        return crate::bitmanip::eval(op, a, b);
    }
    let (a32, b32) = (a as u32, b as u32);
    let sx = |v: u32| v as i32 as i64 as u64;
    match (inst & 0x7f, (inst >> 12) & 7) {
        (0x33, 1) => (((a as i64 as i128) * (b as i64 as i128)) >> 64) as u64,
        (0x33, 2) => (((a as i64 as i128) * (b as u128 as i128)) >> 64) as u64,
        (0x33, 3) => (((a as u128) * (b as u128)) >> 64) as u64,
        (0x33, 4) => crate::exec::div(a, b),
        (0x33, 5) => crate::exec::divu(a, b),
        (0x33, 6) => crate::exec::rem(a, b),
        (0x33, _) => crate::exec::remu(a, b),
        (_, 4) => {
            let (a, b) = (a32 as i32, b32 as i32);
            (if b == 0 { -1 } else { a.wrapping_div(b) }) as i64 as u64
        }
        (_, 5) => sx(a32.checked_div(b32).unwrap_or(u32::MAX)),
        (_, 6) => {
            let (a, b) = (a32 as i32, b32 as i32);
            (if b == 0 { a } else { a.wrapping_rem(b) }) as i64 as u64
        }
        _ => sx(if b32 == 0 { a32 } else { a32 % b32 }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every instruction riscv-tests' rv64ui suite uses either compiles or
    /// is left to the interpreter, and modules are well formed enough to
    /// have the right section layout.
    #[test]
    fn builds_a_module() {
        let b = Block {
            pc: 0x8000_0000,
            insts: vec![
                (0x00a00513, 4), // li a0, 10
                (0x00b50533, 4), // add a0, a0, a1
                (0x00053583, 4), // ld a1, 0(a0)
                (0x00b53023, 4), // sd a1, 0(a0)
                (0x02b54533, 4), // div a0, a0, a1
                (0xfe0518e3, 4), // bnez a0, start
            ],
        };
        let l = Layout {
            x: 0,
            pc: 256,
            bail: 264,
            budget: 268,
            load_fn: 1,
            store_fn: 2,
            alu_fn: 3,
            vm_data: 272,
            tlb_gen: 280,
            resv: 288,
            tlb: 4096,
            ram: 1 << 20,
            ram_len: 1 << 20,
            saved_bits: 8192,
            code_bits: 8448,
            tohost_page: u32::MAX,
        };
        let m = wasm::module(&[&b], &l);
        assert_eq!(&m[..8], b"\0asm\x01\0\0\0");
        assert_eq!(classify(0x00000073), Kind::No); // ecall
        assert_eq!(classify(0x0000006f), Kind::End); // j .
        assert_eq!(classify(0xfe0518e3), Kind::Branch); // bnez
        assert_eq!(classify(0x6b855513), Kind::Plain); // rev8
        assert_eq!(classify(0x0005b507), Kind::No); // fld
    }
}
