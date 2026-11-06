//! IEEE 754 binary32 and binary64 arithmetic in integer code, for the F and
//! D extensions: every rounding mode, the five exception flags, tininess
//! detected after rounding, and RISC-V's canonical NaN.
//!
//! Host floats would be faster, but they only round to nearest, report no
//! exceptions, and NaN bits can differ between hosts. Integer code gives the
//! same bits on every host (native or WebAssembly), which snapshots and time
//! travel rely on.
//!
//! Values are passed as raw bit patterns in a `u64`; `Fmt` says which format.

// fflags bits
pub const NX: u8 = 1; // inexact
pub const UF: u8 = 2; // underflow
pub const OF: u8 = 4; // overflow
pub const DZ: u8 = 8; // divide by zero
pub const NV: u8 = 16; // invalid

// Rounding modes, as encoded in the rm field and frm.
pub const RNE: u8 = 0;
pub const RTZ: u8 = 1;
pub const RDN: u8 = 2;
pub const RUP: u8 = 3;
pub const RMM: u8 = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Fmt {
    exp: u32,
    man: u32,
}

pub const S: Fmt = Fmt { exp: 8, man: 23 };
pub const D: Fmt = Fmt { exp: 11, man: 52 };

impl Fmt {
    const fn bias(self) -> i32 {
        (1 << (self.exp - 1)) - 1
    }
    const fn exp_max(self) -> u64 {
        (1 << self.exp) - 1
    }
    const fn man_mask(self) -> u64 {
        (1 << self.man) - 1
    }
    pub const fn sign(self) -> u64 {
        1 << (self.exp + self.man)
    }
    /// The canonical NaN, which every operation that makes a NaN returns.
    pub const fn qnan(self) -> u64 {
        (self.exp_max() << self.man) | (1 << (self.man - 1))
    }
    const fn inf(self, neg: bool) -> u64 {
        (self.exp_max() << self.man) | self.sign_if(neg)
    }
    const fn max_finite(self, neg: bool) -> u64 {
        ((self.exp_max() - 1) << self.man) | self.man_mask() | self.sign_if(neg)
    }
    const fn zero(self, neg: bool) -> u64 {
        self.sign_if(neg)
    }
    const fn sign_if(self, neg: bool) -> u64 {
        if neg {
            self.sign()
        } else {
            0
        }
    }
    pub fn is_nan(self, v: u64) -> bool {
        (v >> self.man) & self.exp_max() == self.exp_max() && v & self.man_mask() != 0
    }
    pub fn is_snan(self, v: u64) -> bool {
        self.is_nan(v) && v & (1 << (self.man - 1)) == 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Zero,
    Num,
    Inf,
    Nan,
}

/// A decoded value: for `Num`, exactly `sig * 2^exp`.
#[derive(Clone, Copy, Debug)]
struct Val {
    neg: bool,
    kind: Kind,
    exp: i32,
    sig: u128,
}

fn unpack(f: Fmt, v: u64) -> Val {
    let neg = v & f.sign() != 0;
    let e = (v >> f.man) & f.exp_max();
    let m = v & f.man_mask();
    // Weight of a subnormal's lowest bit.
    let lsb = 1 - f.bias() - f.man as i32;
    let (kind, exp, sig) = if e == f.exp_max() {
        (if m == 0 { Kind::Inf } else { Kind::Nan }, 0, 0)
    } else if e == 0 {
        (if m == 0 { Kind::Zero } else { Kind::Num }, lsb, m)
    } else {
        (Kind::Num, lsb + e as i32 - 1, m | 1 << f.man)
    };
    Val { neg, kind, exp, sig: sig as u128 }
}

/// How a dropped remainder compares with half a unit in the last place.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rest {
    Exact,
    Below,
    Half,
    Above,
}

