# rvwasm

A RISC-V emulator written in Rust that compiles to WebAssembly and runs in the browser.
It implements **RV64IMA + Zicsr + Zifencei** with M/S/U privilege modes, traps and
interrupts, Sv39 virtual memory, and the devices needed to boot real operating systems:
a 16550 UART, CLINT, PLIC and a virtio-mmio block device.

- Passes all **196** official [riscv-tests](https://github.com/riscv-software-src/riscv-tests)
  in the rv64ui, rv64um, rv64ua, rv64si and rv64mi suites (both `-p` and `-v` environments).
- Boots **xv6-riscv** to an interactive shell and passes its full `usertests -q` suite.
- Boots **Linux 6.12** to a userspace shell, using an SBI firmware implemented inside the emulator.
- Web front end with an xterm.js terminal and a live panel: registers, current privilege
  mode, instructions per second, time spent per mode, disassembly around the PC, CSRs.
- An instruction trace recorder with a timeline viewer.
- Native vs WebAssembly benchmarks.

## Layout

| Path | What |
|---|---|
| `crates/rvcore` | The emulator: CPU (`cpu.rs`, `exec.rs`), CSRs, Sv39 MMU with a software TLB (`mmu.rs`), bus and devices, ELF loader, disassembler, trace recorder, SBI + device tree for Linux, shared benchmark workloads |
| `crates/rvcli` | Native front end, binary `rvwasm`: run tests, boot kernels on your terminal, benchmark |
| `crates/rvweb` | wasm-bindgen bindings used by the web page |
| `web/` | Static site: `index.html` (terminal + live panel), `trace.html` (timeline viewer), `bench.html` (in-browser benchmark) |
| `third_party/` | Submodules: riscv-tests, xv6-riscv |
| `tests/riscv-tests.mk` | Builds the ISA tests for rv64ima |
| `linux/` | Kernel config fragment and the small `/init` shell |
| `scripts/` | Builds for xv6, Linux, benchmarks and the web bundle |
| `bench/` | WebAssembly benchmark runner (Node) and report generator |

## Building

Requirements: Rust (stable), and on Ubuntu 24.04:

```bash
sudo apt-get install gcc-riscv64-unknown-elf picolibc-riscv64-unknown-elf gcc-riscv64-linux-gnu flex bison bc
```

For the web build, also `rustup target add wasm32-unknown-unknown` and `cargo install wasm-pack`.

```bash
git submodule update --init --recursive
cargo build --release
```

### riscv-tests

```bash
make -f tests/riscv-tests.mk -j"$(nproc)"
./target/release/rvwasm test build/riscv-tests/*
```

### xv6

xv6's Makefile hard-codes `-march=rv64gc`; `scripts/build-xv6.sh` builds a copy with
`-march=rv64ima_zicsr_zifencei -mabi=lp64` so it contains no compressed or FP instructions.

```bash
./scripts/build-xv6.sh
./target/release/rvwasm run build/xv6/kernel/kernel --disk build/xv6/fs.img
```

Press `Ctrl-A x` to quit.

### Linux

```bash
./scripts/build-linux.sh      # shallow-clones v6.12 from github.com/torvalds/linux
./target/release/rvwasm run build/linux/Image --linux --initrd build/linux/initramfs.cpio.gz
```

The kernel is built from `tinyconfig` plus `linux/rvwasm.config` (no C extension, no FPU,
one hart). It boots straight into S-mode: `ecall`s from S-mode are handled by an SBI v2.0
implementation in `rvcore::linux` (base, timer, IPI, RFENCE, HSM, SRST, debug console),
and the device tree is generated at boot. Boot to the shell takes about 48 million instructions.

The initramfs currently contains `linux/init.c`, a tiny libc-free shell (`help`, `ls`,
`cat`, `uname`, `mounts`, `mem`, `write`, ...). BusyBox needs a soft-float rv64ima libc
(musl), which isn't packaged for this target, so it has to be built from source; that
step is not in the build yet.

### Web page

```bash
./scripts/build-web.sh        # builds web/pkg and stages images into web/images
python3 -m http.server -d web 8000
```

Open http://localhost:8000. Pick xv6 or Linux in the header. The emulator runs in a Web
Worker; while the guest sits in `wfi`, guest time advances with real time instead of
spinning the CPU.

## Instruction traces

Record from the web page (Instruction trace → Record → Stop → Open timeline), or natively:

```bash
./target/release/rvwasm run build/xv6/kernel/kernel --disk build/xv6/fs.img \
  --trace 200000 --trace-out trace.json
```

The recorder keeps a ring buffer of the most recent instructions (cycle, pc, encoding,
privilege) plus every exception, interrupt and `xRET`. `web/trace.html` shows a privilege
band, trap markers, a PC-over-time scatter (addresses ranked so code regions stack
compactly), a disassembled and symbolised instruction list, and the hottest functions in
the visible range. Scroll to zoom, drag to pan, click to inspect.

## Benchmarks

Both builds run the same workloads from `rvcore::bench`: nine bare-metal programs from
riscv-tests/benchmarks and an xv6 run (boot, then `forktest`, `wc`, `grep`, `stressfs` from
the shell). Only guest execution is timed.

```bash
./scripts/build-bench.sh
./target/release/rvwasm bench --json build/bench-native.json
./scripts/build-web.sh && node bench/wasm-bench.mjs --json build/bench-wasm.json
node bench/compare.mjs build/bench-native.json build/bench-wasm.json
```

`web/bench.html` runs the WebAssembly side in your browser. CI publishes the comparison in
each run's job summary.

Results on the development machine (4-core cloud VM, Rust 1.97, Node 22):

| workload | instructions | native MIPS | wasm (Node) MIPS | wasm / native |
|---|---:|---:|---:|---:|
| dhrystone | 192,040 | 204.6 | 127.4 | 62% |
| median | 13,352 | 178.4 | 109.0 | 61% |
| memcpy | 29,224 | 210.8 | 121.6 | 58% |
| multiply | 50,216 | 211.4 | 141.9 | 67% |
| qsort | 232,488 | 201.9 | 128.2 | 64% |
| rsort | 370,728 | 214.3 | 126.7 | 59% |
| spmv | 2,385,448 | 216.4 | 133.3 | 62% |
| towers | 9,256 | 145.3 | 96.2 | 66% |
| vvadd | 8,232 | 176.6 | 111.7 | 63% |
| xv6 | 537,279,954 | 186.4 | 129.9 | 70% |
| **geomean** | | **193.3** | **121.9** | **63%** |

Headless Chromium on the same machine measured a 131.5 MIPS geomean (68% of native).

## Design notes

- **Interpreter loop.** Instructions run in chunks of up to 512. A chunk ends early on any
  trap, `xRET`, CSR write or MMIO store, which is when device state and pending interrupts
  are re-checked. Since chunks never cross a privilege change, per-mode instruction counts
  come for free.
- **MMU.** Separate fetch/load/store TLBs (256 entries each) are filled only after the
  permission checks for the current privilege, `SUM` and `MXR` pass, so a hit needs no
  further checks. Any change to those inputs bumps a generation counter, invalidating all
  entries in O(1). A/D bits are updated in hardware (Svadu behaviour).
- **Time.** `mtime` advances at one tick per 10 cycles (10 MHz at a nominal 100 MIPS), so
  runs are deterministic. In `wfi` the emulator skips ahead to the next timer event.
- **Sstc.** `stimecmp` is supported (xv6 uses it); the Linux SBI timer is implemented on top of it.
- **Misaligned accesses** are handled in hardware, including ones that cross a page;
  misaligned atomics trap.

## Continuous integration

`.github/workflows/ci.yml` runs formatting, clippy and unit tests; builds and runs the
riscv-tests; builds xv6 and runs `usertests -q`; builds and boots Linux; builds the
WebAssembly bundle and benchmarks it against native. On pushes to `main` it deploys
`web/` to GitHub Pages (enable Pages with source "GitHub Actions" in the repository settings).

## License

MIT. riscv-tests, xv6-riscv and Linux are under their own licenses.
