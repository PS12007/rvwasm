//! Hart state, run loop, traps, interrupts and the CSR file.

use crate::bus::Bus;
use crate::csr::{self, *};
use crate::mmu::Tlb;
use crate::trace::Trace;

pub const PRV_U: u8 = 0;
pub const PRV_S: u8 = 1;
pub const PRV_M: u8 = 3;

/// Instructions executed between device/interrupt synchronisation points.
const CHUNK: u64 = 512;

pub mod cause {
    pub const INST_MISALIGNED: u64 = 0;
    pub const INST_ACCESS: u64 = 1;
    pub const ILLEGAL_INST: u64 = 2;
    pub const BREAKPOINT: u64 = 3;
    pub const LOAD_MISALIGNED: u64 = 4;
    pub const LOAD_ACCESS: u64 = 5;
    pub const STORE_MISALIGNED: u64 = 6;
    pub const STORE_ACCESS: u64 = 7;
    pub const ECALL_U: u64 = 8;
    pub const ECALL_S: u64 = 9;
    pub const ECALL_M: u64 = 11;
    pub const INST_PAGE_FAULT: u64 = 12;
    pub const LOAD_PAGE_FAULT: u64 = 13;
    pub const STORE_PAGE_FAULT: u64 = 15;
    pub const INTERRUPT: u64 = 1 << 63;
}

/// A synchronous exception: cause code and the value for xtval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trap {
    pub cause: u64,
    pub tval: u64,
}

impl Trap {
    #[inline]
    pub fn new(cause: u64, tval: u64) -> Self {
        Trap { cause, tval }
    }
}

/// Hook for emulating an SBI firmware in the emulator itself (used to boot
/// Linux without OpenSBI). Called on `ecall` from S-mode; return true if handled.
pub type SbiHandler = fn(&mut Cpu) -> bool;

pub struct Cpu {
    pub x: [u64; 32],
    pub pc: u64,
    pub prv: u8,
    pub bus: Bus,

    // Machine-level CSRs
    pub mstatus: u64,
    pub medeleg: u64,
    pub mideleg: u64,
    pub mie: u64,
    /// Software-writable mip bits (SSIP, STIP when Sstc is off, SEIP).
    pub mip_sw: u64,
    /// Hardware-driven mip bits, recomputed at each device sync.
    pub mip_hw: u64,
    pub mtvec: u64,
    pub mcounteren: u64,
    pub menvcfg: u64,
    pub mscratch: u64,
    pub mepc: u64,
    pub mcause: u64,
    pub mtval: u64,
    pub pmpcfg: [u64; 16],
    pub pmpaddr: [u64; 64],

    // Supervisor-level CSRs
    pub stvec: u64,
    pub scounteren: u64,
    pub senvcfg: u64,
    pub sscratch: u64,
    pub sepc: u64,
    pub scause: u64,
    pub stval: u64,
    pub satp: u64,
    pub stimecmp: u64,

    /// Instructions retired (including ones that trap).
    pub instret: u64,
    /// Cycles spent idle in WFI.
    pub idle_cycles: u64,
    mcycle_off: u64,
    minstret_off: u64,
    /// Cycles per mtime tick.
    pub time_div: u64,
    time_frac: u64,
    last_sync: u64,

    pub reservation: Option<u64>,
    pub wfi: bool,
    /// Set to leave the inner loop early (interrupt state may have changed).
    pub(crate) break_loop: bool,

    // MMU caches
    pub(crate) tlb: Tlb,
    pub(crate) vm_fetch: bool,
    pub(crate) vm_data: bool,
    pub(crate) data_prv: u8,

    pub trace: Trace,
    pub sbi: Option<SbiHandler>,
    /// Count of traps taken, for stats.
    pub traps_taken: u64,
    /// Instructions executed in each privilege mode (indexed by U=0, S=1, M=3).
    pub prv_instret: [u64; 4],
    pub halted: bool,
    pub halt_reason: Option<String>,
}

