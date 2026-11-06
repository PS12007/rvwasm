//! Booting Linux: device tree generation and an SBI firmware implemented in
//! the emulator itself, so the kernel starts directly in S-mode without
//! OpenSBI.

use crate::bus::{Bus, RAM_BASE};
use crate::cpu::{cause, Cpu, PRV_S};
use crate::csr::*;
use crate::devices::{plic, uart, virtio};
use crate::fdt::Fdt;

pub const TIMEBASE_HZ: u32 = 10_000_000;

const PHANDLE_INTC: u32 = 1;
const PHANDLE_PLIC: u32 = 2;

// SBI error codes
const SBI_SUCCESS: i64 = 0;
const SBI_ERR_NOT_SUPPORTED: i64 = -2;
const SBI_ERR_INVALID_PARAM: i64 = -3;
const SBI_ERR_ALREADY_AVAILABLE: i64 = -6;

// SBI extension IDs
const EXT_SET_TIMER_LEGACY: u64 = 0x00;
const EXT_PUTCHAR_LEGACY: u64 = 0x01;
const EXT_GETCHAR_LEGACY: u64 = 0x02;
const EXT_SHUTDOWN_LEGACY: u64 = 0x08;
const EXT_BASE: u64 = 0x10;
const EXT_TIME: u64 = 0x5449_4d45;
const EXT_IPI: u64 = 0x0073_5049;
const EXT_RFENCE: u64 = 0x5246_4e43;
const EXT_HSM: u64 = 0x0048_534d;
const EXT_SRST: u64 = 0x5352_5354;
const EXT_DBCN: u64 = 0x4442_434e;

const SUPPORTED: [u64; 7] = [EXT_BASE, EXT_TIME, EXT_IPI, EXT_RFENCE, EXT_HSM, EXT_SRST, EXT_DBCN];

/// Build the device tree describing this machine.
pub fn device_tree(ram_size: u64, bootargs: &str, initrd: Option<(u64, u64)>, has_disk: bool) -> Vec<u8> {
    let mut f = Fdt::new();
    f.begin_node("");
    f.prop_u32("#address-cells", 2);
    f.prop_u32("#size-cells", 2);
    f.prop_str("compatible", "riscv-virtio");
    f.prop_str("model", "rvwasm");

    f.begin_node("chosen");
    f.prop_str("bootargs", bootargs);
    f.prop_str("stdout-path", "/soc/serial@10000000");
    if let Some((start, end)) = initrd {
        f.prop_u64("linux,initrd-start", start);
        f.prop_u64("linux,initrd-end", end);
    }
    f.end_node();

    f.begin_node(&format!("memory@{RAM_BASE:x}"));
    f.prop_str("device_type", "memory");
    f.prop_cells("reg", &[0, RAM_BASE as u32, (ram_size >> 32) as u32, ram_size as u32]);
    f.end_node();

    f.begin_node("cpus");
    f.prop_u32("#address-cells", 1);
    f.prop_u32("#size-cells", 0);
    f.prop_u32("timebase-frequency", TIMEBASE_HZ);
    f.begin_node("cpu@0");
    f.prop_str("device_type", "cpu");
    f.prop_u32("reg", 0);
    f.prop_str("status", "okay");
    f.prop_str("compatible", "riscv");
    f.prop_str("riscv,isa", "rv64imafdc_zicsr_zifencei_zicntr_zicond_zba_zbb_zbc_zbs");
    f.prop_str("riscv,isa-base", "rv64i");
    f.prop_strs(
        "riscv,isa-extensions",
        &["i", "m", "a", "f", "d", "c", "zicsr", "zifencei", "zicntr", "zicond", "zba", "zbb", "zbc", "zbs"],
    );
    f.prop_str("mmu-type", "riscv,sv39");
    f.begin_node("interrupt-controller");
    f.prop_u32("#interrupt-cells", 1);
    f.prop_empty("interrupt-controller");
    f.prop_str("compatible", "riscv,cpu-intc");
    f.prop_u32("phandle", PHANDLE_INTC);
    f.end_node();
    f.end_node();
    f.end_node();

    f.begin_node("soc");
    f.prop_u32("#address-cells", 2);
    f.prop_u32("#size-cells", 2);
    f.prop_str("compatible", "simple-bus");
    f.prop_empty("ranges");

    f.begin_node(&format!("plic@{:x}", plic::BASE));
    f.prop_strs("compatible", &["sifive,plic-1.0.0", "riscv,plic0"]);
    f.prop_cells("reg", &[0, plic::BASE as u32, 0, plic::SIZE as u32]);
    f.prop_u32("#interrupt-cells", 1);
    f.prop_u32("#address-cells", 0);
    f.prop_empty("interrupt-controller");
    // Context 0 = M-mode external (11), context 1 = S-mode external (9).
    f.prop_cells("interrupts-extended", &[PHANDLE_INTC, 11, PHANDLE_INTC, 9]);
    f.prop_u32("riscv,ndev", 31);
    f.prop_u32("phandle", PHANDLE_PLIC);
    f.end_node();

    f.begin_node(&format!("serial@{:x}", uart::BASE));
    f.prop_str("compatible", "ns16550a");
    f.prop_cells("reg", &[0, uart::BASE as u32, 0, uart::SIZE as u32]);
    f.prop_u32("clock-frequency", 3_686_400);
    f.prop_u32("interrupts", uart::IRQ);
    f.prop_u32("interrupt-parent", PHANDLE_PLIC);
    f.end_node();

    if has_disk {
        f.begin_node(&format!("virtio_mmio@{:x}", virtio::BASE));
        f.prop_str("compatible", "virtio,mmio");
        f.prop_cells("reg", &[0, virtio::BASE as u32, 0, virtio::SIZE as u32]);
        f.prop_u32("interrupts", virtio::IRQ);
        f.prop_u32("interrupt-parent", PHANDLE_PLIC);
        f.end_node();
    }
    f.end_node(); // soc
    f.end_node(); // root
    f.finish()
}

