//! The C extension: each 16-bit instruction is expanded to the 32-bit
//! instruction it stands for, so execution has one implementation of each
//! operation. Encodings the spec reserves, and the F/D loads and stores
//! (this hart has no FPU), expand to `None` and raise an illegal-instruction
//! trap.

#[inline(always)]
fn bits(c: u32, hi: u32, lo: u32) -> u32 {
    (c >> lo) & ((1 << (hi - lo + 1)) - 1)
}

/// Sign-extend the low `n` bits of `v`.
#[inline(always)]
fn sext(v: u32, n: u32) -> i32 {
    ((v << (32 - n)) as i32) >> (32 - n)
}

/// A register in the x8-x15 window used by the CIW/CL/CS/CA/CB formats.
#[inline(always)]
fn creg(c: u32, lo: u32) -> u32 {
    8 + bits(c, lo + 2, lo)
}

fn i_type(op: u32, f3: u32, rd: u32, rs1: u32, imm: i32) -> u32 {
    ((imm as u32 & 0xfff) << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
}

fn s_type(op: u32, f3: u32, rs1: u32, rs2: u32, imm: u32) -> u32 {
    ((imm >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | ((imm & 0x1f) << 7) | op
}

fn r_type(op: u32, f7: u32, f3: u32, rd: u32, rs1: u32, rs2: u32) -> u32 {
    (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
}

fn b_type(f3: u32, rs1: u32, rs2: u32, imm: i32) -> u32 {
    let i = imm as u32;
    (((i >> 12) & 1) << 31)
        | (((i >> 5) & 0x3f) << 25)
        | (rs2 << 20)
        | (rs1 << 15)
        | (f3 << 12)
        | (((i >> 1) & 0xf) << 8)
        | (((i >> 11) & 1) << 7)
        | 0x63
}

fn j_type(rd: u32, imm: i32) -> u32 {
    let i = imm as u32;
    (((i >> 20) & 1) << 31)
        | (((i >> 1) & 0x3ff) << 21)
        | (((i >> 11) & 1) << 20)
        | (((i >> 12) & 0xff) << 12)
        | (rd << 7)
        | 0x6f
}

const OP_IMM: u32 = 0x13;
const OP_IMM_32: u32 = 0x1b;
const OP: u32 = 0x33;
const OP_32: u32 = 0x3b;
const LOAD: u32 = 0x03;
const STORE: u32 = 0x23;
const JALR: u32 = 0x67;

/// Expand a 16-bit instruction (low two bits not `11`) to its 32-bit form.
pub fn expand(c: u16) -> Option<u32> {
    let c = c as u32;
    let f3 = bits(c, 15, 13);
    let rd = bits(c, 11, 7);
    let rs2 = bits(c, 6, 2);
    // The 6-bit immediate of CI-format instructions: imm[5] = c[12], imm[4:0] = c[6:2].
    let ci = sext((bits(c, 12, 12) << 5) | rs2, 6);
    Some(match (c & 3, f3) {
        // ---- Quadrant 0
        (0, 0) => {
            // C.ADDI4SPN: nzuimm[5:4|9:6|2|3] = c[12:5]
            let imm = (bits(c, 12, 11) << 4) | (bits(c, 10, 7) << 6) | (bits(c, 6, 6) << 2) | (bits(c, 5, 5) << 3);
            if imm == 0 {
                return None; // includes the all-zero parcel, which is defined illegal
            }
            i_type(OP_IMM, 0, creg(c, 2), 2, imm as i32)
        }
        (0, 2) => {
            // C.LW: uimm[5:3] = c[12:10], uimm[2|6] = c[6:5]
            let imm = (bits(c, 12, 10) << 3) | (bits(c, 6, 6) << 2) | (bits(c, 5, 5) << 6);
            i_type(LOAD, 2, creg(c, 2), creg(c, 7), imm as i32)
        }
        (0, 3) => {
            // C.LD: uimm[5:3] = c[12:10], uimm[7:6] = c[6:5]
            let imm = (bits(c, 12, 10) << 3) | (bits(c, 6, 5) << 6);
            i_type(LOAD, 3, creg(c, 2), creg(c, 7), imm as i32)
        }
        (0, 6) => {
            let imm = (bits(c, 12, 10) << 3) | (bits(c, 6, 6) << 2) | (bits(c, 5, 5) << 6);
            s_type(STORE, 2, creg(c, 7), creg(c, 2), imm) // C.SW
        }
        (0, 7) => {
            let imm = (bits(c, 12, 10) << 3) | (bits(c, 6, 5) << 6);
            s_type(STORE, 3, creg(c, 7), creg(c, 2), imm) // C.SD
        }

        // ---- Quadrant 1
        (1, 0) => i_type(OP_IMM, 0, rd, rd, ci), // C.ADDI (C.NOP when rd = 0)
        (1, 1) => {
            if rd == 0 {
                return None;
            }
            i_type(OP_IMM_32, 0, rd, rd, ci) // C.ADDIW
        }
        (1, 2) => i_type(OP_IMM, 0, rd, 0, ci), // C.LI
        (1, 3) if rd == 2 => {
            // C.ADDI16SP: nzimm[9] = c[12], nzimm[4|6|8:7|5] = c[6:2]
            let imm = (bits(c, 12, 12) << 9)
                | (bits(c, 6, 6) << 4)
                | (bits(c, 5, 5) << 6)
                | (bits(c, 4, 3) << 7)
                | (bits(c, 2, 2) << 5);
            if imm == 0 {
                return None;
            }
            i_type(OP_IMM, 0, 2, 2, sext(imm, 10))
        }
        (1, 3) => {
            // C.LUI: nzimm[17] = c[12], nzimm[16:12] = c[6:2]
            if ci == 0 {
                return None;
            }
            ((ci as u32) << 12) | (rd << 7) | 0x37
        }
        (1, 4) => {
            let rd = creg(c, 7);
            let shamt = (bits(c, 12, 12) << 5) | rs2;
            match bits(c, 11, 10) {
                0 => i_type(OP_IMM, 5, rd, rd, shamt as i32),           // C.SRLI
                1 => i_type(OP_IMM, 5, rd, rd, (0x400 | shamt) as i32), // C.SRAI
                2 => i_type(OP_IMM, 7, rd, rd, ci),                     // C.ANDI
                _ => {
                    let rs2 = creg(c, 2);
                    match (bits(c, 12, 12), bits(c, 6, 5)) {
                        (0, 0) => r_type(OP, 0x20, 0, rd, rd, rs2),    // C.SUB
                        (0, 1) => r_type(OP, 0, 4, rd, rd, rs2),       // C.XOR
                        (0, 2) => r_type(OP, 0, 6, rd, rd, rs2),       // C.OR
                        (0, 3) => r_type(OP, 0, 7, rd, rd, rs2),       // C.AND
                        (1, 0) => r_type(OP_32, 0x20, 0, rd, rd, rs2), // C.SUBW
                        (1, 1) => r_type(OP_32, 0, 0, rd, rd, rs2),    // C.ADDW
                        _ => return None,
                    }
                }
            }
        }
        (1, 5) => {
            // C.J: offset[11|4|9:8|10|6|7|3:1|5] = c[12:2]
            let imm = (bits(c, 12, 12) << 11)
                | (bits(c, 11, 11) << 4)
                | (bits(c, 10, 9) << 8)
                | (bits(c, 8, 8) << 10)
                | (bits(c, 7, 7) << 6)
                | (bits(c, 6, 6) << 7)
                | (bits(c, 5, 3) << 1)
                | (bits(c, 2, 2) << 5);
            j_type(0, sext(imm, 12))
        }
        (1, 6) | (1, 7) => {
            // C.BEQZ / C.BNEZ: offset[8|4:3] = c[12:10], offset[7:6|2:1|5] = c[6:2]
            let imm = (bits(c, 12, 12) << 8)
                | (bits(c, 11, 10) << 3)
                | (bits(c, 6, 5) << 6)
                | (bits(c, 4, 3) << 1)
                | (bits(c, 2, 2) << 5);
            b_type(f3 - 6, creg(c, 7), 0, sext(imm, 9))
        }

        // ---- Quadrant 2
        (2, 0) => i_type(OP_IMM, 1, rd, rd, ((bits(c, 12, 12) << 5) | rs2) as i32), // C.SLLI
        (2, 2) => {
            // C.LWSP: uimm[5] = c[12], uimm[4:2|7:6] = c[6:2]
            if rd == 0 {
                return None;
            }
            let imm = (bits(c, 12, 12) << 5) | (bits(c, 6, 4) << 2) | (bits(c, 3, 2) << 6);
            i_type(LOAD, 2, rd, 2, imm as i32)
        }
        (2, 3) => {
            // C.LDSP: uimm[5] = c[12], uimm[4:3|8:6] = c[6:2]
            if rd == 0 {
                return None;
            }
            let imm = (bits(c, 12, 12) << 5) | (bits(c, 6, 5) << 3) | (bits(c, 4, 2) << 6);
            i_type(LOAD, 3, rd, 2, imm as i32)
        }
        (2, 4) => match (bits(c, 12, 12), rd, rs2) {
            (0, 0, 0) => return None,
            (0, _, 0) => i_type(JALR, 0, 0, rd, 0),    // C.JR
            (0, _, _) => r_type(OP, 0, 0, rd, 0, rs2), // C.MV
            (1, 0, 0) => 0x0010_0073,                  // C.EBREAK
            (1, _, 0) => i_type(JALR, 0, 1, rd, 0),    // C.JALR
            _ => r_type(OP, 0, 0, rd, rd, rs2),        // C.ADD
        },
        (2, 6) => {
            // C.SWSP: uimm[5:2|7:6] = c[12:7]
            let imm = (bits(c, 12, 9) << 2) | (bits(c, 8, 7) << 6);
            s_type(STORE, 2, 2, rs2, imm)
        }
        (2, 7) => {
            // C.SDSP: uimm[5:3|8:6] = c[12:7]
            let imm = (bits(c, 12, 10) << 3) | (bits(c, 9, 7) << 6);
            s_type(STORE, 3, 2, rs2, imm)
        }
        // C.FLD, C.FSD, C.FLDSP, C.FSDSP, and quadrant 0 funct3 = 100.
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::expand;
    use crate::disasm::disasm;

    fn d(c: u16) -> String {
        disasm(expand(c).expect("valid encoding"), 0x1000)
    }

    // Expected text is the 32-bit equivalent of what GNU objdump prints for
    // each encoding.
    #[test]
    fn known_encodings() {
        assert_eq!(d(0x1141), "addi sp, sp, -16"); // c.addi sp,-16
        assert_eq!(d(0xe406), "sd ra, 8(sp)"); // c.sdsp ra,8(sp)
        assert_eq!(d(0x60a2), "ld ra, 8(sp)"); // c.ldsp ra,8(sp)
        assert_eq!(d(0x0141), "addi sp, sp, 16"); // c.addi sp,16
        assert_eq!(d(0x8082), "ret"); // c.jr ra
        assert_eq!(d(0x0800), "addi s0, sp, 16"); // c.addi4spn s0,sp,16
        assert_eq!(d(0x4501), "li a0, 0"); // c.li a0,0
        assert_eq!(d(0x852e), "mv a0, a1"); // c.mv a0,a1 (add a0, zero, a1)
        assert_eq!(d(0x9532), "add a0, a0, a2"); // c.add a0,a2
        assert_eq!(d(0x8d0d), "sub a0, a0, a1"); // c.sub a0,a1
        assert_eq!(d(0x9d2d), "addw a0, a0, a1"); // c.addw a0,a1
        assert_eq!(d(0x2505), "addiw a0, a0, 1"); // c.addiw a0,1
        assert_eq!(d(0x6785), "lui a5, 0x1"); // c.lui a5,0x1
        assert_eq!(d(0x050e), "slli a0, a0, 3"); // c.slli a0,3
        assert_eq!(d(0x8105), "srli a0, a0, 1"); // c.srli a0,1
        assert_eq!(d(0x8505), "srai a0, a0, 1"); // c.srai a0,1
        assert_eq!(d(0x8905), "andi a0, a0, 1"); // c.andi a0,1
        assert_eq!(d(0x4108), "lw a0, 0(a0)"); // c.lw a0,0(a0)
        assert_eq!(d(0x6508), "ld a0, 8(a0)"); // c.ld a0,8(a0)
        assert_eq!(d(0xc10c), "sw a1, 0(a0)"); // c.sw a1,0(a0)
        assert_eq!(d(0xe50c), "sd a1, 8(a0)"); // c.sd a1,8(a0)
        assert_eq!(d(0x9002), "ebreak");
        assert_eq!(d(0x9502), "jalr ra, 0(a0)"); // c.jalr a0
        assert_eq!(d(0xa001), "j 1000"); // c.j . (offset 0)
        assert_eq!(d(0xbffd), "j ffe"); // c.j .-2
        assert_eq!(d(0xc111), "beq a0, zero, 1004"); // c.beqz a0,.+4
        assert_eq!(d(0xfd75), "bne a0, zero, ffc"); // c.bnez a0,.-4
        assert_eq!(d(0x7179), "addi sp, sp, -48"); // c.addi16sp sp,-48
    }

    #[test]
    fn reserved_and_fp() {
        assert_eq!(expand(0x0000), None); // all-zero parcel
        assert_eq!(expand(0x2008), None); // c.fld
        assert_eq!(expand(0x6101), None); // c.addi16sp with zero immediate
        assert_eq!(expand(0x8002), None); // c.jr x0
        assert_eq!(expand(0x2001), None); // c.addiw x0
    }
}
