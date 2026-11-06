//! A compact RV64GC disassembler for the debug panel and traces.

use crate::csr;
use crate::exec::{imm_b, imm_i, imm_j, imm_s};

pub const REG_NAMES: [&str; 32] = [
    "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7", "s2",
    "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4", "t5", "t6",
];

pub const FREG_NAMES: [&str; 32] = [
    "ft0", "ft1", "ft2", "ft3", "ft4", "ft5", "ft6", "ft7", "fs0", "fs1", "fa0", "fa1", "fa2", "fa3", "fa4", "fa5",
    "fa6", "fa7", "fs2", "fs3", "fs4", "fs5", "fs6", "fs7", "fs8", "fs9", "fs10", "fs11", "ft8", "ft9", "ft10", "ft11",
];

fn r(n: u32) -> &'static str {
    REG_NAMES[(n & 31) as usize]
}

fn fr(n: u32) -> &'static str {
    FREG_NAMES[(n & 31) as usize]
}

/// The F/D instructions. A static rounding mode is shown after the operands.
fn disasm_fp(inst: u32) -> Option<String> {
    let rd = (inst >> 7) & 31;
    let rs1 = (inst >> 15) & 31;
    let rs2 = (inst >> 20) & 31;
    let f3 = (inst >> 12) & 7;
    let w = match (inst >> 25) & 3 {
        0 => "s",
        1 => "d",
        _ => return None,
    };
    let rm = match f3 {
        7 => "",
        0 => ", rne",
        1 => ", rtz",
        2 => ", rdn",
        3 => ", rup",
        4 => ", rmm",
        _ => ", ?",
    };
    let ls = |f3: u32| match f3 {
        2 => Some("w"),
        3 => Some("d"),
        _ => None,
    };
    Some(match inst & 0x7f {
        0x07 => format!("fl{} {}, {}({})", ls(f3)?, fr(rd), imm_i(inst) as i64, r(rs1)),
        0x27 => format!("fs{} {}, {}({})", ls(f3)?, fr(rs2), imm_s(inst) as i64, r(rs1)),
        op @ (0x43 | 0x47 | 0x4b | 0x4f) => {
            let n = ["fmadd", "fmsub", "fnmsub", "fnmadd"][((op - 0x43) / 4) as usize];
            format!("{n}.{w} {}, {}, {}, {}{rm}", fr(rd), fr(rs1), fr(rs2), fr(inst >> 27))
        }
        0x53 => {
            let ints = ["w", "wu", "l", "lu"];
            match (inst >> 27, f3, rs2) {
                (op @ 0..=3, _, _) => {
                    let n = ["fadd", "fsub", "fmul", "fdiv"][op as usize];
                    format!("{n}.{w} {}, {}, {}{rm}", fr(rd), fr(rs1), fr(rs2))
                }
                (0x0b, _, 0) => format!("fsqrt.{w} {}, {}{rm}", fr(rd), fr(rs1)),
                (0x04, 0, _) if rs1 == rs2 => format!("fmv.{w} {}, {}", fr(rd), fr(rs1)),
                (0x04, 1, _) if rs1 == rs2 => format!("fneg.{w} {}, {}", fr(rd), fr(rs1)),
                (0x04, 2, _) if rs1 == rs2 => format!("fabs.{w} {}, {}", fr(rd), fr(rs1)),
                (0x04, 0..=2, _) => {
                    format!("{}.{w} {}, {}, {}", ["fsgnj", "fsgnjn", "fsgnjx"][f3 as usize], fr(rd), fr(rs1), fr(rs2))
                }
                (0x05, 0..=1, _) => {
                    format!("{}.{w} {}, {}, {}", ["fmin", "fmax"][f3 as usize], fr(rd), fr(rs1), fr(rs2))
                }
                (0x08, _, 0..=1) => format!("fcvt.{w}.{} {}, {}{rm}", ["s", "d"][rs2 as usize], fr(rd), fr(rs1)),
                (0x14, 0..=2, _) => {
                    format!("{}.{w} {}, {}, {}", ["fle", "flt", "feq"][f3 as usize], r(rd), fr(rs1), fr(rs2))
                }
                (0x18, _, 0..=3) => format!("fcvt.{}.{w} {}, {}{rm}", ints[rs2 as usize], r(rd), fr(rs1)),
                (0x1a, _, 0..=3) => format!("fcvt.{w}.{} {}, {}{rm}", ints[rs2 as usize], fr(rd), r(rs1)),
                (0x1c, 0, 0) => format!("fmv.x.{} {}, {}", if w == "s" { "w" } else { "d" }, r(rd), fr(rs1)),
                (0x1c, 1, 0) => format!("fclass.{w} {}, {}", r(rd), fr(rs1)),
                (0x1e, 0, 0) => format!("fmv.{}.x {}, {}", if w == "s" { "w" } else { "d" }, fr(rd), r(rs1)),
                _ => return None,
            }
        }
        _ => return None,
    })
}

