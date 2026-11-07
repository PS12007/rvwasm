//! Sv39 address translation with a small software TLB, plus the load/store
//! paths used by the interpreter.
//!
//! The TLB holds one table per access type (fetch/load/store), each filled
//! only after the permission checks for the *current* privilege and
//! mstatus.SUM/MXR have passed. Any change to those (traps, xRET, writes to
//! satp/mstatus, sfence.vma) bumps a generation counter, invalidating every
//! entry in O(1).

use crate::bus::RAM_BASE;
use crate::cpu::{cause, Cpu, Trap, PRV_M, PRV_S, PRV_U};
use crate::csr::*;

const TLB_SIZE: usize = 256;
const PAGE: u64 = 4096;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Access {
    Fetch = 0,
    Load = 1,
    Store = 2,
}

#[derive(Clone, Copy)]
#[repr(C)] // compiled code reads entries
pub(crate) struct Entry {
    pub(crate) vpn: u64,
    pub(crate) gen: u64,
    pub(crate) ppage: u64,
}

pub struct Tlb {
    pub(crate) tables: Box<[[Entry; TLB_SIZE]; 3]>,
    pub(crate) gen: u64,
    /// One-entry cache for instruction fetch: the virtual page the PC is in
    /// and that page's byte offset in RAM. u64::MAX when invalid.
    pub(crate) fetch_vpage: u64,
    pub(crate) fetch_ram_off: usize,
}

impl Tlb {
    pub fn new() -> Self {
        Tlb {
            tables: Box::new([[Entry { vpn: 0, gen: 0, ppage: 0 }; TLB_SIZE]; 3]),
            gen: 1,
            fetch_vpage: u64::MAX,
            fetch_ram_off: 0,
        }
    }
    #[inline]
    pub fn flush(&mut self) {
        self.gen += 1;
        self.fetch_vpage = u64::MAX;
    }
}

impl Default for Tlb {
    fn default() -> Self {
        Self::new()
    }
}

const PTE_V: u64 = 1 << 0;
const PTE_R: u64 = 1 << 1;
const PTE_W: u64 = 1 << 2;
const PTE_X: u64 = 1 << 3;
const PTE_U: u64 = 1 << 4;
const PTE_A: u64 = 1 << 6;
const PTE_D: u64 = 1 << 7;

fn page_fault(acc: Access, va: u64) -> Trap {
    let c = match acc {
        Access::Fetch => cause::INST_PAGE_FAULT,
        Access::Load => cause::LOAD_PAGE_FAULT,
        Access::Store => cause::STORE_PAGE_FAULT,
    };
    Trap::new(c, va)
}

fn access_fault(acc: Access, va: u64) -> Trap {
    let c = match acc {
        Access::Fetch => cause::INST_ACCESS,
        Access::Load => cause::LOAD_ACCESS,
        Access::Store => cause::STORE_ACCESS,
    };
    Trap::new(c, va)
}

impl Cpu {
    /// Recompute cached translation state; call whenever prv, satp or mstatus change.
    pub(crate) fn update_mmu(&mut self) {
        self.tlb.flush();
        let sv39 = self.satp >> 60 == 8;
        self.data_prv = if self.prv == PRV_M && self.mstatus & MSTATUS_MPRV != 0 {
            ((self.mstatus & MSTATUS_MPP) >> MSTATUS_MPP_SHIFT) as u8
        } else {
            self.prv
        };
        self.vm_fetch = sv39 && self.prv != PRV_M;
        self.vm_data = sv39 && self.data_prv != PRV_M;
    }

    pub(crate) fn sfence_vma(&mut self) {
        self.tlb.flush();
    }

    #[inline(always)]
    pub fn translate(&mut self, va: u64, acc: Access) -> Result<u64, Trap> {
        let on = if acc == Access::Fetch { self.vm_fetch } else { self.vm_data };
        if !on {
            return Ok(va);
        }
        let vpn = va >> 12;
        let e = &self.tlb.tables[acc as usize][vpn as usize & (TLB_SIZE - 1)];
        if e.vpn == vpn && e.gen == self.tlb.gen {
            return Ok(e.ppage | (va & 0xfff));
        }
        self.translate_slow(va, acc)
    }

    #[cold]
    fn translate_slow(&mut self, va: u64, acc: Access) -> Result<u64, Trap> {
        let pa = self.walk(va, acc)?;
        let vpn = va >> 12;
        let gen = self.tlb.gen;
        self.tlb.tables[acc as usize][vpn as usize & (TLB_SIZE - 1)] = Entry { vpn, gen, ppage: pa & !0xfff };
        Ok(pa)
    }

