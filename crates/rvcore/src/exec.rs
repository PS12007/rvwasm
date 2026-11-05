//! RV64IMA + Zicsr + Zifencei instruction execution.

use crate::cpu::{cause, Cpu, Trap, PRV_M, PRV_S, PRV_U};
use crate::csr::*;
use crate::mmu::Access;

#[inline(always)]
fn rd(i: u32) -> usize {
    ((i >> 7) & 31) as usize
}
#[inline(always)]
fn rs1(i: u32) -> usize {
    ((i >> 15) & 31) as usize
}
#[inline(always)]
fn rs2(i: u32) -> usize {
    ((i >> 20) & 31) as usize
}
#[inline(always)]
fn funct3(i: u32) -> u32 {
    (i >> 12) & 7
}
#[inline(always)]
fn funct7(i: u32) -> u32 {
    i >> 25
}
#[inline(always)]
pub fn imm_i(i: u32) -> u64 {
    ((i as i32) >> 20) as i64 as u64
}
#[inline(always)]
pub fn imm_s(i: u32) -> u64 {
    ((((i as i32) >> 25) << 5) | ((i >> 7) & 0x1f) as i32) as i64 as u64
}
#[inline(always)]
pub fn imm_b(i: u32) -> u64 {
    let v = (((i as i32) >> 31) << 12)
        | (((i >> 7) & 1) << 11) as i32
        | (((i >> 25) & 0x3f) << 5) as i32
        | (((i >> 8) & 0xf) << 1) as i32;
    v as i64 as u64
}
#[inline(always)]
pub fn imm_u(i: u32) -> u64 {
    (i & 0xffff_f000) as i32 as i64 as u64
}
#[inline(always)]
pub fn imm_j(i: u32) -> u64 {
    let v = (((i as i32) >> 31) << 20)
        | (i & 0xff000) as i32
        | (((i >> 20) & 1) << 11) as i32
        | (((i >> 21) & 0x3ff) << 1) as i32;
    v as i64 as u64
}

#[inline(always)]
fn sext32(v: u64) -> u64 {
    v as i32 as i64 as u64
}

#[inline(always)]
fn illegal(inst: u32) -> Result<(), Trap> {
    Err(Trap::new(cause::ILLEGAL_INST, inst as u64))
}

fn div(a: u64, b: u64) -> u64 {
    let (a, b) = (a as i64, b as i64);
    if b == 0 {
        u64::MAX
    } else {
        a.wrapping_div(b) as u64
    }
}
fn divu(a: u64, b: u64) -> u64 {
    if b == 0 { u64::MAX } else { a / b }
}
fn rem(a: u64, b: u64) -> u64 {
    let (a, b) = (a as i64, b as i64);
    if b == 0 {
        a as u64
    } else {
        a.wrapping_rem(b) as u64
    }
}
fn remu(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { a % b }
}