/// `sig >> n`, and how the bits shifted out compare with half of its unit.
fn shr_rest(sig: u128, n: u32) -> (u128, Rest) {
    if n == 0 {
        return (sig, Rest::Exact);
    }
    let (q, rem, half) = match n {
        1..=127 => (sig >> n, sig & ((1 << n) - 1), 1u128 << (n - 1)),
        128 => (0, sig, 1 << 127),
        _ => return (0, if sig == 0 { Rest::Exact } else { Rest::Below }),
    };
    let rest = match rem.cmp(&half) {
        _ if rem == 0 => Rest::Exact,
        std::cmp::Ordering::Less => Rest::Below,
        std::cmp::Ordering::Equal => Rest::Half,
        std::cmp::Ordering::Greater => Rest::Above,
    };
    (q, rest)
}

/// Whether rounding a truncated magnitude goes up by one unit.
fn round_up(rm: u8, neg: bool, odd: bool, rest: Rest) -> bool {
    match rm {
        RNE => rest == Rest::Above || (rest == Rest::Half && odd),
        RTZ => false,
        RDN => rest != Rest::Exact && neg,
        RUP => rest != Rest::Exact && !neg,
        _ => matches!(rest, Rest::Half | Rest::Above), // RMM
    }
}

/// Keep `sig * 2^exp` to a multiple of `2^q`, rounding by `rm`.
fn round_to(sig: u128, exp: i32, q: i32, rm: u8, neg: bool) -> (u128, Rest) {
    let (m, rest) = if q <= exp { (sig << (exp - q), Rest::Exact) } else { shr_rest(sig, (q - exp) as u32) };
    (m + round_up(rm, neg, m & 1 == 1, rest) as u128, rest)
}

/// Round `±sig * 2^exp` to the format and encode it. The lowest bit of `sig`
/// may stand in for a nonzero remainder below it (a sticky bit) as long as
/// `sig` then has at least `man + 3` significant bits.
fn round_pack(f: Fmt, neg: bool, exp: i32, sig: u128, rm: u8, fl: &mut u8) -> u64 {
    if sig == 0 {
        return f.zero(neg);
    }
    let man = f.man as i32;
    let emin = 1 - f.bias();
    // Exponent of the leading bit.
    let top = exp + 127 - sig.leading_zeros() as i32;
    // Weight of the result's lowest bit: full precision, or fixed below emin.
    let q = (top - man).max(emin - man);
    let (mut m, rest) = round_to(sig, exp, q, rm, neg);
    let mut q = q;
    if m >> (man + 1) != 0 {
        // Rounded up to the next power of two.
        m >>= 1;
        q += 1;
    }
    if rest != Rest::Exact {
        *fl |= NX;
        // Tiny if, rounded to full precision as if the exponent range had no
        // lower limit, it would still be below the smallest normal.
        if top < emin && (top < emin - 1 || round_to(sig, exp, top - man, rm, neg).0 >> (man + 1) == 0) {
            *fl |= UF;
        }
    }
    if m >> man == 0 {
        return f.sign_if(neg) | m as u64; // subnormal, or rounded to zero
    }
    let e = q + man;
    if e > f.bias() {
        *fl |= OF | NX;
        let to_inf = match rm {
            RNE | RMM => true,
            RTZ => false,
            RDN => neg,
            _ => !neg,
        };
        return if to_inf { f.inf(neg) } else { f.max_finite(neg) };
    }
    f.sign_if(neg) | (((e + f.bias()) as u64) << f.man) | (m as u64 & f.man_mask())
}