impl Cpu {
    pub fn new(bus: Bus) -> Self {
        let mut cpu = Cpu {
            x: [0; 32],
            pc: crate::bus::RAM_BASE,
            prv: PRV_M,
            bus,
            mstatus: (2 << 32) | (2 << 34),
            medeleg: 0,
            mideleg: 0,
            mie: 0,
            mip_sw: 0,
            mip_hw: 0,
            mtvec: 0,
            mcounteren: 0,
            menvcfg: 0,
            mscratch: 0,
            mepc: 0,
            mcause: 0,
            mtval: 0,
            pmpcfg: [0; 16],
            pmpaddr: [0; 64],
            stvec: 0,
            scounteren: 0,
            senvcfg: 0,
            sscratch: 0,
            sepc: 0,
            scause: 0,
            stval: 0,
            satp: 0,
            stimecmp: u64::MAX,
            instret: 0,
            idle_cycles: 0,
            mcycle_off: 0,
            minstret_off: 0,
            time_div: 10,
            time_frac: 0,
            last_sync: 0,
            reservation: None,
            wfi: false,
            break_loop: false,
            tlb: Tlb::new(),
            vm_fetch: false,
            vm_data: false,
            data_prv: PRV_M,
            trace: Trace::default(),
            sbi: None,
            traps_taken: 0,
            prv_instret: [0; 4],
            halted: false,
            halt_reason: None,
        };
        cpu.update_mmu();
        cpu
    }

    /// Leave the inner execution loop after the current instruction.
    pub fn break_now(&mut self) {
        self.break_loop = true;
    }

    /// Invalidate all cached address translations.
    pub fn flush_tlb(&mut self) {
        self.tlb.flush();
    }

    #[inline]
    pub fn cycle(&self) -> u64 {
        self.instret + self.idle_cycles
    }

    /// Run up to `budget` cycles. Returns the number of cycles consumed.
    pub fn run(&mut self, budget: u64) -> u64 {
        let start = self.cycle();
        let end = start.saturating_add(budget);
        while self.cycle() < end && !self.halted && self.bus.htif_exit.is_none() {
            self.sync_devices();
            self.check_interrupts();
            if self.wfi {
                let skip = self.cycles_until_timer().clamp(1, end - self.cycle());
                self.idle_cycles += skip;
                continue;
            }
            let n = (end - self.cycle()).min(CHUNK);
            if self.trace.enabled {
                self.exec_chunk::<true>(n);
            } else {
                self.exec_chunk::<false>(n);
            }
        }
        self.sync_devices();
        self.cycle() - start
    }

    /// Run up to `n` instructions. Chunks end at any trap or xRET, so every
    /// instruction in a chunk ran at the privilege level it started with.
    #[inline(always)]
    fn exec_chunk<const TRACE: bool>(&mut self, n: u64) {
        let prv = self.prv;
        let start = self.instret;
        for _ in 0..n {
            self.step::<TRACE>();
            if self.break_loop | self.bus.mmio_dirty {
                self.break_loop = false;
                self.bus.mmio_dirty = false;
                break;
            }
        }
        self.prv_instret[prv as usize & 3] += self.instret - start;
    }

    /// Execute exactly one instruction (or take one exception).
    #[inline(always)]
    pub fn step<const TRACE: bool>(&mut self) {
        let pc = self.pc;
        self.instret += 1;
        let raw = match self.fetch(pc) {
            Ok(i) => i,
            Err(t) => {
                self.take_trap(t.cause, t.tval, pc);
                return;
            }
        };
        if TRACE {
            self.trace.record_inst(self.cycle(), pc, raw, self.prv);
        }
        let r = if raw & 3 == 3 {
            self.execute(raw, pc, 4)
        } else {
            match crate::rvc::expand(raw as u16) {
                Some(inst) => self.execute(inst, pc, 2).map_err(|t| {
                    // An illegal compressed instruction reports its own 16 bits.
                    if t.cause == cause::ILLEGAL_INST {
                        Trap::new(t.cause, raw as u64)
                    } else {
                        t
                    }
                }),
                None => Err(Trap::new(cause::ILLEGAL_INST, raw as u64)),
            }
        };
        if let Err(t) = r {
            self.take_trap(t.cause, t.tval, pc);
        }
        self.x[0] = 0;
    }

    /// Advance mtime to match elapsed cycles.
    #[inline]
    pub fn sync_time(&mut self) {
        let now = self.cycle();
        let t = self.time_frac + (now - self.last_sync);
        self.last_sync = now;
        self.bus.clint.mtime = self.bus.clint.mtime.wrapping_add(t / self.time_div);
        self.time_frac = t % self.time_div;
    }

    /// Recompute hardware interrupt lines from device state.
    pub fn sync_devices(&mut self) {
        self.sync_time();
        let mtime = self.bus.clint.mtime;
        let lines = self.bus.irq_lines();
        self.bus.plic.update(lines);
        let mut hw = 0;
        if mtime >= self.bus.clint.mtimecmp {
            hw |= MIP_MTIP;
        }
        if self.bus.clint.msip & 1 != 0 {
            hw |= MIP_MSIP;
        }
        if self.menvcfg & MENVCFG_STCE != 0 && mtime >= self.stimecmp {
            hw |= MIP_STIP;
        }
        if self.bus.plic.irq(0) {
            hw |= MIP_MEIP;
        }
        if self.bus.plic.irq(1) {
            hw |= MIP_SEIP;
        }
        self.mip_hw = hw;
        self.bus.mmio_dirty = false;
    }

