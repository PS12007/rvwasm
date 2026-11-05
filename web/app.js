// Main-thread UI for the console: terminal, instrument panel, command
// drawer, recorder with plain-English summaries, help, tour, shortcuts.
const $ = (id) => document.getElementById(id);
const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* storage unavailable */ } },
};
const REG_NAMES = ['zero', 'ra', 'sp', 'gp', 'tp', 't0', 't1', 't2', 's0', 's1', 'a0', 'a1', 'a2', 'a3', 'a4', 'a5',
  'a6', 'a7', 's2', 's3', 's4', 's5', 's6', 's7', 's8', 's9', 's10', 's11', 't3', 't4', 't5', 't6'];
const MODE_NAMES = { 0: 'user program', 1: 'kernel', 3: 'machine firmware' };

// ------------------------------------------------------------ guests

const GUESTS = {
  xv6: {
    label: 'xv6',
    bootInstructions: 4.3e8, // to the first shell prompt
    ready: (t) => t.includes('$ '),
    prompt: '$ ',
    hint: 'xv6 is a small teaching Unix. Click the screen and type, or pick a command from Try these.',
    groups: [
      ['Look around', [
        ['ls', 'List the files on the disk.'],
        ['cat README', 'Print the README file to the screen.'],
        ['wc README', 'Count the lines, words and letters in README.'],
        ['grep the README', 'Show every line of README that contains "the".'],
      ]],
      ['Make a file', [
        ['echo hi > a.txt', 'Write the word "hi" into a new file called a.txt.'],
        ['cat a.txt', 'Read a.txt back. Your files survive a page reload.'],
        ['mkdir stuff', 'Make a new folder called stuff.'],
        ['rm a.txt', 'Delete a.txt again.'],
      ]],
      ['Processes', [
        ['forktest', 'Start lots of copies of a program at once, then wait for them all to finish.'],
        ['ls | grep README', 'Connect two programs with a pipe: the output of ls feeds grep.'],
      ]],
      ['Stress the CPU', [
        ['stressfs', 'Several processes write and read files at the same time.'],
        ['usertests -q', "Run xv6's own test suite. Billions of instructions; takes a few minutes."],
      ]],
    ],
  },
  linux: {
    label: 'Linux',
    bootInstructions: 1.07e8,
    ready: (t) => t.includes('~#') || t.includes('Try:'),
    prompt: '# ',
    hint: 'Linux 6.12 with BusyBox, a compact set of standard Unix tools. Click the screen and type.',
    groups: [
      ['Look around', [
        ['uname -a', 'Show which kernel this is and what CPU it runs on.'],
        ['cat /proc/cpuinfo', "What Linux knows about this (emulated) processor."],
        ['ls /', 'List the top-level folders of the file system.'],
        ['ls /bin', 'See the programs BusyBox provides.'],
        ['dmesg | tail', 'The last few messages from the kernel boot log.'],
      ]],
      ['Make a file', [
        ['echo hello > /tmp/note', 'Write "hello" into a file in the /tmp folder.'],
        ['cat /tmp/note', 'Read the file back.'],
        ['ls -l /tmp', 'List /tmp with sizes and permissions.'],
      ]],
      ['See processes', [
        ['ps', 'List the programs running right now.'],
        ['top', 'A live view of programs and CPU use. Press q to quit.'],
        ['free', 'How much memory is used and how much is free.'],
        ['uptime', 'How long the system has been running.'],
      ]],
      ['Stress the CPU', [
        ['seq 1 200000 | wc -l', 'Count to 200,000 and count the lines that come out.'],
        ['md5sum /bin/busybox', 'Compute a checksum of the 1.5 MB BusyBox program.'],
      ]],
    ],
  },
  custom: { label: 'your kernel', bootInstructions: 0, ready: () => true, hint: 'Running your own kernel.', groups: [] },
};

// ------------------------------------------------------------ terminal