/// Exact sum of `±xs * 2^xe` and `±ys * 2^ye` (each significand at most 107
/// bits), with a sticky bit when one is too small to keep whole.
fn add_exact(x: (bool, i32, u128), y: (bool, i32, u128)) -> (bool, i32, u128) {
    let ((xn, xe, mut xs), (yn, ye, mut ys)) = if x.1 >= y.1 { (x, y) } else { (y, x) };
    let d = (xe - ye) as u32;
    let bits = 128 - xs.leading_zeros();
    let exp = if d + bits <= 126 {
        xs <<= d;
        ye
    } else {
        // Put x's top bit at bit 125 and shift y down to match. y is then
        // far below x's rounding point, so a sticky bit stands in for it.
        let l = 126 - bits;
        xs <<= l;
        let n = d - l;
        ys = if n >= 128 { (ys != 0) as u128 } else { (ys >> n) | ((ys & ((1 << n) - 1) != 0) as u128) };
        xe - l as i32
    };
    if xn == yn {
        (xn, exp, xs + ys)
    } else if xs >= ys {
        (xn, exp, xs - ys)
    } else {
        (yn, exp, ys - xs)
    }
}

fn nan_result(f: Fmt, inputs: &[u64], fl: &mut u8) -> u64 {
    if inputs.iter().any(|&v| f.is_snan(v)) {
        *fl |= NV;
    }
    f.qnan()
}

fn invalid(f: Fmt, fl: &mut u8) -> u64 {
    *fl |= NV;
    f.qnan()
}

/// An exact zero sum: +0, or -0 when rounding down (IEEE 754 6.3).
fn zero_sum(f: Fmt, rm: u8) -> u64 {
    f.zero(rm == RDN)
}

pub fn add(f: Fmt, a: u64, b: u64, rm: u8, fl: &mut u8) -> u64 {
    let (x, y) = (unpack(f, a), unpack(f, b));
    use Kind::*;
    match (x.kind, y.kind) {
        (Nan, _) | (_, Nan) => nan_result(f, &[a, b], fl),
        (Inf, Inf) if x.neg != y.neg => invalid(f, fl),
        (Inf, _) => a,
        (_, Inf) => b,
        (Zero, Zero) if x.neg != y.neg => zero_sum(f, rm),
        (Zero, _) => b,
        (_, Zero) => a,
        (Num, Num) => {
            let (neg, exp, sig) = add_exact((x.neg, x.exp, x.sig), (y.neg, y.exp, y.sig));
            if sig == 0 {
                zero_sum(f, rm)
            } else {
                round_pack(f, neg, exp, sig, rm, fl)
            }
        }
    }
}

pub fn sub(f: Fmt, a: u64, b: u64, rm: u8, fl: &mut u8) -> u64 {
    add(f, a, b ^ f.sign(), rm, fl)
}

pub fn mul(f: Fmt, a: u64, b: u64, rm: u8, fl: &mut u8) -> u64 {
    let (x, y) = (unpack(f, a), unpack(f, b));
    let neg = x.neg != y.neg;
    use Kind::*;
    match (x.kind, y.kind) {
        (Nan, _) | (_, Nan) => nan_result(f, &[a, b], fl),
        (Inf, Zero) | (Zero, Inf) => invalid(f, fl),
        (Inf, _) | (_, Inf) => f.inf(neg),
        (Zero, _) | (_, Zero) => f.zero(neg),
        (Num, Num) => round_pack(f, neg, x.exp + y.exp, x.sig * y.sig, rm, fl),
    }
}

pub fn div(f: Fmt, a: u64, b: u64, rm: u8, fl: &mut u8) -> u64 {
    let (x, y) = (unpack(f, a), unpack(f, b));
    let neg = x.neg != y.neg;
    use Kind::*;
    match (x.kind, y.kind) {
        (Nan, _) | (_, Nan) => nan_result(f, &[a, b], fl),
        (Inf, Inf) | (Zero, Zero) => invalid(f, fl),
        (Inf, _) => f.inf(neg),
        (_, Inf) | (Zero, _) => f.zero(neg),
        (_, Zero) => {
            *fl |= DZ;
            f.inf(neg)
        }
        (Num, Num) => {
            // Dividend at bit 126, divisor at most 53 bits: a quotient of at
            // least 74 bits, with the remainder as a sticky bit.
            let s = x.sig.leading_zeros() - 1;
            let num = x.sig << s;
            let (q, r) = (num / y.sig, num % y.sig);
            round_pack(f, neg, x.exp - s as i32 - y.exp, q | (r != 0) as u128, rm, fl)
        }
    }
}