    #[inline]
    pub fn mip(&self) -> u64 {
        self.mip_sw | self.mip_hw
    }

    /// Cycles until the next timer comparator fires (u64::MAX if none armed).
    fn cycles_until_timer(&self) -> u64 {
        let mtime = self.bus.clint.mtime;
        let mut next = self.bus.clint.mtimecmp;
        if self.menvcfg & MENVCFG_STCE != 0 {
            next = next.min(self.stimecmp);
        }
        if next == u64::MAX {
            return u64::MAX;
        }
        next.saturating_sub(mtime).saturating_mul(self.time_div).saturating_sub(self.time_frac).max(1)
    }

    /// Take the highest-priority enabled pending interrupt, if any.
    pub fn check_interrupts(&mut self) {
        let pending = self.mip() & self.mie;
        if pending == 0 {
            return;
        }
        // Any pending-and-enabled interrupt wakes WFI, even if globally masked.
        self.wfi = false;
        let m_enabled = self.prv < PRV_M || self.mstatus & MSTATUS_MIE != 0;
        let s_enabled = self.prv < PRV_S || (self.prv == PRV_S && self.mstatus & MSTATUS_SIE != 0);
        let m_pending = pending & !self.mideleg;
        let s_pending = pending & self.mideleg;
        let set = if m_enabled && m_pending != 0 {
            m_pending
        } else if s_enabled && s_pending != 0 && self.prv <= PRV_S {
            s_pending
        } else {
            return;
        };
        for irq in [IRQ_MEI, IRQ_MSI, IRQ_MTI, IRQ_SEI, IRQ_SSI, IRQ_STI] {
            if set & (1 << irq) != 0 {
                let pc = self.pc;
                self.take_trap(cause::INTERRUPT | irq, 0, pc);
                return;
            }
        }
    }

    /// Enter a trap handler. `epc` is the address of the faulting instruction.
    pub fn take_trap(&mut self, cause: u64, tval: u64, epc: u64) {
        let interrupt = cause & cause::INTERRUPT != 0;
        let code = cause & !cause::INTERRUPT;
        if !interrupt && self.prv == PRV_S && code == cause::ECALL_S {
            if let Some(sbi) = self.sbi {
                if sbi(self) {
                    self.pc = epc.wrapping_add(4);
                    return;
                }
            }
        }
        let deleg = self.prv <= PRV_S
            && if interrupt { (self.mideleg >> code) & 1 != 0 } else { (self.medeleg >> code) & 1 != 0 };
        let from = self.prv;
        self.traps_taken += 1;
        if !deleg && self.sbi.is_some() {
            // No M-mode firmware is loaded when the SBI is emulated.
            self.halt_reason = Some(format!(
                "unhandled trap to M-mode: cause={cause:#x} epc={epc:#x} tval={tval:#x} prv={}",
                self.prv
            ));
            self.halted = true;
            return;
        }
        if deleg {
            self.scause = cause;
            self.sepc = epc;
            self.stval = tval;
            let sie = self.mstatus & MSTATUS_SIE != 0;
            self.mstatus &= !(MSTATUS_SPIE | MSTATUS_SIE | MSTATUS_SPP);
            if sie {
                self.mstatus |= MSTATUS_SPIE;
            }
            if self.prv == PRV_S {
                self.mstatus |= MSTATUS_SPP;
            }
            let base = self.stvec & !3;
            self.pc = if interrupt && self.stvec & 1 == 1 { base + 4 * code } else { base };
            self.prv = PRV_S;
        } else {
            self.mcause = cause;
            self.mepc = epc;
            self.mtval = tval;
            let mie = self.mstatus & MSTATUS_MIE != 0;
            self.mstatus &= !(MSTATUS_MPIE | MSTATUS_MIE | MSTATUS_MPP);
            if mie {
                self.mstatus |= MSTATUS_MPIE;
            }
            self.mstatus |= (self.prv as u64) << MSTATUS_MPP_SHIFT;
            let base = self.mtvec & !3;
            self.pc = if interrupt && self.mtvec & 1 == 1 { base + 4 * code } else { base };
            self.prv = PRV_M;
        }
        self.wfi = false;
        self.break_loop = true;
        self.update_mmu();
        if self.trace.enabled {
            self.trace.record_trap(self.cycle(), cause, epc, tval, from, self.prv);
        }
    }

