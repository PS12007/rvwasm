//! The F and D extensions: floating-point registers, fcsr, and the
//! instructions, with the arithmetic in `softfloat`.
//!
//! Single-precision values live NaN-boxed in the 64-bit f registers (upper
//! 32 bits all ones); an operand that isn't properly boxed reads as the
//! canonical NaN. mstatus.FS tracks whether the state is in use: with FS off
//! every FP instruction and fcsr access is illegal, and any change to the f
//! registers or flags marks it dirty so that a kernel knows to save them.

use crate::cpu::{cause, Cpu, Trap};
use crate::csr::MSTATUS_FS;
use crate::exec::{imm_i, imm_s};
use crate::softfloat::{self as sf, Fmt, D, S};

const BOX: u64 = 0xffff_ffff_0000_0000;

fn illegal(inst: u32) -> Result<(), Trap> {
    Err(Trap::new(cause::ILLEGAL_INST, inst as u64))
}

impl Cpu {
    #[inline]
    pub(crate) fn fp_enabled(&self) -> bool {
        self.mstatus & MSTATUS_FS != 0
    }

    #[inline]
    fn fp_dirty(&mut self) {
        self.mstatus |= MSTATUS_FS;
    }

    /// Read f[r] as an operand of format `f`.
    #[inline]
    fn freg(&self, f: Fmt, r: usize) -> u64 {
        let v = self.f[r];
        if f == D {
            v
        } else if v & BOX == BOX {
            v & 0xffff_ffff
        } else {
            S.qnan()
        }
    }

    #[inline]
    fn set_freg(&mut self, f: Fmt, r: usize, v: u64) {
        self.f[r] = if f == D { v } else { v | BOX };
        self.fp_dirty();
    }

    #[inline]
    fn raise(&mut self, flags: u8) {
        if flags != 0 {
            self.fflags |= flags;
            self.fp_dirty();
        }
    }

    /// The rounding mode an instruction uses: its rm field, or frm when that
    /// is 7 (dynamic). The reserved encodings make the instruction illegal.
    fn rounding(&self, inst: u32) -> Option<u8> {
        let rm = ((inst >> 12) & 7) as u8;
        let rm = if rm == 7 { self.frm } else { rm };
        (rm <= sf::RMM).then_some(rm)
    }

    #[inline(never)]
    pub(crate) fn exec_fp(&mut self, inst: u32) -> Result<(), Trap> {
        if !self.fp_enabled() {
            return illegal(inst);
        }
        let rd = ((inst >> 7) & 31) as usize;
        let rs1 = ((inst >> 15) & 31) as usize;
        let rs2 = ((inst >> 20) & 31) as usize;
        let f3 = (inst >> 12) & 7;
        match inst & 0x7f {
            // LOAD-FP: FLW, FLD
            0x07 => {
                let addr = self.x[rs1].wrapping_add(imm_i(inst));
                match f3 {
                    2 => {
                        let v = self.load(addr, 4)?;
                        self.set_freg(S, rd, v);
                    }
                    3 => {
                        let v = self.load(addr, 8)?;
                        self.set_freg(D, rd, v);
                    }
                    _ => return illegal(inst),
                }
            }
            // STORE-FP: FSW, FSD (the raw bits, boxed or not)
            0x27 => {
                let addr = self.x[rs1].wrapping_add(imm_s(inst));
                match f3 {
                    2 => self.store(addr, 4, self.f[rs2])?,
                    3 => self.store(addr, 8, self.f[rs2])?,
                    _ => return illegal(inst),
                }
            }
            // FMADD, FMSUB, FNMSUB, FNMADD
            op @ (0x43 | 0x47 | 0x4b | 0x4f) => {
                let (Some(f), Some(rm)) = (fmt(inst), self.rounding(inst)) else {
                    return illegal(inst);
                };
                let (a, b, c) = (self.freg(f, rs1), self.freg(f, rs2), self.freg(f, (inst >> 27) as usize));
                let mut fl = 0;
                let v = sf::fma(f, a, b, c, op >= 0x4b, op == 0x47 || op == 0x4f, rm, &mut fl);
                self.set_freg(f, rd, v);
                self.raise(fl);
            }
            // OP-FP
            0x53 => return self.exec_op_fp(inst, rd, rs1, rs2, f3),
            _ => return illegal(inst),
        }
        Ok(())
    }

