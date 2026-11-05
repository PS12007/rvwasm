// Timeline viewer for rvwasm-trace-1 JSON (see rvcore::trace::Trace::to_json).
import init, { disassemble } from './pkg/rvweb.js';

const $ = (id) => document.getElementById(id);
const MODES = { 0: 'U', 1: 'S', 3: 'M' };
const MODE_COLORS = { 0: '#4cc38a', 1: '#f0b44c', 3: '#ef6f6c' };
const EXC = ['instruction misaligned', 'instruction access fault', 'illegal instruction', 'breakpoint',
  'load misaligned', 'load access fault', 'store misaligned', 'store access fault', 'ecall from U',
  'ecall from S', 'reserved', 'ecall from M', 'instruction page fault', 'load page fault', 'reserved',
  'store page fault'];
const IRQ = { 1: 'supervisor software', 3: 'machine software', 5: 'supervisor timer', 7: 'machine timer',
  9: 'supervisor external', 11: 'machine external' };
const ROW_H = 18;

let T = null;            // loaded trace
let view = [0, 1];       // visible instruction index range [start, end)
let selected = -1;
const wasm = init().catch(() => null);
let disasmReady = false;
wasm.then((x) => { disasmReady = !!x; renderList(); });

function causeName(e) {
  if (e.kind === 'xret') return `return ${MODES[e.from]}→${MODES[e.to]}`;
  const n = e.kind === 'interrupt' ? (IRQ[e.cause] || `interrupt ${e.cause}`) : (EXC[e.cause] || `exception ${e.cause}`);
  return `${n} ${MODES[e.from]}→${MODES[e.to]}`;
}

// ------------------------------------------------------------ loading

function load(json) {
  const d = typeof json === 'string' ? JSON.parse(json) : json;
  if (d.format !== 'rvwasm-trace-1') throw new Error('not an rvwasm trace');
  const n = d.insts.length;
  const cycle = new Float64Array(n);
  const prv = new Uint8Array(n);
  const pcIdx = new Uint32Array(n);   // index into uniquePcs
  const inst = new Uint32Array(n);
  const pcMap = new Map();
  const pcs = [];
  for (let i = 0; i < n; i++) {
    const r = d.insts[i];
    cycle[i] = r[0];
    inst[i] = parseInt(r[2], 16) >>> 0;
    prv[i] = r[3];
    let k = pcMap.get(r[1]);
    if (k === undefined) { k = pcs.length; pcMap.set(r[1], k); pcs.push(r[1]); }
    pcIdx[i] = k;
  }
  // Rank unique PCs by address so the scatter's y axis is ordered but gap-free.
  const big = pcs.map((h) => BigInt('0x' + h));
  const order = big.map((_, i) => i).sort((a, b) => (big[a] < big[b] ? -1 : big[a] > big[b] ? 1 : 0));
  const rank = new Float32Array(pcs.length);
  order.forEach((u, r) => { rank[u] = pcs.length > 1 ? r / (pcs.length - 1) : 0.5; });

  // Symbolise each unique PC.
  const syms = (d.symbols || []).map(([name, a]) => [name, BigInt('0x' + a)]).sort((a, b) => (a[1] < b[1] ? -1 : 1));
  const symOf = big.map((a) => {
    let lo = 0, hi = syms.length;
    while (lo < hi) { const m = (lo + hi) >> 1; if (syms[m][1] <= a) lo = m + 1; else hi = m; }
    if (lo === 0) return '';
    const [name, base] = syms[lo - 1];
    return a - base < 0x10000n ? name : '';
  });

  // Events, located by instruction index (binary search on cycle).
  const findIdx = (c) => {
    let lo = 0, hi = n;
    while (lo < hi) { const m = (lo + hi) >> 1; if (cycle[m] < c) lo = m + 1; else hi = m; }
    return Math.min(lo, n - 1);
  };
  const events = d.events.map((e) => ({ ...e, idx: findIdx(e.cycle) }));

  T = { n, cycle, prv, pcIdx, inst, pcs, rank, symOf, events };
  view = [0, n];
  selected = -1;
  $('empty').hidden = true;
  $('main').hidden = false;
  renderSummary();
  $('spacer').style.height = `${n * ROW_H}px`;
  renderEvents();
  resize();
}