const AMBER = {
  background: '#00000000',
  foreground: '#ffb84d',
  cursor: '#ffcf80',
  cursorAccent: '#140d05',
  selectionBackground: '#ffb84d55',
  black: '#140d05', red: '#ff8a5c', green: '#ffd88a', yellow: '#ffc24d', blue: '#e0a25a',
  magenta: '#ffa36b', cyan: '#ffcf8f', white: '#ffd9a0',
  brightBlack: '#9a6e33', brightRed: '#ffa07a', brightGreen: '#ffe2a6', brightYellow: '#ffd27a',
  brightBlue: '#efb36c', brightMagenta: '#ffb98a', brightCyan: '#ffdcaa', brightWhite: '#fff0d6',
};

await Promise.race([document.fonts.load('20px VT323'), new Promise((r) => setTimeout(r, 2500))]).catch(() => {});

const term = new Terminal({
  convertEol: true,
  cursorBlink: true,
  cursorStyle: 'block',
  fontFamily: '"VT323", "IBM Plex Mono", monospace',
  fontSize: window.innerWidth < 600 ? 17 : 21,
  lineHeight: 1,
  allowTransparency: true,
  theme: AMBER,
  scrollback: 5000,
});
const fit = new FitAddon.FitAddon();
term.loadAddon(fit);
term.open($('terminal'));
// The WebGL renderer places every glyph in its own cell. The DOM renderer
// measures some VT323 glyphs as wider than a cell and shifts the line.
try {
  const gl = new WebglAddon.WebglAddon();
  gl.onContextLoss(() => gl.dispose());
  term.loadAddon(gl);
  document.body.classList.add('gl-term');
} catch { /* WebGL unavailable: the DOM renderer still works */ }
const refit = () => { try { fit.fit(); } catch { /* not visible yet */ } };
refit();
new ResizeObserver(refit).observe($('terminal'));

const worker = new Worker('worker.js', { type: 'module' });
const encoder = new TextEncoder();
const decoder = new TextDecoder();
let paused = false;
let lastTrace = null;
let currentImage = new URLSearchParams(location.search).get('guest') || 'xv6';
let recording = false;
let recordedInput = '';
let outputTail = '';
let booting = false;
// While recording, after Enter is pressed we watch for the shell prompt and
// stop automatically, so the trace covers the command and not idle time after.
let recWaitingForPrompt = false;
let recTail = '';
let recSeen = 0;
let recAuto = false;

function send(text) {
  if (recording) {
    recordedInput += text;
    if (/[\r\n]/.test(text)) { recWaitingForPrompt = true; recTail = ''; }
  }
  worker.postMessage({ type: 'input', data: encoder.encode(text) });
}
term.onData(send);

worker.onmessage = (ev) => {
  const m = ev.data;
  switch (m.type) {
    case 'output':
      term.write(m.data);
      if (booting) {
        outputTail = (outputTail + decoder.decode(m.data)).slice(-400);
        if ((GUESTS[currentImage] || GUESTS.custom).ready(outputTail)) finishBoot();
      }
      if (recording && recWaitingForPrompt) {
        recTail = (recTail + decoder.decode(m.data)).slice(-200);
        const prompt = (GUESTS[currentImage] || {}).prompt;
        // Ignore the echo of the typed line itself; the prompt comes after a newline.
        if (prompt && /\n/.test(recTail) && recTail.slice(recTail.lastIndexOf('\n')).includes(prompt)) {
          recAuto = true;
          toggleRecord();
        }
      }
      break;
    case 'stats': renderStats(m); break;
    case 'status': setState(m.text); break;
    case 'booted': setState('running'); break;
    case 'error':
      term.write(`\r\n\x1b[31m[rvwasm] ${m.text}\x1b[0m\r\n`);
      setState('error', true);
      finishBoot();
      break;
    case 'trace': onTrace(m.json); break;
    case 'disk': onDisk(m); break;
    case 'disk-file': saveFile(new Blob([m.bytes]), 'fs.img'); break;
  }
};
worker.onerror = (e) => term.write(`\r\n\x1b[31m[rvwasm] worker failed: ${e.message}\x1b[0m\r\n`);

function setState(text, alert = false) {
  $('state').textContent = text;
  $('state').classList.toggle('alert', alert);
}

// ------------------------------------------------------------ boot

function warmUp() {
  const s = $('screen');
  s.classList.remove('warming');
  void s.offsetWidth; // restart the animation
  s.classList.add('warming');
}

