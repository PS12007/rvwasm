//! `rvwasm bench`: time the shared workloads natively.
//!
//!   rvwasm bench [--bench-dir build/bench] [--xv6 build/xv6] [--reps 3] [--json out.json]
//!
//! Each workload is sampled `reps` times; the fastest sample is reported.
//! The WebAssembly counterpart is bench/wasm-bench.mjs.

use std::process::ExitCode;
use std::time::Instant;

use rvcore::bench::{bench_baremetal, bench_xv6, Measurement, BAREMETAL};

pub fn cmd_bench(args: &[String]) -> ExitCode {
    let mut bench_dir = "build/bench".to_string();
    let mut xv6_dir = "build/xv6".to_string();
    let mut reps = 3usize;
    let mut json: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--bench-dir", Some(v)) => bench_dir = v.clone(),
            ("--xv6", Some(v)) => xv6_dir = v.clone(),
            ("--reps", Some(v)) => reps = v.parse().unwrap_or(3).max(1),
            ("--json", Some(v)) => json = Some(v.clone()),
            _ => {
                eprintln!("usage: rvwasm bench [--bench-dir DIR] [--xv6 DIR] [--reps N] [--json FILE]");
                return ExitCode::from(2);
            }
        }
    }

    let epoch = Instant::now();
    let now = move || epoch.elapsed().as_secs_f64();
    let best = |f: &dyn Fn() -> Result<Measurement, String>| -> Result<Measurement, String> {
        let mut best: Option<Measurement> = None;
        for _ in 0..reps {
            let m = f()?;
            if best.is_none_or(|b| m.secs < b.secs) {
                best = Some(m);
            }
        }
        Ok(best.unwrap())
    };

    let mut rows: Vec<(String, Measurement)> = Vec::new();
    for name in BAREMETAL {
        let path = format!("{bench_dir}/{name}.riscv");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skip {name}: {path} not found");
            continue;
        };
        match best(&|| bench_baremetal(&bytes, &now, 0.3, &|_| {})) {
            Ok(m) => rows.push((name.into(), m)),
            Err(e) => {
                eprintln!("{name}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    match (std::fs::read(format!("{xv6_dir}/kernel/kernel")), std::fs::read(format!("{xv6_dir}/fs.img"))) {
        (Ok(k), Ok(d)) => match best(&|| bench_xv6(&k, &d, &now, 0.0, &|_| {})) {
            Ok(m) => rows.push(("xv6".into(), m)),
            Err(e) => {
                eprintln!("xv6: {e}");
                return ExitCode::FAILURE;
            }
        },
        _ => eprintln!("skip xv6: {xv6_dir}/kernel/kernel or fs.img not found"),
    }

    println!("{:<12} {:>12} {:>12} {:>8}", "workload", "instructions", "ms/run", "MIPS");
    for (name, m) in &rows {
        println!("{:<12} {:>12} {:>12.3} {:>8.1}", name, m.instret, m.secs * 1e3, m.mips());
    }
    if let Some(path) = json {
        let items: Vec<String> = rows
            .iter()
            .map(|(n, m)| format!("{{\"name\":\"{}\",\"instret\":{},\"seconds\":{}}}", n, m.instret, m.secs))
            .collect();
        let doc = format!("{{\"engine\":\"native\",\"results\":[{}]}}\n", items.join(","));
        if let Err(e) = std::fs::write(&path, doc) {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