function renderSummary() {
  const counts = { 0: 0, 1: 0, 3: 0 };
  for (let i = 0; i < T.n; i++) counts[T.prv[i]]++;
  const pct = (k) => ((100 * counts[k]) / T.n).toFixed(1) + '%';
  const exc = T.events.filter((e) => e.kind === 'exception').length;
  const irq = T.events.filter((e) => e.kind === 'interrupt').length;
  $('summary').innerHTML = `
    <span>instructions <b>${T.n.toLocaleString()}</b></span>
    <span>cycles <b>${T.cycle[0].toLocaleString()} – ${T.cycle[T.n - 1].toLocaleString()}</b></span>
    <span>unique PCs <b>${T.pcs.length.toLocaleString()}</b></span>
    <span>exceptions <b>${exc}</b></span><span>interrupts <b>${irq}</b></span>
    <span class="legend2"><span><i style="background:${MODE_COLORS[0]}"></i>U ${pct(0)}</span>
    <span><i style="background:${MODE_COLORS[1]}"></i>S ${pct(1)}</span>
    <span><i style="background:${MODE_COLORS[3]}"></i>M ${pct(3)}</span></span>
    <span>scroll to zoom · drag to pan · click to inspect</span>`;
}

// ------------------------------------------------------------ timeline canvas

const canvas = $('tl');
const ctx = canvas.getContext('2d');
const LANES = { axis: [0, 18], mode: [20, 36], events: [40, 56], pc: [72, 0] };

function resize() {
  if (!T) return;
  const dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(canvas.clientWidth * dpr);
  canvas.height = Math.round(canvas.clientHeight * dpr);
  draw();
}
new ResizeObserver(resize).observe(canvas);

const xOf = (i, w) => ((i - view[0]) / (view[1] - view[0])) * w;
const idxOf = (x, w) => Math.floor(view[0] + (x / w) * (view[1] - view[0]));

function draw() {
  if (!T) return;
  const dpr = window.devicePixelRatio || 1;
  const W = canvas.width, H = canvas.height;
  const w = W / dpr, h = H / dpr;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, W, H);

  // PC scatter and mode band rendered per device pixel column.
  const img = ctx.createImageData(W, H);
  const px = img.data;
  const pcTop = Math.round(LANES.pc[0] * dpr), pcBot = H - Math.round(8 * dpr);
  const modeTop = Math.round(LANES.mode[0] * dpr), modeBot = Math.round(LANES.mode[1] * dpr);
  const span = view[1] - view[0];
  const rgb = { 0: [76, 195, 138], 1: [240, 180, 76], 3: [239, 111, 108] };
  const step = Math.max(1, Math.floor(span / (W * 64))); // subsample huge ranges
  const colMode = new Int8Array(W).fill(-1);
  for (let i = view[0]; i < view[1]; i += step) {
    const x = Math.floor(((i - view[0]) / span) * W);
    if (x < 0 || x >= W) continue;
    const p = T.prv[i];
    if (colMode[x] < p) colMode[x] = p; // most privileged mode wins the column
    const y = Math.round(pcBot - T.rank[T.pcIdx[i]] * (pcBot - pcTop));
    const o = (y * W + x) * 4;
    const c = rgb[p];
    px[o] = c[0]; px[o + 1] = c[1]; px[o + 2] = c[2]; px[o + 3] = 255;
  }
  for (let x = 0; x < W; x++) {
    const p = colMode[x];
    if (p < 0) continue;
    const c = rgb[p];
    for (let y = modeTop; y < modeBot; y++) {
      const o = (y * W + x) * 4;
      px[o] = c[0]; px[o + 1] = c[1]; px[o + 2] = c[2]; px[o + 3] = 255;
    }
  }
  ctx.putImageData(img, 0, 0);

  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  // Axis ticks: instruction index and cycle.
  ctx.fillStyle = '#8593a6';
  ctx.font = '10px ui-monospace, monospace';
  ctx.textBaseline = 'top';
  const ticks = 6;
  for (let t = 0; t <= ticks; t++) {
    const i = Math.min(T.n - 1, Math.floor(view[0] + (t / ticks) * (span - 1)));
    const x = xOf(i, w);
    ctx.fillRect(x, 12, 1, 4);
    const label = `#${i.toLocaleString()}`;
    const tw = ctx.measureText(label).width;
    ctx.fillText(label, Math.min(Math.max(0, x - tw / 2), w - tw), 1);
  }
  // Lane labels, drawn on a backing so they stay readable over the data.
  const label = (text, y) => {
    const tw = ctx.measureText(text).width;
    ctx.fillStyle = 'rgba(22,29,39,0.85)';
    ctx.fillRect(2, y - 1, tw + 6, 13);
    ctx.fillStyle = '#c4ccd8';
    ctx.fillText(text, 5, y);
  };
  label('privilege', LANES.mode[0] + 2);
  label('traps', LANES.events[0] + 2);
  label('pc (address rank)', LANES.pc[0] - 14);

  // Event markers.
  for (const e of T.events) {
    if (e.idx < view[0] || e.idx >= view[1]) continue;
    const x = xOf(e.idx, w);
    ctx.fillStyle = e.kind === 'exception' ? '#ef6f6c' : e.kind === 'interrupt' ? '#5ab0ff' : '#5f6b7a';
    const [a, b] = LANES.events;
    if (e.kind === 'xret') ctx.fillRect(x, a + 8, 1, b - a - 8);
    else ctx.fillRect(x - 0.5, a, 2, b - a);
  }

  // Selection cursor.
  if (selected >= view[0] && selected < view[1]) {
    ctx.fillStyle = 'rgba(255,255,255,0.7)';
    ctx.fillRect(xOf(selected, w), 18, 1, h - 18);
  }
  renderFunctions();
}

