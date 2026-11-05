//! rvcore: an RV64IMA + Zicsr + Zifencei emulator.
//!
//! The machine follows QEMU's `virt` layout closely enough to boot xv6-riscv
//! unmodified (apart from building it without the C extension).

// Bus errors carry no payload: the CPU turns them into access faults.
#![allow(clippy::result_unit_err)]

pub mod bench;
pub mod bus;
pub mod cpu;
pub mod csr;
pub mod devices;
pub mod disasm;
pub mod elf;
mod exec;
pub mod fdt;
pub mod linux;
pub mod mmu;
pub mod trace;

pub use bus::{Bus, RAM_BASE};
pub use cpu::{Cpu, Trap, PRV_M, PRV_S, PRV_U};

/// Load every PT_LOAD segment of an ELF into RAM; returns the entry point.
pub fn load_elf(cpu: &mut Cpu, bytes: &[u8]) -> Result<u64, String> {
    let elf = elf::Elf::parse(bytes)?;
    for seg in &elf.segments {
        if !cpu.bus.load_ram(seg.paddr, seg.data) {
            return Err(format!("segment at {:#x} ({} bytes) does not fit in RAM", seg.paddr, seg.data.len()));
        }
        // Zero .bss (RAM starts zeroed, but be explicit for reloads).
        let bss = seg.memsz.saturating_sub(seg.data.len() as u64) as usize;
        if bss > 0 {
            cpu.bus.load_ram(seg.paddr + seg.data.len() as u64, &vec![0; bss]);
        }
    }
    Ok(elf.entry)
}

/// Configuration for a full-system machine.
pub struct MachineConfig {
    pub ram_size: usize,
    pub kernel: Vec<u8>,
    pub disk: Option<Vec<u8>>,
}

/// Build a machine with RAM, devices and the kernel ELF loaded, ready to run
/// from the kernel entry point in M-mode with a0 = hartid, a1 = 0.
pub fn build_machine(cfg: MachineConfig) -> Result<Cpu, String> {
    let mut bus = Bus::new(cfg.ram_size);
    if let Some(d) = cfg.disk {
        bus.virtio = Some(devices::virtio::VirtioBlk::new(d));
    }
    let mut cpu = Cpu::new(bus);
    cpu.pc = load_elf(&mut cpu, &cfg.kernel)?;
    Ok(cpu)
}

/// Run a riscv-tests style ELF until it reports via `tohost`.
/// Returns Ok(()) on pass, Err(test number or reason) otherwise.
pub fn run_isa_test(bytes: &[u8], max_cycles: u64) -> Result<(), String> {
    let elf = elf::Elf::parse(bytes)?;
    let tohost = elf.symbol("tohost").ok_or("no tohost symbol")?;
    let mut cpu = Cpu::new(Bus::new(16 << 20));
    cpu.pc = load_elf(&mut cpu, bytes)?;
    cpu.bus.tohost = Some(tohost);
    cpu.bus.fromhost = elf.symbol("fromhost");
    cpu.run(max_cycles);
    match cpu.bus.htif_exit {
        Some(0) => Ok(()),
        Some(n) => Err(format!("failed test #{} (pc={:#x})", n, cpu.pc)),
        None => Err(format!("timed out after {} cycles (pc={:#x})", max_cycles, cpu.pc)),
    }
}
