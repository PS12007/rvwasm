//! Benchmark workloads shared by the native CLI and the WebAssembly build, so
//! both measure exactly the same emulator code on the same guest programs.
//!
//! Only guest execution is timed: machine setup (allocating and loading RAM)
//! is excluded, since its cost differs between native and WebAssembly in ways
//! unrelated to emulation speed. The caller supplies the clock.

use crate::{build_machine, elf::Elf, load_elf, Bus, Cpu, MachineConfig};

/// The bare-metal riscv-tests benchmarks in the suite.
pub const BAREMETAL: [&str; 9] =
    ["dhrystone", "median", "memcpy", "multiply", "qsort", "rsort", "spmv", "towers", "vvadd"];

/// The xv6 workload: boot, then run a few programs from the shell.
pub const XV6_SCRIPT: &str = "forktest\nwc README\ngrep xv6 README\nstressfs\necho bench-done\n";
pub const XV6_UNTIL: &str = "$ bench-done";

#[derive(Clone, Copy, Debug)]
pub struct Measurement {
    /// Instructions retired by one run of the workload.
    pub instret: u64,
    /// Seconds of guest execution per run.
    pub secs: f64,
    /// How many runs were averaged.
    pub runs: u32,
}

impl Measurement {
    pub fn mips(&self) -> f64 {
        self.instret as f64 / self.secs / 1e6
    }
}

/// Repeat `setup` + timed `exec` until `min_secs` of execution time has accumulated.
pub fn measure(
    min_secs: f64,
    now: &dyn Fn() -> f64,
    mut setup: impl FnMut() -> Result<Cpu, String>,
    mut exec: impl FnMut(&mut Cpu) -> Result<(), String>,
) -> Result<Measurement, String> {
    let mut total = 0.0;
    let mut runs = 0;
    let mut instret = 0;
    while runs == 0 || total < min_secs {
        let mut cpu = setup()?;
        let t = now();
        exec(&mut cpu)?;
        total += now() - t;
        instret = cpu.instret;
        runs += 1;
    }
    Ok(Measurement { instret, secs: total / runs as f64, runs })
}

/// Build a machine for a bare-metal riscv-tests program (exits via `tohost`).
pub fn baremetal_machine(bytes: &[u8]) -> Result<Cpu, String> {
    let elf = Elf::parse(bytes)?;
    let tohost = elf.symbol("tohost").ok_or("no tohost symbol")?;
    let mut cpu = Cpu::new(Bus::new(8 << 20));
    cpu.pc = load_elf(&mut cpu, bytes)?;
    cpu.bus.tohost = Some(tohost);
    cpu.bus.fromhost = elf.symbol("fromhost");
    Ok(cpu)
}

/// Run a bare-metal program until it reports through `tohost`.
pub fn run_to_exit(cpu: &mut Cpu, max_cycles: u64) -> Result<(), String> {
    cpu.run(max_cycles);
    match cpu.bus.htif_exit {
        Some(0) => Ok(()),
        Some(n) => Err(format!("benchmark reported failure code {n}")),
        None => Err("benchmark did not finish".into()),
    }
}

/// Type `input` on the console once the first shell prompt (`$ `) appears,
/// then run until the console output contains `until`. Returns the output.
pub fn run_script(cpu: &mut Cpu, input: &str, until: &str, max_cycles: u64) -> Result<Vec<u8>, String> {
    let mut output: Vec<u8> = Vec::new();
    let mut typed = false;
    let needle = until.as_bytes();
    while cpu.cycle() < max_cycles {
        cpu.run(1_000_000);
        let start = output.len().saturating_sub(needle.len());
        output.extend(cpu.bus.uart.take_output());
        if !typed && output.windows(2).any(|w| w == b"$ ") {
            // Typed only now: xv6 resets the UART FIFO during boot. Its
            // console buffer holds 128 bytes, more than the scripts need.
            cpu.bus.uart.push_input(input.as_bytes());
            typed = true;
        }
        if output[start..].windows(needle.len()).any(|w| w == needle) {
            return Ok(output);
        }
    }
    let tail = String::from_utf8_lossy(&output[output.len().saturating_sub(400)..]).into_owned();
    Err(format!("did not see {until:?} within {max_cycles} cycles; output ends with:\n{tail}"))
}

/// Benchmark a bare-metal riscv-tests program. `prep` configures each
/// fresh machine (the WebAssembly build uses it to turn on the JIT).
pub fn bench_baremetal(
    bytes: &[u8],
    now: &dyn Fn() -> f64,
    min_secs: f64,
    prep: &dyn Fn(&mut Cpu),
) -> Result<Measurement, String> {
    measure(
        min_secs,
        now,
        || baremetal_machine(bytes).map(|mut c| (prep(&mut c), c).1),
        |cpu| run_to_exit(cpu, 2_000_000_000),
    )
}

/// Benchmark the xv6 boot + shell workload.
pub fn bench_xv6(
    kernel: &[u8],
    disk: &[u8],
    now: &dyn Fn() -> f64,
    min_secs: f64,
    prep: &dyn Fn(&mut Cpu),
) -> Result<Measurement, String> {
    measure(
        min_secs,
        now,
        || {
            build_machine(MachineConfig { ram_size: 128 << 20, kernel: kernel.to_vec(), disk: Some(disk.to_vec()) })
                .map(|mut c| (prep(&mut c), c).1)
        },
        |cpu| run_script(cpu, XV6_SCRIPT, XV6_UNTIL, 20_000_000_000).map(|_| ()),
    )
}