/// Build a machine that boots a RISC-V Linux `Image` in S-mode.
///
/// Memory layout: kernel at RAM_BASE + text_offset (from the Image header),
/// device tree in the last 64 KiB of RAM, initramfs just below it.
pub fn build_linux_machine(
    ram_size: usize,
    image: &[u8],
    initrd: Option<&[u8]>,
    disk: Option<Vec<u8>>,
    bootargs: &str,
) -> Result<Cpu, String> {
    if image.len() < 64 || &image[48..56] != b"RISCV\0\0\0" {
        return Err("not a RISC-V Linux Image (missing RISCV magic)".into());
    }
    let text_offset = u64::from_le_bytes(image[8..16].try_into().unwrap());
    let image_size = u64::from_le_bytes(image[16..24].try_into().unwrap()).max(image.len() as u64);
    let kernel_pa = RAM_BASE + text_offset;
    let ram_end = RAM_BASE + ram_size as u64;
    let dtb_pa = ram_end - 0x1_0000;

    let mut bus = Bus::new(ram_size);
    if !bus.load_ram(kernel_pa, image) {
        return Err("kernel does not fit in RAM".into());
    }
    let initrd_range = match initrd {
        Some(data) => {
            let start = (dtb_pa - data.len() as u64) & !0xfff;
            if start < kernel_pa + image_size + (16 << 20) {
                return Err("not enough RAM for kernel + initramfs".into());
            }
            bus.load_ram(start, data);
            Some((start, start + data.len() as u64))
        }
        None => None,
    };
    let has_disk = disk.is_some();
    if let Some(d) = disk {
        bus.virtio = Some(virtio::VirtioBlk::new(d));
    }
    let dtb = device_tree(ram_size as u64, bootargs, initrd_range, has_disk);
    bus.load_ram(dtb_pa, &dtb);
    bus.rom = dtb;

    let mut cpu = Cpu::new(bus);
    // Act as the M-mode firmware would before jumping to the kernel.
    cpu.medeleg = 0xffff & !((1 << cause::ECALL_S) | (1 << cause::ECALL_M) | (1 << 10) | (1 << 14));
    cpu.mideleg = S_INTS;
    cpu.mcounteren = 0xffff_ffff;
    cpu.menvcfg = MENVCFG_STCE | MENVCFG_ADUE;
    cpu.mie = 0;
    cpu.sbi = Some(sbi_call);
    cpu.prv = PRV_S;
    cpu.x[10] = 0; // a0: hart id
    cpu.x[11] = dtb_pa; // a1: device tree
    cpu.pc = kernel_pa;
    cpu.csr_write(SATP, 0);
    Ok(cpu)
}

fn ret(cpu: &mut Cpu, err: i64, val: u64) -> bool {
    cpu.x[10] = err as u64;
    cpu.x[11] = val;
    true
}