function startBoot(label) {
  booting = true;
  outputTail = '';
  $('boot').hidden = false;
  $('boot-title').textContent = `Booting ${label}`;
  $('boot-fill').style.width = '0%';
  $('boot-count').textContent = '0 instructions';
  $('boot-note').textContent = 'Downloading the operating system…';
  warmUp();
}

function finishBoot() {
  booting = false;
  $('boot').hidden = true;
  term.focus();
}

function bootProgress(instret) {
  if (!booting) return;
  const g = GUESTS[currentImage] || GUESTS.custom;
  $('boot-count').textContent = `${fmtCount(instret)} instructions executed`;
  if (g.bootInstructions) {
    $('boot-fill').style.width = `${Math.min(97, (100 * instret) / g.bootInstructions)}%`;
    $('boot-note').textContent = 'The emulated CPU is starting the kernel. It shows a prompt when it is ready.';
  } else {
    $('boot-fill').style.width = '100%';
    $('boot-note').textContent = 'Running.';
    setTimeout(finishBoot, 1200);
  }
}

function boot(freshDisk = false) {
  const g = GUESTS[currentImage];
  $('disk-card').hidden = currentImage !== 'xv6';
  $('hint').textContent = g.hint;
  history.replaceState(null, '', currentImage === 'xv6' ? location.pathname : `?guest=${currentImage}`);
  renderCommands();
  resetMeters();
  term.reset();
  setPaused(false);
  startBoot(g.label);
  worker.postMessage({ type: 'boot', image: currentImage, freshDisk, term: { rows: term.rows, cols: term.cols } });
}

// Offer Linux only if its images were deployed.
fetch('images/linux-Image', { method: 'HEAD' }).then((r) => {
  if (r.ok) $('image').querySelector('[value=linux]').disabled = false;
  else if (currentImage === 'linux') { currentImage = 'xv6'; boot(); }
  $('image').value = currentImage;
}).catch(() => {});
$('image').value = currentImage;
$('image').onchange = () => { currentImage = $('image').value; boot(); };

$('custom-kernel').onchange = () => { $('custom-boot').disabled = !$('custom-kernel').files.length; };
$('custom-boot').onclick = async () => {
  const file = $('custom-kernel').files[0];
  const kernel = new Uint8Array(await file.arrayBuffer());
  const extraFile = $('custom-extra').files[0];
  const extra = extraFile ? new Uint8Array(await extraFile.arrayBuffer()) : null;
  currentImage = 'custom';
  $('disk-card').hidden = true;
  $('hint').textContent = `Running ${file.name}.`;
  renderCommands();
  resetMeters();
  term.reset();
  setPaused(false);
  startBoot(file.name);
  worker.postMessage({ type: 'boot', image: 'custom', kernel, extra, term: { rows: term.rows, cols: term.cols } });
};

// ------------------------------------------------------------ machine keys

function setPaused(p, notify = true) {
  paused = p;
  $('pause-label').textContent = p ? 'Run' : 'Halt';
  $('run-lamp').classList.toggle('halted', p);
  if (notify) worker.postMessage({ type: p ? 'pause' : 'resume' });
}
const togglePause = () => setPaused(!paused);
const step = () => { setPaused(true, false); worker.postMessage({ type: 'step', count: 1 }); };
$('reset').onclick = () => boot();
$('pause').onclick = togglePause;
$('step').onclick = step;

// ------------------------------------------------------------ try-these drawer

function renderCommands() {
  const g = GUESTS[currentImage];
  const box = $('cmds');
  box.innerHTML = '';
  if (!g.groups.length) {
    box.innerHTML = '<p class="drawer-intro">No suggestions for a custom kernel. Type into the terminal.</p>';
    return;
  }
  for (const [title, cmds] of g.groups) {
    const h = document.createElement('div');
    h.className = 'cmd-group';
    h.innerHTML = `<span class="engr">${title}</span>`;
    box.appendChild(h);
    for (const [cmd, what] of cmds) {
      const b = document.createElement('button');
      b.className = 'cmd';
      b.innerHTML = `<code></code><span></span>`;
      b.firstChild.textContent = cmd;
      b.lastChild.textContent = what;
      b.title = `Type "${cmd}" into the terminal and press Enter`;
      b.onclick = () => runCommand(cmd);
      box.appendChild(b);
    }
  }
}