function setView(a, b) {
  const minSpan = Math.min(T.n, 50);
  let s = Math.max(minSpan, Math.round(b - a));
  s = Math.min(s, T.n);
  let start = Math.round(a);
  start = Math.max(0, Math.min(T.n - s, start));
  view = [start, start + s];
  draw();
}

function zoom(factor, centerFrac = 0.5) {
  const span = view[1] - view[0];
  const c = view[0] + centerFrac * span;
  const ns = span * factor;
  setView(c - centerFrac * ns, c - centerFrac * ns + ns);
}

canvas.addEventListener('wheel', (ev) => {
  if (!T) return;
  ev.preventDefault();
  const r = canvas.getBoundingClientRect();
  if (Math.abs(ev.deltaX) > Math.abs(ev.deltaY)) {
    const span = view[1] - view[0];
    setView(view[0] + (ev.deltaX / r.width) * span, view[1] + (ev.deltaX / r.width) * span);
  } else {
    zoom(Math.exp(ev.deltaY * 0.002), (ev.clientX - r.left) / r.width);
  }
}, { passive: false });

let drag = null;
canvas.addEventListener('pointerdown', (ev) => {
  if (!T) return;
  drag = { x: ev.clientX, view: [...view], moved: false };
  canvas.setPointerCapture(ev.pointerId);
});
canvas.addEventListener('pointermove', (ev) => {
  if (!T) return;
  const r = canvas.getBoundingClientRect();
  if (drag) {
    const dx = ev.clientX - drag.x;
    if (Math.abs(dx) > 3) drag.moved = true;
    const span = drag.view[1] - drag.view[0];
    const shift = (-dx / r.width) * span;
    setView(drag.view[0] + shift, drag.view[1] + shift);
  }
  showTip(ev, r);
});
canvas.addEventListener('pointerup', (ev) => {
  if (drag && !drag.moved) {
    const r = canvas.getBoundingClientRect();
    select(idxOf(ev.clientX - r.left, r.width), true);
  }
  drag = null;
});
canvas.addEventListener('pointerleave', () => { $('tip').style.display = 'none'; });

function showTip(ev, r) {
  const x = ev.clientX - r.left, y = ev.clientY - r.top;
  const i = Math.max(0, Math.min(T.n - 1, idxOf(x, r.width)));
  let text = describe(i);
  if (y >= LANES.events[0] - 4 && y <= LANES.events[1] + 4) {
    const near = T.events.filter((e) => Math.abs(xOf(e.idx, r.width) - x) < 4).slice(0, 4);
    if (near.length) text = near.map((e) => `${causeName(e)}\n  pc ${e.pc}${e.tval !== '0' ? `  tval ${e.tval}` : ''}`).join('\n');
  }
  const tip = $('tip');
  tip.textContent = text;
  tip.style.display = 'block';
  const tw = tip.offsetWidth;
  tip.style.left = `${Math.min(x + 12, r.width - tw - 4)}px`;
  tip.style.top = `${Math.min(y + 12, r.height - tip.offsetHeight - 4)}px`;
}

function dis(i) {
  if (!disasmReady) return T.inst[i].toString(16).padStart(8, '0');
  try { return disassemble(T.inst[i], BigInt('0x' + T.pcs[T.pcIdx[i]])); } catch { return '?'; }
}

function describe(i) {
  const sym = T.symOf[T.pcIdx[i]];
  return `#${i.toLocaleString()}  cycle ${T.cycle[i].toLocaleString()}  [${MODES[T.prv[i]]}]\n` +
    `${T.pcs[T.pcIdx[i]]}  ${dis(i)}${sym ? `\n<${sym}>` : ''}`;
}