impl Cpu {
    #[inline(always)]
    fn jump(&mut self, target: u64) -> Result<(), Trap> {
        if target & 3 != 0 {
            return Err(Trap::new(cause::INST_MISALIGNED, target));
        }
        self.pc = target;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn execute(&mut self, inst: u32, pc: u64) -> Result<(), Trap> {
        self.pc = pc.wrapping_add(4);
        let x = &self.x;
        match inst & 0x7f {
            // LUI
            0x37 => self.x[rd(inst)] = imm_u(inst),
            // AUIPC
            0x17 => self.x[rd(inst)] = pc.wrapping_add(imm_u(inst)),
            // JAL
            0x6f => {
                let t = pc.wrapping_add(imm_j(inst));
                self.jump(t)?;
                self.x[rd(inst)] = pc.wrapping_add(4);
            }
            // JALR
            0x67 => {
                if funct3(inst) != 0 {
                    return illegal(inst);
                }
                let t = x[rs1(inst)].wrapping_add(imm_i(inst)) & !1;
                self.jump(t)?;
                self.x[rd(inst)] = pc.wrapping_add(4);
            }
            // BRANCH
            0x63 => {
                let (a, b) = (x[rs1(inst)], x[rs2(inst)]);
                let taken = match funct3(inst) {
                    0 => a == b,
                    1 => a != b,
                    4 => (a as i64) < (b as i64),
                    5 => (a as i64) >= (b as i64),
                    6 => a < b,
                    7 => a >= b,
                    _ => return illegal(inst),
                };
                if taken {
                    self.jump(pc.wrapping_add(imm_b(inst)))?;
                }
            }
            // LOAD
            0x03 => {
                let addr = x[rs1(inst)].wrapping_add(imm_i(inst));
                let v = match funct3(inst) {
                    0 => self.load(addr, 1)? as i8 as i64 as u64,
                    1 => self.load(addr, 2)? as i16 as i64 as u64,
                    2 => self.load(addr, 4)? as i32 as i64 as u64,
                    3 => self.load(addr, 8)?,
                    4 => self.load(addr, 1)?,
                    5 => self.load(addr, 2)?,
                    6 => self.load(addr, 4)?,
                    _ => return illegal(inst),
                };
                self.x[rd(inst)] = v;
            }
            // STORE
            0x23 => {
                let addr = x[rs1(inst)].wrapping_add(imm_s(inst));
                let v = x[rs2(inst)];
                match funct3(inst) {
                    0 => self.store(addr, 1, v)?,
                    1 => self.store(addr, 2, v)?,
                    2 => self.store(addr, 4, v)?,
                    3 => self.store(addr, 8, v)?,
                    _ => return illegal(inst),
                }
            }
            // OP-IMM
            0x13 => {
                let a = x[rs1(inst)];
                let imm = imm_i(inst);
                let shamt = (inst >> 20) & 0x3f;
                let v = match funct3(inst) {
                    0 => a.wrapping_add(imm),
                    1 => {
                        if inst >> 26 != 0 {
                            return illegal(inst);
                        }
                        a << shamt
                    }
                    2 => ((a as i64) < (imm as i64)) as u64,
                    3 => (a < imm) as u64,
                    4 => a ^ imm,
                    5 => match inst >> 26 {
                        0x00 => a >> shamt,
                        0x10 => ((a as i64) >> shamt) as u64,
                        _ => return illegal(inst),
                    },
                    6 => a | imm,
                    _ => a & imm,
                };
                self.x[rd(inst)] = v;
            }
            // OP-IMM-32
            0x1b => {
                let a = x[rs1(inst)];
                let shamt = (inst >> 20) & 0x1f;
                let v = match (funct3(inst), funct7(inst)) {
                    (0, _) => sext32(a.wrapping_add(imm_i(inst))),
                    (1, 0x00) => sext32(((a as u32) << shamt) as u64),
                    (5, 0x00) => sext32(((a as u32) >> shamt) as u64),
                    (5, 0x20) => ((a as i32) >> shamt) as i64 as u64,
                    _ => return illegal(inst),
                };
                self.x[rd(inst)] = v;
            }
            // OP
            0x33 => {
                let (a, b) = (x[rs1(inst)], x[rs2(inst)]);
                let v = match (funct7(inst), funct3(inst)) {
                    (0x00, 0) => a.wrapping_add(b),
                    (0x20, 0) => a.wrapping_sub(b),
                    (0x00, 1) => a << (b & 63),
                    (0x00, 2) => ((a as i64) < (b as i64)) as u64,
                    (0x00, 3) => (a < b) as u64,
                    (0x00, 4) => a ^ b,
                    (0x00, 5) => a >> (b & 63),
                    (0x20, 5) => ((a as i64) >> (b & 63)) as u64,
                    (0x00, 6) => a | b,
                    (0x00, 7) => a & b,
                    // M extension
                    (0x01, 0) => a.wrapping_mul(b),
                    (0x01, 1) => (((a as i64 as i128) * (b as i64 as i128)) >> 64) as u64,
                    (0x01, 2) => (((a as i64 as i128) * (b as u128 as i128)) >> 64) as u64,
                    (0x01, 3) => (((a as u128) * (b as u128)) >> 64) as u64,
                    (0x01, 4) => div(a, b),
                    (0x01, 5) => divu(a, b),
                    (0x01, 6) => rem(a, b),
                    (0x01, 7) => remu(a, b),
                    _ => return illegal(inst),
                };
                self.x[rd(inst)] = v;
            }
            // OP-32
            0x3b => {
                let (a, b) = (x[rs1(inst)], x[rs2(inst)]);
                let (a32, b32) = (a as u32, b as u32);
                let v = match (funct7(inst), funct3(inst)) {
                    (0x00, 0) => sext32(a32.wrapping_add(b32) as u64),
                    (0x20, 0) => sext32(a32.wrapping_sub(b32) as u64),
                    (0x00, 1) => sext32((a32 << (b & 31)) as u64),
                    (0x00, 5) => sext32((a32 >> (b & 31)) as u64),
                    (0x20, 5) => ((a32 as i32) >> (b & 31)) as i64 as u64,
                    (0x01, 0) => sext32(a32.wrapping_mul(b32) as u64),
                    (0x01, 4) => {
                        let (a, b) = (a32 as i32, b32 as i32);
                        (if b == 0 { -1 } else { a.wrapping_div(b) }) as i64 as u64
                    }
                    (0x01, 5) => sext32((if b32 == 0 { u32::MAX } else { a32 / b32 }) as u64),
                    (0x01, 6) => {
                        let (a, b) = (a32 as i32, b32 as i32);
                        (if b == 0 { a } else { a.wrapping_rem(b) }) as i64 as u64
                    }
                    (0x01, 7) => sext32((if b32 == 0 { a32 } else { a32 % b32 }) as u64),
                    _ => return illegal(inst),
                };
                self.x[rd(inst)] = v;
            }
            // MISC-MEM: FENCE / FENCE.I (no caches to maintain)
            0x0f => match funct3(inst) {
                0 | 1 => {}
                _ => return illegal(inst),
            },
            // AMO
            0x2f => return self.exec_amo(inst),
            // SYSTEM
            0x73 => return self.exec_system(inst, pc),
            _ => return illegal(inst),
        }
        Ok(())
    }

    fn exec_amo(&mut self, inst: u32) -> Result<(), Trap> {
        let size = match funct3(inst) {
            2 => 4,
            3 => 8,
            _ => return illegal(inst),
        };
        let addr = self.x[rs1(inst)];
        let src = self.x[rs2(inst)];
        let op = inst >> 27;
        let sx = |v: u64| if size == 4 { sext32(v) } else { v };
        match op {
            // LR
            0x02 => {
                if rs2(inst) != 0 {
                    return illegal(inst);
                }
                let pa = self.atomic_addr(addr, size, Access::Load)?;
                let v = self.phys_rw(pa, size);
                self.reservation = Some(pa);
                self.x[rd(inst)] = sx(v);
            }
            // SC
            0x03 => {
                let pa = self.atomic_addr(addr, size, Access::Store)?;
                let ok = self.reservation == Some(pa);
                self.reservation = None;
                if ok {
                    self.phys_store_atomic(pa, size, src);
                }
                self.x[rd(inst)] = (!ok) as u64;
            }
            _ => {
                let pa = self.atomic_addr(addr, size, Access::Store)?;
                let old = sx(self.phys_rw(pa, size));
                let s = sx(src);
                let new = match op {
                    0x01 => s,                    // AMOSWAP
                    0x00 => old.wrapping_add(s),  // AMOADD
                    0x04 => old ^ s,              // AMOXOR
                    0x0c => old & s,              // AMOAND
                    0x08 => old | s,              // AMOOR
                    0x10 => (old as i64).min(s as i64) as u64, // AMOMIN
                    0x14 => (old as i64).max(s as i64) as u64, // AMOMAX
                    0x18 => if size == 4 { (old as u32).min(s as u32) as u64 } else { old.min(s) }, // AMOMINU
                    0x1c => if size == 4 { (old as u32).max(s as u32) as u64 } else { old.max(s) }, // AMOMAXU
                    _ => return illegal(inst),
                };
                self.phys_store_atomic(pa, size, new);
                self.x[rd(inst)] = old;
            }
        }
        Ok(())
    }

    fn exec_system(&mut self, inst: u32, pc: u64) -> Result<(), Trap> {
        let f3 = funct3(inst);
        if f3 == 0 {
            if rd(inst) != 0 {
                return illegal(inst);
            }
            if funct7(inst) == 0x09 {
                // SFENCE.VMA
                if self.prv == PRV_U || (self.prv == PRV_S && self.mstatus & MSTATUS_TVM != 0) {
                    return illegal(inst);
                }
                self.sfence_vma();
                return Ok(());
            }
            if rs1(inst) != 0 {
                return illegal(inst);
            }
            return match inst >> 20 {
                0x000 => Err(Trap::new(
                    match self.prv {
                        PRV_U => cause::ECALL_U,
                        PRV_S => cause::ECALL_S,
                        _ => cause::ECALL_M,
                    },
                    0,
                )),
                0x001 => Err(Trap::new(cause::BREAKPOINT, pc)),
                0x102 => {
                    // SRET
                    if self.prv == PRV_U || (self.prv == PRV_S && self.mstatus & MSTATUS_TSR != 0) {
                        return illegal(inst);
                    }
                    self.do_sret();
                    Ok(())
                }
                0x302 => {
                    if self.prv != PRV_M {
                        return illegal(inst);
                    }
                    self.do_mret();
                    Ok(())
                }
                0x105 => {
                    // WFI
                    if self.prv == PRV_U || (self.prv == PRV_S && self.mstatus & MSTATUS_TW != 0) {
                        return illegal(inst);
                    }
                    if self.mip() & self.mie == 0 {
                        self.wfi = true;
                    }
                    self.break_loop = true;
                    Ok(())
                }
                _ => illegal(inst),
            };
        }
        if f3 == 4 {
            return illegal(inst);
        }
        // Zicsr
        let addr = (inst >> 20) as u16;
        let rs1i = rs1(inst);
        let src = if f3 & 4 != 0 { rs1i as u64 } else { self.x[rs1i] };
        let op = f3 & 3;
        let writes = op == 1 || rs1i != 0;
        if !self.csr_allowed(addr, writes) {
            return illegal(inst);
        }
        let old = match self.csr_read(addr) {
            Some(v) => v,
            None => return illegal(inst),
        };
        if writes {
            // Read-modify-write of mip/sip acts on the software-writable bits
            // only, so hardware-driven pending bits never get latched.
            let base = match addr {
                MIP => self.mip_sw,
                SIP => self.mip_sw & self.mideleg,
                _ => old,
            };
            let new = match op {
                1 => src,
                2 => base | src,
                _ => base & !src,
            };
            if self.csr_write(addr, new).is_none() {
                return illegal(inst);
            }
        }
        self.x[rd(inst)] = old;
        Ok(())
    }
}