    fn exec_op_fp(&mut self, inst: u32, rd: usize, rs1: usize, rs2: usize, f3: u32) -> Result<(), Trap> {
        let Some(f) = fmt(inst) else {
            return illegal(inst);
        };
        let mut fl = 0;
        let rm = self.rounding(inst);
        let (a, b) = (self.freg(f, rs1), self.freg(f, rs2));
        match inst >> 27 {
            // FADD, FSUB, FMUL, FDIV
            op @ 0x00..=0x03 => {
                let Some(rm) = rm else { return illegal(inst) };
                let v = match op {
                    0 => sf::add(f, a, b, rm, &mut fl),
                    1 => sf::sub(f, a, b, rm, &mut fl),
                    2 => sf::mul(f, a, b, rm, &mut fl),
                    _ => sf::div(f, a, b, rm, &mut fl),
                };
                self.set_freg(f, rd, v);
            }
            // FSQRT
            0x0b => {
                let (Some(rm), 0) = (rm, rs2) else { return illegal(inst) };
                let v = sf::sqrt(f, a, rm, &mut fl);
                self.set_freg(f, rd, v);
            }
            // FSGNJ, FSGNJN, FSGNJX
            0x04 if f3 <= 2 => self.set_freg(f, rd, sf::sign_inject(f, a, b, f3)),
            // FMIN, FMAX
            0x05 if f3 <= 1 => {
                let v = sf::min_max(f, a, b, f3 == 1, &mut fl);
                self.set_freg(f, rd, v);
            }
            // FCVT.S.D, FCVT.D.S
            0x08 => {
                let from = match (f == S, rs2) {
                    (true, 1) => D,
                    (false, 0) => S,
                    _ => return illegal(inst),
                };
                let Some(rm) = rm else { return illegal(inst) };
                let v = sf::convert(from, f, self.freg(from, rs1), rm, &mut fl);
                self.set_freg(f, rd, v);
            }
            // FLE, FLT, FEQ
            0x14 if f3 <= 2 => {
                self.x[rd] = match f3 {
                    2 => sf::eq(f, a, b, &mut fl),
                    1 => sf::lt(f, a, b, false, &mut fl),
                    _ => sf::lt(f, a, b, true, &mut fl),
                } as u64;
            }
            // FCVT.W, WU, L, LU from float
            0x18 if rs2 <= 3 => {
                let Some(rm) = rm else { return illegal(inst) };
                self.x[rd] = sf::to_int(f, a, rs2 & 1 == 0, if rs2 < 2 { 32 } else { 64 }, rm, &mut fl);
            }
            // FCVT to float from W, WU, L, LU
            0x1a if rs2 <= 3 => {
                let Some(rm) = rm else { return illegal(inst) };
                let v = sf::from_int(f, self.x[rs1], rs2 & 1 == 0, if rs2 < 2 { 32 } else { 64 }, rm, &mut fl);
                self.set_freg(f, rd, v);
            }
            // FMV.X.W / FMV.X.D (raw bits, sign-extended), FCLASS
            0x1c if rs2 == 0 && f3 <= 1 => {
                self.x[rd] = match (f3, f == S) {
                    (0, true) => self.f[rs1] as i32 as i64 as u64,
                    (0, false) => self.f[rs1],
                    _ => sf::class(f, a),
                };
            }
            // FMV.W.X / FMV.D.X
            0x1e if rs2 == 0 && f3 == 0 => {
                self.set_freg(f, rd, if f == S { self.x[rs1] & 0xffff_ffff } else { self.x[rs1] })
            }
            _ => return illegal(inst),
        }
        self.raise(fl);
        Ok(())
    }
}

/// The format field: S or D (H and Q are not implemented).
fn fmt(inst: u32) -> Option<Fmt> {
    match (inst >> 25) & 3 {
        0 => Some(S),
        1 => Some(D),
        _ => None,
    }
}
