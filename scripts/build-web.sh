#!/usr/bin/env bash
# Build the WebAssembly package into web/pkg and stage guest images into web/images.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT/crates/rvweb"
wasm-pack build --release --target web --out-dir "$ROOT/web/pkg" --no-typescript
rm -f "$ROOT/web/pkg/.gitignore" "$ROOT/web/pkg/package.json" "$ROOT/web/pkg/README.md"
# Older wasm-opt releases (< 116) corrupt wasm-bindgen's reference-type
# tables, so only optimise with a recent one.
WASM_OPT_VER=$(wasm-opt --version 2>/dev/null | grep -oE '[0-9]+' | head -1 || true)
if [ "${WASM_OPT:-1}" = 1 ] && [ "${WASM_OPT_VER:-0}" -ge 116 ]; then
  wasm-opt -O3 --all-features \
    "$ROOT/web/pkg/rvweb_bg.wasm" -o "$ROOT/web/pkg/rvweb_bg.wasm"
fi
mkdir -p "$ROOT/web/images"
for f in build/xv6/kernel/kernel:xv6-kernel build/xv6/fs.img:xv6-fs.img \
         build/linux/Image:linux-Image build/linux/initramfs.cpio.gz:linux-initramfs.cpio.gz; do
  src="$ROOT/${f%%:*}"; dst="$ROOT/web/images/${f##*:}"
  if [ -f "$src" ]; then cp "$src" "$dst"; echo "staged $dst"; fi
done
mkdir -p "$ROOT/web/images/bench"
for f in "$ROOT"/build/bench/*.riscv; do [ -f "$f" ] && cp "$f" "$ROOT/web/images/bench/"; done
[ -f "$ROOT/build/bench-native.json" ] && cp "$ROOT/build/bench-native.json" "$ROOT/web/images/" || true