    /// Sv39 page-table walk, with permission checks and hardware A/D updates.
    fn walk(&mut self, va: u64, acc: Access) -> Result<u64, Trap> {
        // Bits 63..39 must equal bit 38.
        if ((va as i64) << 25 >> 25) as u64 != va {
            return Err(page_fault(acc, va));
        }
        let prv = if acc == Access::Fetch { self.prv } else { self.data_prv };
        let mut table = (self.satp & ((1 << 44) - 1)) * PAGE;
        for level in (0..3).rev() {
            let vpn_i = (va >> (12 + 9 * level)) & 0x1ff;
            let pte_addr = table + vpn_i * 8;
            let mut pte = self.bus.read(pte_addr, 8).map_err(|_| access_fault(acc, va))?;
            if pte & PTE_V == 0 || (pte & PTE_R == 0 && pte & PTE_W != 0) || pte >> 54 != 0 {
                return Err(page_fault(acc, va));
            }
            let ppn = (pte >> 10) & ((1 << 44) - 1);
            if pte & (PTE_R | PTE_X) == 0 {
                // Pointer to the next level.
                if level == 0 || pte & (PTE_A | PTE_D | PTE_U) != 0 {
                    return Err(page_fault(acc, va));
                }
                table = ppn * PAGE;
                continue;
            }
            // Leaf: permission checks.
            let user = pte & PTE_U != 0;
            match prv {
                PRV_U if !user => return Err(page_fault(acc, va)),
                PRV_S if user && (acc == Access::Fetch || self.mstatus & MSTATUS_SUM == 0) => {
                    return Err(page_fault(acc, va))
                }
                _ => {}
            }
            let ok = match acc {
                Access::Fetch => pte & PTE_X != 0,
                Access::Load => pte & PTE_R != 0 || (self.mstatus & MSTATUS_MXR != 0 && pte & PTE_X != 0),
                Access::Store => pte & PTE_W != 0,
            };
            if !ok {
                return Err(page_fault(acc, va));
            }
            // Superpages must be aligned.
            let low_mask = (1u64 << (9 * level)) - 1;
            if ppn & low_mask != 0 {
                return Err(page_fault(acc, va));
            }
            // Hardware A/D update (Svadu behaviour).
            let need = PTE_A | if acc == Access::Store { PTE_D } else { 0 };
            if pte & need != need {
                pte |= need;
                self.bus.write(pte_addr, 8, pte).map_err(|_| access_fault(acc, va))?;
            }
            let offset_mask = (1u64 << (12 + 9 * level)) - 1;
            return Ok(((ppn * PAGE) & !offset_mask) | (va & offset_mask));
        }
        Err(page_fault(acc, va))
    }

    // ------------------------------------------------------------ memory ops

    #[inline(always)]
    fn phys_read(&mut self, pa: u64, size: usize) -> Result<u64, ()> {
        if pa < RAM_BASE {
            self.sync_time();
        }
        self.bus.read(pa, size)
    }

    /// Fetch the instruction at `pc`. A 16-bit instruction comes back in the
    /// low half (its low two bits are not `11`); the upper half is then
    /// whatever follows it and must be ignored.
    #[inline(always)]
    pub(crate) fn fetch(&mut self, pc: u64) -> Result<u32, Trap> {
        // Fast path: still in the page of the previous fetch, which is in RAM,
        // and not in the last two bytes of it (where a 32-bit instruction
        // would continue on the next page).
        if pc >> 12 == self.tlb.fetch_vpage && pc & 0xfff != 0xffe {
            let o = self.tlb.fetch_ram_off + (pc & 0xfff) as usize;
            if let Some(b) = self.bus.ram.get(o..o + 4) {
                return Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            }
        }
        self.fetch_slow(pc)
    }

    #[inline(never)]
    fn fetch_slow(&mut self, pc: u64) -> Result<u32, Trap> {
        let pa = self.translate(pc, Access::Fetch)?;
        let off = (pa & !0xfff).wrapping_sub(RAM_BASE);
        if off < self.bus.ram.len() as u64 {
            self.tlb.fetch_vpage = pc >> 12;
            self.tlb.fetch_ram_off = off as usize;
        }
        let lo = self.bus.read(pa, 2).map_err(|_| Trap::new(cause::INST_ACCESS, pc))? as u32;
        if lo & 3 != 3 {
            return Ok(lo);
        }
        // The second half of a 32-bit instruction. It may be on the next page,
        // whose fault reports the address of that half (xepc stays at pc).
        let pc2 = pc.wrapping_add(2);
        let pa2 = if pc2 & 0xfff == 0 { self.translate(pc2, Access::Fetch)? } else { pa + 2 };
        let hi = self.bus.read(pa2, 2).map_err(|_| Trap::new(cause::INST_ACCESS, pc2))? as u32;
        Ok(lo | (hi << 16))
    }