function runCommand(cmd) {
  if (paused) setPaused(false);
  if (booting) {
    term.write('\x1b[90m(still booting; your command will run when the prompt appears)\x1b[0m');
  }
  send(cmd + '\r');
  term.focus();
}

const wideScreen = () => window.innerWidth > 1280;
function setDrawer(open) {
  document.querySelector('.chassis').classList.toggle('no-drawer', !open);
  $('drawer-btn').setAttribute('aria-expanded', String(open));
  $('drawer-btn').classList.toggle('down', open);
  store.set('rvwasm-drawer', open ? 'open' : 'closed');
  requestAnimationFrame(refit);
}
const drawerOpen = () => !document.querySelector('.chassis').classList.contains('no-drawer');
$('drawer-btn').onclick = () => setDrawer(!drawerOpen());
$('drawer-close').onclick = () => setDrawer(false);
setDrawer(store.get('rvwasm-drawer') ? store.get('rvwasm-drawer') === 'open' : wideScreen());

// ------------------------------------------------------------ instruments

function fmtCount(n) {
  if (n >= 1e9) return (n / 1e9).toFixed(2) + ' G';
  if (n >= 1e6) return (n / 1e6).toFixed(1) + ' M';
  if (n >= 1e3) return (n / 1e3).toFixed(1) + ' k';
  return String(Math.round(n));
}
function fmtTime(s) {
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = Math.floor(s % 60);
  return h ? `${h}:${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}` : `${m}:${String(sec).padStart(2, '0')}`;
}
// Dim the leading zeros of a 64-bit hex value, like an unlit LED digit.
function ledHex(h) {
  const v = h.padStart(16, '0');
  const i = v.search(/[^0]/);
  const cut = i < 0 ? 15 : i;
  return `<span class="z">${v.slice(0, cut)}</span>${v.slice(cut)}`;
}

const regEls = REG_NAMES.map((name) => {
  const d = document.createElement('div');
  d.className = 'reg';
  d.innerHTML = `<span class="n">${name}</span><span class="v">${ledHex('0')}</span>`;
  $('regs').appendChild(d);
  return d.lastChild;
});
let prevRegs = [];
let prevPriv = [];
let privShare = [0, 0, 0];

function resetMeters() {
  prevPriv = [];
  prevRegs = [];
  privShare = [0, 0, 0];
}

