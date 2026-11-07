
export function jit_install(bytes, memory, table) {
  try {
    const inst = new WebAssembly.Instance(new WebAssembly.Module(bytes), { e: { m: memory, t: table } });
    const n = Object.keys(inst.exports).length;
    const base = table.grow(n);
    for (let i = 0; i < n; i++) table.set(base + i, inst.exports['f' + i]);
    return base;
  } catch (e) {
    console.error('rvwasm: JIT disabled:', e);
    return 0;
  }
}
