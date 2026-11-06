//! Instruction trace recorder.
//!
//! Records the last `capacity` executed instructions in a ring buffer, plus
//! control-flow events (exceptions, interrupts, xRET) for the timeline viewer.

use std::fmt::Write;

#[derive(Clone, Copy, Debug)]
pub struct InstRecord {
    pub cycle: u64,
    pub pc: u64,
    pub inst: u32,
    pub prv: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Exception,
    Interrupt,
    Xret,
}

#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub cycle: u64,
    pub kind: EventKind,
    pub cause: u64,
    /// For traps: the epc. For xRET: the return target.
    pub pc: u64,
    pub tval: u64,
    pub from: u8,
    pub to: u8,
}

pub struct Trace {
    pub enabled: bool,
    capacity: usize,
    insts: Vec<InstRecord>,
    inst_pos: usize,
    events: Vec<Event>,
    event_pos: usize,
    pub total_recorded: u64,
}

impl Default for Trace {
    fn default() -> Self {
        Trace::new(0)
    }
}

impl Trace {
    pub fn new(capacity: usize) -> Self {
        Trace {
            enabled: false,
            capacity,
            insts: Vec::new(),
            inst_pos: 0,
            events: Vec::new(),
            event_pos: 0,
            total_recorded: 0,
        }
    }

    /// Start recording into a fresh ring buffer of `capacity` instructions.
    pub fn start(&mut self, capacity: usize) {
        *self = Trace::new(capacity.max(1));
        self.insts.reserve(self.capacity);
        self.enabled = true;
    }

    pub fn stop(&mut self) {
        self.enabled = false;
    }

    fn event_capacity(&self) -> usize {
        (self.capacity / 4).max(1024)
    }

    #[inline]
    pub fn record_inst(&mut self, cycle: u64, pc: u64, inst: u32, prv: u8) {
        // A 16-bit instruction is stored without the bytes that follow it.
        let inst = if inst & 3 == 3 { inst } else { inst & 0xffff };
        let r = InstRecord { cycle, pc, inst, prv };
        if self.insts.len() < self.capacity {
            self.insts.push(r);
        } else {
            self.insts[self.inst_pos] = r;
            self.inst_pos = (self.inst_pos + 1) % self.capacity;
        }
        self.total_recorded += 1;
    }

    fn push_event(&mut self, e: Event) {
        let cap = self.event_capacity();
        if self.events.len() < cap {
            self.events.push(e);
        } else {
            self.events[self.event_pos] = e;
            self.event_pos = (self.event_pos + 1) % cap;
        }
    }

    pub fn record_trap(&mut self, cycle: u64, cause: u64, epc: u64, tval: u64, from: u8, to: u8) {
        let kind = if cause >> 63 != 0 { EventKind::Interrupt } else { EventKind::Exception };
        self.push_event(Event { cycle, kind, cause: cause & !(1 << 63), pc: epc, tval, from, to });
    }

    pub fn record_xret(&mut self, cycle: u64, target: u64, from: u8, to: u8) {
        self.push_event(Event { cycle, kind: EventKind::Xret, cause: 0, pc: target, tval: 0, from, to });
    }

    /// Recorded instructions, oldest first.
    pub fn instructions(&self) -> Vec<InstRecord> {
        let mut v = Vec::with_capacity(self.insts.len());
        v.extend_from_slice(&self.insts[self.inst_pos..]);
        v.extend_from_slice(&self.insts[..self.inst_pos]);
        v
    }

    /// Events within the window of recorded instructions, oldest first.
    pub fn events(&self) -> Vec<Event> {
        let mut v = Vec::with_capacity(self.events.len());
        v.extend_from_slice(&self.events[self.event_pos..]);
        v.extend_from_slice(&self.events[..self.event_pos]);
        v
    }

    /// Serialise to the JSON format the timeline viewer loads.
    ///
    /// `insts` rows are `[cycle, pc, inst, prv]`; pc and inst are hex strings
    /// since JS numbers can't hold 64-bit addresses exactly. `symbols` is an
    /// optional sorted list of (name, address) used to label functions.
    pub fn to_json(&self, symbols: &[(String, u64)]) -> String {
        let insts = self.instructions();
        let first = insts.first().map(|r| r.cycle).unwrap_or(0);
        let mut s = String::with_capacity(insts.len() * 40 + 1024);
        s.push_str("{\"format\":\"rvwasm-trace-1\",\"insts\":[");
        for (i, r) in insts.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            if r.inst & 3 == 3 {
                let _ = write!(s, "[{},\"{:x}\",\"{:08x}\",{}]", r.cycle, r.pc, r.inst, r.prv);
            } else {
                let _ = write!(s, "[{},\"{:x}\",\"{:04x}\",{}]", r.cycle, r.pc, r.inst & 0xffff, r.prv);
            }
        }
        s.push_str("],\"events\":[");
        for (n, e) in self.events().iter().filter(|e| e.cycle >= first).enumerate() {
            if n > 0 {
                s.push(',');
            }
            let kind = match e.kind {
                EventKind::Exception => "exception",
                EventKind::Interrupt => "interrupt",
                EventKind::Xret => "xret",
            };
            let _ = write!(
                s,
                "{{\"cycle\":{},\"kind\":\"{}\",\"cause\":{},\"pc\":\"{:x}\",\"tval\":\"{:x}\",\"from\":{},\"to\":{}}}",
                e.cycle, kind, e.cause, e.pc, e.tval, e.from, e.to
            );
        }
        s.push_str("],\"symbols\":[");
        for (i, (name, addr)) in symbols.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let escaped: String = name.chars().filter(|c| *c != '"' && *c != '\\' && !c.is_control()).collect();
            let _ = write!(s, "[\"{}\",\"{:x}\"]", escaped, addr);
        }
        s.push_str("]}");
        s
    }
}