function renderStats(m) {
  bootProgress(m.instret);
  if (m.paused) {
    $('ips').textContent = 'HALT';
    $('ips-unit').textContent = '';
  } else if (m.idle && m.ips < 1e5) {
    $('ips').textContent = 'IDLE';
    $('ips-unit').textContent = 'waiting';
  } else {
    const [v, u] = m.ips >= 1e6 ? [(m.ips / 1e6).toFixed(1), 'MIPS'] : [(m.ips / 1e3).toFixed(0), 'kIPS'];
    $('ips').textContent = v;
    $('ips-unit').textContent = u;
  }
  if (!booting) setState(m.paused ? 'halted' : m.idle ? 'idle, waiting for input' : 'running', m.paused);
  $('instret').textContent = fmtCount(m.instret);
  $('uptime').textContent = fmtTime(m.uptime);
  $('traps').textContent = fmtCount(m.csrs._traps);
  $('disk').textContent = fmtCount(m.csrs._disk_requests);

  // Lamps: where the CPU is right now, plus a smoothed share of recent time.
  $('lamp-m').classList.toggle('on', m.privilege === 3);
  $('lamp-s').classList.toggle('on', m.privilege === 1 && !m.idle);
  $('lamp-u').classList.toggle('on', m.privilege === 0);
  $('lamp-idle').classList.toggle('on', m.idle);
  const delta = m.privCounts.map((c, i) => c - (prevPriv[i] || 0));
  prevPriv = m.privCounts;
  const dsum = delta.reduce((a, b) => a + b, 0);
  if (dsum > 0) privShare = privShare.map((v, i) => 0.7 * v + 0.3 * (delta[i] / dsum));
  const total = privShare.reduce((a, b) => a + b, 0) || 1;
  $('duty-u').style.width = `${(100 * privShare[0]) / total}%`;
  $('duty-s').style.width = `${(100 * privShare[1]) / total}%`;
  $('duty-m').style.width = `${(100 * privShare[2]) / total}%`;
  $('duty-load').style.width = `${100 * (1 - m.load)}%`;

  m.regs.forEach((h, i) => {
    if (prevRegs[i] === h) return;
    regEls[i].innerHTML = ledHex(h);
    if (prevRegs.length) {
      regEls[i].classList.add('changed');
      requestAnimationFrame(() => requestAnimationFrame(() => regEls[i].classList.remove('changed')));
    }
  });
  prevRegs = m.regs;

  $('pc').innerHTML = ledHex(m.pc);
  $('disasm').innerHTML = m.disasm.trim().split('\n').map((l) => {
    const [addr, hex, text, sym] = l.split('|');
    const s = `${addr.slice(-8).padStart(8)} ${(text || '').padEnd(24)}${sym ? `<span class="sym">${sym}</span>` : ''}`;
    return addr === m.pc ? `<span class="cur">${s}</span>` : s + '\n';
  }).join('');

  if ($('csr-module').open) {
    $('csrs').innerHTML = Object.entries(m.csrs)
      .filter(([k]) => !k.startsWith('_'))
      .map(([k, v]) => `<div><span class="n">${k}</span><span>${v.padStart(16, '0')}</span></div>`)
      .join('');
  }

  drawScope(m.ipsHistory);
  if (m.tracing) recSeen = m.traceCount;
  if (m.tracing) $('trace-status').textContent = `Recording: ${fmtCount(m.traceCount)} instructions so far. Run a command, then press Stop.`;
}

// Speed trace drawn like an oscilloscope: graticule, phosphor line with glow.
function drawScope(hist) {
  const c = $('spark');
  const dpr = window.devicePixelRatio || 1;
  const w = c.clientWidth, h = c.clientHeight;
  if (c.width !== Math.round(w * dpr)) { c.width = Math.round(w * dpr); c.height = Math.round(h * dpr); }
  const g = c.getContext('2d');
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, w, h);
  g.strokeStyle = css('--phosphor-faint');
  g.globalAlpha = 0.5;
  g.lineWidth = 1;
  g.setLineDash([1, 3]);
  for (let x = 0; x <= 10; x++) { g.beginPath(); g.moveTo(Math.round((x * w) / 10) + 0.5, 0); g.lineTo(Math.round((x * w) / 10) + 0.5, h); g.stroke(); }
  for (let y = 0; y <= 4; y++) { g.beginPath(); g.moveTo(0, Math.round((y * h) / 4) + 0.5); g.lineTo(w, Math.round((y * h) / 4) + 0.5); g.stroke(); }
  g.setLineDash([]);
  g.globalAlpha = 1;
  if (hist.length < 2) return;
  const max = Math.max(...hist, 1);
  const stepX = w / 119;
  g.beginPath();
  hist.forEach((v, i) => {
    const x = w - (hist.length - 1 - i) * stepX;
    const y = h - 4 - (v / max) * (h - 12);
    i ? g.lineTo(x, y) : g.moveTo(x, y);
  });
  g.strokeStyle = css('--phosphor');
  g.shadowColor = css('--phosphor');
  g.shadowBlur = 6;
  g.lineWidth = 1.6;
  g.stroke();
  g.shadowBlur = 0;
  g.fillStyle = css('--phosphor-dim');
  g.font = '15px VT323, monospace';
  g.fillText(`PEAK ${fmtCount(max)}IPS`.replace(/(\d)IPS/, '$1 IPS'), 6, 14);
}

// ------------------------------------------------------------ recorder

