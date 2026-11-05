//! Platform-level interrupt controller (one hart, two contexts: M and S).
//!
//! Context 0 drives MEIP, context 1 drives SEIP. Sources are level triggered.

pub const BASE: u64 = 0x0c00_0000;
pub const SIZE: u64 = 0x0400_0000;
pub const NUM_SOURCES: usize = 64;
pub const NUM_CONTEXTS: usize = 2;

#[derive(Clone)]
pub struct Plic {
    pub priority: [u32; NUM_SOURCES],
    pub pending: u64,
    pub claimed: u64,
    pub enable: [u64; NUM_CONTEXTS],
    pub threshold: [u32; NUM_CONTEXTS],
}

impl Default for Plic {
    fn default() -> Self {
        Self::new()
    }
}

impl Plic {
    pub fn new() -> Self {
        Plic {
            priority: [0; NUM_SOURCES],
            pending: 0,
            claimed: 0,
            enable: [0; NUM_CONTEXTS],
            threshold: [0; NUM_CONTEXTS],
        }
    }

    /// Feed the current device interrupt lines (bit n = source n).
    pub fn update(&mut self, lines: u64) {
        self.pending = lines & !self.claimed & !1;
    }

    /// Highest-priority pending, enabled source above threshold for `ctx`.
    fn best(&self, ctx: usize) -> u32 {
        let mut cand = self.pending & self.enable[ctx] & !self.claimed;
        let mut best = 0u32;
        let mut best_prio = self.threshold[ctx];
        while cand != 0 {
            let src = cand.trailing_zeros();
            cand &= cand - 1;
            let p = self.priority[src as usize];
            if p > best_prio {
                best_prio = p;
                best = src;
            }
        }
        best
    }

    pub fn irq(&self, ctx: usize) -> bool {
        self.best(ctx) != 0
    }

    pub fn read(&mut self, off: u64, _size: usize) -> Option<u64> {
        let v = match off {
            0..=0xffc => self.priority.get((off / 4) as usize).copied().unwrap_or(0) as u64,
            0x1000..=0x107f => {
                let w = (off - 0x1000) / 4;
                if w < 2 {
                    (self.pending >> (w * 32)) & 0xffff_ffff
                } else {
                    0
                }
            }
            0x2000..=0x1f_ffff => {
                let ctx = ((off - 0x2000) / 0x80) as usize;
                let w = ((off - 0x2000) % 0x80) / 4;
                if ctx < NUM_CONTEXTS && w < 2 {
                    (self.enable[ctx] >> (w * 32)) & 0xffff_ffff
                } else {
                    0
                }
            }
            0x20_0000..=0x3ff_ffff => {
                let ctx = ((off - 0x20_0000) / 0x1000) as usize;
                let reg = (off - 0x20_0000) % 0x1000;
                if ctx >= NUM_CONTEXTS {
                    0
                } else if reg == 0 {
                    self.threshold[ctx] as u64
                } else if reg == 4 {
                    let src = self.best(ctx);
                    if src != 0 {
                        self.pending &= !(1u64 << src);
                        self.claimed |= 1u64 << src;
                    }
                    src as u64
                } else {
                    0
                }
            }
            _ => 0,
        };
        Some(v)
    }

    pub fn write(&mut self, off: u64, _size: usize, val: u64) {
        let val32 = val as u32;
        match off {
            0..=0xffc => {
                if let Some(p) = self.priority.get_mut((off / 4) as usize) {
                    *p = val32 & 7;
                }
            }
            0x2000..=0x1f_ffff => {
                let ctx = ((off - 0x2000) / 0x80) as usize;
                let w = ((off - 0x2000) % 0x80) / 4;
                if ctx < NUM_CONTEXTS && w < 2 {
                    let shift = w * 32;
                    let mask = 0xffff_ffffu64 << shift;
                    self.enable[ctx] = (self.enable[ctx] & !mask) | (((val32 as u64) << shift) & mask);
                    self.enable[ctx] &= !1; // source 0 does not exist
                }
            }
            0x20_0000..=0x3ff_ffff => {
                let ctx = ((off - 0x20_0000) / 0x1000) as usize;
                let reg = (off - 0x20_0000) % 0x1000;
                if ctx < NUM_CONTEXTS {
                    if reg == 0 {
                        self.threshold[ctx] = val32 & 7;
                    } else if reg == 4 && (val32 as usize) < NUM_SOURCES {
                        self.claimed &= !(1u64 << val32);
                    }
                }
            }
            _ => {}
        }
    }
}
