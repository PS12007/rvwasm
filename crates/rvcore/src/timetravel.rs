//! Time travel: stepping a running machine backwards.
//!
//! Execution is deterministic: given the same state and the same input bytes
//! arriving at the same cycles, the machine does exactly the same thing, no
//! matter how the host slices its `run` calls (see `Cpu::chunk_left`). So
//! going back only needs
//!
//! - checkpoints of the small machine state every `interval` instructions,
//! - the old contents of each RAM/disk page the first time it is written
//!   after a checkpoint (the journal), and
//! - a log of every input byte with the cycle it arrived at.
//!
//! To reach an earlier instruction, write back the saved pages from newest
//! to oldest until memory is as it was at the nearest checkpoint before it,
//! load that checkpoint's state, and run forward to the target.
//!
//! The newest point ever reached is the frontier. Behind it the machine
//! replays: logged input is fed back at its original cycles and output is
//! dropped (the host already showed it). Input sent while behind the
//! frontier is held until the replay reaches the frontier.

use crate::cpu::Cpu;
use crate::journal::{self, SavedPage};
use std::collections::VecDeque;

struct Checkpoint {
    instret: u64,
    cycle: u64,
    state: Vec<u8>,
    /// Index into the input log of the first input after this checkpoint.
    input_pos: u64,
    /// Old contents of the pages written between this checkpoint and the
    /// next, i.e. their contents at this checkpoint.
    ram: Vec<SavedPage>,
    disk: Vec<SavedPage>,
}

impl Checkpoint {
    fn bytes(&self) -> usize {
        self.state.len() + self.ram.iter().chain(&self.disk).map(|(_, b)| b.len()).sum::<usize>()
    }
}

pub struct TimeTravel {
    /// Instructions between checkpoints. Stepping back replays at most this many.
    pub interval: u64,
    /// Memory budget for checkpoints and saved pages; the oldest go first.
    pub max_bytes: usize,
    ckpts: VecDeque<Checkpoint>,
    bytes: usize,
    /// (cycle, bytes) of every input since the oldest checkpoint.
    inputs: VecDeque<(u64, Vec<u8>)>,
    /// Absolute index of `inputs[0]`.
    inputs_base: u64,
    /// Absolute index of the next logged input to feed back while replaying.
    next_input: u64,
    frontier: u64,
    held: Vec<u8>,
}

impl TimeTravel {
    /// Start recording `cpu` from its current state.
    pub fn new(cpu: &mut Cpu, interval: u64, max_bytes: usize) -> Self {
        cpu.bus.journal.on = true;
        cpu.bus.journal.take();
        let mut tt = TimeTravel {
            interval: interval.max(1),
            max_bytes,
            ckpts: VecDeque::new(),
            bytes: 0,
            inputs: VecDeque::new(),
            inputs_base: 0,
            next_input: 0,
            frontier: cpu.cycle(),
            held: Vec::new(),
        };
        tt.checkpoint(cpu);
        tt
    }

    /// Stop recording; the machine carries on normally.
    pub fn stop(self, cpu: &mut Cpu) {
        cpu.bus.journal.on = false;
        cpu.bus.journal.take();
    }

    /// True while the machine is behind the newest point it has reached.
    pub fn replaying(&self, cpu: &Cpu) -> bool {
        cpu.cycle() < self.frontier
    }

    /// Instruction count of the oldest point that can still be reached.
    pub fn oldest_instret(&self) -> u64 {
        self.ckpts.front().map_or(0, |c| c.instret)
    }

    pub fn checkpoints(&self) -> usize {
        self.ckpts.len()
    }

    /// Bytes held for going back.
    pub fn bytes_held(&self, cpu: &Cpu) -> usize {
        self.bytes + cpu.bus.journal.bytes()
    }

    /// Input from the host. Fed to the guest now, or once the replay
    /// catches up with the frontier.
    pub fn push_input(&mut self, cpu: &mut Cpu, bytes: &[u8]) {
        if self.replaying(cpu) || !self.held.is_empty() {
            self.held.extend_from_slice(bytes);
        } else {
            self.feed(cpu, bytes.to_vec());
        }
    }

    fn feed(&mut self, cpu: &mut Cpu, bytes: Vec<u8>) {
        cpu.bus.uart.push_input(&bytes);
        self.inputs.push_back((cpu.cycle(), bytes));
        self.next_input = self.inputs_base + self.inputs.len() as u64;
    }

    /// Run for up to `budget` cycles, like `Cpu::run`, taking checkpoints and
    /// replaying logged input when behind the frontier.
    pub fn run(&mut self, cpu: &mut Cpu, budget: u64) -> u64 {
        self.advance(cpu, budget, u64::MAX)
    }

