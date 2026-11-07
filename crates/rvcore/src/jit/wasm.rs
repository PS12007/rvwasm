//! Translates blocks of RISC-V instructions into a WebAssembly module, one
//! exported function per block, which the browser compiles to native code.
//!
//! A block function has type `(cpu: i32) -> i32`. It loads the guest
//! registers it uses from the `Cpu` in linear memory into locals, runs, writes
//! back the registers it changed and the next pc, and returns how many
//! instructions it retired.
//!
//! The common case of a load or store, an aligned access to RAM through a
//! TLB hit, runs inline: compiled code reads the TLB entry, the RAM bytes and
//! (for stores) the journal's page bitmaps in place. Anything else calls a
//! Rust helper, which does translation, permission checks and journaling
//! exactly as the interpreter does. If that needs the interpreter instead (a
//! fault, MMIO, a misaligned access, a write to a page with compiled code)
//! the helper sets `cpu.jit_bail` and does nothing; the block then stops in
//! front of that instruction, with the pc pointing at it, and the
//! interpreter runs it.
//!
//! Conditional branches do not end a block: a taken branch leaves it, and a
//! branch back to the block's start runs it again as a loop until the
//! instruction budget (`cpu.jit_budget`) would be exceeded, so a hot loop
//! never leaves WebAssembly between iterations.

use crate::bitmanip::{self, Form};
use crate::exec::{imm_b, imm_i, imm_j, imm_s, imm_u};

/// Where things are: field offsets in `Cpu`, addresses of the structures
/// compiled code reads in place, and table indices of the Rust helpers.
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub x: u32,
    pub pc: u32,
    pub bail: u32,
    pub budget: u32,
    pub load_fn: u32,
    pub store_fn: u32,
    pub alu_fn: u32,
    /// Offsets of `cpu.vm_data` (bool), `cpu.tlb.gen` (u64), `cpu.jit_resv` (u64).
    pub vm_data: u32,
    pub tlb_gen: u32,
    pub resv: u32,
    /// Addresses: the TLB's tables (3 x 256 entries of 24 bytes), RAM, and
    /// the journal's saved-page and code-page bitmaps.
    pub tlb: u32,
    pub ram: u32,
    pub ram_len: u64,
    pub saved_bits: u32,
    pub code_bits: u32,
    /// RAM page holding `tohost` (stores there go through the helper), or u32::MAX.
    pub tohost_page: u32,
}

