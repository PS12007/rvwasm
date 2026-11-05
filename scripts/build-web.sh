#!/usr/bin/env bash
# Build the WebAssembly package into web/pkg and stage guest images into web/images.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT/crates/rvweb"
wasm-pack build --release --target web --out-dir "$ROOT/web/pkg" --no-typescript
rm -f "$ROOT/web/pkg/.gitignore" "$ROOT/web/pkg/package.json" "$ROOT/web/pkg/README.md"
# wasm-opt shrinks the module by ~17% (it does not measurably change speed).
# Releases older than 116 corrupt wasm-bindgen's reference-type tables, so use
# the system binary only if it is new enough, else binaryen from npm if npx works.
WASM_OPT_BIN=""
WASM_OPT_VER=$(wasm-opt --version 2>/dev/null | grep -oE '[0-9]+' | head -1 || true)
if [ "${WASM_OPT_VER:-0}" -ge 116 ]; then
  WASM_OPT_BIN="wasm-opt"
elif command -v npx >/dev/null && npx --yes -p binaryen@123.0.0 wasm-opt --version >/dev/null 2>&1; then
  WASM_OPT_BIN="npx --yes -p binaryen@123.0.0 wasm-opt"
fi
if [ "${WASM_OPT:-1}" = 1 ] && [ -n "$WASM_OPT_BIN" ]; then
  $WASM_OPT_BIN -O3 --all-features "$ROOT/web/pkg/rvweb_bg.wasm" -o "$ROOT/web/pkg/rvweb_bg.wasm.opt" &&
    mv "$ROOT/web/pkg/rvweb_bg.wasm.opt" "$ROOT/web/pkg/rvweb_bg.wasm"
else
  echo "note: skipping wasm-opt (none >= 116 available)"
fi
mkdir -p "$ROOT/web/images"
for f in build/xv6/kernel/kernel:xv6-kernel build/xv6/fs.img:xv6-fs.img \
         build/linux/Image:linux-Image build/linux/initramfs.cpio.gz:linux-initramfs.cpio.gz \
         build/linux/System.map:linux-System.map; do
  src="$ROOT/${f%%:*}"; dst="$ROOT/web/images/${f##*:}"
  if [ -f "$src" ]; then cp "$src" "$dst"; echo "staged $dst"; fi
done
mkdir -p "$ROOT/web/images/bench"
for f in "$ROOT"/build/bench/*.riscv; do [ -f "$f" ] && cp "$f" "$ROOT/web/images/bench/"; done
[ -f "$ROOT/build/bench-native.json" ] && cp "$ROOT/build/bench-native.json" "$ROOT/web/images/" || true