    #[inline(always)]
    pub(crate) fn load(&mut self, va: u64, size: usize) -> Result<u64, Trap> {
        if va & (size as u64 - 1) != 0 {
            return self.load_misaligned(va, size);
        }
        let pa = self.translate(va, Access::Load)?;
        self.phys_read(pa, size).map_err(|_| Trap::new(cause::LOAD_ACCESS, va))
    }

    #[inline(always)]
    pub(crate) fn store(&mut self, va: u64, size: usize, val: u64) -> Result<(), Trap> {
        if va & (size as u64 - 1) != 0 {
            return self.store_misaligned(va, size, val);
        }
        let pa = self.translate(va, Access::Store)?;
        self.phys_write(pa, size, val, va)
    }

    #[inline(always)]
    pub(crate) fn phys_write(&mut self, pa: u64, size: usize, val: u64, va: u64) -> Result<(), Trap> {
        if let Some(r) = self.reservation {
            if r & !7 == pa & !7 {
                self.set_reservation(None);
            }
        }
        if pa < RAM_BASE {
            self.sync_time();
        }
        self.bus.write(pa, size, val).map_err(|_| Trap::new(cause::STORE_ACCESS, va))
    }

    /// Translate both pages an access touches; returns the physical address of each byte's page.
    fn split_pages(&mut self, va: u64, size: usize, acc: Access) -> Result<(u64, u64), Trap> {
        let pa0 = self.translate(va, acc)?;
        let last = va.wrapping_add(size as u64 - 1);
        let pa1 = if (va ^ last) >> 12 == 0 { pa0 } else { self.translate(last & !0xfff, acc)? };
        Ok((pa0, pa1))
    }

    #[cold]
    fn load_misaligned(&mut self, va: u64, size: usize) -> Result<u64, Trap> {
        let (pa0, pa1) = self.split_pages(va, size, Access::Load)?;
        let mut v = 0u64;
        for i in 0..size as u64 {
            let a = va.wrapping_add(i);
            let pa = if (a ^ va) >> 12 == 0 { pa0 + i } else { (pa1 & !0xfff) | (a & 0xfff) };
            let b = self.phys_read(pa, 1).map_err(|_| Trap::new(cause::LOAD_ACCESS, va))?;
            v |= b << (8 * i);
        }
        Ok(v)
    }

    #[cold]
    fn store_misaligned(&mut self, va: u64, size: usize, val: u64) -> Result<(), Trap> {
        let (pa0, pa1) = self.split_pages(va, size, Access::Store)?;
        for i in 0..size as u64 {
            let a = va.wrapping_add(i);
            let pa = if (a ^ va) >> 12 == 0 { pa0 + i } else { (pa1 & !0xfff) | (a & 0xfff) };
            self.phys_write(pa, 1, (val >> (8 * i)) & 0xff, va)?;
        }
        Ok(())
    }

    /// Translate an aligned atomic access (AMO/SC are stores, LR is a load).
    pub(crate) fn atomic_addr(&mut self, va: u64, size: usize, acc: Access) -> Result<u64, Trap> {
        if va & (size as u64 - 1) != 0 {
            let c = if acc == Access::Load { cause::LOAD_MISALIGNED } else { cause::STORE_MISALIGNED };
            return Err(Trap::new(c, va));
        }
        let pa = self.translate(va, acc)?;
        // Atomics are only supported on RAM.
        if pa.wrapping_sub(RAM_BASE) >= self.bus.ram.len() as u64 {
            let c = if acc == Access::Load { cause::LOAD_ACCESS } else { cause::STORE_ACCESS };
            return Err(Trap::new(c, va));
        }
        Ok(pa)
    }

    pub(crate) fn phys_rw(&mut self, pa: u64, size: usize) -> u64 {
        self.bus.read(pa, size).unwrap_or(0)
    }

    pub(crate) fn phys_store_atomic(&mut self, pa: u64, size: usize, val: u64) {
        if let Some(r) = self.reservation {
            if r & !7 == pa & !7 {
                self.set_reservation(None);
            }
        }
        let _ = self.bus.write(pa, size, val);
    }

    /// Debugger helper: read guest virtual memory without side effects on traps.
    pub fn peek_virt(&mut self, va: u64, size: usize) -> Option<u64> {
        let pa = self.translate(va, Access::Load).ok()?;
        self.bus.read(pa, size).ok()
    }

    #[allow(dead_code)]
    pub(crate) fn prv_name(prv: u8) -> &'static str {
        match prv {
            PRV_U => "U",
            PRV_S => "S",
            _ => "M",
        }
    }
}
