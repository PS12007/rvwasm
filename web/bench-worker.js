// Runs the shared benchmark suite (rvcore::bench) inside a worker.
import init, { bench_baremetal, bench_xv6, bench_names } from './pkg/rvweb.js';

async function bytes(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${url}: HTTP ${r.status}`);
  return new Uint8Array(await r.arrayBuffer());
}

onmessage = async (ev) => {
  const { reps = 3, xv6 = true } = ev.data || {};
  try {
    await init();
    const best = (fn) => {
      let b = null;
      for (let i = 0; i < reps; i++) {
        const m = JSON.parse(fn());
        if (!b || m.seconds < b.seconds) b = m;
      }
      return b;
    };
    for (const name of bench_names()) {
      postMessage({ type: 'progress', name });
      const elf = await bytes(`images/bench/${name}.riscv`);
      postMessage({ type: 'result', name, ...best(() => bench_baremetal(elf, 0.3, true)) });
    }
    if (xv6) {
      postMessage({ type: 'progress', name: 'xv6' });
      const [k, d] = await Promise.all([bytes('images/xv6-kernel'), bytes('images/xv6-fs.img')]);
      postMessage({ type: 'result', name: 'xv6', ...best(() => bench_xv6(k, d, true)) });
    }
    postMessage({ type: 'done' });
  } catch (e) {
    postMessage({ type: 'error', text: String(e && e.message ? e.message : e) });
  }
};