function toggleRecord() {
  if (recording) {
    recording = false;
    recWaitingForPrompt = false;
    worker.postMessage({ type: 'trace-stop' });
    $('trace-btn').classList.remove('armed');
    $('rec-lamp').classList.remove('on', 'blink');
    $('trace-label').textContent = 'Record';
    $('trace-status').textContent = 'Reading the recording…';
  } else {
    recording = true;
    recordedInput = '';
    recAuto = false;
    recSeen = 0;
    worker.postMessage({ type: 'trace-start', capacity: Number($('trace-cap').value) });
    $('trace-btn').classList.add('armed');
    $('rec-lamp').classList.add('on', 'blink');
    $('trace-label').textContent = 'Stop';
    $('trace-status').textContent = 'Recording. Run a command, then press Stop.';
    $('explain').hidden = true;
  }
}
$('trace-btn').onclick = toggleRecord;

function onTrace(json) {
  lastTrace = json;
  let data = null;
  try { data = JSON.parse(json); } catch { /* fall back to counting */ }
  const n = data ? data.insts.length : (json.match(/\],\[/g) || []).length + 1;
  $('trace-status').textContent = `Captured ${fmtCount(n)} instructions${recAuto ? ', stopped when the command finished' : ''}.`;
  $('trace-open').disabled = false;
  $('trace-dl').disabled = false;
  if (data) {
    let html = explain(data, recordedInput);
    if (recSeen > n * 1.05) {
      html += `<p style="opacity:.75">The command ran for about ${fmtCount(recSeen)} instructions, more than the recorder keeps, so this covers its last ${fmtCount(n)}. Pick a larger "Keep last" to see more.</p>`;
    }
    $('explain').innerHTML = html;
    $('explain').hidden = false;
  }
}

const plural = (n, one, many) => `${n.toLocaleString()} ${n === 1 ? one : many}`;
const pct = (a, b) => {
  if (!b || !a) return '0';
  const v = Math.round((100 * a) / b);
  return v === 0 ? 'under 1' : v === 100 && a < b ? 'over 99' : String(v);
};
const esc = (s) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));

// Summarise a trace in plain language.
function explain(d, typed) {
  const n = d.insts.length;
  const byMode = { 0: 0, 1: 0, 3: 0 };
  for (const r of d.insts) byMode[r[3]]++;
  const ev = d.events;
  const count = (f) => ev.filter(f).length;
  const syscalls = count((e) => e.kind === 'exception' && e.cause === 8);
  const sbi = count((e) => e.kind === 'exception' && e.cause === 9);
  const faults = count((e) => e.kind === 'exception' && [12, 13, 15].includes(e.cause));
  const timer = count((e) => e.kind === 'interrupt' && (e.cause === 5 || e.cause === 7));
  const external = count((e) => e.kind === 'interrupt' && (e.cause === 9 || e.cause === 11));
  const intoKernel = count((e) => e.kind !== 'xret' && e.from === 0);

  // Busiest function, using the symbol table shipped with the trace.
  let hot = null;
  if (d.symbols && d.symbols.length) {
    const syms = d.symbols.map(([name, a]) => [name, parseInt(a.slice(-12), 16)]).sort((a, b) => a[1] - b[1]);
    const counts = new Map();
    for (const r of d.insts) {
      const a = parseInt(r[1].slice(-12), 16);
      let lo = 0, hi = syms.length;
      while (lo < hi) { const m = (lo + hi) >> 1; if (syms[m][1] <= a) lo = m + 1; else hi = m; }
      if (lo) counts.set(syms[lo - 1][0], (counts.get(syms[lo - 1][0]) || 0) + 1);
    }
    const top = [...counts.entries()].sort((a, b) => b[1] - a[1])[0];
    if (top) hot = top;
  }

  const lines = typed.split(/\r|\n/).map((l) => l.trim()).filter(Boolean);
  const intro = lines.length
    ? `While you ran ${lines.slice(-3).map((l) => `<code>${esc(l)}</code>`).join(', ')}:`
    : 'In the recorded stretch:';
  const out = [`<h4>What just happened</h4><p>${intro}</p>`];
  out.push(`<p>The CPU executed ${plural(n, 'instruction', 'instructions')}. ${pct(byMode[0], n)}% were your programs (user mode) and ${pct(byMode[1], n)}% were the kernel${byMode[3] ? `, ${pct(byMode[3], n)}% firmware` : ''}.</p>`);
  if (syscalls) out.push(`<p>Programs asked the kernel for help ${plural(syscalls, 'time', 'times')}. Each of these <b>system calls</b> (to print text, read a file, start a program…) switches the CPU from user mode into the kernel and back.</p>`);
  else if (byMode[0] === 0) out.push('<p>No user program ran in this stretch: the kernel was busy on its own, or waiting for something to happen.</p>');
  if (timer) out.push(`<p>The timer interrupted ${plural(timer, 'time', 'times')}, giving the kernel a chance to switch to another program.</p>`);
  if (external) out.push(`<p>Hardware (the keyboard or the disk) interrupted ${plural(external, 'time', 'times')}.</p>`);
  if (faults) out.push(`<p>${plural(faults, 'page fault', 'page faults')}: a program touched memory that wasn't set up yet, and the kernel mapped it in.</p>`);
  if (sbi) out.push(`<p>The kernel called the firmware ${plural(sbi, 'time', 'times')}, for example to set the next timer alarm.</p>`);
  if (intoKernel && intoKernel !== syscalls) out.push(`<p>In total the CPU switched from a program into the kernel ${plural(intoKernel, 'time', 'times')}.</p>`);
  if (hot) out.push(`<p>The busiest code was <code>${esc(hot[0])}</code> (${pct(hot[1], n)}% of instructions).</p>`);
  out.push(`<p style="opacity:.75">Open the timeline to scroll through every instruction.</p>`);
  return out.join('');
}

