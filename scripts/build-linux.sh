#!/usr/bin/env bash
# Build a minimal RV64IMA Linux kernel and a BusyBox initramfs for rvwasm.
#
# Outputs:
#   build/linux/Image                 kernel (boots in S-mode on rvwasm's built-in SBI)
#   build/linux/initramfs.cpio.gz     initramfs: BusyBox + linux/rootfs (or the mini
#                                     shell from linux/init.c when BUSYBOX=0)
#
# Sources (shallow clones, pinned):
#   Linux    github.com/torvalds/linux   tag v6.12
#   musl     github.com/kraj/musl        tag v1.2.5   (mirror of git.musl-libc.org)
#   BusyBox  github.com/mirror/busybox   tag 1_36_1   (mirror of git.busybox.net)
#
# Userspace is soft-float (lp64) with no compressed instructions. No distro ships
# a libc for that, so musl is built here; libgcc and crt files come from the
# rv64im/lp64 multilib of the bare-metal toolchain (gcc-riscv64-unknown-elf).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRCDIR="$ROOT/build/linux-src"
OBJ="$ROOT/build/linux-obj"
OUT="$ROOT/build/linux"
MUSL="$ROOT/build/musl"
BBOBJ="$ROOT/build/busybox-obj"
CROSS="${CROSS_COMPILE:-riscv64-linux-gnu-}"
MARCH="-march=rv64ima_zicsr_zifencei -mabi=lp64"
BUSYBOX="${BUSYBOX:-1}"
SOFT_MULTILIB="${SOFT_MULTILIB:-$(riscv64-unknown-elf-gcc -march=rv64im -mabi=lp64 -print-file-name=libgcc.a | xargs dirname)}"
JOBS="$(nproc)"

# clone DIR URL TAG COMMIT: shallow clone and check the tag resolves to COMMIT.
clone() {
  local dir="$SRCDIR/$1"
  if [ ! -d "$dir/.git" ]; then
    mkdir -p "$SRCDIR"
    git -c advice.detachedHead=false clone -q --depth 1 --branch "$3" "$2" "$dir"
  fi
  local got
  got="$(git -C "$dir" rev-parse HEAD)"
  if [ -n "$4" ] && [ "$got" != "$4" ]; then
    echo "error: $1 is at $got, expected $4" >&2
    exit 1
  fi
}

mkdir -p "$OBJ" "$OUT"

# ---------------------------------------------------------------- kernel
clone linux https://github.com/torvalds/linux v6.12 ""
kmake() { make -C "$SRCDIR/linux" O="$OBJ" ARCH=riscv CROSS_COMPILE="$CROSS" "$@"; }
kmake -s tinyconfig
"$SRCDIR/linux/scripts/kconfig/merge_config.sh" -m -O "$OBJ" "$OBJ/.config" "$ROOT/linux/rvwasm.config" >/dev/null
kmake -s olddefconfig
kmake -j"$JOBS" Image
cp "$OBJ/arch/riscv/boot/Image" "$OUT/Image"

LIST="$OUT/initramfs.list"
cat > "$LIST" <<EOF
dir /dev 755 0 0
nod /dev/console 600 0 0 c 5 1
nod /dev/null 666 0 0 c 1 3
dir /proc 755 0 0
dir /sys 755 0 0
dir /tmp 1777 0 0
dir /root 700 0 0
EOF

if [ "$BUSYBOX" = 1 ]; then
  # ------------------------------------------------------------ musl
  clone musl https://github.com/kraj/musl v1.2.5 0784374d561435f7c787a555aeab8ede699ed298
  if [ ! -f "$MUSL/lib/libc.a" ]; then
    (cd "$SRCDIR/musl" &&
      ./configure --target=riscv64-linux-gnu --prefix="$MUSL" --disable-shared --enable-wrapper=gcc \
        CC="${CROSS}gcc $MARCH" LIBCC="$SOFT_MULTILIB/libgcc.a" >/dev/null &&
      make -j"$JOBS" >/dev/null && make install >/dev/null)
    kmake -s INSTALL_HDR_PATH="$MUSL" headers_install
  fi
  # Static-only specs: the soft-float multilib has crtbegin.o but no crtbeginS.o.
  sed -e 's|Scrt1.o|crt1.o|; s|crtbeginS.o|crtbegin.o|; s|crtendS.o|crtend.o|' \
    "$MUSL/lib/musl-gcc.specs" > "$MUSL/lib/rv-static.specs"
  cat > "$MUSL/bin/rv-musl-gcc" <<EOF
