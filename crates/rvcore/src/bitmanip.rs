//! Bit manipulation (Zba, Zbb, Zbc, Zbs) and Zicond. These instructions live
//! in the encoding space that `exec` treats as illegal for the base ISA, so
//! the hot path only reaches this module after the base match falls through.

/// One bit-manipulation instruction. Register forms take `b = x[rs2]`;
/// immediate forms take `b = shamt`; unary forms ignore `b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    // Zba
    Sh1add,
    Sh2add,
    Sh3add,
    AddUw,
    Sh1addUw,
    Sh2addUw,
    Sh3addUw,
    SlliUw,
    // Zbb
    Andn,
    Orn,
    Xnor,
    Clz,
    Ctz,
    Cpop,
    Clzw,
    Ctzw,
    Cpopw,
    Min,
    Minu,
    Max,
    Maxu,
    SextB,
    SextH,
    ZextH,
    Rol,
    Ror,
    Rori,
    Rolw,
    Rorw,
    Roriw,
    OrcB,
    Rev8,
    // Zbc
    Clmul,
    Clmulh,
    Clmulr,
    // Zbs
    Bclr,
    Bext,
    Binv,
    Bset,
    Bclri,
    Bexti,
    Binvi,
    Bseti,
    // Zicond
    CzeroEqz,
    CzeroNez,
}

/// How the disassembler prints the operands.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Reg,
    Imm,
    Unary,
}

/// Decode a 32-bit instruction, or None if it is not one of ours.
pub fn decode(inst: u32) -> Option<Op> {
    use Op::*;
    let f3 = (inst >> 12) & 7;
    let f7 = inst >> 25;
    let f6 = inst >> 26;
    let imm12 = inst >> 20;
    let rs2 = (inst >> 20) & 31;
    Some(match inst & 0x7f {
        0x33 => match (f7, f3) {
            (0x10, 2) => Sh1add,
            (0x10, 4) => Sh2add,
            (0x10, 6) => Sh3add,
            (0x20, 7) => Andn,
            (0x20, 6) => Orn,
            (0x20, 4) => Xnor,
            (0x05, 1) => Clmul,
            (0x05, 2) => Clmulr,
            (0x05, 3) => Clmulh,
            (0x05, 4) => Min,
            (0x05, 5) => Minu,
            (0x05, 6) => Max,
            (0x05, 7) => Maxu,
            (0x30, 1) => Rol,
            (0x30, 5) => Ror,
            (0x24, 1) => Bclr,
            (0x24, 5) => Bext,
            (0x34, 1) => Binv,
            (0x14, 1) => Bset,
            (0x07, 5) => CzeroEqz,
            (0x07, 7) => CzeroNez,
            _ => return None,
        },
        0x3b => match (f7, f3) {
            (0x04, 0) => AddUw,
            (0x04, 4) if rs2 == 0 => ZextH,
            (0x10, 2) => Sh1addUw,
            (0x10, 4) => Sh2addUw,
            (0x10, 6) => Sh3addUw,
            (0x30, 1) => Rolw,
            (0x30, 5) => Rorw,
            _ => return None,
        },
        0x13 => match (f3, imm12, f6) {
            (1, 0x600, _) => Clz,
            (1, 0x601, _) => Ctz,
            (1, 0x602, _) => Cpop,
            (1, 0x604, _) => SextB,
            (1, 0x605, _) => SextH,
            (1, _, 0x12) => Bclri,
            (1, _, 0x1a) => Binvi,
            (1, _, 0x0a) => Bseti,
            (5, 0x287, _) => OrcB,
            (5, 0x6b8, _) => Rev8,
            (5, _, 0x18) => Rori,
            (5, _, 0x12) => Bexti,
            _ => return None,
        },
        0x1b => match (f3, imm12, f7) {
            (1, 0x600, _) => Clzw,
            (1, 0x601, _) => Ctzw,
            (1, 0x602, _) => Cpopw,
            (1, _, 0x04 | 0x05) => SlliUw,
            (5, _, 0x30) => Roriw,
            _ => return None,
        },
        _ => return None,
    })
}