/// Size in bytes of the instruction whose first 16 bits are `inst`.
pub fn inst_len(inst: u32) -> u64 {
    if inst & 3 == 3 {
        4
    } else {
        2
    }
}

/// Disassemble one instruction located at `pc`. A 16-bit instruction (low
/// two bits not `11`) is shown as the 32-bit instruction it expands to.
pub fn disasm(inst: u32, pc: u64) -> String {
    if inst & 3 != 3 {
        return match crate::rvc::expand(inst as u16) {
            Some(e) => disasm(e, pc),
            None => format!("unknown 0x{:04x}", inst & 0xffff),
        };
    }
    let rd = (inst >> 7) & 31;
    let rs1 = (inst >> 15) & 31;
    let rs2 = (inst >> 20) & 31;
    let f3 = (inst >> 12) & 7;
    let f7 = inst >> 25;
    let ii = imm_i(inst) as i64;
    match inst & 0x7f {
        0x37 => format!("lui {}, 0x{:x}", r(rd), (inst >> 12)),
        0x17 => format!("auipc {}, 0x{:x}", r(rd), (inst >> 12)),
        0x6f => {
            let t = pc.wrapping_add(imm_j(inst));
            if rd == 0 {
                format!("j {:x}", t)
            } else {
                format!("jal {}, {:x}", r(rd), t)
            }
        }
        0x67 => {
            if rd == 0 && rs1 == 1 && ii == 0 {
                "ret".into()
            } else {
                format!("jalr {}, {}({})", r(rd), ii, r(rs1))
            }
        }
        0x63 => {
            let n = ["beq", "bne", "?", "?", "blt", "bge", "bltu", "bgeu"][f3 as usize];
            format!("{} {}, {}, {:x}", n, r(rs1), r(rs2), pc.wrapping_add(imm_b(inst)))
        }
        0x03 => {
            let n = ["lb", "lh", "lw", "ld", "lbu", "lhu", "lwu", "?"][f3 as usize];
            format!("{} {}, {}({})", n, r(rd), ii, r(rs1))
        }
        0x23 => {
            let n = ["sb", "sh", "sw", "sd", "?", "?", "?", "?"][f3 as usize];
            format!("{} {}, {}({})", n, r(rs2), imm_s(inst) as i64, r(rs1))
        }
        0x13 => {
            if inst == 0x13 {
                return "nop".into();
            }
            let sh = (inst >> 20) & 0x3f;
            match f3 {
                0 if rs1 == 0 => format!("li {}, {}", r(rd), ii),
                0 if ii == 0 => format!("mv {}, {}", r(rd), r(rs1)),
                1 => format!("slli {}, {}, {}", r(rd), r(rs1), sh),
                5 => format!("{} {}, {}, {}", if inst >> 30 & 1 == 1 { "srai" } else { "srli" }, r(rd), r(rs1), sh),
                _ => {
                    let n = ["addi", "", "slti", "sltiu", "xori", "", "ori", "andi"][f3 as usize];
                    format!("{} {}, {}, {}", n, r(rd), r(rs1), ii)
                }
            }
        }
        0x1b => {
            let sh = (inst >> 20) & 0x1f;
            match f3 {
                0 if ii == 0 => format!("sext.w {}, {}", r(rd), r(rs1)),
                0 => format!("addiw {}, {}, {}", r(rd), r(rs1), ii),
                1 => format!("slliw {}, {}, {}", r(rd), r(rs1), sh),
                5 => format!("{} {}, {}, {}", if f7 == 0x20 { "sraiw" } else { "srliw" }, r(rd), r(rs1), sh),
                _ => format!("unknown 0x{:08x}", inst),
            }
        }
        0x33 | 0x3b => {
            let w = if inst & 0x7f == 0x3b { "w" } else { "" };
            if (f7, f3, rs1, w) == (0, 0, 0, "") {
                return format!("mv {}, {}", r(rd), r(rs2));
            }
            let n = match (f7, f3) {
                (0, 0) => "add",
                (0x20, 0) => "sub",
                (0, 1) => "sll",
                (0, 2) => "slt",
                (0, 3) => "sltu",
                (0, 4) => "xor",
                (0, 5) => "srl",
                (0x20, 5) => "sra",
                (0, 6) => "or",
                (0, 7) => "and",
                (1, 0) => "mul",
                (1, 1) => "mulh",
                (1, 2) => "mulhsu",
                (1, 3) => "mulhu",
                (1, 4) => "div",
                (1, 5) => "divu",
                (1, 6) => "rem",
                (1, 7) => "remu",
                _ => return format!("unknown 0x{:08x}", inst),
            };
            format!("{}{} {}, {}, {}", n, w, r(rd), r(rs1), r(rs2))
        }
        0x0f => {
            if f3 == 1 {
                "fence.i".into()
            } else {
                "fence".into()
            }
        }
        0x2f => {
            let sz = if f3 == 2 { ".w" } else { ".d" };
            let n = match inst >> 27 {
                0x02 => return format!("lr{} {}, ({})", sz, r(rd), r(rs1)),
                0x03 => "sc",
                0x01 => "amoswap",
                0x00 => "amoadd",
                0x04 => "amoxor",
                0x0c => "amoand",
                0x08 => "amoor",
                0x10 => "amomin",
                0x14 => "amomax",
                0x18 => "amominu",
                0x1c => "amomaxu",
                _ => return format!("unknown 0x{:08x}", inst),
            };
            format!("{}{} {}, {}, ({})", n, sz, r(rd), r(rs2), r(rs1))
        }
        0x73 => {
            if f3 == 0 {
                return match inst {
                    0x0000_0073 => "ecall".into(),
                    0x0010_0073 => "ebreak".into(),
                    0x1020_0073 => "sret".into(),
                    0x3020_0073 => "mret".into(),
                    0x1050_0073 => "wfi".into(),
                    _ if f7 == 0x09 => format!("sfence.vma {}, {}", r(rs1), r(rs2)),
                    _ => format!("unknown 0x{:08x}", inst),
                };
            }
            let addr = (inst >> 20) as u16;
            let name = csr::name(addr).map(String::from).unwrap_or_else(|| format!("0x{:03x}", addr));
            let n = ["", "csrrw", "csrrs", "csrrc", "", "csrrwi", "csrrsi", "csrrci"][f3 as usize];
            if f3 >= 5 {
                format!("{} {}, {}, {}", n, r(rd), name, rs1)
            } else {
                format!("{} {}, {}, {}", n, r(rd), name, r(rs1))
            }
        }
        0x07 | 0x27 | 0x43 | 0x47 | 0x4b | 0x4f | 0x53 => {
            disasm_fp(inst).unwrap_or_else(|| format!("unknown 0x{:08x}", inst))
        }
        _ => format!("unknown 0x{:08x}", inst),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn basics() {
        assert_eq!(disasm(0x00000013, 0), "nop");
        assert_eq!(disasm(0x00008067, 0), "ret");
        assert_eq!(disasm(0x30200073, 0), "mret");
        assert_eq!(disasm(0x02a5853b, 0), "mulw a0, a1, a0");
        assert_eq!(disasm(0x1141, 0), "addi sp, sp, -16");
        assert_eq!(disasm(0x0000, 0), "unknown 0x0000");
        assert_eq!(disasm(0x02b57553, 0), "fadd.d fa0, fa0, fa1");
        assert_eq!(disasm(0xc2051553, 0), "fcvt.w.d a0, fa0, rtz");
        assert_eq!(disasm(0x22a50553, 0), "fmv.d fa0, fa0");
        assert_eq!(disasm(0x2008, 0), "fld fa0, 0(s0)"); // c.fld
    }
}
