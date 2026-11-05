//! Minimal ELF64 little-endian loader (PT_LOAD segments and symbol lookup).

pub struct Segment<'a> {
    pub paddr: u64,
    pub data: &'a [u8],
    pub memsz: u64,
}

pub struct Elf<'a> {
    bytes: &'a [u8],
    pub entry: u64,
    pub segments: Vec<Segment<'a>>,
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

impl<'a> Elf<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.get(0..4) != Some(b"\x7fELF") {
            return Err("not an ELF file".into());
        }
        if bytes[4] != 2 || bytes[5] != 1 {
            return Err("only little-endian ELF64 is supported".into());
        }
        if u16_at(bytes, 18) != Some(243) {
            return Err("not a RISC-V ELF".into());
        }
        let bad = || "truncated ELF".to_string();
        let entry = u64_at(bytes, 24).ok_or_else(bad)?;
        let phoff = u64_at(bytes, 32).ok_or_else(bad)? as usize;
        let phentsize = u16_at(bytes, 54).ok_or_else(bad)? as usize;
        let phnum = u16_at(bytes, 56).ok_or_else(bad)? as usize;
        let mut segments = Vec::new();
        for i in 0..phnum {
            let p = phoff + i * phentsize;
            if u32_at(bytes, p).ok_or_else(bad)? != 1 {
                continue; // not PT_LOAD
            }
            let offset = u64_at(bytes, p + 8).ok_or_else(bad)? as usize;
            let paddr = u64_at(bytes, p + 24).ok_or_else(bad)?;
            let filesz = u64_at(bytes, p + 32).ok_or_else(bad)? as usize;
            let memsz = u64_at(bytes, p + 40).ok_or_else(bad)?;
            let data = bytes.get(offset..offset + filesz).ok_or_else(bad)?;
            segments.push(Segment { paddr, data, memsz });
        }
        Ok(Elf { bytes, entry, segments })
    }

    /// Look up a symbol's value in .symtab.
    pub fn symbol(&self, name: &str) -> Option<u64> {
        self.symbols().into_iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    /// All named symbols in .symtab as (name, value).
    pub fn symbols(&self) -> Vec<(String, u64)> {
        let b = self.bytes;
        let mut out = Vec::new();
        let (Some(shoff), Some(shentsize), Some(shnum)) = (u64_at(b, 40), u16_at(b, 58), u16_at(b, 60)) else {
            return out;
        };
        let sh = |i: usize| shoff as usize + i * shentsize as usize;
        for i in 0..shnum as usize {
            if u32_at(b, sh(i) + 4) != Some(2) {
                continue; // SHT_SYMTAB
            }
            let (Some(off), Some(size), Some(link), Some(entsize)) =
                (u64_at(b, sh(i) + 24), u64_at(b, sh(i) + 32), u32_at(b, sh(i) + 40), u64_at(b, sh(i) + 56))
            else {
                continue;
            };
            let Some(stroff) = u64_at(b, sh(link as usize) + 24) else { continue };
            for s in 0..(size / entsize.max(1)) as usize {
                let e = off as usize + s * entsize as usize;
                let (Some(name_off), Some(value)) = (u32_at(b, e), u64_at(b, e + 8)) else { break };
                let start = stroff as usize + name_off as usize;
                let Some(rest) = b.get(start..) else { continue };
                let end = rest.iter().position(|&c| c == 0).unwrap_or(0);
                if end > 0 {
                    out.push((String::from_utf8_lossy(&rest[..end]).into_owned(), value));
                }
            }
        }
        out
    }
}
