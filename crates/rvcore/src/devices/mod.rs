//! Memory-mapped devices, laid out like QEMU's `virt` machine.

pub mod clint;
pub mod plic;
pub mod uart;
pub mod virtio;

/// Read `size` bytes at byte offset `off` (within an 8-byte register) out of `val`.
#[inline]
pub(crate) fn reg_read(val: u64, off: u64, size: usize) -> u64 {
    let v = val >> ((off & 7) * 8);
    if size >= 8 {
        v
    } else {
        v & ((1u64 << (size * 8)) - 1)
    }
}

/// Merge a `size`-byte write at byte offset `off` into the 8-byte register `old`.
#[inline]
pub(crate) fn reg_write(old: u64, off: u64, size: usize, val: u64) -> u64 {
    if size >= 8 {
        return val;
    }
    let shift = (off & 7) * 8;
    let mask = ((1u64 << (size * 8)) - 1) << shift;
    (old & !mask) | ((val << shift) & mask)
}