#!/bin/sh
exec ${CROSS}gcc $MARCH -specs "$MUSL/lib/rv-static.specs" -B$SOFT_MULTILIB/ -static -no-pie "\$@"
EOF
  chmod +x "$MUSL/bin/rv-musl-gcc"

  # ------------------------------------------------------------ BusyBox
  clone busybox https://github.com/mirror/busybox 1_36_1 1a64f6a20aaf6ea4dbba68bbfa8cc1ab7e5c57c4
  mkdir -p "$BBOBJ"
  bbmake() { make -C "$SRCDIR/busybox" O="$BBOBJ" CC="$MUSL/bin/rv-musl-gcc" CROSS_COMPILE="$CROSS" HOSTCC=gcc "$@"; }
  if [ ! -f "$BBOBJ/.config" ]; then
    bbmake -s defconfig >/dev/null
    # Static build; tc does not compile against current kernel headers;
    # the SHA hardware-acceleration options are x86-only.
    sed -i -e 's/^# CONFIG_STATIC is not set/CONFIG_STATIC=y/' \
      -e 's/^CONFIG_TC=y/# CONFIG_TC is not set/' \
      -e 's/^CONFIG_FEATURE_TC_INGRESS=y/# CONFIG_FEATURE_TC_INGRESS is not set/' \
      -e 's/^CONFIG_SHA1_HWACCEL=y/# CONFIG_SHA1_HWACCEL is not set/' \
      -e 's/^CONFIG_SHA256_HWACCEL=y/# CONFIG_SHA256_HWACCEL is not set/' "$BBOBJ/.config"
    # Accept defaults for any remaining questions (`yes` exits on SIGPIPE, so not a pipe under pipefail).
    bbmake -s oldconfig < <(yes '') >/dev/null
  fi
  bbmake -j"$JOBS" busybox >/dev/null
  bbmake -s busybox.links >/dev/null
  cp "$BBOBJ/busybox" "$OUT/busybox"

  {
    echo "dir /bin 755 0 0"
    echo "dir /sbin 755 0 0"
    echo "dir /usr 755 0 0"
    echo "dir /usr/bin 755 0 0"
    echo "dir /usr/sbin 755 0 0"
    echo "dir /etc 755 0 0"
    echo "file /bin/busybox $OUT/busybox 755 0 0"
    grep -v '^/bin/busybox$' "$BBOBJ/busybox.links" | sort -u | while read -r l; do
      echo "slink $l /bin/busybox 777 0 0"
    done
    echo "file /init $ROOT/linux/rootfs/init 755 0 0"
    for f in "$ROOT"/linux/rootfs/etc/*; do
      echo "file /etc/$(basename "$f") $f 644 0 0"
    done
  } >> "$LIST"
  CHECK="$OUT/busybox"
else
  # Mini shell (no libc needed).
  "${CROSS}gcc" $MARCH -static -nostdlib -ffreestanding -fno-builtin -fno-stack-protector -O2 -Wall \
    -o "$OUT/init" "$ROOT/linux/init.c"
  echo "file /init $OUT/init 755 0 0" >> "$LIST"
  CHECK="$OUT/init"
fi

"$OBJ/usr/gen_init_cpio" "$LIST" | gzip -9 > "$OUT/initramfs.cpio.gz"

# Everything must be plain RV64IMA: no compressed or floating-point instructions.
if "${CROSS}objdump" -d "$CHECK" "$OBJ/vmlinux" |
  grep -qE '^\s+[0-9a-f]+:\s+[0-9a-f]{4}\s+c\.|\sf(add|sub|mul|div|ld|sd|lw|sw|mv|cvt)\.'; then
  echo "error: compressed or FP instructions found" >&2
  exit 1
fi
ls -la "$OUT/Image" "$OUT/initramfs.cpio.gz"