pub fn sqrt(f: Fmt, a: u64, rm: u8, fl: &mut u8) -> u64 {
    let x = unpack(f, a);
    match x.kind {
        Kind::Nan => nan_result(f, &[a], fl),
        Kind::Zero => a,
        _ if x.neg => invalid(f, fl),
        Kind::Inf => a,
        Kind::Num => {
            // Shift to 125 or 126 bits so that the exponent is even; the root
            // then has 63 bits, with the remainder as a sticky bit.
            let mut s = x.sig.leading_zeros() - 2;
            if (x.exp - s as i32) & 1 != 0 {
                s -= 1;
            }
            let v = x.sig << s;
            let r = v.isqrt();
            round_pack(f, false, (x.exp - s as i32) / 2, r | (r * r != v) as u128, rm, fl)
        }
    }
}

/// Fused `(±a * b) ± c` with a single rounding.
#[allow(clippy::too_many_arguments)]
pub fn fma(f: Fmt, a: u64, b: u64, c: u64, neg_prod: bool, neg_c: bool, rm: u8, fl: &mut u8) -> u64 {
    let (x, y, z) = (unpack(f, a), unpack(f, b), unpack(f, c));
    use Kind::*;
    let inf_times_zero = matches!((x.kind, y.kind), (Inf, Zero) | (Zero, Inf));
    if x.kind == Nan || y.kind == Nan || z.kind == Nan {
        // Infinity times zero is invalid even when the addend is a quiet NaN.
        if inf_times_zero {
            *fl |= NV;
        }
        return nan_result(f, &[a, b, c], fl);
    }
    if inf_times_zero {
        return invalid(f, fl);
    }
    let pn = x.neg ^ y.neg ^ neg_prod;
    let cn = z.neg ^ neg_c;
    if x.kind == Inf || y.kind == Inf {
        return if z.kind == Inf && cn != pn { invalid(f, fl) } else { f.inf(pn) };
    }
    if z.kind == Inf {
        return f.inf(cn);
    }
    if x.kind == Zero || y.kind == Zero {
        return match z.kind {
            Zero if pn != cn => zero_sum(f, rm),
            Zero => f.zero(pn),
            _ => c ^ f.sign_if(neg_c),
        };
    }
    let p = (pn, x.exp + y.exp, x.sig * y.sig);
    if z.kind == Zero {
        return round_pack(f, p.0, p.1, p.2, rm, fl);
    }
    let (neg, exp, sig) = add_exact(p, (cn, z.exp, z.sig));
    if sig == 0 {
        zero_sum(f, rm)
    } else {
        round_pack(f, neg, exp, sig, rm, fl)
    }
}

/// Convert between formats (FCVT.S.D, FCVT.D.S).
pub fn convert(from: Fmt, to: Fmt, a: u64, rm: u8, fl: &mut u8) -> u64 {
    let x = unpack(from, a);
    match x.kind {
        Kind::Nan => {
            nan_result(from, &[a], fl);
            to.qnan()
        }
        Kind::Inf => to.inf(x.neg),
        Kind::Zero => to.zero(x.neg),
        Kind::Num => round_pack(to, x.neg, x.exp, x.sig, rm, fl),
    }
}

