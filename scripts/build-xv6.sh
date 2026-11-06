#!/usr/bin/env bash
# Build xv6-riscv exactly as upstream does (its Makefile targets rv64gc),
# in a copy of the submodule so build products stay out of it. Outputs:
#   build/xv6/kernel/kernel   (ELF)
#   build/xv6/fs.img          (disk image)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/third_party/xv6-riscv"
OUT="$ROOT/build/xv6"
PREFIX="${TOOLPREFIX:-riscv64-linux-gnu-}"

if [ ! -f "$SRC/Makefile" ]; then
  echo "xv6 submodule missing; run: git submodule update --init" >&2
  exit 1
fi

mkdir -p "$OUT"
# Sync sources (keeps build products between runs).
cp -a "$SRC/." "$OUT/"

make -C "$OUT" TOOLPREFIX="$PREFIX" kernel/kernel -j"$(nproc)"
# fs.img depends on every user program; xv6's Makefile is not parallel-safe here.
make -C "$OUT" TOOLPREFIX="$PREFIX" fs.img

# Report how much of the code is compressed.
dump="$("${PREFIX}objdump" -d "$OUT/kernel/kernel" "$OUT"/user/_*)"
total="$(grep -cE '^\s+[0-9a-f]+:\s+[0-9a-f]{4,8}\s' <<<"$dump")"
short="$(grep -cE '^\s+[0-9a-f]+:\s+[0-9a-f]{4}\s' <<<"$dump")"
echo "xv6: $short of $total instructions are compressed"
[ "$short" -gt 0 ] || { echo "error: expected compressed instructions" >&2; exit 1; }
echo "built $OUT/kernel/kernel and $OUT/fs.img"
