//! A minimal flattened device tree (DTB) writer.

use std::collections::HashMap;

const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_END: u32 = 9;

#[derive(Default)]
pub struct Fdt {
    structs: Vec<u8>,
    strings: Vec<u8>,
    offsets: HashMap<String, u32>,
}

impl Fdt {
    pub fn new() -> Self {
        Self::default()
    }

    fn u32(&mut self, v: u32) {
        self.structs.extend_from_slice(&v.to_be_bytes());
    }

    fn pad(&mut self) {
        while !self.structs.len().is_multiple_of(4) {
            self.structs.push(0);
        }
    }

    pub fn begin_node(&mut self, name: &str) {
        self.u32(FDT_BEGIN_NODE);
        self.structs.extend_from_slice(name.as_bytes());
        self.structs.push(0);
        self.pad();
    }

    pub fn end_node(&mut self) {
        self.u32(FDT_END_NODE);
    }

    pub fn prop(&mut self, name: &str, value: &[u8]) {
        let off = match self.offsets.get(name) {
            Some(&o) => o,
            None => {
                let o = self.strings.len() as u32;
                self.strings.extend_from_slice(name.as_bytes());
                self.strings.push(0);
                self.offsets.insert(name.to_string(), o);
                o
            }
        };
        self.u32(FDT_PROP);
        self.u32(value.len() as u32);
        self.u32(off);
        self.structs.extend_from_slice(value);
        self.pad();
    }

    pub fn prop_empty(&mut self, name: &str) {
        self.prop(name, &[]);
    }

    pub fn prop_u32(&mut self, name: &str, v: u32) {
        self.prop(name, &v.to_be_bytes());
    }

    pub fn prop_cells(&mut self, name: &str, cells: &[u32]) {
        let bytes: Vec<u8> = cells.iter().flat_map(|c| c.to_be_bytes()).collect();
        self.prop(name, &bytes);
    }

    pub fn prop_u64(&mut self, name: &str, v: u64) {
        self.prop(name, &v.to_be_bytes());
    }

    pub fn prop_str(&mut self, name: &str, s: &str) {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        self.prop(name, &b);
    }

    pub fn prop_strs(&mut self, name: &str, list: &[&str]) {
        let mut b = Vec::new();
        for s in list {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        self.prop(name, &b);
    }

    /// Serialise to a DTB blob (version 17).
    pub fn finish(mut self) -> Vec<u8> {
        self.u32(FDT_END);
        let header = 40usize;
        let rsvmap = header;
        let structs = rsvmap + 16;
        let strings = structs + self.structs.len();
        let total = strings + self.strings.len();
        let mut out = Vec::with_capacity(total);
        for v in [
            0xd00d_feed,
            total as u32,
            structs as u32,
            strings as u32,
            rsvmap as u32,
            17,
            16,
            0,
            self.strings.len() as u32,
            self.structs.len() as u32,
        ] {
            out.extend_from_slice(&u32::to_be_bytes(v));
        }
        out.extend_from_slice(&[0; 16]); // empty memory reservation map
        out.extend_from_slice(&self.structs);
        out.extend_from_slice(&self.strings);
        out
    }
}