function saveFile(blob, name) {
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}
$('trace-dl').onclick = () => saveFile(new Blob([lastTrace], { type: 'application/json' }), `rvwasm-trace-${Date.now()}.json`);
$('trace-open').onclick = () => {
  const w = window.open('trace.html#from-opener', '_blank');
  const onReady = (ev) => {
    if (ev.source === w && ev.data === 'rvwasm-trace-ready') {
      w.postMessage({ type: 'rvwasm-trace', json: lastTrace }, location.origin);
      window.removeEventListener('message', onReady);
    }
  };
  window.addEventListener('message', onReady);
};

// ------------------------------------------------------------ disk

function onDisk(m) {
  const el = $('disk-status');
  if (m.error) el.textContent = m.error;
  else if (m.savedAt) {
    const t = new Date(m.savedAt).toLocaleTimeString();
    el.textContent = m.restored ? `Started from your saved disk (saved ${t}).` : `Saved in this browser at ${t}.`;
  } else el.textContent = 'Started from a fresh disk. Files you change are saved in this browser.';
}
$('disk-dl').onclick = () => worker.postMessage({ type: 'disk-download' });
$('disk-reset').onclick = () => boot(true);

// ------------------------------------------------------------ tooltips

const tip = $('tip');
function showTip(el) {
  tip.textContent = el.dataset.tip;
  tip.hidden = false;
  const r = el.getBoundingClientRect();
  const tw = tip.offsetWidth, th = tip.offsetHeight;
  let x = Math.min(window.innerWidth - tw - 12, Math.max(12, r.right - tw));
  let y = r.bottom + 8;
  if (y + th > window.innerHeight - 8) y = r.top - th - 8;
  tip.style.left = `${x}px`;
  tip.style.top = `${y}px`;
}
const hideTip = () => { tip.hidden = true; };
document.querySelectorAll('.qmark').forEach((q) => {
  q.setAttribute('aria-label', q.dataset.tip);
  q.addEventListener('mouseenter', () => showTip(q));
  q.addEventListener('mouseleave', hideTip);
  q.addEventListener('focus', () => showTip(q));
  q.addEventListener('blur', hideTip);
  // Inside <summary>, a click would toggle the panel; show the tip instead.
  q.addEventListener('click', (e) => { e.preventDefault(); e.stopPropagation(); tip.hidden ? showTip(q) : hideTip(); });
});
window.addEventListener('scroll', hideTip, true);

// ------------------------------------------------------------ sheets

function openSheet(id) {
  hideTip();
  $(id).showModal();
}
$('help-btn').onclick = () => openSheet('help');
$('about-btn').onclick = () => openSheet('about');
document.querySelectorAll('[data-close]').forEach((b) => { b.onclick = () => b.closest('dialog').close(); });
document.querySelectorAll('dialog').forEach((d) => d.addEventListener('click', (e) => { if (e.target === d) d.close(); }));
$('tour-again').onclick = () => { $('help').close(); startTour(); };

