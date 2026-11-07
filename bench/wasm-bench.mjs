// Runs the benchmark suite on the WebAssembly build under Node (V8, the same
// engine as Chrome), mirroring `rvwasm bench` for the native build.
//
//   node bench/wasm-bench.mjs [--bench-dir build/bench] [--xv6 build/xv6] [--reps 3] [--json out.json] [--jit on|off]
//
// With --jit on (the default) hot code is compiled to WebAssembly, as on the site.
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => (a.startsWith('--') ? [...acc, [a.slice(2), all[i + 1]]] : acc), []),
);
const benchDir = args['bench-dir'] || path.join(root, 'build/bench');
const xv6Dir = args.xv6 || path.join(root, 'build/xv6');
const reps = Number(args.reps || 3);

const jit = (args.jit || 'on') !== 'off';
const pkg = await import(pathToFileURL(path.join(root, 'web/pkg/rvweb.js')).href);
pkg.initSync({ module: readFileSync(path.join(root, 'web/pkg/rvweb_bg.wasm')) });

function best(fn) {
  let b = null;
  for (let i = 0; i < reps; i++) {
    const m = JSON.parse(fn());
    if (!b || m.seconds < b.seconds) b = m;
  }
  return b;
}

const results = [];
for (const name of pkg.bench_names()) {
  const f = path.join(benchDir, `${name}.riscv`);
  if (!existsSync(f)) { console.error(`skip ${name}: ${f} not found`); continue; }
  const elf = readFileSync(f);
  results.push({ name, ...best(() => pkg.bench_baremetal(elf, 0.3, jit)) });
}
const kf = path.join(xv6Dir, 'kernel/kernel');
const df = path.join(xv6Dir, 'fs.img');
if (existsSync(kf) && existsSync(df)) {
  const kernel = readFileSync(kf);
  const disk = readFileSync(df);
  results.push({ name: 'xv6', ...best(() => pkg.bench_xv6(kernel, disk, jit)) });
} else {
  console.error('skip xv6: images not found');
}

console.log(`${'workload'.padEnd(12)} ${'instructions'.padStart(12)} ${'ms/run'.padStart(12)} ${'MIPS'.padStart(8)}`);
for (const r of results) {
  console.log(`${r.name.padEnd(12)} ${String(r.instret).padStart(12)} ${(r.seconds * 1e3).toFixed(3).padStart(12)} ${(r.instret / r.seconds / 1e6).toFixed(1).padStart(8)}`);
}
if (args.json) {
  writeFileSync(args.json, JSON.stringify({ engine: `wasm${jit ? ' + JIT' : ''} (node ${process.version})`, results }) + '\n');
}
