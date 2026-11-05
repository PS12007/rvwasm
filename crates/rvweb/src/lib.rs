//! WebAssembly bindings: a thin `Emulator` class for the browser worker.

use rvcore::{build_machine, disasm::disasm, Cpu, MachineConfig};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Emulator {
    cpu: Cpu,
    symbols: Vec<(String, u64)>,
}

fn sorted_symbols(kernel: &[u8]) -> Vec<(String, u64)> {
    let mut s: Vec<_> = rvcore::elf::Elf::parse(kernel)
        .map(|e| e.symbols())
        .unwrap_or_default()
        .into_iter()
        .filter(|(n, v)| *v != 0 && !n.starts_with('.') && !n.starts_with("$"))
        .collect();
    s.sort_by_key(|(_, v)| *v);
    s
}

#[wasm_bindgen]
impl Emulator {
    /// Boot a bare-metal kernel ELF (e.g. xv6) with an optional virtio disk image.
    #[wasm_bindgen(constructor)]
    pub fn new(kernel: &[u8], disk: Option<Vec<u8>>, ram_mb: u32) -> Result<Emulator, JsError> {
        let cpu = build_machine(MachineConfig { ram_size: (ram_mb as usize) << 20, kernel: kernel.to_vec(), disk })
            .map_err(|e| JsError::new(&e))?;
        Ok(Emulator { cpu, symbols: sorted_symbols(kernel) })
    }

    /// Boot a Linux `Image` with the built-in SBI and an optional initramfs.
    pub fn new_linux(
        kernel: &[u8],
        initrd: Option<Vec<u8>>,
        ram_mb: u32,
        cmdline: &str,
    ) -> Result<Emulator, JsError> {
        let cpu = rvcore::linux::build_linux_machine((ram_mb as usize) << 20, kernel, initrd.as_deref(), None, cmdline)
            .map_err(|e| JsError::new(&e))?;
        Ok(Emulator { cpu, symbols: Vec::new() })
    }

    /// Run for up to `cycles` cycles; returns cycles consumed.
    pub fn run(&mut self, cycles: f64) -> f64 {
        self.cpu.run(cycles as u64) as f64
    }

    pub fn push_input(&mut self, bytes: &[u8]) {
        self.cpu.bus.uart.push_input(bytes);
    }

    pub fn input_pending(&self) -> usize {
        self.cpu.bus.uart.rx.len()
    }

    pub fn take_output(&mut self) -> Vec<u8> {
        self.cpu.bus.uart.take_output()
    }

    pub fn instret(&self) -> f64 {
        self.cpu.instret as f64
    }

    pub fn cycle(&self) -> f64 {
        self.cpu.cycle() as f64
    }

    /// Current privilege mode: 0 = U, 1 = S, 3 = M.
    pub fn privilege(&self) -> u8 {
        self.cpu.prv
    }

    /// True while the hart is waiting in WFI.
    pub fn idle(&self) -> bool {
        self.cpu.wfi
    }

    pub fn halted(&self) -> bool {
        self.cpu.halted || self.cpu.bus.htif_exit.is_some()
    }

    pub fn pc(&self) -> u64 {
        self.cpu.pc
    }

    /// x0..x31 as a BigUint64Array.
    pub fn registers(&self) -> Vec<u64> {
        self.cpu.x.to_vec()
    }

    /// Instructions executed per privilege mode: [U, S, M].
    pub fn privilege_counts(&self) -> Vec<f64> {
        let c = &self.cpu.prv_instret;
        vec![c[0] as f64, c[1] as f64, c[3] as f64]
    }

    /// JSON object of CSR name -> hex value, plus a few device counters.
    pub fn csrs_json(&mut self) -> String {
        let mut s = String::from("{");
        for (i, (name, v)) in self.cpu.csr_dump().iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!("\"{name}\":\"{v:x}\""));
        }
        let disk = self.cpu.bus.virtio.as_ref().map(|v| v.requests).unwrap_or(0);
        s.push_str(&format!(",\"_traps\":{},\"_disk_requests\":{}}}", self.cpu.traps_taken, disk));
        s
    }

    /// Disassembly of `count` instructions starting at `pc - 4*before`.
    /// One line per instruction: "addr|hex|text|symbol".
    pub fn disassemble_around_pc(&mut self, before: u32, count: u32) -> String {
        let start = self.cpu.pc.wrapping_sub(4 * before as u64);
        let mut out = String::new();
        for i in 0..count as u64 {
            let a = start.wrapping_add(4 * i);
            let line = match self.cpu.peek_virt(a, 4) {
                Some(w) => format!("{:x}|{:08x}|{}|{}\n", a, w, disasm(w as u32, a), self.symbol_for(a)),
                None => format!("{:x}||<unmapped>|\n", a),
            };
            out.push_str(&line);
        }
        out
    }

    fn symbol_for(&self, addr: u64) -> String {
        match self.symbols.partition_point(|(_, v)| *v <= addr) {
            0 => String::new(),
            i => {
                let (n, v) = &self.symbols[i - 1];
                if addr - v < 0x10000 { format!("{}+{:#x}", n, addr - v) } else { String::new() }
            }
        }
    }

    pub fn trace_start(&mut self, capacity: u32) {
        self.cpu.trace.start(capacity as usize);
    }

    pub fn trace_stop(&mut self) {
        self.cpu.trace.stop();
    }

    pub fn trace_active(&self) -> bool {
        self.cpu.trace.enabled
    }

    pub fn trace_count(&self) -> f64 {
        self.cpu.trace.total_recorded as f64
    }

    pub fn trace_json(&self) -> String {
        self.cpu.trace.to_json(&self.symbols)
    }

    /// Snapshot of the disk image (to let users download a modified fs.img).
    pub fn disk_image(&self) -> Vec<u8> {
        self.cpu.bus.virtio.as_ref().map(|v| v.disk.clone()).unwrap_or_default()
    }
}

/// Disassemble a single instruction (used by the trace viewer).
#[wasm_bindgen]
pub fn disassemble(inst: u32, pc: u64) -> String {
    disasm(inst, pc)
}

/// Run a CPU-bound benchmark: boot the given kernel and run for `cycles`.
/// Returns elapsed instructions; timing is measured by the caller.
#[wasm_bindgen]
pub fn bench_run(kernel: &[u8], disk: Option<Vec<u8>>, input: &str, cycles: f64) -> Result<f64, JsError> {
    let mut cpu = build_machine(MachineConfig { ram_size: 128 << 20, kernel: kernel.to_vec(), disk })
        .map_err(|e| JsError::new(&e))?;
    cpu.bus.uart.push_input(input.as_bytes());
    cpu.run(cycles as u64);
    Ok(cpu.instret as f64)
}