    pub(crate) fn do_mret(&mut self) {
        let mpp = ((self.mstatus & MSTATUS_MPP) >> MSTATUS_MPP_SHIFT) as u8;
        let mpie = self.mstatus & MSTATUS_MPIE != 0;
        self.mstatus &= !(MSTATUS_MIE | MSTATUS_MPP);
        if mpie {
            self.mstatus |= MSTATUS_MIE;
        }
        self.mstatus |= MSTATUS_MPIE;
        if mpp != PRV_M {
            self.mstatus &= !MSTATUS_MPRV;
        }
        let from = self.prv;
        self.prv = mpp;
        self.pc = self.mepc;
        self.after_xret(from);
    }

    pub(crate) fn do_sret(&mut self) {
        let spp = if self.mstatus & MSTATUS_SPP != 0 { PRV_S } else { PRV_U };
        let spie = self.mstatus & MSTATUS_SPIE != 0;
        self.mstatus &= !(MSTATUS_SIE | MSTATUS_SPP | MSTATUS_MPRV);
        if spie {
            self.mstatus |= MSTATUS_SIE;
        }
        self.mstatus |= MSTATUS_SPIE;
        let from = self.prv;
        self.prv = spp;
        self.pc = self.sepc;
        self.after_xret(from);
    }

    fn after_xret(&mut self, from: u8) {
        self.update_mmu();
        self.break_loop = true;
        if self.trace.enabled {
            self.trace.record_xret(self.cycle(), self.pc, from, self.prv);
        }
    }

    // ---------------------------------------------------------------- CSRs

    /// Check privilege and read-only rules for a CSR access.
    pub(crate) fn csr_allowed(&self, addr: u16, write: bool) -> bool {
        let min_prv = ((addr >> 8) & 3) as u8;
        if self.prv < min_prv {
            return false;
        }
        if write && (addr >> 10) & 3 == 3 {
            return false;
        }
        match addr {
            // Counters visible to lower privilege levels via [ms]counteren.
            0xc00..=0xc1f => {
                let bit = 1u64 << (addr - 0xc00);
                if self.prv < PRV_M && self.mcounteren & bit == 0 {
                    return false;
                }
                if self.prv < PRV_S && self.scounteren & bit == 0 {
                    return false;
                }
                true
            }
            SATP => !(self.prv == PRV_S && self.mstatus & MSTATUS_TVM != 0),
            STIMECMP => self.prv == PRV_M || (self.menvcfg & MENVCFG_STCE != 0 && self.mcounteren & 2 != 0),
            _ => true,
        }
    }

    pub fn csr_read(&mut self, addr: u16) -> Option<u64> {
        Some(match addr {
            CYCLE | MCYCLE => self.cycle().wrapping_add(self.mcycle_off),
            INSTRET | MINSTRET => self.instret.wrapping_add(self.minstret_off),
            TIME => {
                self.sync_time();
                self.bus.clint.mtime
            }
            0xc03..=0xc1f | 0xb03..=0xb1f | 0x323..=0x33f => 0,
            SSTATUS => self.sstatus(),
            SIE => self.mie & self.mideleg,
            STVEC => self.stvec,
            SCOUNTEREN => self.scounteren,
            SENVCFG => self.senvcfg,
            SSCRATCH => self.sscratch,
            SEPC => self.sepc,
            SCAUSE => self.scause,
            STVAL => self.stval,
            SIP => self.mip() & self.mideleg,
            STIMECMP => self.stimecmp,
            SATP => self.satp,
            MVENDORID | MARCHID | MIMPID | MHARTID | MCONFIGPTR => 0,
            MSTATUS => self.mstatus_read(),
            MISA => MISA_VALUE,
            MEDELEG => self.medeleg,
            MIDELEG => self.mideleg,
            MIE => self.mie,
            MTVEC => self.mtvec,
            MCOUNTEREN => self.mcounteren,
            MENVCFG => self.menvcfg,
            MCOUNTINHIBIT => 0,
            MSCRATCH => self.mscratch,
            MEPC => self.mepc,
            MCAUSE => self.mcause,
            MTVAL => self.mtval,
            MIP => self.mip(),
            0x3a0..=0x3af => {
                let i = (addr - 0x3a0) as usize;
                if i & 1 == 1 {
                    return None; // odd pmpcfg registers don't exist on RV64
                }
                self.pmpcfg[i]
            }
            0x3b0..=0x3ef => self.pmpaddr[(addr - 0x3b0) as usize],
            MSECCFG => 0,
            TSELECT | TDATA1 | TDATA2 | TDATA3 => 0,
            _ => return None,
        })
    }

