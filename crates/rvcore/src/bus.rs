//! Physical address space: RAM plus memory-mapped devices.
//!
//! | Range                      | Device              |
//! |----------------------------|---------------------|
//! | 0x0000_1000 .. 0x0001_0000 | boot ROM (DTB etc.) |
//! | 0x0200_0000 .. 0x0201_0000 | CLINT               |
//! | 0x0c00_0000 .. 0x1000_0000 | PLIC                |
//! | 0x1000_0000 .. 0x1000_0100 | UART 16550          |
//! | 0x1000_1000 .. 0x1000_2000 | virtio-mmio block   |
//! | 0x8000_0000 .. +ram_size   | RAM                 |

use crate::devices::{clint, clint::Clint, plic, plic::Plic, uart, uart::Uart, virtio, virtio::VirtioBlk};

pub const RAM_BASE: u64 = 0x8000_0000;
pub const ROM_BASE: u64 = 0x1000;
pub const ROM_SIZE: u64 = 0xf000;

pub struct Bus {
    pub ram: Vec<u8>,
    pub rom: Vec<u8>,
    pub clint: Clint,
    pub plic: Plic,
    pub uart: Uart,
    pub virtio: Option<VirtioBlk>,
    /// Physical address of the HTIF `tohost` word (riscv-tests), if any.
    pub tohost: Option<u64>,
    pub fromhost: Option<u64>,
    /// Set once the guest writes an exit code to `tohost`.
    pub htif_exit: Option<u64>,
    /// Set by any MMIO store; tells the CPU loop to resync device state.
    pub mmio_dirty: bool,
    /// Saves old page contents for rewinding (see `timetravel`).
    pub journal: crate::journal::Journal,
}

impl Bus {
    pub fn new(ram_size: usize) -> Self {
        Bus {
            ram: vec![0; ram_size],
            rom: Vec::new(),
            clint: Clint::new(),
            plic: Plic::new(),
            uart: Uart::new(),
            virtio: None,
            tohost: None,
            fromhost: None,
            htif_exit: None,
            mmio_dirty: false,
            journal: Default::default(),
        }
    }

    /// Copy bytes into RAM at a physical address. Returns false if out of range.
    pub fn load_ram(&mut self, pa: u64, data: &[u8]) -> bool {
        let Some(off) = pa.checked_sub(RAM_BASE) else { return false };
        let off = off as usize;
        if off + data.len() > self.ram.len() {
            return false;
        }
        self.ram[off..off + data.len()].copy_from_slice(data);
        true
    }

    #[inline(always)]
    pub fn read(&mut self, pa: u64, size: usize) -> Result<u64, ()> {
        let off = pa.wrapping_sub(RAM_BASE);
        if off < self.ram.len() as u64 {
            let o = off as usize;
            if let Some(b) = self.ram.get(o..o + size) {
                return Ok(match size {
                    1 => b[0] as u64,
                    2 => u16::from_le_bytes([b[0], b[1]]) as u64,
                    4 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64,
                    _ => u64::from_le_bytes(b.try_into().unwrap()),
                });
            }
        }
        self.read_slow(pa, size)
    }

    #[inline(always)]
    pub fn write(&mut self, pa: u64, size: usize, val: u64) -> Result<(), ()> {
        let off = pa.wrapping_sub(RAM_BASE);
        if off < self.ram.len() as u64 {
            let o = off as usize;
            // Writes are naturally aligned here, so they stay in one page.
            self.journal.ram_write(&self.ram, o);
            if let Some(b) = self.ram.get_mut(o..o + size) {
                b.copy_from_slice(&val.to_le_bytes()[..size]);
                if self.tohost.is_some() {
                    self.check_htif(pa);
                }
                return Ok(());
            }
        }
        self.write_slow(pa, size, val)
    }

