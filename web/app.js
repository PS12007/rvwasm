// Main-thread UI: terminal, live register/privilege panel, trace controls.
const $ = (id) => document.getElementById(id);
const REG_NAMES = ['zero', 'ra', 'sp', 'gp', 'tp', 't0', 't1', 't2', 's0', 's1', 'a0', 'a1', 'a2', 'a3', 'a4', 'a5',
  'a6', 'a7', 's2', 's3', 's4', 's5', 's6', 's7', 's8', 's9', 's10', 's11', 't3', 't4', 't5', 't6'];
const MODE = { 0: ['U', 'User'], 1: ['S', 'Supervisor'], 3: ['M', 'Machine'] };

const term = new Terminal({
  convertEol: true,
  cursorBlink: true,
  fontSize: 14,
  fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
  theme: { background: '#000000' },
  scrollback: 5000,
});
const fit = new FitAddon.FitAddon();
term.loadAddon(fit);
term.open($('terminal'));
fit.fit();
new ResizeObserver(() => fit.fit()).observe($('terminal'));
term.focus();

const worker = new Worker('worker.js', { type: 'module' });
const encoder = new TextEncoder();
let paused = false;
let lastTrace = null;
let currentImage = new URLSearchParams(location.search).get('guest') || 'xv6';

term.onData((d) => worker.postMessage({ type: 'input', data: encoder.encode(d) }));

worker.onmessage = (ev) => {
  const m = ev.data;
  switch (m.type) {
    case 'output': term.write(m.data); break;
    case 'stats': renderStats(m); break;
    case 'status': $('state').textContent = m.text; break;
    case 'booted': $('state').textContent = 'running'; break;
    case 'error':
      term.write(`\r\n\x1b[31m[rvwasm] ${m.text}\x1b[0m\r\n`);
      $('state').textContent = 'error';
      break;
    case 'trace': onTrace(m.json); break;
  }
};
worker.onerror = (e) => term.write(`\r\n\x1b[31m[rvwasm] worker failed: ${e.message}\x1b[0m\r\n`);

function boot() {
  prevPriv = [];
  privShare = [0, 0, 0];
  term.reset();
  term.write(`\x1b[90m[rvwasm] booting ${currentImage}…\x1b[0m\r\n`);
  setPaused(false);
  worker.postMessage({ type: 'boot', image: currentImage });
}

// Offer Linux only if its images were deployed.
fetch('images/linux-Image', { method: 'HEAD' }).then((r) => {
  if (r.ok) $('image').querySelector('[value=linux]').disabled = false;
  else if (currentImage === 'linux') currentImage = 'xv6';
  $('image').value = currentImage;
}).catch(() => {});
$('image').value = currentImage;
$('image').onchange = () => { currentImage = $('image').value; boot(); };
$('reset').onclick = boot;
$('pause').onclick = () => setPaused(!paused);
$('step').onclick = () => { setPaused(true, false); worker.postMessage({ type: 'step', count: 1 }); };

function setPaused(p, notify = true) {
  paused = p;
  $('pause').textContent = p ? 'Resume' : 'Pause';
  if (notify) worker.postMessage({ type: p ? 'pause' : 'resume' });
}

// ------------------------------------------------------------ stats panel

function fmtCount(n) {
  if (n >= 1e9) return (n / 1e9).toFixed(2) + ' G';
  if (n >= 1e6) return (n / 1e6).toFixed(1) + ' M';
  if (n >= 1e3) return (n / 1e3).toFixed(1) + ' k';
  return String(Math.round(n));
}
function fmtTime(s) {
  const m = Math.floor(s / 60);
  return m ? `${m}m ${Math.floor(s % 60)}s` : `${s.toFixed(1)}s`;
}
const pad16 = (h) => h.padStart(16, '0');

const regEls = [];
for (let i = 0; i < 32; i++) {
  const d = document.createElement('div');
  d.innerHTML = `<span class="n">${REG_NAMES[i]}</span><span class="v">0000000000000000</span>`;
  $('regs').appendChild(d);
  regEls.push(d.lastChild);
}
let prevRegs = [];
let prevPriv = [];
let privShare = [0, 0, 0];

