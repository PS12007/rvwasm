//! Native command-line front end.
//!
//!   rvwasm test <elf>...                      run riscv-tests binaries
//!   rvwasm run <kernel> [--disk fs.img] ...   boot a kernel on the console
//!   rvwasm bench                              native benchmark suite

use std::io::{Read, Write};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rvcore::{build_machine, run_isa_test, Cpu, MachineConfig};

mod bench;

fn usage() -> ExitCode {
    eprintln!(
        "usage:
  rvwasm test <elf>...
  rvwasm run <kernel.elf> [--disk FILE] [--ram MB] [--linux] [--initrd FILE]
             [--max-cycles N] [--trace N --trace-out FILE]
  rvwasm bench [--bench-dir DIR] [--xv6 DIR] [--reps N] [--json FILE]"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("test") => cmd_test(&args[1..]),
        Some("run") => cmd_run(&args[1..]),
        Some("bench") => bench::cmd_bench(&args[1..]),
        _ => usage(),
    }
}

fn cmd_test(files: &[String]) -> ExitCode {
    if files.is_empty() {
        return usage();
    }
    let (mut pass, mut fail) = (0, 0);
    for f in files {
        let name = std::path::Path::new(f).file_name().unwrap().to_string_lossy().into_owned();
        let bytes = match std::fs::read(f) {
            Ok(b) => b,
            Err(e) => {
                println!("FAIL {name}: {e}");
                fail += 1;
                continue;
            }
        };
        match run_isa_test(&bytes, 50_000_000) {
            Ok(()) => {
                println!("PASS {name}");
                pass += 1;
            }
            Err(e) => {
                println!("FAIL {name}: {e}");
                fail += 1;
            }
        }
    }
    println!("\n{pass} passed, {fail} failed");
    if fail == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// Options shared by `run` and `bench`.
pub struct Opts {
    pub kernel: String,
    pub disk: Option<String>,
    pub ram_mb: usize,
    pub linux: bool,
    pub initrd: Option<String>,
    pub max_cycles: u64,
    pub trace: usize,
    pub trace_out: Option<String>,
    pub rest: Vec<(String, String)>,
    pub flags: Vec<String>,
}

pub fn parse_opts(args: &[String]) -> Option<Opts> {
    let mut o = Opts {
        kernel: String::new(),
        disk: None,
        ram_mb: 128,
        linux: false,
        initrd: None,
        max_cycles: u64::MAX,
        trace: 0,
        trace_out: None,
        rest: vec![],
        flags: vec![],
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--disk" => o.disk = Some(it.next()?.clone()),
            "--ram" => o.ram_mb = it.next()?.parse().ok()?,
            "--linux" => o.linux = true,
            "--initrd" => o.initrd = Some(it.next()?.clone()),
            "--max-cycles" => o.max_cycles = it.next()?.parse().ok()?,
            "--trace" => o.trace = it.next()?.parse().ok()?,
            "--trace-out" => o.trace_out = Some(it.next()?.clone()),
            "--json" => o.flags.push("json".into()),
            s if s.starts_with("--") => o.rest.push((s[2..].to_string(), it.next()?.clone())),
            s => o.kernel = s.to_string(),
        }
    }
    (!o.kernel.is_empty()).then_some(o)
}

pub fn make_machine(o: &Opts) -> Result<Cpu, String> {
    let kernel = std::fs::read(&o.kernel).map_err(|e| format!("{}: {e}", o.kernel))?;
    let disk = match &o.disk {
        Some(p) => Some(std::fs::read(p).map_err(|e| format!("{p}: {e}"))?),
        None => None,
    };
    if o.linux {
        let initrd = match &o.initrd {
            Some(p) => Some(std::fs::read(p).map_err(|e| format!("{p}: {e}"))?),
            None => None,
        };
        return rvcore::linux::build_linux_machine(o.ram_mb << 20, &kernel, initrd.as_deref(), disk, "console=ttyS0 earlycon=sbi");
    }
    build_machine(MachineConfig { ram_size: o.ram_mb << 20, kernel, disk })
}

/// Put the terminal in raw mode for the lifetime of this guard.
struct RawTerminal(bool);

impl RawTerminal {
    fn enable() -> Self {
        let ok = std::process::Command::new("stty")
            .args(["raw", "-echo"])
            .stdin(std::process::Stdio::inherit())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        RawTerminal(ok)
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        if self.0 {
            let _ = std::process::Command::new("stty").arg("sane").stdin(std::process::Stdio::inherit()).status();
        }
    }
}

fn cmd_run(args: &[String]) -> ExitCode {
    let Some(o) = parse_opts(args) else { return usage() };
    let mut cpu = match make_machine(&o) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    if o.trace > 0 {
        cpu.trace.start(o.trace);
    }

    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let _raw = interactive.then(RawTerminal::enable);
    let (tx, rx) = mpsc::channel::<u8>();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 64];
        while let Ok(n) = stdin.read(&mut buf) {
            if n == 0 {
                break;
            }
            for &b in &buf[..n] {
                if tx.send(b).is_err() {
                    return;
                }
            }
        }
    });

    let mut stdout = std::io::stdout();
    let start = Instant::now();
    let mut ctrl_a = false;
    let mut last_input = Instant::now();
    'outer: while cpu.cycle() < o.max_cycles && !cpu.halted {
        // Feed console input slowly enough for the guest to keep up with pastes.
        if cpu.bus.uart.rx.len() < 16 {
            while let Ok(b) = rx.try_recv() {
                // Ctrl-A x exits, like QEMU.
                if ctrl_a && b == b'x' {
                    break 'outer;
                }
                ctrl_a = b == 1;
                if !ctrl_a {
                    cpu.bus.uart.push_input(&[b]);
                }
                last_input = Instant::now();
            }
        }
        let budget = (o.max_cycles - cpu.cycle()).min(2_000_000);
        cpu.run(budget);
        let out = cpu.bus.uart.take_output();
        if !out.is_empty() {
            let _ = stdout.write_all(&out);
            let _ = stdout.flush();
        }
        if cpu.bus.htif_exit.is_some() {
            break;
        }
        // Idle guests (WFI) consume cycles instantly; don't spin the host.
        if cpu.wfi && last_input.elapsed() > Duration::from_millis(50) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    drop(_raw);
    let secs = start.elapsed().as_secs_f64();
    eprintln!(
        "\r\n[rvwasm] {} instructions in {:.2}s ({:.1} MIPS), pc={:#x}",
        cpu.instret,
        secs,
        cpu.instret as f64 / secs / 1e6,
        cpu.pc
    );
    if let (true, Some(path)) = (o.trace > 0, &o.trace_out) {
        let syms = std::fs::read(&o.kernel)
            .ok()
            .and_then(|b| rvcore::elf::Elf::parse(&b).ok().map(|e| function_symbols(&e)))
            .unwrap_or_default();
        if let Err(e) = std::fs::write(path, cpu.trace.to_json(&syms)) {
            eprintln!("could not write trace: {e}");
        } else {
            eprintln!("[rvwasm] wrote trace of {} instructions to {path}", cpu.trace.instructions().len());
        }
    }
    ExitCode::SUCCESS
}

pub fn function_symbols(elf: &rvcore::elf::Elf) -> Vec<(String, u64)> {
    let mut s: Vec<_> = elf.symbols().into_iter().filter(|(n, v)| *v != 0 && !n.starts_with('.')).collect();
    s.sort_by_key(|(_, v)| *v);
    s
}
