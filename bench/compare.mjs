// Merge native and WebAssembly benchmark JSON into a Markdown table.
//   node bench/compare.mjs build/bench-native.json build/bench-wasm.json [more.json...]
import { readFileSync } from 'node:fs';

const runs = process.argv.slice(2).map((f) => JSON.parse(readFileSync(f, 'utf8')));
if (runs.length < 2) {
  console.error('usage: compare.mjs native.json wasm.json [...]');
  process.exit(2);
}
const [base, ...others] = runs;
const mips = (r) => r.instret / r.seconds / 1e6;
const geo = (xs) => Math.exp(xs.reduce((a, x) => a + Math.log(x), 0) / xs.length);

let out = `| workload | instructions | ${runs.map((r) => `${r.engine} MIPS`).join(' | ')} | ${others.map((r) => `${r.engine} / ${base.engine}`).join(' | ')} |\n`;
out += `|---|---:|${runs.map(() => '---:').join('|')}|${others.map(() => '---:').join('|')}|\n`;
const ratios = others.map(() => []);
for (const b of base.results) {
  const row = [b.name, b.instret.toLocaleString('en-US'), mips(b).toFixed(1)];
  const rel = [];
  others.forEach((o, i) => {
    const r = o.results.find((x) => x.name === b.name);
    row.push(r ? mips(r).toFixed(1) : '—');
    if (r) { const q = mips(r) / mips(b); ratios[i].push(q); rel.push(`${(q * 100).toFixed(0)}%`); } else rel.push('—');
  });
  out += `| ${[...row, ...rel].join(' | ')} |\n`;
}
const geoBase = geo(base.results.map(mips));
out += `| **geomean** | | ${[geoBase, ...others.map((o) => geo(o.results.map(mips)))].map((x) => `**${x.toFixed(1)}**`).join(' | ')} | ${ratios.map((r) => `**${(geo(r) * 100).toFixed(0)}%**`).join(' | ')} |\n`;
console.log(out);