// ------------------------------------------------------------ instruction list

const list = $('list');
list.addEventListener('scroll', () => renderList());
new ResizeObserver(() => renderList()).observe(list);

function renderList() {
  if (!T) return;
  const first = Math.floor(list.scrollTop / ROW_H);
  const count = Math.ceil(list.clientHeight / ROW_H) + 2;
  const rows = [];
  for (let i = first; i < Math.min(T.n, first + count); i++) {
    const p = T.prv[i];
    const sym = T.symOf[T.pcIdx[i]];
    rows.push(`<div class="row${i === selected ? ' sel' : ''}" data-i="${i}" style="top:${i * ROW_H}px">` +
      `<span class="p ${MODES[p]}">${MODES[p]}</span>${String(i).padStart(8)}  ${T.pcs[T.pcIdx[i]].padStart(16)}  ` +
      `${T.inst[i].toString(16).padStart(8, '0')}  ${dis(i).padEnd(32)}<span class="sym">${sym}</span></div>`);
  }
  list.querySelectorAll('.row').forEach((r) => r.remove());
  list.insertAdjacentHTML('beforeend', rows.join(''));
}
list.addEventListener('click', (ev) => {
  const row = ev.target.closest('.row');
  if (row) select(Number(row.dataset.i), false);
});

function select(i, scrollList) {
  selected = Math.max(0, Math.min(T.n - 1, i));
  if (selected < view[0] || selected >= view[1]) {
    const span = view[1] - view[0];
    setView(selected - span / 2, selected + span / 2);
  } else draw();
  if (scrollList) list.scrollTop = Math.max(0, selected * ROW_H - list.clientHeight / 2);
  renderList();
}

// ------------------------------------------------------------ side panels

function renderEvents() {
  $('ev-count').textContent = `(${T.events.length})`;
  const shown = T.events.slice(0, 3000);
  $('events').innerHTML = shown.map((e, k) =>
    `<div class="ev" data-k="${k}"><span class="k ${e.kind}"></span>#${e.idx} ${causeName(e)}</div>`).join('') +
    (T.events.length > shown.length ? `<div class="small">…${T.events.length - shown.length} more</div>` : '');
}
$('events').addEventListener('click', (ev) => {
  const el = ev.target.closest('.ev');
  if (el) select(T.events[Number(el.dataset.k)].idx, true);
});

function renderFunctions() {
  const counts = new Map();
  const span = view[1] - view[0];
  const step = Math.max(1, Math.floor(span / 200000));
  for (let i = view[0]; i < view[1]; i += step) {
    const s = T.symOf[T.pcIdx[i]] || '(unknown)';
    counts.set(s, (counts.get(s) || 0) + step);
  }
  const top = [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 12);
  const max = top.length ? top[0][1] : 1;
  $('fns').innerHTML = top.map(([name, c]) =>
    `<span>${name}</span><span>${((100 * c) / span).toFixed(1)}%</span><div class="bar" style="width:${(100 * c) / max}%"></div>`).join('');
}

// ------------------------------------------------------------ controls & input

$('zoom-in').onclick = () => T && zoom(0.5);
$('zoom-out').onclick = () => T && zoom(2);
$('zoom-reset').onclick = () => T && setView(0, T.n);
window.addEventListener('keydown', (ev) => {
  if (!T || ev.target.tagName === 'INPUT') return;
  if (ev.key === 'ArrowRight' || ev.key === 'j') select(selected + 1, true);
  if (ev.key === 'ArrowLeft' || ev.key === 'k') select(selected - 1, true);
  if (ev.key === '+' || ev.key === '=') zoom(0.5);
  if (ev.key === '-') zoom(2);
});

function fail(e) { alert('Could not load trace: ' + (e.message || e)); }
$('file').onchange = async () => {
  const f = $('file').files[0];
  if (f) try { load(await f.text()); } catch (e) { fail(e); }
};
document.addEventListener('dragover', (ev) => ev.preventDefault());
document.addEventListener('drop', async (ev) => {
  ev.preventDefault();
  const f = ev.dataTransfer.files[0];
  if (f) try { load(await f.text()); } catch (e) { fail(e); }
});
window.addEventListener('message', (ev) => {
  if (ev.data && ev.data.type === 'rvwasm-trace' && ev.origin === location.origin) {
    try { load(ev.data.json); } catch (e) { fail(e); }
  }
});
if (window.opener && location.hash === '#from-opener') window.opener.postMessage('rvwasm-trace-ready', location.origin);