/// Float to a `bits`-wide integer (FCVT.W/WU/L/LU). Out-of-range values and
/// NaN saturate and raise invalid; 32-bit results are sign-extended.
pub fn to_int(f: Fmt, a: u64, signed: bool, bits: u32, rm: u8, fl: &mut u8) -> u64 {
    let x = unpack(f, a);
    let (min, max): (i128, i128) =
        if signed { (-(1 << (bits - 1)), (1 << (bits - 1)) - 1) } else { (0, (1 << bits) - 1) };
    let saturate = |fl: &mut u8, neg: bool| {
        *fl |= NV;
        if neg {
            min
        } else {
            max
        }
    };
    let v = match x.kind {
        Kind::Nan => saturate(fl, false),
        Kind::Inf => saturate(fl, x.neg),
        Kind::Zero => 0,
        Kind::Num => {
            let (m, rest) = if x.exp > 64 { (u128::MAX, Rest::Exact) } else { round_to(x.sig, x.exp, 0, rm, x.neg) };
            let v = if m > 1 << 64 { None } else { Some(if x.neg { -(m as i128) } else { m as i128 }) };
            match v {
                Some(v) if (min..=max).contains(&v) => {
                    if rest != Rest::Exact {
                        *fl |= NX;
                    }
                    v
                }
                _ => saturate(fl, x.neg),
            }
        }
    };
    if bits == 32 {
        v as i32 as i64 as u64
    } else {
        v as u64
    }
}

/// A `bits`-wide integer to float (FCVT.S/D.W/WU/L/LU).
pub fn from_int(f: Fmt, v: u64, signed: bool, bits: u32, rm: u8, fl: &mut u8) -> u64 {
    let (neg, mag) = match (signed, bits) {
        (true, 32) => ((v as i32) < 0, (v as i32).unsigned_abs() as u64),
        (true, _) => ((v as i64) < 0, (v as i64).unsigned_abs()),
        (false, 32) => (false, v as u32 as u64),
        (false, _) => (false, v),
    };
    round_pack(f, neg, 0, mag as u128, rm, fl)
}

/// Value order with -0 = +0; only meaningful for non-NaNs.
fn key(f: Fmt, v: u64) -> i128 {
    let m = (v & !f.sign()) as i128;
    if v & f.sign() != 0 {
        -m
    } else {
        m
    }
}

pub fn eq(f: Fmt, a: u64, b: u64, fl: &mut u8) -> bool {
    if f.is_snan(a) || f.is_snan(b) {
        *fl |= NV;
    }
    !f.is_nan(a) && !f.is_nan(b) && key(f, a) == key(f, b)
}

/// FLT (`or_equal` false) and FLE: any NaN operand is invalid.
pub fn lt(f: Fmt, a: u64, b: u64, or_equal: bool, fl: &mut u8) -> bool {
    if f.is_nan(a) || f.is_nan(b) {
        *fl |= NV;
        return false;
    }
    let (ka, kb) = (key(f, a), key(f, b));
    ka < kb || (or_equal && ka == kb)
}

/// FMIN/FMAX: a NaN operand gives the other operand, and -0 < +0.
pub fn min_max(f: Fmt, a: u64, b: u64, max: bool, fl: &mut u8) -> u64 {
    if f.is_snan(a) || f.is_snan(b) {
        *fl |= NV;
    }
    match (f.is_nan(a), f.is_nan(b)) {
        (true, true) => return f.qnan(),
        (true, false) => return b,
        (false, true) => return a,
        _ => {}
    }
    // Order -0 just below +0.
    let ord = |v: u64| key(f, v) * 2 - (v & f.sign() != 0) as i128;
    if (ord(a) < ord(b)) != max {
        a
    } else {
        b
    }
}

/// FSGNJ (op 0), FSGNJN (1), FSGNJX (2).
pub fn sign_inject(f: Fmt, a: u64, b: u64, op: u32) -> u64 {
    let s = f.sign();
    match op {
        0 => (a & !s) | (b & s),
        1 => (a & !s) | (!b & s),
        _ => a ^ (b & s),
    }
}

