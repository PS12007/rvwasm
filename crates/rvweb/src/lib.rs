//! WebAssembly bindings: a thin `Emulator` class for the browser worker.

use rvcore::{
    build_machine,
    disasm::{disasm, inst_len},
    timetravel::TimeTravel,
    Cpu, MachineConfig,
};
use std::sync::atomic::{AtomicU32, Ordering};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Emulator {
    cpu: Cpu,
    symbols: Vec<(String, u64)>,
    /// Always recording, so the debugger can step backwards.
    tt: TimeTravel,
}

/// Instructions between checkpoints: stepping back replays at most this many.
const TT_INTERVAL: u64 = 1_000_000;
/// Memory for going back; the oldest history is dropped beyond this.
const TT_BYTES: usize = 64 << 20;

impl Emulator {
    fn with(mut cpu: Cpu, symbols: Vec<(String, u64)>) -> Emulator {
        let tt = TimeTravel::new(&mut cpu, TT_INTERVAL, TT_BYTES);
        cpu.enable_jit(install_module);
        Emulator { cpu, symbols, tt }
    }
}

// The JIT hands modules of compiled blocks to this, which compiles them
// (synchronously: fine in a worker or Node) against this module's memory
// and function table, and appends their functions to the table.
#[wasm_bindgen(inline_js = r#"
export function jit_install(bytes, memory, table) {
  try {
    const inst = new WebAssembly.Instance(new WebAssembly.Module(bytes), { e: { m: memory, t: table } });
    const n = Object.keys(inst.exports).length;
    const base = table.grow(n);
    for (let i = 0; i < n; i++) table.set(base + i, inst.exports['f' + i]);
    return base;
  } catch (e) {
    console.error('rvwasm: JIT disabled:', e);
    return 0;
  }
}
"#)]
extern "C" {
    fn jit_install(bytes: &[u8], memory: JsValue, table: JsValue) -> u32;
}

static INSTALL_FAILURES: AtomicU32 = AtomicU32::new(0);

fn install_module(bytes: &[u8]) -> Option<u32> {
    match jit_install(bytes, wasm_bindgen::memory(), wasm_bindgen::function_table()) {
        0 => {
            INSTALL_FAILURES.fetch_add(1, Ordering::Relaxed);
            None
        }
        base => Some(base),
    }
}