    pub fn csr_write(&mut self, addr: u16, val: u64) -> Option<()> {
        match addr {
            MCYCLE => self.mcycle_off = val.wrapping_sub(self.cycle()),
            MINSTRET => {
                // The writing instruction's own retirement is suppressed, so
                // the next instruction reads exactly `val`.
                self.minstret_off = val.wrapping_sub(self.instret + 1)
            }
            0xb03..=0xb1f | 0x323..=0x33f | MCOUNTINHIBIT => {}
            SSTATUS => {
                let v = (self.mstatus & !SSTATUS_MASK) | (val & SSTATUS_MASK);
                self.mstatus_write(v);
            }
            SIE => self.mie = (self.mie & !self.mideleg) | (val & self.mideleg),
            STVEC => self.stvec = val & !2,
            SCOUNTEREN => self.scounteren = val & 0xffff_ffff,
            SENVCFG => self.senvcfg = val & 1,
            SSCRATCH => self.sscratch = val,
            SEPC => self.sepc = val & !1,
            SCAUSE => self.scause = val,
            STVAL => self.stval = val,
            SIP => {
                let mask = MIP_SSIP & self.mideleg;
                self.mip_sw = (self.mip_sw & !mask) | (val & mask);
            }
            STIMECMP => self.stimecmp = val,
            SATP => {
                let mode = val >> 60;
                if mode == 0 || mode == 8 {
                    // MODE, ASID and PPN cover all 64 bits.
                    self.satp = val;
                    self.update_mmu();
                }
            }
            MSTATUS => self.mstatus_write(val),
            MISA => {}
            MEDELEG => self.medeleg = val & 0xb3ff, // ecall-from-M (11) and reserved not delegable
            MIDELEG => self.mideleg = val & S_INTS,
            MIE => self.mie = val & ALL_INTS,
            MTVEC => self.mtvec = val & !2,
            MCOUNTEREN => self.mcounteren = val & 0xffff_ffff,
            MENVCFG => self.menvcfg = val & (MENVCFG_STCE | MENVCFG_ADUE | 1),
            MSCRATCH => self.mscratch = val,
            MEPC => self.mepc = val & !1,
            MCAUSE => self.mcause = val,
            MTVAL => self.mtval = val,
            MIP => {
                let mut mask = MIP_SSIP | MIP_SEIP;
                if self.menvcfg & MENVCFG_STCE == 0 {
                    mask |= MIP_STIP;
                }
                self.mip_sw = (self.mip_sw & !mask) | (val & mask);
            }
            0x3a0..=0x3af => {
                let i = (addr - 0x3a0) as usize;
                if i & 1 == 1 {
                    return None;
                }
                self.pmpcfg[i] = val;
            }
            0x3b0..=0x3ef => self.pmpaddr[(addr - 0x3b0) as usize] = val & ((1 << 54) - 1),
            MSECCFG | TSELECT | TDATA1 | TDATA2 | TDATA3 => {}
            CYCLE | TIME | INSTRET | MVENDORID | MARCHID | MIMPID | MHARTID | MCONFIGPTR => return None,
            _ => {
                self.csr_read(addr)?;
            }
        }
        self.break_loop = true;
        Some(())
    }

    fn mstatus_read(&self) -> u64 {
        self.mstatus | (2 << 32) | (2 << 34)
    }

    fn sstatus(&self) -> u64 {
        self.mstatus_read() & SSTATUS_MASK
    }

    fn mstatus_write(&mut self, val: u64) {
        let mut v = val & MSTATUS_WMASK;
        // MPP is WARL: the reserved value 2 keeps the old setting.
        if (v & MSTATUS_MPP) >> MSTATUS_MPP_SHIFT == 2 {
            v = (v & !MSTATUS_MPP) | (self.mstatus & MSTATUS_MPP);
        }
        self.mstatus = (self.mstatus & !MSTATUS_WMASK) | v;
        self.update_mmu();
    }

    /// Name and value of every CSR the UI shows.
    pub fn csr_dump(&mut self) -> Vec<(&'static str, u64)> {
        const SHOWN: [u16; 22] = [
            MSTATUS, MISA, MEDELEG, MIDELEG, MIE, MIP, MTVEC, MEPC, MCAUSE, MTVAL, MSCRATCH, MENVCFG, SSTATUS, STVEC,
            SEPC, SCAUSE, STVAL, SSCRATCH, SATP, STIMECMP, TIME, INSTRET,
        ];
        SHOWN.iter().map(|&a| (csr::name(a).unwrap_or("?"), self.csr_read(a).unwrap_or(0))).collect()
    }
}
