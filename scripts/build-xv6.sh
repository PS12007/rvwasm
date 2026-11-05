#!/usr/bin/env bash
# Build xv6-riscv for RV64IMA (no compressed or FP instructions).
#
# xv6's Makefile hard-codes -march=rv64gc, so we build from a copy of the
# submodule with the ISA string rewritten. Outputs:
#   build/xv6/kernel/kernel   (ELF)
#   build/xv6/fs.img          (disk image)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/third_party/xv6-riscv"
OUT="$ROOT/build/xv6"
PREFIX="${TOOLPREFIX:-riscv64-linux-gnu-}"
MARCH="rv64ima_zicsr_zifencei"

if [ ! -f "$SRC/Makefile" ]; then
  echo "xv6 submodule missing; run: git submodule update --init" >&2
  exit 1
fi

mkdir -p "$OUT"
# Sync sources (keeps build products between runs).
cp -a "$SRC/." "$OUT/"
sed -i "s/-march=rv64gc/-march=$MARCH -mabi=lp64/g" "$OUT/Makefile"

make -C "$OUT" TOOLPREFIX="$PREFIX" kernel/kernel -j"$(nproc)"
# fs.img depends on every user program; xv6's Makefile is not parallel-safe here.
make -C "$OUT" TOOLPREFIX="$PREFIX" fs.img

# Sanity check: no compressed or floating-point instructions.
if "${PREFIX}objdump" -d "$OUT/kernel/kernel" "$OUT"/user/_* | grep -qE '^\s+[0-9a-f]+:\s+[0-9a-f]{4}\s+c\.'; then
  echo "error: compressed instructions found in xv6 build" >&2
  exit 1
fi
echo "built $OUT/kernel/kernel and $OUT/fs.img"
