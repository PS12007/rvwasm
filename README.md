# rvwasm

A RISC-V computer that runs in a browser tab. The CPU is written in Rust, compiled to
WebAssembly, and good enough to boot xv6 and Linux.

**Try it: [ps12007.github.io/rvwasm](https://ps12007.github.io/rvwasm/)**

![xv6 booting in the browser, then running ls, cat and forktest](docs/media/xv6.gif)

That's xv6, MIT's teaching Unix, booting from a virtio disk image in a few seconds and
taking commands. The panel on the right is the emulator itself: the registers, which
privilege level the CPU is in, how many guest instructions per second it is retiring, and
the code around the program counter.

## What it emulates

- An RV64IMA hart with Zicsr and Zifencei: the base integer ISA, multiply/divide, atomics.
- Machine, supervisor and user mode, with traps, delegation and interrupts.
- Sv39 paging, with the accessed/dirty bits updated in hardware.
- A 16550 UART, the CLINT timer, a PLIC, and a virtio-mmio block device, at the same
  addresses as QEMU's `virt` board.
- The Sstc timer extension, which current xv6 relies on.

It passes all 196 tests in the official [riscv-tests](https://github.com/riscv-software-src/riscv-tests)
suites for this ISA (rv64ui, um, ua, si, mi, in both the physical and virtual-memory
variants), and xv6's own `usertests` runs to the end with ALL TESTS PASSED. CI checks both on
every push.

## Linux

![Linux 6.12 booting, then uname, /proc/cpuinfo and top in BusyBox](docs/media/linux.gif)

Pick "Linux 6.12 + BusyBox" in the header. There is no OpenSBI here: when the kernel makes
an SBI call, the emulator answers it directly, so Linux starts in supervisor mode on a
device tree the emulator writes at boot. The kernel is built from `tinyconfig` with no
compressed instructions and no FPU, and userland is BusyBox on musl, all compiled for plain
rv64ima with soft float. It reaches a shell after about 100 million instructions.

## Looking inside

![The timeline viewer showing a user-mode ecall entering the kernel's trap handler](docs/media/timeline.png)

Hit Record on the main page, do something, hit Stop, then Open timeline. The recorder keeps
the most recent instructions and every trap, interrupt and return, and the viewer lets you
scroll along them: privilege level across the top, trap markers, where the PC was, and the
disassembled, symbolised instructions underneath. Above, a user program's `ecall` lands in
xv6's trampoline at `0x3ffffff000`.

The command-line runner records the same format:

```bash
rvwasm run build/xv6/kernel/kernel --disk build/xv6/fs.img --trace 200000 --trace-out trace.json
```

## Speed

It's a straightforward interpreter. On my 4-core cloud VM:

| | million instructions per second |
|---|---:|
| native (`rvwasm bench`) | ~190 |
| WebAssembly in Chromium | ~130 |
| WebAssembly in Node 22 | ~120 |

So the browser build runs at about two thirds of native speed. Both builds run identical
workloads (nine programs from riscv-tests/benchmarks plus an xv6 session) through the same
Rust code in `rvcore::bench`, and only guest execution is timed. You can run the browser half
yourself on the [benchmark page](https://ps12007.github.io/rvwasm/bench.html); CI publishes
both sets of numbers in each run's summary.

## Building it

You need Rust, and on Ubuntu 24.04:

```bash
sudo apt-get install gcc-riscv64-unknown-elf picolibc-riscv64-unknown-elf \
  gcc-riscv64-linux-gnu flex bison bc libelf-dev rsync
git submodule update --init --recursive
cargo build --release
```

Then any of:

```bash
# the ISA tests
make -f tests/riscv-tests.mk -j"$(nproc)"
./target/release/rvwasm test build/riscv-tests/*

# xv6 on your terminal (Ctrl-A x quits)
./scripts/build-xv6.sh
./target/release/rvwasm run build/xv6/kernel/kernel --disk build/xv6/fs.img

# Linux + BusyBox
./scripts/build-linux.sh
./target/release/rvwasm run build/linux/Image --linux --initrd build/linux/initramfs.cpio.gz

# the web page (needs wasm32-unknown-unknown and wasm-pack)
./scripts/build-web.sh
python3 -m http.server -d web 8000
```

xv6 normally builds for rv64gc, so `build-xv6.sh` compiles a copy with the ISA string
changed to rv64ima. `build-linux.sh` shallow-clones Linux v6.12, musl 1.2.5 and BusyBox 1.36.1
(the last two from their GitHub mirrors, pinned to exact commits) and refuses to finish if a
compressed or floating-point instruction ends up in the output.

## How it's put together

- `crates/rvcore` is the machine. `cpu.rs` has the run loop, traps and CSRs, `exec.rs` the
  instruction set, `mmu.rs` the page-table walker and TLBs, `devices/` the UART, CLINT, PLIC
  and virtio disk, and `linux.rs` the SBI firmware and device tree.
- `crates/rvcli` is the `rvwasm` command: `test`, `run` and `bench`.
- `crates/rvweb` is the wasm-bindgen wrapper the page talks to.
- `web/` is the site. The emulator runs in a Web Worker so typing stays responsive, and
  when the guest is idle in `wfi` it stops burning CPU and lets guest time follow the clock.

A few choices that made it fast enough. Instructions run in chunks that stop at any trap,
return or device write, which is when interrupts get rechecked; between those points nothing
else is looked at. Each TLB entry is only filled once the permission checks for the current
mode have passed, so a hit is a compare and an add, and the whole TLB is flushed by bumping a
counter. Instruction fetch remembers the page it's in. Guest time is derived from the
instruction count, so a run is deterministic: the same disk and the same keystrokes give
the same result every time.

## Not done yet

- No compressed (C) or floating-point (F/D) instructions, so stock rv64gc binaries won't run.
- One hart only.
- No snapshots: a reload boots from scratch.
- The disk lives in memory; changes vanish when you close the tab.

## License

MIT. riscv-tests, xv6, Linux, musl and BusyBox keep their own licenses.