fn jit_prep(on: bool) -> impl Fn(&mut Cpu) {
    move |cpu: &mut Cpu| {
        if on {
            cpu.enable_jit(install_module);
        }
    }
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
        Ok(Emulator::with(cpu, sorted_symbols(kernel)))
    }

    /// Boot a Linux `Image` with the built-in SBI and an optional initramfs.
    ///
    /// `system_map` is the kernel's System.map text (optional), used to label
    /// addresses in the debug panel and traces.
    pub fn new_linux(
        kernel: &[u8],
        initrd: Option<Vec<u8>>,
        ram_mb: u32,
        cmdline: &str,
        system_map: Option<String>,
    ) -> Result<Emulator, JsError> {
        let cpu = rvcore::linux::build_linux_machine((ram_mb as usize) << 20, kernel, initrd.as_deref(), None, cmdline)
            .map_err(|e| JsError::new(&e))?;
        let mut symbols: Vec<(String, u64)> = system_map
            .as_deref()
            .unwrap_or("")
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                let addr = u64::from_str_radix(it.next()?, 16).ok()?;
                let kind = it.next()?;
                let name = it.next()?;
                matches!(kind, "t" | "T" | "W" | "w").then(|| (name.to_string(), addr))
            })
            .collect();
        symbols.sort_by_key(|(_, v)| *v);
        Ok(Emulator::with(cpu, symbols))
    }

    /// Carry on from a snapshot made by `save_snapshot` (or `rvwasm run --save`).
    pub fn from_snapshot(bytes: &[u8]) -> Result<Emulator, JsError> {
        let cpu = Cpu::from_snapshot(bytes).map_err(|e| JsError::new(&e))?;
        Ok(Emulator::with(cpu, Vec::new()))
    }

    /// True if `bytes` is a snapshot file.
    pub fn is_snapshot(bytes: &[u8]) -> bool {
        rvcore::snapshot::is_snapshot(bytes)
    }

    /// The whole machine, RAM and disk included, as a file.
    pub fn save_snapshot(&mut self) -> Vec<u8> {
        self.cpu.save_snapshot()
    }

    /// Run for up to `cycles` cycles; returns cycles consumed.
    pub fn run(&mut self, cycles: f64) -> f64 {
        self.tt.run(&mut self.cpu, cycles as u64) as f64
    }

    /// Keyboard input. While the machine is behind the present (after going
    /// back), it waits until the replay catches up.
    pub fn push_input(&mut self, bytes: &[u8]) {
        self.tt.push_input(&mut self.cpu, bytes);
    }

    // ------------------------------------------------------------ time travel

    /// Go to the point where `instret` instructions had retired, backwards
    /// or forwards (forwards stops at the present). Returns where it got to.
    pub fn seek(&mut self, instret: f64) -> f64 {
        let target = (instret.max(0.0) as u64).min(self.tt.frontier_instret());
        self.tt.seek_instret(&mut self.cpu, target) as f64
    }

    /// Run exactly `n` more instructions (or as far as the guest gets).
    pub fn step_forward(&mut self, n: u32) -> f64 {
        let target = self.cpu.instret + n as u64;
        self.tt.seek_instret(&mut self.cpu, target) as f64
    }

    pub fn step_back(&mut self, n: u32) -> f64 {
        self.tt.step_back(&mut self.cpu, n as u64) as f64
    }

    /// Back to just before the most recent trap; false if out of reach.
    pub fn back_to_last_trap(&mut self) -> bool {
        self.tt.back_to_last_trap(&mut self.cpu)
    }

    // ------------------------------------------------------------------ JIT

    /// Turn compiling hot code to WebAssembly on or off (on by default).
    pub fn set_jit(&mut self, on: bool) {
        if on {
            if self.cpu.jit.is_none() {
                self.cpu.enable_jit(install_module);
            }
        } else {
            self.cpu.disable_jit();
        }
    }

    /// Tuning for tests: blocks compile after `hot` starts, in batches of
    /// `batch` (or after `delay` instructions).
    pub fn jit_tune(&mut self, hot: u32, batch: u32, delay: f64) {
        if let Some(j) = self.cpu.jit.as_deref_mut() {
            j.hot = hot.clamp(1, 65533) as u16;
            j.batch = batch.max(1) as usize;
            j.delay = delay as u64;
        }
    }

    /// [enabled, blocks compiled, modules, blocks invalidated, instructions
    /// run as compiled code].
    pub fn jit_stats(&self) -> Vec<f64> {
        match self.cpu.jit.as_deref() {
            Some(j) => {
                let s = j.stats;
                vec![1.0, s.blocks as f64, s.modules as f64, s.invalidated as f64, s.insts as f64]
            }
            None => vec![0.0; 5],
        }
    }

    /// True if `bytes` (a bare-metal test) has exited through `tohost`;
    /// the exit code, or -1 while still running.
    pub fn exit_code(&self) -> f64 {
        self.cpu.bus.htif_exit.map_or(-1.0, |c| c as f64)
    }

    /// True while behind the present.
    pub fn replaying(&self) -> bool {
        self.tt.replaying(&self.cpu)
    }

    /// Instruction counts [oldest reachable, now, present] and bytes held.
    pub fn history(&self) -> Vec<f64> {
        vec![
            self.tt.oldest_instret() as f64,
            self.cpu.instret as f64,
            self.tt.frontier_instret() as f64,
            self.tt.bytes_held(&self.cpu) as f64,
        ]
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

    /// f0..f31 (raw bits; single-precision values are NaN-boxed).
    pub fn fp_registers(&self) -> Vec<u64> {
        self.cpu.f.to_vec()
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

    /// Disassembly of `count` instructions, `before` of them ahead of the pc.
    /// One line per instruction: "addr|hex|text|symbol".
    pub fn disassemble_around_pc(&mut self, before: u32, count: u32) -> String {
        let pc = self.cpu.pc;
        let start = self.start_before(pc, before as usize);
        let mut out = String::new();
        let mut a = start;
        for _ in 0..count {
            match self.cpu.peek_virt(a, 2) {
                Some(lo) => {
                    let w = if lo & 3 == 3 { lo | self.cpu.peek_virt(a + 2, 2).unwrap_or(0) << 16 } else { lo };
                    let hex = if lo & 3 == 3 { format!("{w:08x}") } else { format!("{w:04x}") };
                    out.push_str(&format!("{:x}|{}|{}|{}\n", a, hex, disasm(w as u32, a), self.symbol_for(a)));
                    a = a.wrapping_add(inst_len(w as u32));
                }
                None => {
                    out.push_str(&format!("{a:x}||<unmapped>|\n"));
                    a = a.wrapping_add(4);
                }
            }
        }
        out
    }

    /// Where to start decoding so that `before` instructions come ahead of
    /// `pc`. With 16- and 32-bit instructions mixed, walking backwards is
    /// ambiguous, so decode forward from a few candidate starting points and
    /// take the first that lands exactly on `pc`.
    fn start_before(&mut self, pc: u64, before: usize) -> u64 {
        if before == 0 {
            return pc;
        }
        // Starting further back gives the decoder more room to fall into step
        // with the real instruction boundaries, so try the farthest first, and
        // reject decodings whose last instructions are not valid ones.
        let mut fallback = None;
        for back in (before as u64..=4 * before as u64 + 16).rev() {
            let mut a = pc.wrapping_sub(2 * back);
            let mut starts = Vec::new();
            while a < pc {
                starts.push(a);
                match self.cpu.peek_virt(a, 2) {
                    Some(lo) => a = a.wrapping_add(inst_len(lo as u32)),
                    None => break,
                }
            }
            if a != pc || starts.len() < before {
                continue;
            }
            let first = starts[starts.len() - before];
            let valid = starts[starts.len() - before..].iter().all(|&s| {
                let lo = self.cpu.peek_virt(s, 2).unwrap_or(0);
                let w = if lo & 3 == 3 { lo | self.cpu.peek_virt(s + 2, 2).unwrap_or(0) << 16 } else { lo };
                !disasm(w as u32, s).starts_with("unknown")
            });
            if valid {
                return first;
            }
            fallback.get_or_insert(first);
        }
        fallback.unwrap_or(pc.wrapping_sub(4 * before as u64))
    }

    fn symbol_for(&self, addr: u64) -> String {
        match self.symbols.partition_point(|(_, v)| *v <= addr) {
            0 => String::new(),
            i => {
                let (n, v) = &self.symbols[i - 1];
                if addr - v < 0x10000 {
                    format!("{}+{:#x}", n, addr - v)
                } else {
                    String::new()
                }
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

    /// Write requests the guest has sent to the disk so far.
    pub fn disk_writes(&self) -> f64 {
        self.cpu.bus.virtio.as_ref().map(|v| v.writes as f64).unwrap_or(0.0)
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

#[wasm_bindgen]
extern "C" {
    /// `performance.now()` (milliseconds), available in windows, workers and Node.
    #[wasm_bindgen(js_namespace = performance, js_name = now)]
    fn perf_now() -> f64;
}

fn now_secs() -> f64 {
    perf_now() / 1000.0
}

fn measurement_json(m: rvcore::bench::Measurement) -> String {
    format!("{{\"instret\":{},\"seconds\":{},\"runs\":{}}}", m.instret, m.secs, m.runs)
}

/// Benchmark a bare-metal riscv-tests program for at least `min_secs` of
/// execution. Returns JSON `{instret, seconds, runs}` (seconds per run).
#[wasm_bindgen]
pub fn bench_baremetal(elf: &[u8], min_secs: f64, jit: bool) -> Result<String, JsError> {
    rvcore::bench::bench_baremetal(elf, &now_secs, min_secs, &jit_prep(jit))
        .map(measurement_json)
        .map_err(|e| JsError::new(&e))
}

/// Benchmark the xv6 boot + shell workload. Returns JSON like `bench_baremetal`.
#[wasm_bindgen]
pub fn bench_xv6(kernel: &[u8], disk: &[u8], jit: bool) -> Result<String, JsError> {
    rvcore::bench::bench_xv6(kernel, disk, &now_secs, 0.0, &jit_prep(jit))
        .map(measurement_json)
        .map_err(|e| JsError::new(&e))
}

/// Run a riscv-tests ELF; returns "" on pass, else the failure. With `jit`,
/// every block is compiled the first time it runs, to test the compiler.
#[wasm_bindgen]
pub fn run_isa_test(elf: &[u8], jit: bool) -> String {
    let prep = |cpu: &mut Cpu| {
        if jit {
            cpu.enable_jit(install_module);
            if let Some(j) = cpu.jit.as_deref_mut() {
                (j.hot, j.batch, j.delay) = (1, 1, 0);
            }
        }
    };
    let failures = INSTALL_FAILURES.load(Ordering::Relaxed);
    match rvcore::run_isa_test_with(elf, 100_000_000, &prep) {
        Ok(()) if INSTALL_FAILURES.load(Ordering::Relaxed) != failures => "the JIT was disabled".into(),
        Ok(()) => String::new(),
        Err(e) => e,
    }
}

/// Names of the bare-metal benchmark programs.
#[wasm_bindgen]
pub fn bench_names() -> Vec<String> {
    rvcore::bench::BAREMETAL.iter().map(|s| s.to_string()).collect()
}
