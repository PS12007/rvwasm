// Runs the emulator off the main thread. Protocol:
//   main -> worker: {type:'boot', image}, {type:'input', data}, {type:'pause'},
//                   {type:'resume'}, {type:'step', count}, {type:'trace-start', capacity},
//                   {type:'trace-stop'}, {type:'back', count}, {type:'back-trap'},
//                   {type:'seek', instret}, {type:'snapshot'}
//   worker -> main: {type:'output', data}, {type:'stats', ...}, {type:'status', text},
//                   {type:'error', text}, {type:'trace', json}, {type:'booted'},
//                   {type:'snapshot-file', bytes}, {type:'travel', text}
import init, { Emulator } from './pkg/rvweb.js';

const SLICE_MS = 12;          // run this long before yielding to the event loop
const CHUNK = 400_000;        // cycles per emu.run() call while busy
const IDLE_HZ = 100_000_000;  // guest cycles per real second while the guest idles in WFI
const STATS_MS = 250;

let emu = null;
let paused = false;
let scheduled = false;
let lastLoop = 0;
let lastStats = 0;
let lastStatsInstret = 0;
let lastStatsCycle = 0;
let busyMs = 0;
let ipsHistory = [];
let startTime = 0;

const wasmReady = init();

async function fetchBytes(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${url}: HTTP ${r.status}`);
  return new Uint8Array(await r.arrayBuffer());
}

// ------------------------------------------------------------ saved disk
// xv6's disk is kept in this browser's IndexedDB so files survive a reload.
// Saves happen once the guest has stopped writing for a couple of seconds.
const DISK_KEY = 'xv6-fs.img';
let diskWrites = 0;
let diskDirtyAt = 0;
let diskSavedAt = null;

function idb() {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open('rvwasm', 1);
    req.onupgradeneeded = () => req.result.createObjectStore('disks');
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}
async function idbOp(mode, fn) {
  const db = await idb();
  return new Promise((resolve, reject) => {
    const tx = db.transaction('disks', mode);
    const req = fn(tx.objectStore('disks'));
    tx.oncomplete = () => { db.close(); resolve(req.result); };
    tx.onerror = () => { db.close(); reject(tx.error); };
  });
}
const loadDisk = () => idbOp('readonly', (s) => s.get(DISK_KEY)).catch(() => undefined);
const storeDisk = (bytes) => idbOp('readwrite', (s) => s.put({ bytes, savedAt: Date.now() }, DISK_KEY));
const forgetDisk = () => idbOp('readwrite', (s) => s.delete(DISK_KEY)).catch(() => {});

function diskStatus(extra = {}) {
  postMessage({ type: 'disk', savedAt: diskSavedAt, ...extra });
}

async function maybeSaveDisk(now) {
  // While looking at the past, the disk is as it was then: don't save that.
  if (!emu || currentImage !== 'xv6' || emu.replaying()) return;
  const w = emu.disk_writes();
  if (w !== diskWrites) { diskWrites = w; diskDirtyAt = now; return; }
  if (diskDirtyAt && now - diskDirtyAt > 2000) {
    diskDirtyAt = 0;
    try {
      await storeDisk(emu.disk_image());
      diskSavedAt = Date.now();
      diskStatus();
    } catch (e) {
      diskStatus({ error: 'Could not save the disk in this browser.' });
    }
  }
}

let currentImage = null;

// A Linux Image has "RISCV\0\0\0" at byte 48; anything else is treated as an ELF.
const isLinuxImage = (b) => b.length > 56 && String.fromCharCode(...b.slice(48, 53)) === 'RISCV';

async function boot(image, term, freshDisk = false, custom = null) {
  currentImage = image;
  diskWrites = 0;
  diskDirtyAt = 0;
  diskSavedAt = null;
  await wasmReady;
  emu?.free();
  emu = null;
  paused = false;
  ipsHistory = [];
  postMessage({ type: 'status', text: `Downloading ${image} images…` });
  if (image === 'custom' && Emulator.is_snapshot(custom.kernel)) {
    emu = Emulator.from_snapshot(custom.kernel);
  } else if (image === 'custom') {
    const size = term ? ` rvwasm_rows=${term.rows} rvwasm_cols=${term.cols}` : '';
    emu = isLinuxImage(custom.kernel)
      ? Emulator.new_linux(custom.kernel, custom.extra, 256, 'console=ttyS0 earlycon=sbi' + size, null)
      : new Emulator(custom.kernel, custom.extra, 128);
  } else if (image === 'linux') {
    const [kernel, initrd] = await Promise.all([
      fetchBytes('images/linux-Image'),
      fetchBytes('images/linux-initramfs.cpio.gz'),
    ]);
    // System.map is optional: it only labels kernel addresses in the panel.
    const map = await fetch('images/linux-System.map').then((r) => (r.ok ? r.text() : null)).catch(() => null);
    // Unknown key=value boot parameters reach /init as environment variables;
    // /etc/profile uses these to set the tty size.
    const size = term ? ` rvwasm_rows=${term.rows} rvwasm_cols=${term.cols}` : '';
    emu = Emulator.new_linux(kernel, initrd, 256, 'console=ttyS0 earlycon=sbi' + size, map);
  } else {
    if (freshDisk) await forgetDisk();
    const saved = freshDisk ? undefined : await loadDisk();
    const [kernel, disk] = await Promise.all([
      fetchBytes('images/xv6-kernel'),
      saved ? Promise.resolve(saved.bytes) : fetchBytes('images/xv6-fs.img'),
    ]);
    diskSavedAt = saved ? saved.savedAt : null;
    emu = new Emulator(kernel, disk, 128);
    diskStatus({ restored: !!saved });
  }
  startTime = performance.now();
  lastLoop = lastStats = startTime;
  lastStatsInstret = 0;
  lastStatsCycle = 0;
  busyMs = 0;
  postMessage({ type: 'booted', image });
  schedule(0);
}

// MessageChannel gives a zero-delay macrotask (setTimeout clamps to >= 4ms).
const channel = new MessageChannel();
channel.port1.onmessage = () => { scheduled = false; loop(); };
function schedule(delay) {
  if (scheduled) return;
  scheduled = true;
  if (delay > 0) setTimeout(() => { scheduled = false; loop(); }, delay);
  else channel.port2.postMessage(0);
}

function flushOutput() {
  const out = emu.take_output();
  if (out.length) postMessage({ type: 'output', data: out }, [out.buffer]);
}

function loop() {
  if (!emu || paused) return;
  const t0 = performance.now();
  // While idle, let guest time advance roughly in step with real time.
  let idleBudget = Math.min(t0 - lastLoop, 100) * (IDLE_HZ / 1000);
  lastLoop = t0;
  const deadline = t0 + SLICE_MS;
  let busy = 0;
  try {
    while (!emu.halted()) {
      if (emu.replaying()) {
        // Catching up after going back: replay at full speed, idle or not.
        emu.run(CHUNK * 25);
      } else if (emu.idle()) {
        if (idleBudget <= 0) break;
        idleBudget -= emu.run(Math.min(idleBudget, CHUNK * 10));
      } else {
        const s = performance.now();
        emu.run(CHUNK);
        busy += performance.now() - s;
      }
      if (performance.now() >= deadline) break;
    }
  } catch (e) {
    postMessage({ type: 'error', text: String(e) });
    paused = true;
    return;
  }
  busyMs += busy;
  flushOutput();
  const now = performance.now();
  if (now - lastStats >= STATS_MS) {
    sendStats(now);
    maybeSaveDisk(now);
  }
  if (emu.halted()) {
    sendStats(now);
    postMessage({ type: 'status', text: 'Guest halted.' });
    return;
  }
  schedule(emu.idle() ? 4 : 0);
}

function sendStats(now) {
  const instret = emu.instret();
  const cycle = emu.cycle();
  const dt = (now - lastStats) / 1000;
  const ips = dt > 0 ? (instret - lastStatsInstret) / dt : 0;
  const load = dt > 0 ? Math.min(1, busyMs / 1000 / dt) : 0;
  ipsHistory.push(ips);
  if (ipsHistory.length > 120) ipsHistory.shift();
  lastStats = now;
  lastStatsInstret = instret;
  lastStatsCycle = cycle;
  busyMs = 0;
  postMessage({
    type: 'stats',
    ips,
    load,
    ipsHistory,
    instret,
    cycle,
    uptime: (now - startTime) / 1000,
    privilege: emu.privilege(),
    idle: emu.idle(),
    pc: emu.pc().toString(16),
    regs: Array.from(emu.registers(), (v) => v.toString(16)),
    fregs: Array.from(emu.fp_registers(), (v) => v.toString(16)),
    privCounts: Array.from(emu.privilege_counts()),
    csrs: JSON.parse(emu.csrs_json()),
    disasm: emu.disassemble_around_pc(2, 7),
    paused,
    tracing: emu.trace_active(),
    traceCount: emu.trace_count(),
    history: Array.from(emu.history()),
    replaying: emu.replaying(),
  });
}

onmessage = async (ev) => {
  const m = ev.data;
  try {
    switch (m.type) {
      case 'boot':
        await boot(m.image, m.term, !!m.freshDisk, m.image === 'custom' ? { kernel: m.kernel, extra: m.extra } : null);
        break;
      case 'disk-download':
        if (emu) {
          const bytes = emu.disk_image();
          postMessage({ type: 'disk-file', bytes }, [bytes.buffer]);
        }
        break;
      case 'input':
        emu?.push_input(m.data);
        if (emu && !paused) schedule(0);
        break;
      case 'pause':
        paused = true;
        if (emu) sendStats(performance.now());
        break;
      case 'resume':
        paused = false;
        lastLoop = performance.now();
        schedule(0);
        break;
      case 'step':
        if (emu) {
          paused = true;
          emu.step_forward(m.count || 1);
          flushOutput();
          sendStats(performance.now());
        }
        break;
      case 'back':
      case 'back-trap':
      case 'seek':
        if (emu) {
          paused = true;
          flushOutput(); // output so far belongs to the present; keep it
          const t0 = performance.now();
          const from = emu.instret();
          let ok = true;
          if (m.type === 'back') emu.step_back(m.count || 1);
          else if (m.type === 'back-trap') ok = emu.back_to_last_trap();
          else emu.seek(m.instret);
          const ms = performance.now() - t0;
          const moved = emu.instret() - from;
          postMessage({
            type: 'travel',
            text: !ok ? 'No trap within reach.'
              : moved === 0 ? 'Already as far as it goes.'
                : `${moved < 0 ? 'Back' : 'Forward'} ${Math.abs(moved).toLocaleString()} instruction${Math.abs(moved) === 1 ? '' : 's'} in ${ms.toFixed(0)} ms.`,
          });
          sendStats(performance.now());
        }
        break;
      case 'snapshot':
        if (emu) {
          const bytes = emu.save_snapshot();
          postMessage({ type: 'snapshot-file', bytes }, [bytes.buffer]);
        }
        break;
      case 'trace-start':
        emu?.trace_start(m.capacity);
        break;
      case 'trace-stop':
        if (emu) {
          emu.trace_stop();
          postMessage({ type: 'trace', json: emu.trace_json() });
        }
        break;
    }
  } catch (e) {
    postMessage({ type: 'error', text: String(e && e.message ? e.message : e) });
  }
};
