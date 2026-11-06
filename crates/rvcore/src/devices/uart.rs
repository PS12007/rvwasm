//! A 16550-compatible UART. Transmit is instantaneous; output bytes are
//! queued for the host to drain, input bytes are pushed in by the host.

use std::collections::VecDeque;

pub const BASE: u64 = 0x1000_0000;
pub const SIZE: u64 = 0x100;
pub const IRQ: u32 = 10;

const IER_RX: u8 = 1;
const IER_TX: u8 = 2;
const LSR_DR: u8 = 1;
const LSR_THRE: u8 = 0x20;
const LSR_TEMT: u8 = 0x40;

#[derive(Clone, Default)]
pub struct Uart {
    pub rx: VecDeque<u8>,
    pub tx: Vec<u8>,
    pub(crate) ier: u8,
    pub(crate) lcr: u8,
    pub(crate) mcr: u8,
    pub(crate) scr: u8,
    pub(crate) dll: u8,
    pub(crate) dlm: u8,
    /// Transmitter-empty interrupt is pending (cleared by reading IIR or writing THR).
    pub(crate) thre_ip: bool,
}

impl Uart {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn irq(&self) -> bool {
        (self.ier & IER_RX != 0 && !self.rx.is_empty()) || (self.ier & IER_TX != 0 && self.thre_ip)
    }

    pub fn push_input(&mut self, bytes: &[u8]) {
        self.rx.extend(bytes.iter().copied());
    }

    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.tx)
    }

    pub fn read(&mut self, off: u64) -> u8 {
        let dlab = self.lcr & 0x80 != 0;
        match off {
            0 if dlab => self.dll,
            0 => self.rx.pop_front().unwrap_or(0),
            1 if dlab => self.dlm,
            1 => self.ier,
            2 => {
                // IIR: FIFOs enabled (0xc0), then interrupt identification.
                if self.ier & IER_RX != 0 && !self.rx.is_empty() {
                    0xc4
                } else if self.ier & IER_TX != 0 && self.thre_ip {
                    self.thre_ip = false;
                    0xc2
                } else {
                    0xc1
                }
            }
            3 => self.lcr,
            4 => self.mcr,
            5 => LSR_THRE | LSR_TEMT | if self.rx.is_empty() { 0 } else { LSR_DR },
            6 => 0xb0, // DCD, DSR, CTS
            7 => self.scr,
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u64, val: u8) {
        let dlab = self.lcr & 0x80 != 0;
        match off {
            0 if dlab => self.dll = val,
            0 => {
                self.tx.push(val);
                self.thre_ip = true;
            }
            1 if dlab => self.dlm = val,
            1 => {
                // Enabling the TX interrupt while the holding register is
                // empty raises it immediately, as on real hardware.
                if val & IER_TX != 0 && self.ier & IER_TX == 0 {
                    self.thre_ip = true;
                }
                self.ier = val & 0x0f;
            }
            2 => {
                if val & 2 != 0 {
                    self.rx.clear();
                }
            }
            3 => self.lcr = val,
            4 => self.mcr = val,
            7 => self.scr = val,
            _ => {}
        }
    }
}