// ------------------------------------------------------------ guided tour

const TOUR = [
  ['screen', 'This is the terminal', "It's the screen of the emulated computer. Click it and type a command, then press Enter."],
  ['drawer', 'Not sure what to type?', 'Click any of these commands and it gets typed into the terminal for you.', () => { if (!drawerOpen()) setDrawer(true); }],
  ['regs-module', "These are the CPU's registers", 'The processor keeps everything it is working on in these 32 slots. Watch the values flicker as programs run.'],
  ['mode-lamps', 'Kernel or program?', 'The lit lamp shows who the CPU is working for: S is the operating system kernel, U is an ordinary program like ls. WFI means it is idle.'],
  ['rec-module', 'Try Record', "Press Record, run a command, then press Stop. You'll get a plain-English account of what the CPU did, and a timeline to explore."],
];
let tourAt = -1;
function startTour() { tourAt = 0; $('tour').hidden = false; showTourStep(); }
function endTour() { $('tour').hidden = true; tourAt = -1; store.set('rvwasm-toured', '1'); term.focus(); }
function showTourStep() {
  const [id, title, text, before] = TOUR[tourAt];
  if (before) before();
  const el = $(id);
  if (el.tagName === 'DETAILS') el.open = true;
  el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  requestAnimationFrame(() => {
    const r = el.getBoundingClientRect();
    const ring = $('tour-ring');
    const pad = 6;
    const top = Math.max(4, r.top - pad), left = Math.max(4, r.left - pad);
    ring.style.top = `${top}px`;
    ring.style.left = `${left}px`;
    ring.style.width = `${Math.min(window.innerWidth - 8, r.width + 2 * pad)}px`;
    ring.style.height = `${Math.min(window.innerHeight - 8, r.height + 2 * pad)}px`;
    $('tour-step').textContent = `${tourAt + 1} of ${TOUR.length} · ${title}`;
    $('tour-text').textContent = text;
    $('tour-next').querySelector('.cap').textContent = tourAt === TOUR.length - 1 ? 'Done' : 'Next';
    const card = $('tour-card');
    const cw = card.offsetWidth, ch = card.offsetHeight;
    // Beside the target if there's room, else below or above it.
    let x, y;
    if (r.left - cw - 20 > 0) { x = r.left - cw - 16; y = r.top; }
    else if (r.right + cw + 20 < window.innerWidth) { x = r.right + 16; y = r.top; }
    else { x = r.left; y = r.bottom + 14 + ch < window.innerHeight ? r.bottom + 14 : r.top - ch - 14; }
    card.style.left = `${Math.min(window.innerWidth - cw - 12, Math.max(12, x))}px`;
    card.style.top = `${Math.min(window.innerHeight - ch - 12, Math.max(12, y))}px`;
  });
}
$('tour-next').onclick = () => { if (++tourAt >= TOUR.length) endTour(); else showTourStep(); };
$('tour-skip').onclick = endTour;
window.addEventListener('resize', () => { if (tourAt >= 0) showTourStep(); });

// ------------------------------------------------------------ shortcuts

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && tourAt >= 0) { endTour(); return; }
  const t = e.target;
  if (t.closest && (t.closest('.xterm') || t.closest('dialog'))) return;
  if (['INPUT', 'SELECT', 'TEXTAREA'].includes(t.tagName) || e.ctrlKey || e.metaKey || e.altKey) return;
  if (tourAt >= 0) return;
  const k = e.key.toLowerCase();
  if (k === ' ') { e.preventDefault(); togglePause(); }
  else if (k === 's') step();
  else if (k === 'r') toggleRecord();
  else if (k === 't') setDrawer(!drawerOpen());
  else if (e.key === '?') openSheet('help');
});

// On phones, start with only the headline instruments open.
if (window.matchMedia('(max-width: 860px)').matches) {
  for (const id of ['pc-module', 'regs-module', 'rec-module']) $(id).open = false;
}

boot();
if (!store.get('rvwasm-toured')) setTimeout(startTour, 900);
