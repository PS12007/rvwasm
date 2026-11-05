#!/usr/bin/env bash
# Build a minimal RV64IMA Linux kernel and initramfs for rvwasm.
#
# Outputs:
#   build/linux/Image                 kernel (boots in S-mode on rvwasm's built-in SBI)
#   build/linux/initramfs.cpio.gz     initramfs with /init (linux/init.c)
#
# Kernel source: official tag from github.com/torvalds/linux (shallow clone),
# configured from tinyconfig + linux/rvwasm.config.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KVER="${KVER:-v6.12}"
SRC="${LINUX_SRC:-$ROOT/build/linux-src/linux}"
OBJ="$ROOT/build/linux-obj"
OUT="$ROOT/build/linux"
CROSS="${CROSS_COMPILE:-riscv64-linux-gnu-}"
MARCH="-march=rv64ima_zicsr_zifencei -mabi=lp64"

if [ ! -f "$SRC/Makefile" ]; then
  mkdir -p "$(dirname "$SRC")"
  git clone --depth 1 --branch "$KVER" https://github.com/torvalds/linux "$SRC"
fi

mkdir -p "$OBJ" "$OUT"
kmake() { make -C "$SRC" O="$OBJ" ARCH=riscv CROSS_COMPILE="$CROSS" "$@"; }
kmake -s tinyconfig
"$SRC/scripts/kconfig/merge_config.sh" -m -O "$OBJ" "$OBJ/.config" "$ROOT/linux/rvwasm.config" >/dev/null
kmake -s olddefconfig
kmake -j"$(nproc)" Image
cp "$OBJ/arch/riscv/boot/Image" "$OUT/Image"

# /init: the mini shell (no libc needed).
"${CROSS}gcc" $MARCH -static -nostdlib -ffreestanding -fno-builtin -fno-stack-protector -O2 -Wall \
  -o "$OUT/init" "$ROOT/linux/init.c"

cat > "$OUT/initramfs.list" <<EOF
dir /dev 755 0 0
nod /dev/console 600 0 0 c 5 1
dir /proc 755 0 0
dir /sys 755 0 0
dir /tmp 1777 0 0
dir /root 700 0 0
file /init $OUT/init 755 0 0
EOF
"$OBJ/usr/gen_init_cpio" "$OUT/initramfs.list" | gzip -9 > "$OUT/initramfs.cpio.gz"

if "${CROSS}objdump" -d "$OUT/init" "$OBJ/vmlinux" | grep -qE '^\s+[0-9a-f]+:\s+[0-9a-f]{4}\s+c\.'; then
  echo "error: compressed instructions found" >&2
  exit 1
fi
ls -la "$OUT/Image" "$OUT/initramfs.cpio.gz"