    #[cold]
    fn read_slow(&mut self, pa: u64, size: usize) -> Result<u64, ()> {
        // Byte-wise read for accesses straddling the end of RAM.
        if pa.wrapping_sub(RAM_BASE) < self.ram.len() as u64 {
            return Err(());
        }
        if (clint::BASE..clint::BASE + clint::SIZE).contains(&pa) {
            return self.clint.read(pa - clint::BASE, size).ok_or(());
        }
        if (plic::BASE..plic::BASE + plic::SIZE).contains(&pa) {
            return self.plic.read(pa - plic::BASE, size).ok_or(());
        }
        if (uart::BASE..uart::BASE + uart::SIZE).contains(&pa) {
            return Ok(self.uart.read(pa - uart::BASE) as u64);
        }
        if (virtio::BASE..virtio::BASE + virtio::SIZE).contains(&pa) {
            return match &self.virtio {
                Some(v) => {
                    let off = pa - virtio::BASE;
                    let w = v.read(off & !3) as u64;
                    Ok(crate::devices::reg_read(w, off & 3, size))
                }
                None => Err(()),
            };
        }
        if (ROM_BASE..ROM_BASE + ROM_SIZE).contains(&pa) {
            let o = (pa - ROM_BASE) as usize;
            let mut v = 0u64;
            for i in 0..size {
                v |= (*self.rom.get(o + i).unwrap_or(&0) as u64) << (8 * i);
            }
            return Ok(v);
        }
        Err(())
    }

    #[cold]
    fn write_slow(&mut self, pa: u64, size: usize, val: u64) -> Result<(), ()> {
        if pa.wrapping_sub(RAM_BASE) < self.ram.len() as u64 {
            return Err(());
        }
        self.mmio_dirty = true;
        if (clint::BASE..clint::BASE + clint::SIZE).contains(&pa) {
            self.clint.write(pa - clint::BASE, size, val);
            return Ok(());
        }
        if (plic::BASE..plic::BASE + plic::SIZE).contains(&pa) {
            self.plic.write(pa - plic::BASE, size, val);
            return Ok(());
        }
        if (uart::BASE..uart::BASE + uart::SIZE).contains(&pa) {
            self.uart.write(pa - uart::BASE, val as u8);
            return Ok(());
        }
        if (virtio::BASE..virtio::BASE + virtio::SIZE).contains(&pa) {
            return match &mut self.virtio {
                Some(v) => {
                    v.write(pa - virtio::BASE, val as u32, &mut self.ram, &mut self.journal);
                    Ok(())
                }
                None => Err(()),
            };
        }
        Err(())
    }

    #[cold]
    fn check_htif(&mut self, pa: u64) {
        let Some(tohost) = self.tohost else { return };
        if pa & !7 != tohost {
            return;
        }
        let val = self.read(tohost, 8).unwrap_or(0);
        if val == 0 {
            return;
        }
        if val & 1 == 1 {
            self.htif_exit = Some(val >> 1);
        } else {
            // Syscall request: magic_mem[0]=num, [1..]=args. Support write(1/2, buf, len).
            let which = self.read(val, 8).unwrap_or(0);
            if which == 64 {
                let buf = self.read(val + 16, 8).unwrap_or(0);
                let len = self.read(val + 24, 8).unwrap_or(0);
                for i in 0..len.min(1 << 20) {
                    let b = self.read(buf + i, 1).unwrap_or(0) as u8;
                    self.uart.tx.push(b);
                }
                let _ = self.write_raw(val, len);
            }
            let _ = self.write_raw(tohost, 0);
            if let Some(fh) = self.fromhost {
                let _ = self.write_raw(fh, 1);
            }
        }
    }

    fn write_raw(&mut self, pa: u64, val: u64) -> Result<(), ()> {
        let off = pa.checked_sub(RAM_BASE).ok_or(())? as usize;
        if off + 8 > self.ram.len() {
            return Err(());
        }
        self.journal.ram_range(&self.ram, off, 8);
        self.ram[off..off + 8].copy_from_slice(&val.to_le_bytes());
        Ok(())
    }

    /// Bitmask of device interrupt lines, indexed by PLIC source number.
    pub fn irq_lines(&self) -> u64 {
        let mut lines = 0;
        if self.uart.irq() {
            lines |= 1 << uart::IRQ;
        }
        if self.virtio.as_ref().is_some_and(|v| v.irq()) {
            lines |= 1 << virtio::IRQ;
        }
        lines
    }
}