/// A run of instructions starting at `pc`, each as (32-bit form, length in bytes).
pub(crate) struct Block {
    pub pc: u64,
    pub insts: Vec<(u32, u8)>,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub(crate) enum Kind {
    /// Runs inline or through a helper and falls through.
    Plain,
    /// A conditional branch: leaves the block if taken.
    Branch,
    /// A jump: ends the block.
    End,
    /// Not compiled: the block stops before it.
    No,
}

/// What the compiler does with a (32-bit form of an) instruction.
pub(crate) fn classify(inst: u32) -> Kind {
    let f3 = (inst >> 12) & 7;
    let f7 = inst >> 25;
    match inst & 0x7f {
        0x37 | 0x17 => Kind::Plain,
        0x13 => match f3 {
            1 if inst >> 26 != 0 => bm(inst),
            5 if !matches!(inst >> 26, 0x00 | 0x10) => bm(inst),
            _ => Kind::Plain,
        },
        0x1b => match (f3, f7) {
            (0, _) | (1, 0x00) | (5, 0x00) | (5, 0x20) => Kind::Plain,
            _ => bm(inst),
        },
        0x33 => match (f7, f3) {
            (0x00, _) | (0x20, 0) | (0x20, 5) | (0x01, _) => Kind::Plain,
            _ => bm(inst),
        },
        0x3b => match (f7, f3) {
            (0x00, 0 | 1 | 5) | (0x20, 0 | 5) | (0x01, 0 | 4 | 5 | 6 | 7) => Kind::Plain,
            _ => bm(inst),
        },
        0x03 if f3 != 7 => Kind::Plain,
        0x23 if f3 < 4 => Kind::Plain,
        0x0f if f3 < 2 => Kind::Plain,
        0x6f => Kind::End,
        0x67 if f3 == 0 => Kind::End,
        0x63 if f3 != 2 && f3 != 3 => Kind::Branch,
        _ => Kind::No,
    }
}

fn bm(inst: u32) -> Kind {
    if bitmanip::decode(inst).is_some() {
        Kind::Plain
    } else {
        Kind::No
    }
}

// Value types and opcodes.
const I32: u8 = 0x7f;
const I64: u8 = 0x7e;
const BLOCK: u8 = 0x02;
const LOOP: u8 = 0x03;
const IF: u8 = 0x04;
const ELSE: u8 = 0x05;
const END: u8 = 0x0b;
const BR: u8 = 0x0c;
const BR_IF: u8 = 0x0d;
const CALL_INDIRECT: u8 = 0x11;
const I32_LOAD: u8 = 0x28;
const I64_LOAD: u8 = 0x29;
const I32_LOAD8_U: u8 = 0x2d;
const I64_STORE: u8 = 0x37;
const I32_EQZ: u8 = 0x45;
const I32_EQ: u8 = 0x46;
const I32_LE_U: u8 = 0x4d;
const I64_EQ: u8 = 0x51;
const I64_NE: u8 = 0x52;
const I64_LT_S: u8 = 0x53;
const I64_LT_U: u8 = 0x54;
const I64_GT_U: u8 = 0x56;
const I64_GE_S: u8 = 0x59;
const I64_GE_U: u8 = 0x5a;
const I32_ADD: u8 = 0x6a;
const I32_MUL: u8 = 0x6c;
const I32_AND: u8 = 0x71;
const I32_SHR_U: u8 = 0x76;
const I64_ADD: u8 = 0x7c;
const I64_SUB: u8 = 0x7d;
const I64_MUL: u8 = 0x7e;
const I64_AND: u8 = 0x83;
const I64_OR: u8 = 0x84;
const I64_XOR: u8 = 0x85;
const I64_SHL: u8 = 0x86;
const I64_SHR_S: u8 = 0x87;
const I64_SHR_U: u8 = 0x88;
const I32_WRAP_I64: u8 = 0xa7;
const I64_EXTEND_I32_U: u8 = 0xad;
const I64_EXTEND32_S: u8 = 0xc4;
/// (opcode, log2 size) of i64.load8_s, load16_s, load32_s, load, load8_u,
/// load16_u, load32_u: indexed by the RISC-V load's funct3.
const LOADS: [(u8, u32); 7] = [(0x30, 0), (0x32, 1), (0x34, 2), (0x29, 3), (0x31, 0), (0x33, 1), (0x35, 2)];
/// i64.store8, store16, store32, store: indexed by the store's funct3.
const STORES: [(u8, u32); 4] = [(0x3c, 0), (0x3d, 1), (0x3e, 2), (0x37, 3)];

// Function types.
const T_BLOCK: u32 = 0;
const T_LOAD: u32 = 1;
const T_STORE: u32 = 2;
const T_ALU: u32 = 3;

// Locals: 0 is the cpu pointer, 1..=31 hold x1..x31, then i64 temporaries
// and i32 ones.
const TMP: u32 = 32;
const VA: u32 = 33;
const PA: u32 = 34;
const RET: u32 = 35;
const TOTAL: u32 = 36;
const BUDGET: u32 = 37;
const EA: u32 = 38;

fn uleb(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn sleb(out: &mut Vec<u8>, mut v: i64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0) {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn name(out: &mut Vec<u8>, s: &str) {
    uleb(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn section(out: &mut Vec<u8>, id: u8, body: &[u8]) {
    out.push(id);
    uleb(out, body.len() as u64);
    out.extend_from_slice(body);
}

/// Code for one function body.
struct Func<'a> {
    c: Vec<u8>,
    l: &'a Layout,
    /// Open blocks/loops/ifs; the exit block is at depth 1, the loop at 2.
    depth: u32,
}

impl Func<'_> {
    fn op(&mut self, b: u8) {
        self.c.push(b);
    }
    fn i64c(&mut self, v: u64) {
        self.c.push(0x42);
        sleb(&mut self.c, v as i64);
    }
    fn i32c(&mut self, v: u32) {
        self.c.push(0x41);
        sleb(&mut self.c, v as i32 as i64);
    }
    fn get(&mut self, l: u32) {
        self.c.push(0x20);
        uleb(&mut self.c, l as u64);
    }
    fn set(&mut self, l: u32) {
        self.c.push(0x21);
        uleb(&mut self.c, l as u64);
    }
    fn mem(&mut self, op: u8, align: u32, off: u32) {
        self.c.push(op);
        uleb(&mut self.c, align as u64);
        uleb(&mut self.c, off as u64);
    }
    /// Push guest register `r` (x0 reads as 0).
    fn reg(&mut self, r: u32) {
        if r == 0 {
            self.i64c(0);
        } else {
            self.get(r);
        }
    }
    /// Open a block, loop or if (without a result); returns its depth.
    fn open(&mut self, kind: u8) -> u32 {
        self.op(kind);
        self.op(0x40);
        self.depth += 1;
        self.depth
    }
    fn close(&mut self) {
        self.op(END);
        self.depth -= 1;
    }
    /// Branch to the construct opened at `depth`.
    fn br(&mut self, depth: u32) {
        self.op(BR);
        uleb(&mut self.c, (self.depth - depth) as u64);
    }
    fn br_if(&mut self, depth: u32) {
        self.op(BR_IF);
        uleb(&mut self.c, (self.depth - depth) as u64);
    }
    fn set_pc_const(&mut self, pc: u64) {
        self.get(0);
        self.i64c(pc);
        self.mem(I64_STORE, 3, self.l.pc);
    }
    /// ret = total + n; leave the block.
    fn exit_with(&mut self, n: u32) {
        self.get(TOTAL);
        self.i32c(n);
        self.op(I32_ADD);
        self.set(RET);
        self.br(1);
    }
    fn call(&mut self, ty: u32, func: u32) {
        self.i32c(func);
        self.op(CALL_INDIRECT);
        uleb(&mut self.c, ty as u64);
        self.op(0);
    }
    /// Stop in front of instruction `i` (at `pc`) for the interpreter.
    fn bail(&mut self, pc: u64, i: u32) {
        self.set_pc_const(pc);
        self.exit_with(i);
    }

    /// PA = the physical address of VA through TLB table `acc` (or VA itself
    /// without paging); on a TLB miss, branch to `slow`.
    fn translate_inline(&mut self, acc: u32, slow: u32) {
        let l = *self.l;
        let t = l.tlb + acc * 256 * 24;
        self.get(0);
        self.mem(I32_LOAD8_U, 0, l.vm_data);
        self.open(IF);
        // EA = byte offset of the entry for VA's page.
        self.get(VA);
        self.i64c(12);
        self.op(I64_SHR_U);
        self.op(I32_WRAP_I64);
        self.i32c(255);
        self.op(I32_AND);
        self.i32c(24);
        self.op(I32_MUL);
        self.set(EA);
        // entry.vpn == va >> 12 && entry.gen == tlb.gen
        self.get(EA);
        self.mem(I64_LOAD, 3, t);
        self.get(VA);
        self.i64c(12);
        self.op(I64_SHR_U);
        self.op(I64_NE);
        self.br_if(slow);
        self.get(EA);
        self.mem(I64_LOAD, 3, t + 8);
        self.get(0);
        self.mem(I64_LOAD, 3, l.tlb_gen);
        self.op(I64_NE);
        self.br_if(slow);
        // PA = entry.ppage | (va & 0xfff)
        self.get(EA);
        self.mem(I64_LOAD, 3, t + 16);
        self.get(VA);
        self.i64c(0xfff);
        self.op(I64_AND);
        self.op(I64_OR);
        self.set(PA);
        self.op(ELSE);
        self.get(VA);
        self.set(PA);
        self.close();
    }

    /// PA -= RAM_BASE, then branch to `slow` unless the access of
    /// 2^`log` bytes is aligned and inside RAM.
    fn ram_check(&mut self, log: u32, slow: u32) {
        let l = *self.l;
        self.get(PA);
        self.i64c(crate::bus::RAM_BASE);
        self.op(I64_SUB);
        self.set(PA);
        self.get(PA);
        self.i64c(l.ram_len - (1 << log));
        self.op(I64_GT_U);
        self.br_if(slow);
        if log > 0 {
            self.get(VA);
            self.i64c((1 << log) - 1);
            self.op(I64_AND);
            self.i64c(0);
            self.op(I64_NE);
            self.br_if(slow);
        }
    }

    /// Branch to `slow` if bit EA (a page number) of the bitmap at `bits`
    /// is `if_set`.
    fn page_bit(&mut self, bits: u32, if_set: bool, slow: u32) {
        self.get(EA);
        self.i32c(3);
        self.op(I32_SHR_U);
        self.mem(I32_LOAD8_U, 0, bits);
        self.get(EA);
        self.i32c(7);
        self.op(I32_AND);
        self.op(I32_SHR_U);
        self.i32c(1);
        self.op(I32_AND);
        if !if_set {
            self.op(I32_EQZ);
        }
        self.br_if(slow);
    }
}

fn rd(i: u32) -> u32 {
    (i >> 7) & 31
}
fn rs1(i: u32) -> u32 {
    (i >> 15) & 31
}
fn rs2(i: u32) -> u32 {
    (i >> 20) & 31
}

/// Registers read and written by an instruction (as bit masks).
fn regs_used(inst: u32) -> (u32, u32) {
    let (d, s1, s2) = (1 << rd(inst), 1 << rs1(inst), 1 << rs2(inst));
    match inst & 0x7f {
        0x37 | 0x17 | 0x6f => (0, d),
        0x13 | 0x1b | 0x03 | 0x67 => (s1, d),
        0x33 | 0x3b => (s1 | s2, d),
        0x23 | 0x63 => (s1 | s2, 0),
        _ => (0, 0),
    }
}

/// Emit the body of one block function.
fn block_body(b: &Block, l: &Layout) -> Vec<u8> {
    let mut f = Func { c: Vec::new(), l, depth: 0 };
    let (mut read, mut written) = (0u32, 0u32);
    for &(inst, _) in &b.insts {
        let (r, w) = regs_used(inst);
        read |= r;
        written |= w;
    }
    // Written registers are loaded too: an early exit writes them all back.
    // x0 is never loaded or stored.
    let (read, written) = ((read | written) & !1, written & !1);

    // Locals: x1..x31, TMP, VA, PA (i64), then RET, TOTAL, BUDGET, EA (i32).
    uleb(&mut f.c, 2);
    uleb(&mut f.c, 34);
    f.op(I64);
    uleb(&mut f.c, 4);
    f.op(I32);

    for r in 1..32 {
        if read >> r & 1 != 0 {
            f.get(0);
            f.mem(I64_LOAD, 3, l.x + 8 * r);
            f.set(r);
        }
    }
    f.get(0);
    f.mem(I32_LOAD, 2, l.budget);
    f.set(BUDGET);

    let n = b.insts.len() as u32;
    f.open(BLOCK);
    f.open(LOOP);
    let mut pc = b.pc;
    let mut ended = false;
    for (i, &(inst, len)) in b.insts.iter().enumerate() {
        let next = pc.wrapping_add(len as u64);
        ended = emit(&mut f, inst, pc, next, i as u32, n, b.pc);
        pc = next;
    }
    if !ended {
        f.set_pc_const(pc);
        f.exit_with(n);
    }
    f.close(); // loop
    f.close(); // block

    for r in 1..32 {
        if written >> r & 1 != 0 {
            f.get(0);
            f.get(r);
            f.mem(I64_STORE, 3, l.x + 8 * r);
        }
    }
    f.get(RET);
    f.op(END);
    f.c
}

/// Leave the block for `target` after `k` instructions of this pass: or go
/// around the loop again, if `target` is the block's start and the budget
/// allows another full pass (of up to `n` instructions).
fn goto(f: &mut Func, target: u64, start: u64, k: u32, n: u32) {
    if target == start {
        // total += k; if total + n <= budget, go round again.
        f.get(TOTAL);
        f.i32c(k);
        f.op(I32_ADD);
        f.set(TOTAL);
        f.get(TOTAL);
        f.i32c(n);
        f.op(I32_ADD);
        f.get(BUDGET);
        f.op(I32_LE_U);
        f.br_if(2); // the loop
        f.set_pc_const(target);
        f.exit_with(0);
    } else {
        f.set_pc_const(target);
        f.exit_with(k);
    }
}

/// Emit one instruction (number `i` of `n`). Returns true if it ended the block.
fn emit(f: &mut Func, inst: u32, pc: u64, next: u64, i: u32, n: u32, start: u64) -> bool {
    let l = *f.l;
    let (d, a, b) = (rd(inst), rs1(inst), rs2(inst));
    let f3 = (inst >> 12) & 7;
    let f7 = inst >> 25;
    let shamt = ((inst >> 20) & 63) as u64;
    match inst & 0x7f {
        0x37 => {
            if d != 0 {
                f.i64c(imm_u(inst));
                f.set(d);
            }
        }
        0x17 => {
            if d != 0 {
                f.i64c(pc.wrapping_add(imm_u(inst)));
                f.set(d);
            }
        }
        0x13 | 0x1b | 0x33 | 0x3b if d == 0 => {}
        _ if bitmanip::decode(inst).is_some() => helper_alu(f, inst, a, d),
        0x13 => {
            f.reg(a);
            match f3 {
                1 => {
                    f.i64c(shamt);
                    f.op(I64_SHL);
                }
                5 => {
                    f.i64c(shamt);
                    f.op(if inst >> 30 & 1 == 1 { I64_SHR_S } else { I64_SHR_U });
                }
                _ => {
                    f.i64c(imm_i(inst));
                    match f3 {
                        0 => f.op(I64_ADD),
                        2 => {
                            f.op(I64_LT_S);
                            f.op(I64_EXTEND_I32_U);
                        }
                        3 => {
                            f.op(I64_LT_U);
                            f.op(I64_EXTEND_I32_U);
                        }
                        4 => f.op(I64_XOR),
                        6 => f.op(I64_OR),
                        _ => f.op(I64_AND),
                    }
                }
            }
            f.set(d);
        }
        0x1b => {
            let sh = shamt & 31;
            f.reg(a);
            match (f3, f7) {
                (0, _) => {
                    f.i64c(imm_i(inst));
                    f.op(I64_ADD);
                    f.op(I64_EXTEND32_S);
                }
                (1, _) => {
                    f.i64c(sh);
                    f.op(I64_SHL);
                    f.op(I64_EXTEND32_S);
                }
                (5, 0x00) => {
                    f.i64c(0xffff_ffff);
                    f.op(I64_AND);
                    f.i64c(sh);
                    f.op(I64_SHR_U);
                    f.op(I64_EXTEND32_S);
                }
                _ => {
                    f.op(I64_EXTEND32_S);
                    f.i64c(sh);
                    f.op(I64_SHR_S);
                }
            }
            f.set(d);
        }
        0x33 | 0x3b if f7 == 1 && f3 != 0 => helper_alu(f, inst, a, d),
        0x33 => {
            f.reg(a);
            f.reg(b);
            match (f7, f3) {
                (0x00, 0) => f.op(I64_ADD),
                (0x20, 0) => f.op(I64_SUB),
                (0x00, 1) => f.op(I64_SHL),
                (0x00, 2) => {
                    f.op(I64_LT_S);
                    f.op(I64_EXTEND_I32_U);
                }
                (0x00, 3) => {
                    f.op(I64_LT_U);
                    f.op(I64_EXTEND_I32_U);
                }
                (0x00, 4) => f.op(I64_XOR),
                (0x00, 5) => f.op(I64_SHR_U),
                (0x20, 5) => f.op(I64_SHR_S),
                (0x00, 6) => f.op(I64_OR),
                (0x00, 7) => f.op(I64_AND),
                _ => f.op(I64_MUL), // (0x01, 0)
            }
            f.set(d);
        }
        0x3b => {
            match (f7, f3) {
                (_, 0) => {
                    f.reg(a);
                    f.reg(b);
                    f.op(match f7 {
                        0x00 => I64_ADD,
                        0x20 => I64_SUB,
                        _ => I64_MUL,
                    });
                    f.op(I64_EXTEND32_S);
                }
                (0x00, 1) => {
                    f.reg(a);
                    f.reg(b);
                    f.i64c(31);
                    f.op(I64_AND);
                    f.op(I64_SHL);
                    f.op(I64_EXTEND32_S);
                }
                (0x00, 5) => {
                    f.reg(a);
                    f.i64c(0xffff_ffff);
                    f.op(I64_AND);
                    f.reg(b);
                    f.i64c(31);
                    f.op(I64_AND);
                    f.op(I64_SHR_U);
                    f.op(I64_EXTEND32_S);
                }
                _ => {
                    // (0x20, 5): sraw
                    f.reg(a);
                    f.op(I64_EXTEND32_S);
                    f.reg(b);
                    f.i64c(31);
                    f.op(I64_AND);
                    f.op(I64_SHR_S);
                }
            }
            f.set(d);
        }
        0x03 => {
            let (op, log) = LOADS[f3 as usize];
            f.reg(a);
            f.i64c(imm_i(inst));
            f.op(I64_ADD);
            f.set(VA);
            let done = f.open(BLOCK);
            let slow = f.open(BLOCK);
            f.translate_inline(1, slow);
            f.ram_check(log, slow);
            f.get(PA);
            f.op(I32_WRAP_I64);
            f.mem(op, log, l.ram);
            f.set(TMP);
            f.br(done);
            f.close();
            // Slow path: the helper.
            f.get(0);
            f.get(VA);
            f.i32c(f3);
            f.call(T_LOAD, l.load_fn);
            f.set(TMP);
            f.get(0);
            f.mem(I32_LOAD8_U, 0, l.bail);
            f.open(IF);
            f.bail(pc, i);
            f.close();
            f.close();
            if d != 0 {
                f.get(TMP);
                f.set(d);
            }
        }
        0x23 => {
            let (op, log) = STORES[f3 as usize];
            f.reg(a);
            f.i64c(imm_s(inst));
            f.op(I64_ADD);
            f.set(VA);
            let done = f.open(BLOCK);
            let slow = f.open(BLOCK);
            f.translate_inline(2, slow);
            f.ram_check(log, slow);
            // EA = the page: the journal must already have saved it (always
            // true while the journal is off), it must hold no compiled code,
            // and must not be tohost's.
            f.get(PA);
            f.i64c(12);
            f.op(I64_SHR_U);
            f.op(I32_WRAP_I64);
            f.set(EA);
            f.page_bit(l.saved_bits, false, slow);
            f.page_bit(l.code_bits, true, slow);
            if l.tohost_page != u32::MAX {
                f.get(EA);
                f.i32c(l.tohost_page);
                f.op(I32_EQ);
                f.br_if(slow);
            }
            // No LR reservation outstanding (the store might break it).
            f.get(0);
            f.mem(I64_LOAD, 3, l.resv);
            f.i64c(u64::MAX);
            f.op(I64_NE);
            f.br_if(slow);
            f.get(PA);
            f.op(I32_WRAP_I64);
            f.reg(b);
            f.mem(op, log, l.ram);
            f.br(done);
            f.close();
            // Slow path: the helper (nonzero: the interpreter must do it).
            f.get(0);
            f.get(VA);
            f.reg(b);
            f.i32c(1 << f3);
            f.call(T_STORE, l.store_fn);
            f.open(IF);
            f.bail(pc, i);
            f.close();
            f.close();
        }
        0x0f => {}
        0x6f => {
            if d != 0 {
                f.i64c(next);
                f.set(d);
            }
            goto(f, pc.wrapping_add(imm_j(inst)), start, i + 1, n);
            return true;
        }
        0x67 => {
            // The target first: rd may be rs1.
            f.get(0);
            f.reg(a);
            f.i64c(imm_i(inst));
            f.op(I64_ADD);
            f.i64c(!1);
            f.op(I64_AND);
            f.mem(I64_STORE, 3, l.pc);
            if d != 0 {
                f.i64c(next);
                f.set(d);
            }
            f.exit_with(i + 1);
            return true;
        }
        0x63 => {
            f.reg(a);
            f.reg(b);
            f.op([I64_EQ, I64_NE, 0, 0, I64_LT_S, I64_GE_S, I64_LT_U, I64_GE_U][f3 as usize]);
            f.open(IF);
            goto(f, pc.wrapping_add(imm_b(inst)), start, i + 1, n);
            f.close();
        }
        _ => unreachable!("classify let through 0x{inst:08x}"),
    }
    false
}

/// rd = alu_helper(inst, x[rs1], operand b).
fn helper_alu(f: &mut Func, inst: u32, a: u32, d: u32) {
    let l = *f.l;
    f.i32c(inst);
    f.reg(a);
    match bitmanip::decode(inst) {
        Some(op) if op.form() != Form::Reg => f.i64c(bitmanip::operand_b(op, inst, &[0; 32])),
        _ => f.reg(rs2(inst)),
    }
    f.call(T_ALU, l.alu_fn);
    f.set(d);
}

/// A module holding one function per block, exported as "f0", "f1", ...
pub(crate) fn module(blocks: &[&Block], l: &Layout) -> Vec<u8> {
    let mut m = b"\0asm\x01\0\0\0".to_vec();

    let mut s = Vec::new();
    let types: [(&[u8], &[u8]); 4] =
        [(&[I32], &[I32]), (&[I32, I64, I32], &[I64]), (&[I32, I64, I64, I32], &[I32]), (&[I32, I64, I64], &[I64])];
    uleb(&mut s, types.len() as u64);
    for (p, r) in types {
        s.push(0x60);
        uleb(&mut s, p.len() as u64);
        s.extend_from_slice(p);
        uleb(&mut s, r.len() as u64);
        s.extend_from_slice(r);
    }
    section(&mut m, 1, &s);

    let mut s = Vec::new();
    uleb(&mut s, 2);
    name(&mut s, "e");
    name(&mut s, "m");
    s.extend_from_slice(&[0x02, 0x00, 0x01]); // memory, min 1 page, no max
    name(&mut s, "e");
    name(&mut s, "t");
    s.extend_from_slice(&[0x01, 0x70, 0x00, 0x01]); // funcref table, min 1, no max
    section(&mut m, 2, &s);

    let mut s = Vec::new();
    uleb(&mut s, blocks.len() as u64);
    for _ in blocks {
        uleb(&mut s, T_BLOCK as u64);
    }
    section(&mut m, 3, &s);

    let mut s = Vec::new();
    uleb(&mut s, blocks.len() as u64);
    for i in 0..blocks.len() {
        name(&mut s, &format!("f{i}"));
        s.push(0x00);
        uleb(&mut s, i as u64);
    }
    section(&mut m, 7, &s);

    let mut s = Vec::new();
    uleb(&mut s, blocks.len() as u64);
    for b in blocks {
        let body = block_body(b, l);
        uleb(&mut s, body.len() as u64);
        s.extend_from_slice(&body);
    }
    section(&mut m, 10, &s);
    m
}
