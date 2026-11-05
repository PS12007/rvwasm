// Runs the emulator off the main thread. Protocol:
//   main -> worker: {type:'boot', image}, {type:'input', data}, {type:'pause'},
//                   {type:'resume'}, {type:'step', count}, {type:'trace-start', capacity},
//                   {type:'trace-stop'}
//   worker -> main: {type:'output', data}, {type:'stats', ...}, {type:'status', text},
//                   {type:'error', text}, {type:'trace', json}, {type:'booted'}
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

async function boot(image) {
  await wasmReady;
  emu?.free();
  emu = null;
  paused = false;
  ipsHistory = [];
  postMessage({ type: 'status', text: `Downloading ${image} images…` });
  if (image === 'linux') {
    const [kernel, initrd] = await Promise.all([
      fetchBytes('images/linux-Image'),
      fetchBytes('images/linux-initramfs.cpio.gz'),
    ]);
    emu = Emulator.new_linux(kernel, initrd, 256, 'console=ttyS0 earlycon=sbi');
  } else {
    const [kernel, disk] = await Promise.all([
      fetchBytes('images/xv6-kernel'),
      fetchBytes('images/xv6-fs.img'),
    ]);
    emu = new Emulator(kernel, disk, 128);
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
      if (emu.idle()) {
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
  if (now - lastStats >= STATS_MS) sendStats(now);
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
    privCounts: Array.from(emu.privilege_counts()),
    csrs: JSON.parse(emu.csrs_json()),
    disasm: emu.disassemble_around_pc(2, 7),
    paused,
    tracing: emu.trace_active(),
    traceCount: emu.trace_count(),
  });
}

onmessage = async (ev) => {
  const m = ev.data;
  try {
    switch (m.type) {
      case 'boot':
        await boot(m.image);
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
          for (let i = 0; i < (m.count || 1); i++) emu.run(1);
          flushOutput();
          sendStats(performance.now());
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
