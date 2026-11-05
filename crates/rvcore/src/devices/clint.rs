//! Core-local interruptor: machine software interrupt and machine timer.

use super::{reg_read, reg_write};

pub const BASE: u64 = 0x0200_0000;
pub const SIZE: u64 = 0x1_0000;

#[derive(Clone, Default)]
pub struct Clint {
    pub msip: u32,
    pub mtimecmp: u64,
    pub mtime: u64,
}

impl Clint {
    pub fn new() -> Self {
        Clint { msip: 0, mtimecmp: u64::MAX, mtime: 0 }
    }

    pub fn read(&self, off: u64, size: usize) -> Option<u64> {
        Some(match off {
            0x0..=0x3 => reg_read(self.msip as u64, off, size),
            0x4000..=0x4007 => reg_read(self.mtimecmp, off, size),
            0xbff8..=0xbfff => reg_read(self.mtime, off, size),
            _ => 0,
        })
    }

    pub fn write(&mut self, off: u64, size: usize, val: u64) {
        match off {
            0x0..=0x3 => self.msip = (val & 1) as u32,
            0x4000..=0x4007 => self.mtimecmp = reg_write(self.mtimecmp, off, size, val),
            0xbff8..=0xbfff => self.mtime = reg_write(self.mtime, off, size, val),
            _ => {}
        }
    }
}