/// The second operand: x[rs2] for register forms, the shift amount for
/// immediate forms (6 bits, or 5 for RORIW).
pub fn operand_b(op: Op, inst: u32, x: &[u64; 32]) -> u64 {
    match op.form() {
        Form::Reg => x[((inst >> 20) & 31) as usize],
        Form::Imm if op == Op::Roriw => ((inst >> 20) & 31) as u64,
        Form::Imm => ((inst >> 20) & 63) as u64,
        Form::Unary => 0,
    }
}

fn clmul_full(a: u64, b: u64) -> u128 {
    let mut r = 0u128;
    for i in 0..64 {
        if b >> i & 1 != 0 {
            r ^= (a as u128) << i;
        }
    }
    r
}

#[inline]
fn sx32(v: u32) -> u64 {
    v as i32 as i64 as u64
}

pub fn eval(op: Op, a: u64, b: u64) -> u64 {
    use Op::*;
    let a32 = a as u32;
    let bit = 1u64 << (b & 63);
    match op {
        Sh1add => (a << 1).wrapping_add(b),
        Sh2add => (a << 2).wrapping_add(b),
        Sh3add => (a << 3).wrapping_add(b),
        AddUw => (a32 as u64).wrapping_add(b),
        Sh1addUw => ((a32 as u64) << 1).wrapping_add(b),
        Sh2addUw => ((a32 as u64) << 2).wrapping_add(b),
        Sh3addUw => ((a32 as u64) << 3).wrapping_add(b),
        SlliUw => (a32 as u64) << b,
        Andn => a & !b,
        Orn => a | !b,
        Xnor => !(a ^ b),
        Clz => a.leading_zeros() as u64,
        Ctz => a.trailing_zeros() as u64,
        Cpop => a.count_ones() as u64,
        Clzw => a32.leading_zeros() as u64,
        Ctzw => a32.trailing_zeros() as u64,
        Cpopw => a32.count_ones() as u64,
        Min => (a as i64).min(b as i64) as u64,
        Minu => a.min(b),
        Max => (a as i64).max(b as i64) as u64,
        Maxu => a.max(b),
        SextB => a as i8 as i64 as u64,
        SextH => a as i16 as i64 as u64,
        ZextH => a as u16 as u64,
        Rol => a.rotate_left((b & 63) as u32),
        Ror | Rori => a.rotate_right((b & 63) as u32),
        Rolw => sx32(a32.rotate_left((b & 31) as u32)),
        Rorw | Roriw => sx32(a32.rotate_right((b & 31) as u32)),
        OrcB => {
            let mut r = 0;
            for i in 0..8 {
                if (a >> (i * 8)) & 0xff != 0 {
                    r |= 0xff << (i * 8);
                }
            }
            r
        }
        Rev8 => a.swap_bytes(),
        Clmul => clmul_full(a, b) as u64,
        Clmulh => (clmul_full(a, b) >> 64) as u64,
        Clmulr => (clmul_full(a, b) >> 63) as u64,
        Bclr | Bclri => a & !bit,
        Bext | Bexti => (a >> (b & 63)) & 1,
        Binv | Binvi => a ^ bit,
        Bset | Bseti => a | bit,
        CzeroEqz => {
            if b == 0 {
                0
            } else {
                a
            }
        }
        CzeroNez => {
            if b != 0 {
                0
            } else {
                a
            }
        }
    }
}

impl Op {
    pub fn form(self) -> Form {
        use Op::*;
        match self {
            SlliUw | Rori | Roriw | Bclri | Bexti | Binvi | Bseti => Form::Imm,
            Clz | Ctz | Cpop | Clzw | Ctzw | Cpopw | SextB | SextH | ZextH | OrcB | Rev8 => Form::Unary,
            _ => Form::Reg,
        }
    }

