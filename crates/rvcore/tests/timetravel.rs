//! Determinism, snapshots and time travel, checked on xv6.
//!
//! Needs build/xv6 (`scripts/build-xv6.sh`), or RVWASM_XV6 pointing at a
//! directory with kernel/kernel and fs.img; skipped otherwise.

use rvcore::timetravel::TimeTravel;
use rvcore::{build_machine, Cpu, MachineConfig};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

fn xv6() -> Option<Cpu> {
    let dir = std::env::var_os("RVWASM_XV6")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../build/xv6"));
    let kernel = std::fs::read(dir.join("kernel/kernel")).ok()?;
    let disk = std::fs::read(dir.join("fs.img")).ok()?;
    Some(build_machine(MachineConfig { ram_size: 128 << 20, kernel, disk: Some(disk) }).unwrap())
}

macro_rules! machine {
    () => {
        match xv6() {
            Some(c) => c,
            None => {
                return;
            }
        }
    };
}

/// Everything that defines the machine: small state, RAM and disk.
fn fingerprint(cpu: &mut Cpu) -> (Vec<u8>, u64, u64) {
    let mut h = DefaultHasher::new();
    cpu.bus.ram.hash(&mut h);
    let ram = h.finish();
    let mut h = DefaultHasher::new();
    cpu.bus.virtio.as_ref().unwrap().disk.hash(&mut h);
    (cpu.save_state(), ram, h.finish())
}

fn same(cpu: &mut Cpu, want: &(Vec<u8>, u64, u64), at: u64) {
    let got = fingerprint(cpu);
    let first = got.0.iter().zip(&want.0).position(|(a, b)| a != b);
    assert!(
        got == *want,
        "state differs at instret {at}: ram {} disk {} first state byte {:?} of {}",
        got.1 == want.1,
        got.2 == want.2,
        first,
        got.0.len()
    );
}

/// Run until exactly `target` instructions have retired.
fn run_to_instret(cpu: &mut Cpu, target: u64) {
    while cpu.instret < target {
        if cpu.wfi {
            // One cycle first: a pending interrupt wakes it and runs one
            // instruction. Still idle, nothing can wake it before the timer.
            cpu.run(1);
            if cpu.wfi {
                cpu.run(cpu.cycles_until_timer());
            }
        } else {
            cpu.run(target - cpu.instret);
        }
    }
}

// xv6 reaches its shell after about 430 million instructions (most of it
// kinit filling free memory); type once it is waiting at the prompt.
const BOOT: u64 = 700_000_000;
const AFTER: u64 = 300_000_000;
const INPUT: &[u8] = b"echo hi > f\ncat f\nls\n";

/// The reference run: boot, type a command, let it finish. Large slices.
fn reference(cpu: &mut Cpu) -> Vec<u8> {
    cpu.run(BOOT);
    cpu.bus.uart.push_input(INPUT);
    cpu.run(AFTER);
    cpu.bus.uart.take_output()
}

#[test]
fn slicing_does_not_change_execution() {
    let mut a = machine!();
    let out_a = reference(&mut a);
    let text = String::from_utf8_lossy(&out_a);
    assert!(text.contains("$ hi\n"), "command did not run: {text:?}");

    let mut b = machine!();
    let mut seed = 12345u64;
    let mut slice = |cpu: &mut Cpu, until: u64| {
        while cpu.cycle() < until {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let n = 1 + (seed >> 33) % 200_000;
            cpu.run(n.min(until - cpu.cycle()));
        }
    };
    slice(&mut b, BOOT);
    b.bus.uart.push_input(INPUT);
    slice(&mut b, BOOT + AFTER);
    let out_b = b.bus.uart.take_output();

    assert_eq!(out_a, out_b);
    assert_eq!(fingerprint(&mut a), fingerprint(&mut b));
}

#[test]
fn snapshot_round_trip() {
    let mut a = machine!();
    a.run(BOOT);
    a.bus.uart.take_output();
    let snap = a.save_snapshot();
    let mut b = Cpu::from_snapshot(&snap).unwrap();
    assert_eq!(fingerprint(&mut a), fingerprint(&mut b));
    for cpu in [&mut a, &mut b] {
        cpu.bus.uart.push_input(INPUT);
        cpu.run(AFTER);
    }
    assert_eq!(a.bus.uart.take_output(), b.bus.uart.take_output());
    assert_eq!(fingerprint(&mut a), fingerprint(&mut b));
}

#[test]
fn stepping_back_matches_the_original_run() {
    // Reference states at a few instruction counts, from a plain run.
    let mut r = machine!();
    let mut tt_cpu = machine!();
    let mut tt = TimeTravel::new(&mut tt_cpu, 2_000_000, 1 << 30);

    tt.run(&mut tt_cpu, BOOT);
    let input_instret = tt_cpu.instret;
    tt.push_input(&mut tt_cpu, INPUT);
    tt.run(&mut tt_cpu, AFTER);
    let live_out = tt_cpu.bus.uart.take_output();
    let end = fingerprint(&mut tt_cpu);
    let end_instret = tt_cpu.instret;
    assert!(tt.checkpoints() > 10);

    let targets = [input_instret - 1_234_567, input_instret + 3, input_instret + 5_000_001, end_instret - 1];
    let mut want = Vec::new();
    run_to_instret(&mut r, targets[0]);
    want.push(fingerprint(&mut r));
    r.run(BOOT - r.cycle());
    assert_eq!(r.instret, input_instret);
    r.bus.uart.push_input(INPUT);
    for &t in &targets[1..] {
        run_to_instret(&mut r, t);
        want.push(fingerprint(&mut r));
    }

    // Go back to each (newest first, then out of order), and compare.
    for &i in &[3usize, 2, 0, 1] {
        assert_eq!(tt.seek_instret(&mut tt_cpu, targets[i]), targets[i]);
        assert!(tt.replaying(&tt_cpu));
        same(&mut tt_cpu, &want[i], targets[i]);
    }

    // Single steps backwards and forwards.
    let here = tt_cpu.instret;
    tt.step_back(&mut tt_cpu, 1);
    assert_eq!(tt_cpu.instret, here - 1);
    tt.seek_instret(&mut tt_cpu, here);
    assert_eq!(fingerprint(&mut tt_cpu), want[1]);

    // Back to the last trap, then one instruction on takes it.
    let traps = tt_cpu.traps_taken;
    assert!(tt.back_to_last_trap(&mut tt_cpu));
    let before = tt_cpu.traps_taken;
    assert!(before < traps || tt_cpu.instret < here);
    tt.run(&mut tt_cpu, 1);
    assert_eq!(tt_cpu.traps_taken, before + 1);

    // Catching up to the frontier replays the input without repeating output.
    tt.seek_instret(&mut tt_cpu, end_instret);
    assert!(!tt.replaying(&tt_cpu) || tt_cpu.cycle() <= tt.frontier());
    assert!(tt_cpu.bus.uart.take_output().is_empty(), "replay repeated output");
    // Exactly to the frontier: past it, the run would carry on live.
    let rest = tt.frontier() - tt_cpu.cycle();
    tt.run(&mut tt_cpu, rest);
    assert!(!tt.replaying(&tt_cpu));
    let at = tt_cpu.instret;
    same(&mut tt_cpu, &end, at);
    assert!(!live_out.is_empty());
}