    /// `run`, but also stop once `max_instret` instructions have retired.
    fn advance(&mut self, cpu: &mut Cpu, budget: u64, max_instret: u64) -> u64 {
        let start = cpu.cycle();
        let end = start.saturating_add(budget);
        // Idle and stopping at an instruction count: run one cycle first, which
        // wakes it for a pending interrupt (and runs at most one instruction).
        // Still idle after that, nothing wakes it before the timer.
        let mut probed = false;
        while cpu.cycle() < end && cpu.instret < max_instret && !cpu.halted && cpu.bus.htif_exit.is_none() {
            let now = cpu.cycle();
            let mut stop = end;
            if now < self.frontier {
                // Feed back input logged for this cycle, then run to the next
                // logged input (or the frontier).
                while let Some(&(at, ref bytes)) = self.logged(self.next_input) {
                    if at > now {
                        stop = stop.min(at);
                        break;
                    }
                    let bytes = bytes.clone();
                    cpu.bus.uart.push_input(&bytes);
                    self.next_input += 1;
                    probed = false; // input can wake it
                }
                stop = stop.min(self.frontier);
            } else if !self.held.is_empty() {
                let held = std::mem::take(&mut self.held);
                self.feed(cpu, held);
                probed = false;
            }
            // Stop at the next checkpoint too. Busy, a cycle is an instruction,
            // so this never overshoots it.
            let next_ckpt = self.ckpts.back().map_or(0, |c| c.instret) + self.interval;
            let to_ckpt = next_ckpt.saturating_sub(cpu.instret).max(1);
            let mut slice = (stop - now).min(to_ckpt);
            if !cpu.wfi {
                slice = slice.min(max_instret - cpu.instret);
                probed = false;
            } else if max_instret != u64::MAX {
                slice = if probed { slice.min(cpu.cycles_until_timer()) } else { 1 };
                probed = true;
            }
            cpu.run(slice);
            if cpu.instret >= next_ckpt {
                self.checkpoint(cpu);
            }
            if cpu.cycle() <= self.frontier {
                cpu.bus.uart.tx.clear(); // already shown
            } else {
                self.frontier = cpu.cycle();
            }
        }
        cpu.cycle() - start
    }

    fn logged(&self, abs: u64) -> Option<&(u64, Vec<u8>)> {
        abs.checked_sub(self.inputs_base).and_then(|i| self.inputs.get(i as usize))
    }

    fn checkpoint(&mut self, cpu: &mut Cpu) {
        let (ram, disk) = cpu.bus.journal.take();
        if let Some(prev) = self.ckpts.back_mut() {
            self.bytes += ram.iter().chain(&disk).map(|(_, b)| b.len()).sum::<usize>();
            prev.ram = ram;
            prev.disk = disk;
        }
        let c = Checkpoint {
            instret: cpu.instret,
            cycle: cpu.cycle(),
            state: cpu.save_state(),
            input_pos: self.next_input,
            ram: Vec::new(),
            disk: Vec::new(),
        };
        self.bytes += c.state.len();
        self.ckpts.push_back(c);
        while self.bytes > self.max_bytes && self.ckpts.len() > 2 {
            let old = self.ckpts.pop_front().unwrap();
            self.bytes -= old.bytes();
        }
        // Inputs before the oldest checkpoint can never be replayed again.
        let keep_from = self.ckpts.front().map_or(0, |c| c.input_pos);
        while self.inputs_base < keep_from {
            self.inputs.pop_front();
            self.inputs_base += 1;
        }
    }

    /// Put the machine back at checkpoint `k` (dropping the later ones).
    fn restore(&mut self, cpu: &mut Cpu, k: usize) {
        let (ram, disk) = cpu.bus.journal.take();
        journal::restore(&mut cpu.bus.ram, &ram);
        if let Some(v) = cpu.bus.virtio.as_mut() {
            journal::restore(&mut v.disk, &disk);
        }
        // Newest first: each checkpoint holds its own contents of the pages
        // changed after it.
        while self.ckpts.len() > k + 1 {
            let c = self.ckpts.pop_back().unwrap();
            journal::restore(&mut cpu.bus.ram, &c.ram);
            if let Some(v) = cpu.bus.virtio.as_mut() {
                journal::restore(&mut v.disk, &c.disk);
            }
            self.bytes -= c.bytes();
        }
        let c = self.ckpts.back_mut().unwrap();
        journal::restore(&mut cpu.bus.ram, &c.ram);
        if let Some(v) = cpu.bus.virtio.as_mut() {
            journal::restore(&mut v.disk, &c.disk);
        }
        self.bytes -= c.ram.iter().chain(&c.disk).map(|(_, b)| b.len()).sum::<usize>();
        c.ram.clear();
        c.disk.clear();
        cpu.load_state(&c.state).expect("checkpoint from this machine");
        self.next_input = c.input_pos;
    }

    /// Go back to the point where `target` instructions had been executed
    /// (or the oldest reachable point, if that is later). Returns the
    /// instruction count reached.
    pub fn seek_instret(&mut self, cpu: &mut Cpu, target: u64) -> u64 {
        let target = target.max(self.oldest_instret());
        if target < cpu.instret {
            let k = self.ckpts.iter().rposition(|c| c.instret <= target).unwrap_or(0);
            self.restore(cpu, k);
        }
        while cpu.instret < target && !cpu.halted && cpu.bus.htif_exit.is_none() {
            if cpu.wfi && !self.replaying(cpu) && cpu.cycles_until_timer() == u64::MAX {
                break; // idle with nothing that could wake it
            }
            // Slices end at each logged input and, when busy, after at most the
            // instructions still to go, so this stops exactly at `target`.
            if self.advance(cpu, u64::MAX / 2, target) == 0 {
                break;
            }
        }
        cpu.instret
    }

    /// Step back `n` instructions.
    pub fn step_back(&mut self, cpu: &mut Cpu, n: u64) -> u64 {
        let target = cpu.instret.saturating_sub(n);
        self.seek_instret(cpu, target)
    }

    /// Go back to just before the most recent trap or interrupt was taken.
    /// Returns false if there is none within reach.
    pub fn back_to_last_trap(&mut self, cpu: &mut Cpu) -> bool {
        match cpu.last_trap_instret {
            Some(i) if i >= self.oldest_instret() => {
                self.seek_instret(cpu, i);
                true
            }
            _ => false,
        }
    }

    /// Cycle of the newest point reached.
    pub fn frontier(&self) -> u64 {
        self.frontier
    }

    /// Cycle of the newest checkpoint (for tests and stats).
    pub fn last_checkpoint_cycle(&self) -> u64 {
        self.ckpts.back().map_or(0, |c| c.cycle)
    }
}
