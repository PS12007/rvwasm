#!/usr/bin/env bash
# Build the riscv-tests benchmark programs for rv64imac into build/bench/.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/third_party/riscv-tests/benchmarks"
OUT="$ROOT/build/bench"
LIBC_INC="${LIBC_INC:-/usr/lib/picolibc/riscv64-unknown-elf/include}"
mkdir -p "$OUT"
# mm is omitted: it needs libm's fma().
make -C "$OUT" -f "$SRC/Makefile" src_dir="$SRC" XLEN=64 \
  RISCV_PREFIX=riscv64-unknown-elf- \
  RISCV_GCC="riscv64-unknown-elf-gcc -isystem $LIBC_INC" \
  RISCV_MARCH=rv64imac_zicsr_zifencei ABI=lp64 \
  RISCV_LINK_OPTS="-static -nostdlib -nostartfiles -lgcc -T $SRC/common/test.ld" \
  -j"$(nproc)" \
  dhrystone.riscv median.riscv memcpy.riscv multiply.riscv qsort.riscv rsort.riscv spmv.riscv towers.riscv vvadd.riscv
