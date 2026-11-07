import { jit_install } from './snippets/rvweb-395ab209c14ec0c4/inline0.js';


export class Emulator {
    static __wrap(ptr) {
        const obj = Object.create(Emulator.prototype);
        obj.__wbg_ptr = ptr;
        EmulatorFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        EmulatorFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_emulator_free(ptr, 0);
    }
    /**
     * Back to just before the most recent trap; false if out of reach.
     * @returns {boolean}
     */
    back_to_last_trap() {
        const ret = wasm.emulator_back_to_last_trap(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * JSON object of CSR name -> hex value, plus a few device counters.
     * @returns {string}
     */
    csrs_json() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.emulator_csrs_json(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * @returns {number}
     */
    cycle() {
        const ret = wasm.emulator_cycle(this.__wbg_ptr);
        return ret;
    }
    /**
     * Disassembly of `count` instructions, `before` of them ahead of the pc.
     * One line per instruction: "addr|hex|text|symbol".
     * @param {number} before
     * @param {number} count
     * @returns {string}
     */
    disassemble_around_pc(before, count) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.emulator_disassemble_around_pc(this.__wbg_ptr, before, count);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Snapshot of the disk image (to let users download a modified fs.img).
     * @returns {Uint8Array}
     */
    disk_image() {
        const ret = wasm.emulator_disk_image(this.__wbg_ptr);
        var v1 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        return v1;
    }
    /**
     * Write requests the guest has sent to the disk so far.
     * @returns {number}
     */
    disk_writes() {
        const ret = wasm.emulator_disk_writes(this.__wbg_ptr);
        return ret;
    }
    /**
     * True if `bytes` (a bare-metal test) has exited through `tohost`;
     * the exit code, or -1 while still running.
     * @returns {number}
     */
    exit_code() {
        const ret = wasm.emulator_exit_code(this.__wbg_ptr);
        return ret;
    }
    /**
     * f0..f31 (raw bits; single-precision values are NaN-boxed).
     * @returns {BigUint64Array}
     */
    fp_registers() {
        const ret = wasm.emulator_fp_registers(this.__wbg_ptr);
        var v1 = getArrayU64FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 8, 8);
        return v1;
    }
    /**
     * Carry on from a snapshot made by `save_snapshot` (or `rvwasm run --save`).
     * @param {Uint8Array | number[]} bytes
     * @returns {Emulator}
     */
    static from_snapshot(bytes) {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.emulator_from_snapshot(ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return Emulator.__wrap(ret[0]);
    }
    /**
     * @returns {boolean}
     */
    halted() {
        const ret = wasm.emulator_halted(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Instruction counts [oldest reachable, now, present] and bytes held.
     * @returns {Float64Array}
     */
    history() {
        const ret = wasm.emulator_history(this.__wbg_ptr);
        var v1 = getArrayF64FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 8, 8);
        return v1;
    }
    /**
     * True while the hart is waiting in WFI.
     * @returns {boolean}
     */
    idle() {
        const ret = wasm.emulator_idle(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * @returns {number}
     */
    input_pending() {
        const ret = wasm.emulator_input_pending(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * @returns {number}
     */
    instret() {
        const ret = wasm.emulator_instret(this.__wbg_ptr);
        return ret;
    }
    /**
     * True if `bytes` is a snapshot file.
     * @param {Uint8Array | number[]} bytes
     * @returns {boolean}
     */
    static is_snapshot(bytes) {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.emulator_is_snapshot(ptr0, len0);
        return ret !== 0;
    }
    /**
     * [enabled, blocks compiled, modules, blocks invalidated, instructions
     * run as compiled code].
     * @returns {Float64Array}
     */
    jit_stats() {
        const ret = wasm.emulator_jit_stats(this.__wbg_ptr);
        var v1 = getArrayF64FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 8, 8);
        return v1;
    }
    /**
     * Tuning for tests: blocks compile after `hot` starts, in batches of
     * `batch` (or after `delay` instructions).
     * @param {number} hot
     * @param {number} batch
     * @param {number} delay
     */
    jit_tune(hot, batch, delay) {
        wasm.emulator_jit_tune(this.__wbg_ptr, hot, batch, delay);
    }
    /**
     * Boot a bare-metal kernel ELF (e.g. xv6) with an optional virtio disk image.
     * @param {Uint8Array | number[]} kernel
     * @param {Uint8Array | number[] | null | undefined} disk
     * @param {number} ram_mb
     */
    constructor(kernel, disk, ram_mb) {
        const val3 = +ram_mb;
        let ptr0, len0, ptr1, len1;
        try {
            ptr0 = passArray8ToWasm0(kernel, wasm.__wbindgen_malloc);
            len0 = WASM_VECTOR_LEN;
            ptr1 = isLikeNone(disk) ? 0 : passArray8ToWasm0(disk, wasm.__wbindgen_malloc);
            len1 = WASM_VECTOR_LEN;
        } catch (e) {
            if (ptr1) { wasm.__wbindgen_free(ptr1, len1 * 1, 1); }
            if (ptr0) { wasm.__wbindgen_free(ptr0, len0 * 1, 1); }
            throw e;
        }
        const ret = wasm.emulator_new(ptr0, len0, ptr1, len1, val3);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        this.__wbg_ptr = ret[0];
        EmulatorFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Boot a Linux `Image` with the built-in SBI and an optional initramfs.
     *
     * `system_map` is the kernel's System.map text (optional), used to label
     * addresses in the debug panel and traces.
     * @param {Uint8Array | number[]} kernel
     * @param {Uint8Array | number[] | null | undefined} initrd
     * @param {number} ram_mb
     * @param {string} cmdline
     * @param {string | null} [system_map]
     * @returns {Emulator}
     */
    static new_linux(kernel, initrd, ram_mb, cmdline, system_map) {
        const val5 = +ram_mb;
        let ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3;
        try {
            ptr0 = passArray8ToWasm0(kernel, wasm.__wbindgen_malloc);
            len0 = WASM_VECTOR_LEN;
            ptr1 = isLikeNone(initrd) ? 0 : passArray8ToWasm0(initrd, wasm.__wbindgen_malloc);
            len1 = WASM_VECTOR_LEN;
            ptr2 = passStringToWasm0(cmdline, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            len2 = WASM_VECTOR_LEN;
            ptr3 = isLikeNone(system_map) ? 0 : passStringToWasm0(system_map, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            len3 = WASM_VECTOR_LEN;
        } catch (e) {
            if (ptr3) { wasm.__wbindgen_free(ptr3, len3 * 1, 1); }
            if (ptr2) { wasm.__wbindgen_free(ptr2, len2 * 1, 1); }
            if (ptr1) { wasm.__wbindgen_free(ptr1, len1 * 1, 1); }
            if (ptr0) { wasm.__wbindgen_free(ptr0, len0 * 1, 1); }
            throw e;
        }
        const ret = wasm.emulator_new_linux(ptr0, len0, ptr1, len1, val5, ptr2, len2, ptr3, len3);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return Emulator.__wrap(ret[0]);
    }
    /**
     * @returns {bigint}
     */
    pc() {
        const ret = wasm.emulator_pc(this.__wbg_ptr);
        return BigInt.asUintN(64, ret);
    }
    /**
     * Current privilege mode: 0 = U, 1 = S, 3 = M.
     * @returns {number}
     */
    privilege() {
        const ret = wasm.emulator_privilege(this.__wbg_ptr);
        return ret;
    }
    /**
     * Instructions executed per privilege mode: [U, S, M].
     * @returns {Float64Array}
     */
    privilege_counts() {
        const ret = wasm.emulator_privilege_counts(this.__wbg_ptr);
        var v1 = getArrayF64FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 8, 8);
        return v1;
    }
    /**
     * Keyboard input. While the machine is behind the present (after going
     * back), it waits until the replay catches up.
     * @param {Uint8Array | number[]} bytes
     */
    push_input(bytes) {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.emulator_push_input(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * x0..x31 as a BigUint64Array.
     * @returns {BigUint64Array}
     */
    registers() {
        const ret = wasm.emulator_registers(this.__wbg_ptr);
        var v1 = getArrayU64FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 8, 8);
        return v1;
    }
    /**
     * True while behind the present.
     * @returns {boolean}
     */
    replaying() {
        const ret = wasm.emulator_replaying(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Run for up to `cycles` cycles; returns cycles consumed.
     * @param {number} cycles
     * @returns {number}
     */
    run(cycles) {
        const ret = wasm.emulator_run(this.__wbg_ptr, cycles);
        return ret;
    }
    /**
     * The whole machine, RAM and disk included, as a file.
     * @returns {Uint8Array}
     */
    save_snapshot() {
        const ret = wasm.emulator_save_snapshot(this.__wbg_ptr);
        var v1 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        return v1;
    }
    /**
     * Go to the point where `instret` instructions had retired, backwards
     * or forwards (forwards stops at the present). Returns where it got to.
     * @param {number} instret
     * @returns {number}
     */
    seek(instret) {
        const ret = wasm.emulator_seek(this.__wbg_ptr, instret);
        return ret;
    }
    /**
     * Turn compiling hot code to WebAssembly on or off (on by default).
     * @param {boolean} on
     */
    set_jit(on) {
        wasm.emulator_set_jit(this.__wbg_ptr, on);
    }
    /**
     * @param {number} n
     * @returns {number}
     */
    step_back(n) {
        const ret = wasm.emulator_step_back(this.__wbg_ptr, n);
        return ret;
    }
    /**
     * Run exactly `n` more instructions (or as far as the guest gets).
     * @param {number} n
     * @returns {number}
     */
    step_forward(n) {
        const ret = wasm.emulator_step_forward(this.__wbg_ptr, n);
        return ret;
    }
    /**
     * @returns {Uint8Array}
     */
    take_output() {
        const ret = wasm.emulator_take_output(this.__wbg_ptr);
        var v1 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        return v1;
    }
    /**
     * @returns {boolean}
     */
    trace_active() {
        const ret = wasm.emulator_trace_active(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * @returns {number}
     */
    trace_count() {
        const ret = wasm.emulator_trace_count(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {string}
     */
    trace_json() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.emulator_trace_json(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * @param {number} capacity
     */
    trace_start(capacity) {
        wasm.emulator_trace_start(this.__wbg_ptr, capacity);
    }
    trace_stop() {
        wasm.emulator_trace_stop(this.__wbg_ptr);
    }
}
if (Symbol.dispose) Emulator.prototype[Symbol.dispose] = Emulator.prototype.free;

/**
 * Benchmark a bare-metal riscv-tests program for at least `min_secs` of
 * execution. Returns JSON `{instret, seconds, runs}` (seconds per run).
 * @param {Uint8Array | number[]} elf
 * @param {number} min_secs
 * @param {boolean} jit
 * @returns {string}
 */
export function bench_baremetal(elf, min_secs, jit) {
    let deferred5_0;
    let deferred5_1;
    try {
        const val2 = +min_secs;
        const val3 = +jit;
        const ptr0 = passArray8ToWasm0(elf, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.bench_baremetal(ptr0, len0, val2, val3);
        var ptr4 = ret[0];
        var len4 = ret[1];
        if (ret[3]) {
            ptr4 = 0; len4 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred5_0 = ptr4;
        deferred5_1 = len4;
        return getStringFromWasm0(ptr4, len4);
    } finally {
        wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
    }
}

/**
 * Names of the bare-metal benchmark programs.
 * @returns {string[]}
 */
export function bench_names() {
    const ret = wasm.bench_names();
    var v1 = getArrayJsValueFromWasm0(ret[0], ret[1]);
    wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
    return v1;
}

/**
 * Benchmark the xv6 boot + shell workload. Returns JSON like `bench_baremetal`.
 * @param {Uint8Array | number[]} kernel
 * @param {Uint8Array | number[]} disk
 * @param {boolean} jit
 * @returns {string}
 */
export function bench_xv6(kernel, disk, jit) {
    let deferred5_0;
    let deferred5_1;
    try {
        const val3 = +jit;
        let ptr0, len0, ptr1, len1;
        try {
            ptr0 = passArray8ToWasm0(kernel, wasm.__wbindgen_malloc);
            len0 = WASM_VECTOR_LEN;
            ptr1 = passArray8ToWasm0(disk, wasm.__wbindgen_malloc);
            len1 = WASM_VECTOR_LEN;
        } catch (e) {
            if (ptr1) { wasm.__wbindgen_free(ptr1, len1 * 1, 1); }
            if (ptr0) { wasm.__wbindgen_free(ptr0, len0 * 1, 1); }
            throw e;
        }
        const ret = wasm.bench_xv6(ptr0, len0, ptr1, len1, val3);
        var ptr4 = ret[0];
        var len4 = ret[1];
        if (ret[3]) {
            ptr4 = 0; len4 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred5_0 = ptr4;
        deferred5_1 = len4;
        return getStringFromWasm0(ptr4, len4);
    } finally {
        wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
    }
}

/**
 * Disassemble a single instruction (used by the trace viewer).
 * @param {number} inst
 * @param {bigint} pc
 * @returns {string}
 */
export function disassemble(inst, pc) {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.disassemble(inst, pc);
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * Run a riscv-tests ELF; returns "" on pass, else the failure. With `jit`,
 * every block is compiled the first time it runs, to test the compiler.
 * @param {Uint8Array | number[]} elf
 * @param {boolean} jit
 * @returns {string}
 */
export function run_isa_test(elf, jit) {
    let deferred3_0;
    let deferred3_1;
    try {
        const val2 = +jit;
        const ptr0 = passArray8ToWasm0(elf, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.run_isa_test(ptr0, len0, val2);
        deferred3_0 = ret[0];
        deferred3_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg_Error_208446807d0d1e30: function(arg0, arg1) {
            const ret = Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg___wbindgen_function_table_dd9abe976e6c95f6: function() {
            const ret = wasm.__wbindgen_export;
            return ret;
        },
        __wbg___wbindgen_memory_17a1178bc3328ea3: function() {
            const ret = wasm.memory;
            return ret;
        },
        __wbg___wbindgen_throw_4c88d8bf264fd984: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg_jit_install_d1ea3e84b7dcaa09: function(arg0, arg1, arg2, arg3) {
            const ret = jit_install(getArrayU8FromWasm0(arg0, arg1), arg2, arg3);
            return ret;
        },
        __wbg_now_148fb9bc643a5648: function() {
            const ret = performance.now();
            return ret;
        },
        __wbindgen_generic_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            table.set(0, undefined);
            table.set(1024, undefined);
            table.set(1025, null);
            table.set(1026, true);
            table.set(1027, false);
        },
    };
    return {
        __proto__: null,
        "./rvweb_bg.js": import0,
    };
}

const EmulatorFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_emulator_free(ptr, 1));

function getArrayF64FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getFloat64ArrayMemory0().subarray(ptr / 8, ptr / 8 + len);
}

function getArrayJsValueFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    const mem = getDataViewMemory0();
    const result = [];
    for (let i = ptr; i < ptr + 4 * len; i += 4) {
        result.push(wasm.__wbindgen_externrefs.get(mem.getUint32(i, true)));
    }
    wasm.__externref_drop_slice(ptr, len);
    return result;
}

function getArrayU64FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getBigUint64ArrayMemory0().subarray(ptr / 8, ptr / 8 + len);
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedBigUint64ArrayMemory0 = null;
function getBigUint64ArrayMemory0() {
    if (cachedBigUint64ArrayMemory0 === null || cachedBigUint64ArrayMemory0.byteLength === 0) {
        cachedBigUint64ArrayMemory0 = new BigUint64Array(wasm.memory.buffer);
    }
    return cachedBigUint64ArrayMemory0;
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

let cachedFloat64ArrayMemory0 = null;
function getFloat64ArrayMemory0() {
    if (cachedFloat64ArrayMemory0 === null || cachedFloat64ArrayMemory0.byteLength === 0) {
        cachedFloat64ArrayMemory0 = new Float64Array(wasm.memory.buffer);
    }
    return cachedFloat64ArrayMemory0;
}

function getStringFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return decodeText(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function passArray8ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 1, 1) >>> 0;
    getUint8ArrayMemory0().set(arg, ptr / 1);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(bytes) {
    numBytesDecoded += bytes.length;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true });
        cachedTextDecoder.decode();
        numBytesDecoded = bytes.length;
    }
    return cachedTextDecoder.decode(bytes);
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedBigUint64ArrayMemory0 = null;
    cachedDataViewMemory0 = null;
    cachedFloat64ArrayMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (!module.ok) {
            throw new Error(`failed to fetch Wasm: ${module.status} ${module.statusText} fetching '${module.url}'`);
        }

        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

let __wbg_init_promise;
async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;
    if (__wbg_init_promise === undefined) {
        __wbg_init_promise = __wbg_init_once(module_or_path);
        __wbg_init_promise.catch(() => { __wbg_init_promise = undefined; });
    }
    return __wbg_init_promise;
}

async function __wbg_init_once(module_or_path) {

    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('rvweb_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
