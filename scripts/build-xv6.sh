#!/usr/bin/env bash
# Build xv6-riscv for RV64IMAC (rv64gc without floating point).
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
MARCH="rv64imac_zicsr_zifencei"

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

# Sanity check: no floating-point instructions, and the C extension is in use.
dump="$("${PREFIX}objdump" -d "$OUT/kernel/kernel" "$OUT"/user/_*)"
if grep -qE '\sf(add|sub|mul|div|ld|sd|lw|sw|mv|cvt)\.' <<<"$dump"; then
  echo "error: floating-point instructions found in xv6 build" >&2
  exit 1
fi
total="$(grep -cE '^\s+[0-9a-f]+:\s+[0-9a-f]{4,8}\s' <<<"$dump")"
short="$(grep -cE '^\s+[0-9a-f]+:\s+[0-9a-f]{4}\s' <<<"$dump")"
echo "xv6: $short of $total instructions are compressed"
[ "$short" -gt 0 ] || { echo "error: expected compressed instructions" >&2; exit 1; }
echo "built $OUT/kernel/kernel and $OUT/fs.img"
