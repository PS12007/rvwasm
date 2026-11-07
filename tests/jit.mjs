// Tests the WebAssembly JIT under Node (V8, as in Chrome).
//
//   node tests/jit.mjs [--isa build/riscv-tests] [--xv6 build/xv6] [--linux build/linux] [--slices N]
//
// 1. Every riscv-tests program, with every block compiled the first time it
//    is seen.
// 2. xv6 and Linux, each booted twice from the same images, once with the
//    JIT and once interpreted, fed the same input at the same cycles. The
//    two machines must be byte-for-byte identical at every checkpoint
//    (registers, CSRs, devices, RAM and disk), and the JIT must have run
//    most of the instructions.
import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => (a.startsWith('--') ? [...acc, [a.slice(2), all[i + 1]]] : acc), []),
);
const isaDir = args.isa || path.join(root, 'build/riscv-tests');
const xv6Dir = args.xv6 || path.join(root, 'build/xv6');
const linuxDir = args.linux || path.join(root, 'build/linux');
const slices = Number(args.slices || 40);

const pkg = await import(pathToFileURL(path.join(root, 'web/pkg/rvweb.js')).href);
pkg.initSync({ module: readFileSync(path.join(root, 'web/pkg/rvweb_bg.wasm')) });
let failed = 0;

// ------------------------------------------------------------ riscv-tests
if (existsSync(isaDir)) {
  const names = readdirSync(isaDir).filter((n) => !n.includes('.')).sort();
  let pass = 0;
  for (const n of names) {
    const r = pkg.run_isa_test(readFileSync(path.join(isaDir, n)), true);
    if (r) {
      console.log(`FAIL ${n}: ${r}`);
      failed++;
    } else pass++;
  }
  console.log(`riscv-tests with the JIT: ${pass} passed, ${names.length - pass} failed`);
} else console.log(`skip riscv-tests: ${isaDir} not found`);

// --------------------------------------------------- JIT vs interpreter
const dec = new TextDecoder();
function differential(label, make, prompt, script) {
  const a = make();
  const b = make();
  b.set_jit(false);
  // Compile nearly everything that runs more than a few times, to test more code.
  a.jit_tune(8, 16, 100_000);
  let out = '';
  let typed = false;
  const SLICE = 25_000_000;
  for (let i = 0; i < slices; i++) {
    a.run(SLICE);
    b.run(SLICE);
    const oa = dec.decode(a.take_output());
    const ob = dec.decode(b.take_output());
    if (oa !== ob) {
      console.log(`FAIL ${label}: console output differs at slice ${i}`);
      return false;
    }
    out += oa;
    if (!typed && out.includes(prompt)) {
      const bytes = new TextEncoder().encode(script);
      a.push_input(bytes);
      b.push_input(bytes);
      typed = true;
    }
    if (i % 4 === 3 || i === slices - 1) {
      const sa = a.save_snapshot();
      const sb = b.save_snapshot();
      if (sa.length !== sb.length || !sa.every((v, k) => v === sb[k])) {
        console.log(`FAIL ${label}: machines differ after ${a.instret()} instructions (slice ${i})`);
        return false;
      }
    }
  }
  const [, blocks, modules, inval, insts] = a.jit_stats();
  const share = (100 * insts) / a.instret();
  console.log(
    `${label}: identical after ${(a.instret() / 1e6).toFixed(0)}M instructions; ` +
      `${blocks} blocks in ${modules} modules, ${inval} invalidated, ${share.toFixed(1)}% of instructions compiled`,
  );
  if (!typed) console.log(`  (note: never saw ${JSON.stringify(prompt)}; output ends ${JSON.stringify(out.slice(-200))})`);
  if (share < 40) {
    console.log(`FAIL ${label}: the JIT ran only ${share.toFixed(1)}% of instructions`);
    return false;
  }
  return true;
}

const kf = path.join(xv6Dir, 'kernel/kernel');
const df = path.join(xv6Dir, 'fs.img');
if (existsSync(kf) && existsSync(df)) {
  const kernel = readFileSync(kf);
  const disk = readFileSync(df);
  if (!differential('xv6', () => new pkg.Emulator(kernel, disk, 128), '$ ', 'forktest\nwc README\nstressfs\nls\n')) failed++;
} else console.log('skip xv6: images not found');

const lk = path.join(linuxDir, 'Image');
const li = path.join(linuxDir, 'initramfs.cpio.gz');
if (existsSync(lk) && existsSync(li)) {
  const kernel = readFileSync(lk);
  const initrd = readFileSync(li);
  const make = () => pkg.Emulator.new_linux(kernel, initrd, 256, 'console=ttyS0 earlycon=sbi', null);
  if (!differential('linux', make, 'Try:', 'uname -a\nls -l /bin | head -3\necho hello | sed s/hello/bye/\n')) failed++;
} else console.log('skip linux: images not found');

if (failed) {
  console.log(`${failed} failure(s)`);
  process.exit(1);
}