    pub fn name(self) -> &'static str {
        use Op::*;
        match self {
            Sh1add => "sh1add",
            Sh2add => "sh2add",
            Sh3add => "sh3add",
            AddUw => "add.uw",
            Sh1addUw => "sh1add.uw",
            Sh2addUw => "sh2add.uw",
            Sh3addUw => "sh3add.uw",
            SlliUw => "slli.uw",
            Andn => "andn",
            Orn => "orn",
            Xnor => "xnor",
            Clz => "clz",
            Ctz => "ctz",
            Cpop => "cpop",
            Clzw => "clzw",
            Ctzw => "ctzw",
            Cpopw => "cpopw",
            Min => "min",
            Minu => "minu",
            Max => "max",
            Maxu => "maxu",
            SextB => "sext.b",
            SextH => "sext.h",
            ZextH => "zext.h",
            Rol => "rol",
            Ror => "ror",
            Rori => "rori",
            Rolw => "rolw",
            Rorw => "rorw",
            Roriw => "roriw",
            OrcB => "orc.b",
            Rev8 => "rev8",
            Clmul => "clmul",
            Clmulh => "clmulh",
            Clmulr => "clmulr",
            Bclr => "bclr",
            Bext => "bext",
            Binv => "binv",
            Bset => "bset",
            Bclri => "bclri",
            Bexti => "bexti",
            Binvi => "binvi",
            Bseti => "bseti",
            CzeroEqz => "czero.eqz",
            CzeroNez => "czero.nez",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(f7: u32, f3: u32, opc: u32) -> u32 {
        (f7 << 25) | (2 << 20) | (1 << 15) | (f3 << 12) | (3 << 7) | opc
    }
    fn i(imm12: u32, f3: u32, opc: u32) -> u32 {
        (imm12 << 20) | (1 << 15) | (f3 << 12) | (3 << 7) | opc
    }

    #[test]
    fn decodes_and_evaluates() {
        use Op::*;
        assert_eq!(decode(r(0x10, 2, 0x33)), Some(Sh1add));
        assert_eq!(decode(r(0x04, 0, 0x3b)), Some(AddUw));
        assert_eq!(decode(i(0x080, 4, 0x3b) & !(31 << 20)), Some(ZextH)); // zext.h: rs2 = 0
        assert_eq!(decode(i(0x6b8, 5, 0x13)), Some(Rev8));
        assert_eq!(decode(i(0x287, 5, 0x13)), Some(OrcB));
        assert_eq!(decode(i(0x600 | 7, 1, 0x13)), None); // reserved count op
        assert_eq!(decode(i(0x4a0, 1, 0x13)), Some(Bclri)); // bclri 32
        assert_eq!(decode(r(0x07, 7, 0x33)), Some(CzeroNez));
        assert_eq!(decode(r(0x00, 0, 0x33)), None); // plain add stays in exec
        assert_eq!(decode(r(0x30, 5, 0x1b)), Some(Roriw));
        assert_eq!(decode(r(0x31, 5, 0x1b)), None); // roriw with shamt bit 5

        assert_eq!(eval(OrcB, 0x0012_0000_3400_0001, 0), 0x00ff_0000_ff00_00ff);
        assert_eq!(eval(Rev8, 0x0102_0304_0506_0708, 0), 0x0807_0605_0403_0201);
        assert_eq!(eval(Clzw, 0xffff_ffff_0000_0001, 0), 31);
        assert_eq!(eval(Rorw, 0x1, 1), 0xffff_ffff_8000_0000);
        assert_eq!(eval(Clmul, 0b11, 0b11,), 0b101);
        assert_eq!(eval(Clmulh, 1 << 63, 1 << 63), 1 << 62);
        assert_eq!(eval(Clmulr, 1 << 63, 1 << 63), 1 << 63);
        assert_eq!(eval(SlliUw, 0xffff_ffff_8000_0000, 4), 0x8_0000_0000);
        assert_eq!(eval(Bexti, 1 << 40, 40), 1);
        assert_eq!(eval(CzeroEqz, 7, 0), 0);
        assert_eq!(eval(CzeroEqz, 7, 1), 7);
    }
}
