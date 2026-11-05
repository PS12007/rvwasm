//! A compact RV64IMA + Zicsr disassembler for the debug panel and traces.

use crate::csr;
use crate::exec::{imm_b, imm_i, imm_j, imm_s};

pub const REG_NAMES: [&str; 32] = [
    "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7",
    "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4", "t5", "t6",
];

fn r(n: u32) -> &'static str {
    REG_NAMES[(n & 31) as usize]
}

/// Disassemble one instruction located at `pc`.
pub fn disasm(inst: u32, pc: u64) -> String {
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
            if rd == 0 { format!("j {:x}", t) } else { format!("jal {}, {:x}", r(rd), t) }
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
        0x0f => if f3 == 1 { "fence.i".into() } else { "fence".into() },
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
    }
}