/// FCLASS: one bit set, from -inf (bit 0) to quiet NaN (bit 9).
pub fn class(f: Fmt, a: u64) -> u64 {
    let x = unpack(f, a);
    let subnormal = (a >> f.man) & f.exp_max() == 0;
    let bit = match (x.kind, x.neg) {
        (Kind::Nan, _) => {
            if f.is_snan(a) {
                8
            } else {
                9
            }
        }
        (Kind::Inf, true) => 0,
        (Kind::Num, true) if !subnormal => 1,
        (Kind::Num, true) => 2,
        (Kind::Zero, true) => 3,
        (Kind::Zero, false) => 4,
        (Kind::Num, false) if subnormal => 5,
        (Kind::Num, false) => 6,
        (Kind::Inf, false) => 7,
    };
    1 << bit
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small deterministic generator, biased towards the awkward values.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn f64(&mut self) -> u64 {
            let r = self.next();
            match r % 8 {
                0 => r >> 12,                                                                     // subnormal
                1 => (r & D.sign()) | ((r >> 20) & 0xf) << 52 | 0x7f << 56 | (r & 0xf_ffff_ffff), // huge, inf, NaN
                2 => (r & D.sign()) | (0x3ff << 52) | (r >> 40), // near 1, short mantissa
                3 => r & 0xfff0_0000_0000_0000,                  // powers of two
                _ => r,
            }
        }
        fn f32(&mut self) -> u64 {
            let r = self.next();
            match r % 8 {
                0 => (r >> 41) & 0x7f_ffff,
                1 => (r as u32 & 0x8000_0000 | 0x7e80_0000 | (r >> 40) as u32 & 0x7f_ffff) as u64,
                2 => (r as u32 & 0x8000_0000 | 0x3f80_0000 | (r >> 50) as u32) as u64,
                _ => r & 0xffff_ffff,
            }
        }
    }

    fn same64(got: u64, want: f64) -> bool {
        if want.is_nan() {
            got == D.qnan()
        } else {
            got == want.to_bits()
        }
    }
    fn same32(got: u64, want: f32) -> bool {
        if want.is_nan() {
            got == S.qnan()
        } else {
            got == want.to_bits() as u64
        }
    }

    // The host rounds to nearest-even correctly, so it checks that mode.
    #[test]
    fn matches_host_f64() {
        let mut r = Rng(0x9e37_79b9_7f4a_7c15);
        let mut fl = 0;
        for _ in 0..300_000 {
            let (a, b, c) = (r.f64(), r.f64(), r.f64());
            let (x, y, z) = (f64::from_bits(a), f64::from_bits(b), f64::from_bits(c));
            assert!(same64(add(D, a, b, RNE, &mut fl), x + y), "add {a:#x} {b:#x}");
            assert!(same64(sub(D, a, b, RNE, &mut fl), x - y), "sub {a:#x} {b:#x}");
            assert!(same64(mul(D, a, b, RNE, &mut fl), x * y), "mul {a:#x} {b:#x}");
            assert!(same64(div(D, a, b, RNE, &mut fl), x / y), "div {a:#x} {b:#x}");
            assert!(same64(sqrt(D, a, RNE, &mut fl), x.sqrt()), "sqrt {a:#x}");
            assert!(same64(fma(D, a, b, c, false, false, RNE, &mut fl), x.mul_add(y, z)), "fma {a:#x} {b:#x} {c:#x}");
            assert!(same32(convert(D, S, a, RNE, &mut fl), x as f32), "cvt.s.d {a:#x}");
            let i = r.next() >> (r.next() % 64);
            assert!(same64(from_int(D, i, true, 64, RNE, &mut fl), i as i64 as f64), "cvt.d.l {i:#x}");
            assert!(same64(from_int(D, i, false, 64, RNE, &mut fl), i as f64), "cvt.d.lu {i:#x}");
        }
    }

    #[test]
    fn matches_host_f32() {
        let mut r = Rng(0x2545_f491_4f6c_dd1d);
        let mut fl = 0;
        for _ in 0..300_000 {
            let (a, b, c) = (r.f32(), r.f32(), r.f32());
            let (x, y, z) = (f32::from_bits(a as u32), f32::from_bits(b as u32), f32::from_bits(c as u32));
            assert!(same32(add(S, a, b, RNE, &mut fl), x + y), "add {a:#x} {b:#x}");
            assert!(same32(mul(S, a, b, RNE, &mut fl), x * y), "mul {a:#x} {b:#x}");
            assert!(same32(div(S, a, b, RNE, &mut fl), x / y), "div {a:#x} {b:#x}");
            assert!(same32(sqrt(S, a, RNE, &mut fl), x.sqrt()), "sqrt {a:#x}");
            assert!(same32(fma(S, a, b, c, false, false, RNE, &mut fl), x.mul_add(y, z)), "fma {a:#x} {b:#x} {c:#x}");
            assert!(same64(convert(S, D, a, RNE, &mut fl), x as f64), "cvt.d.s {a:#x}");
            let i = r.next() as u32 as u64;
            assert!(same32(from_int(S, i, true, 32, RNE, &mut fl), i as i32 as f32), "cvt.s.w {i:#x}");
        }
    }

    #[test]
    fn flags_and_modes() {
        let one = 1f64.to_bits();
        let three = 3f64.to_bits();
        let mut fl = 0;
        // 1/3 rounds down to nearest; up and down modes differ by one ulp.
        let third = div(D, one, three, RNE, &mut fl);
        assert_eq!(fl, NX);
        assert_eq!(third, (1.0f64 / 3.0).to_bits());
        assert_eq!(div(D, one, three, RUP, &mut fl), third + 1);
        assert_eq!(div(D, one, three, RDN, &mut fl), third);
        assert_eq!(div(D, one ^ D.sign(), three, RDN, &mut fl), (third + 1) ^ D.sign());
        // Overflow: to infinity, or the largest finite value toward zero.
        fl = 0;
        assert_eq!(mul(D, f64::MAX.to_bits(), 2f64.to_bits(), RNE, &mut fl), f64::INFINITY.to_bits());
        assert_eq!(fl, OF | NX);
        assert_eq!(mul(D, f64::MAX.to_bits(), 2f64.to_bits(), RTZ, &mut fl), f64::MAX.to_bits());
        // Invalid, divide by zero, and the sign of exact zero sums.
        fl = 0;
        assert_eq!(sqrt(D, (-1f64).to_bits(), RNE, &mut fl), D.qnan());
        assert_eq!(fl, NV);
        fl = 0;
        assert_eq!(div(D, one, 0, RNE, &mut fl), f64::INFINITY.to_bits());
        assert_eq!(fl, DZ);
        assert_eq!(sub(D, one, one, RDN, &mut fl), D.sign());
        assert_eq!(sub(D, one, one, RNE, &mut fl), 0);
        // Ties: 2.5 to integer.
        let x = 2.5f64.to_bits();
        assert_eq!(to_int(D, x, true, 32, RNE, &mut fl), 2);
        assert_eq!(to_int(D, x, true, 32, RMM, &mut fl), 3);
        assert_eq!(to_int(D, x ^ D.sign(), true, 32, RMM, &mut fl) as i64, -3);
        // Saturation.
        fl = 0;
        assert_eq!(to_int(D, D.qnan(), true, 32, RNE, &mut fl), i32::MAX as u64);
        assert_eq!(to_int(D, (-1f64).to_bits(), false, 64, RNE, &mut fl), 0);
        assert_eq!(fl, NV);
        // Underflow is tininess after rounding plus inexact.
        fl = 0;
        let tiny = mul(D, f64::MIN_POSITIVE.to_bits(), 0.75f64.to_bits(), RNE, &mut fl);
        assert_eq!(tiny, (f64::MIN_POSITIVE * 0.75).to_bits());
        assert_eq!(fl, 0, "exact subnormal result raises nothing");
        let x = f64::from_bits(f64::MIN_POSITIVE.to_bits() + 1);
        mul(D, x.to_bits(), (1.0 - f64::EPSILON / 2.0).to_bits(), RNE, &mut fl);
        assert_eq!(fl, NX, "rounds up to the smallest normal: not tiny");
    }

    #[test]
    fn min_max_and_compare() {
        let mut fl = 0;
        let (p0, n0) = (0u64, D.sign());
        assert_eq!(min_max(D, p0, n0, false, &mut fl), n0);
        assert_eq!(min_max(D, n0, p0, true, &mut fl), p0);
        assert_eq!(min_max(D, D.qnan(), one(), false, &mut fl), one());
        assert_eq!(fl, 0);
        assert!(eq(D, p0, n0, &mut fl));
        assert!(!lt(D, n0, p0, false, &mut fl));
        assert!(!eq(D, D.qnan(), D.qnan(), &mut fl));
        assert_eq!(fl, 0, "feq on quiet NaNs is quiet");
        lt(D, D.qnan(), p0, false, &mut fl);
        assert_eq!(fl, NV);
        assert_eq!(class(D, n0), 1 << 3);
        assert_eq!(class(D, 1), 1 << 5);
        assert_eq!(class(D, D.qnan()), 1 << 9);
        assert_eq!(class(D, f64::NEG_INFINITY.to_bits()), 1);
    }

    /// The next value up in the format (finite or infinite x, not NaN).
    fn next_up(f: Fmt, x: u64) -> u64 {
        if x == f.sign() {
            1 // -0 -> smallest subnormal
        } else if x & f.sign() != 0 {
            x - 1
        } else {
            x + 1
        }
    }

    // Rounding down and up must bracket the exact result with neighbouring
    // values (or agree when it is exact), and the other modes must pick the
    // right one of the two.
    #[test]
    fn directed_modes_bracket() {
        let mut r = Rng(0x1234_5678_9abc_def1);
        for f in [S, D] {
            for i in 0..400_000 {
                let gen = |r: &mut Rng| if f == S { r.f32() } else { r.f64() };
                let (a, b, c) = (gen(&mut r), gen(&mut r), gen(&mut r));
                let op = |rm: u8, fl: &mut u8| match i % 6 {
                    0 => add(f, a, b, rm, fl),
                    1 => mul(f, a, b, rm, fl),
                    2 => div(f, a, b, rm, fl),
                    3 => sqrt(f, a, rm, fl),
                    4 => fma(f, a, b, c, false, true, rm, fl),
                    _ => convert(D, S, r.0 ^ a, rm, fl),
                };
                let f = if i % 6 == 5 { S } else { f };
                let mut fl = 0;
                let ne = op(RNE, &mut fl);
                if f.is_nan(ne) {
                    continue;
                }
                let mut x = 0;
                let (dn, up, tz, mm) = (op(RDN, &mut x), op(RUP, &mut x), op(RTZ, &mut x), op(RMM, &mut x));
                let ctx = format!("op {} {a:#x} {b:#x} {c:#x}: ne {ne:#x} dn {dn:#x} up {up:#x}", i % 6);
                if fl & NX == 0 {
                    assert!(dn == ne && up == ne && tz == ne && mm == ne, "exact {ctx}");
                    continue;
                }
                assert_eq!(next_up(f, dn), up, "bracket {ctx}");
                assert!(ne == dn || ne == up, "nearest {ctx}");
                assert!(mm == dn || mm == up, "max-magnitude {ctx}");
                // Toward zero: the one with the smaller magnitude.
                let small = if (dn & !f.sign()) < (up & !f.sign()) { dn } else { up };
                assert_eq!(tz, small, "toward zero {ctx}");
            }
        }
    }

    fn one() -> u64 {
        1f64.to_bits()
    }
}