function renderStats(m) {
  $('ips').textContent = m.paused ? 'paused' : (m.idle && m.ips < 1e5 ? 'idle' : fmtCount(m.ips) + 'IPS');
  const [letter, name] = MODE[m.privilege] || ['?', '?'];
  $('mode').textContent = `${letter} · ${name}`;
  $('mode').className = 'mode ' + letter;
  $('state').textContent = m.paused ? 'paused' : m.idle ? 'idle (wfi)' : 'running';
  $('load').textContent = Math.round(m.load * 100) + '%';
  $('instret').textContent = fmtCount(m.instret);
  $('uptime').textContent = fmtTime(m.uptime);
  $('traps').textContent = fmtCount(m.csrs._traps);
  $('disk').textContent = fmtCount(m.csrs._disk_requests);

  // Share of instructions per mode over the last few seconds (exponentially
  // smoothed), so it reflects what the guest is doing now rather than boot.
  const delta = m.privCounts.map((c, i) => c - (prevPriv[i] || 0));
  prevPriv = m.privCounts;
  const dsum = delta.reduce((a, b) => a + b, 0);
  if (dsum > 0) privShare = privShare.map((v, i) => 0.7 * v + 0.3 * (delta[i] / dsum));
  const total = privShare.reduce((a, b) => a + b, 0) || 1;
  const pct = privShare.map((c) => (100 * c) / total);
  ['u', 's', 'm'].forEach((k, i) => {
    $('share').querySelector('.' + k).style.width = pct[i] + '%';
    $('p' + k).textContent = pct[i].toFixed(1) + '%';
  });

  m.regs.forEach((h, i) => {
    const v = pad16(h);
    if (regEls[i].textContent !== v) {
      regEls[i].textContent = v;
      if (prevRegs.length) {
        regEls[i].classList.add('changed');
        requestAnimationFrame(() => requestAnimationFrame(() => regEls[i].classList.remove('changed')));
      }
    }
  });
  prevRegs = m.regs;

  $('pc').textContent = pad16(m.pc);
  const lines = m.disasm.trim().split('\n').map((l) => {
    const [addr, hex, text, sym] = l.split('|');
    const cur = addr === m.pc;
    const s = `${addr.slice(-8).padStart(8)}  ${hex.padEnd(8)}  ${text.padEnd(28)}${sym ? ` <span class="sym">${sym}</span>` : ''}`;
    return cur ? `<span class="cur">${s}</span>` : s + '\n';
  });
  $('disasm').innerHTML = lines.join('');

  $('csrs').innerHTML = Object.entries(m.csrs)
    .filter(([k]) => !k.startsWith('_'))
    .map(([k, v]) => `<div><span class="n">${k}</span><span>${pad16(v)}</span></div>`)
    .join('');

  drawSpark(m.ipsHistory);
  if (m.tracing) $('trace-status').textContent = `Recording… ${fmtCount(m.traceCount)} instructions seen`;
}

function drawSpark(hist) {
  const c = $('spark');
  const dpr = window.devicePixelRatio || 1;
  const w = c.clientWidth, h = 44;
  if (c.width !== w * dpr) { c.width = w * dpr; c.height = h * dpr; }
  const g = c.getContext('2d');
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, w, h);
  if (hist.length < 2) return;
  const max = Math.max(...hist, 1);
  const step = w / 119;
  g.beginPath();
  hist.forEach((v, i) => {
    const x = w - (hist.length - 1 - i) * step;
    const y = h - 2 - (v / max) * (h - 6);
    i ? g.lineTo(x, y) : g.moveTo(x, y);
  });
  g.strokeStyle = '#5ab0ff';
  g.lineWidth = 1.5;
  g.stroke();
  g.lineTo(w, h); g.lineTo(w - (hist.length - 1) * step, h); g.closePath();
  g.fillStyle = 'rgba(90,176,255,0.12)';
  g.fill();
  g.fillStyle = '#8593a6';
  g.font = '10px ui-monospace, monospace';
  g.fillText('peak ' + fmtCount(max) + 'IPS', 2, 10);
}

// ------------------------------------------------------------ tracing

$('trace-btn').onclick = () => {
  const btn = $('trace-btn');
  if (btn.classList.contains('active')) {
    worker.postMessage({ type: 'trace-stop' });
    btn.classList.remove('active');
    btn.textContent = 'Record';
    $('trace-status').textContent = 'Serialising trace…';
  } else {
    worker.postMessage({ type: 'trace-start', capacity: Number($('trace-cap').value) });
    btn.classList.add('active');
    btn.textContent = 'Stop';
    $('trace-status').textContent = 'Recording…';
  }
};

function onTrace(json) {
  lastTrace = json;
  const n = (json.match(/\],\[/g) || []).length + 1;
  $('trace-status').textContent = `Captured ${fmtCount(n)} instructions (${(json.length / 1e6).toFixed(1)} MB).`;
  $('trace-open').disabled = false;
  $('trace-dl').disabled = false;
}

$('trace-dl').onclick = () => {
  const a = document.createElement('a');
  a.href = URL.createObjectURL(new Blob([lastTrace], { type: 'application/json' }));
  a.download = `rvwasm-trace-${Date.now()}.json`;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
};

$('trace-open').onclick = () => {
  const w = window.open('trace.html#from-opener', '_blank');
  const onReady = (ev) => {
    if (ev.source === w && ev.data === 'rvwasm-trace-ready') {
      w.postMessage({ type: 'rvwasm-trace', json: lastTrace }, '*');
      window.removeEventListener('message', onReady);
    }
  };
  window.addEventListener('message', onReady);
};

boot();