/// Handle an `ecall` from S-mode as SBI v2.0 firmware would.
pub fn sbi_call(cpu: &mut Cpu) -> bool {
    let (eid, fid) = (cpu.x[17], cpu.x[16]);
    let a0 = cpu.x[10];
    match eid {
        EXT_SET_TIMER_LEGACY => {
            cpu.stimecmp = a0;
            cpu.x[10] = 0;
            true
        }
        EXT_PUTCHAR_LEGACY => {
            cpu.bus.uart.tx.push(a0 as u8);
            cpu.x[10] = 0;
            true
        }
        EXT_GETCHAR_LEGACY => {
            cpu.x[10] = cpu.bus.uart.rx.pop_front().map(|b| b as u64).unwrap_or(u64::MAX);
            true
        }
        EXT_SHUTDOWN_LEGACY => {
            cpu.halted = true;
            true
        }
        EXT_BASE => match fid {
            0 => ret(cpu, SBI_SUCCESS, 2 << 24), // spec version 2.0
            1 => ret(cpu, SBI_SUCCESS, 0x7277),  // implementation id ("rw")
            2 => ret(cpu, SBI_SUCCESS, 1),
            3 => {
                let supported = SUPPORTED.contains(&a0) || a0 <= EXT_SHUTDOWN_LEGACY;
                ret(cpu, SBI_SUCCESS, supported as u64)
            }
            4..=6 => ret(cpu, SBI_SUCCESS, 0),
            _ => ret(cpu, SBI_ERR_NOT_SUPPORTED, 0),
        },
        EXT_TIME if fid == 0 => {
            cpu.stimecmp = a0;
            cpu.break_now();
            ret(cpu, SBI_SUCCESS, 0)
        }
        EXT_IPI if fid == 0 => {
            // Single hart: an IPI to ourselves raises SSIP.
            let (mask, base) = (cpu.x[10], cpu.x[11]);
            if base == u64::MAX || (base == 0 && mask & 1 != 0) {
                cpu.mip_sw |= MIP_SSIP;
            }
            ret(cpu, SBI_SUCCESS, 0)
        }
        EXT_RFENCE => {
            cpu.flush_tlb();
            ret(cpu, SBI_SUCCESS, 0)
        }
        EXT_HSM => match fid {
            0 => ret(cpu, SBI_ERR_ALREADY_AVAILABLE, 0), // hart_start
            2 => {
                if a0 == 0 {
                    ret(cpu, SBI_SUCCESS, 0) // started
                } else {
                    ret(cpu, SBI_ERR_INVALID_PARAM, 0)
                }
            }
            _ => ret(cpu, SBI_ERR_NOT_SUPPORTED, 0),
        },
        EXT_SRST if fid == 0 => {
            cpu.halted = true;
            ret(cpu, SBI_SUCCESS, 0)
        }
        EXT_DBCN => match fid {
            0 => {
                // console_write(num_bytes, base_lo, base_hi)
                let (len, addr) = (cpu.x[10], cpu.x[11] | (cpu.x[12] << 32));
                let mut n = 0;
                while n < len.min(4096) {
                    match cpu.bus.read(addr + n, 1) {
                        Ok(b) => cpu.bus.uart.tx.push(b as u8),
                        Err(_) => break,
                    }
                    n += 1;
                }
                ret(cpu, SBI_SUCCESS, n)
            }
            1 => {
                // console_read(num_bytes, base_lo, base_hi)
                let (len, addr) = (cpu.x[10], cpu.x[11] | (cpu.x[12] << 32));
                let mut n = 0;
                while n < len {
                    let Some(b) = cpu.bus.uart.rx.pop_front() else { break };
                    let _ = cpu.bus.write(addr + n, 1, b as u64);
                    n += 1;
                }
                ret(cpu, SBI_SUCCESS, n)
            }
            2 => {
                cpu.bus.uart.tx.push(a0 as u8);
                ret(cpu, SBI_SUCCESS, 0)
            }
            _ => ret(cpu, SBI_ERR_NOT_SUPPORTED, 0),
        },
        _ => ret(cpu, SBI_ERR_NOT_SUPPORTED, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dtb_header() {
        let d = device_tree(128 << 20, "console=ttyS0", Some((0x8700_0000, 0x8710_0000)), true);
        assert_eq!(&d[0..4], &[0xd0, 0x0d, 0xfe, 0xed]);
        assert_eq!(u32::from_be_bytes(d[4..8].try_into().unwrap()) as usize, d.len());
    }
}
