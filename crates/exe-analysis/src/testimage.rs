//! Builds small PE32+ images and x64 code for tests. File offsets equal RVAs.

use pe_image::{DIRECTORY_EXCEPTION, DIRECTORY_EXPORT, DIRECTORY_IMPORT};

pub const BASE: u64 = 0x1_4000_0000;

pub struct Builder {
    bytes: Vec<u8>,
    directories: [(u32, u32); 16],
    functions: Vec<(u32, u32, u32)>,
    unwind_next: u32,
}

impl Builder {
    pub const TEXT: u32 = 0x1000;
    pub const RDATA: u32 = 0x3000;
    pub const DATA: u32 = 0x6000;
    pub const PDATA: u32 = 0x7000;
    pub const SIZE: u32 = 0x8000;
    /// Import and export tables live in the first 0x800 bytes of `.rdata`.
    pub const FREE_RDATA: u32 = Self::RDATA + 0x800;

    pub fn new() -> Self {
        Builder {
            bytes: vec![0; Self::SIZE as usize],
            directories: [(0, 0); 16],
            functions: Vec::new(),
            unwind_next: Self::PDATA + 0x400,
        }
    }

    pub fn put(&mut self, rva: u32, data: &[u8]) {
        self.bytes[rva as usize..rva as usize + data.len()].copy_from_slice(data);
    }

    pub fn put32(&mut self, rva: u32, value: u32) {
        self.put(rva, &value.to_le_bytes());
    }

    pub fn put64(&mut self, rva: u32, value: u64) {
        self.put(rva, &value.to_le_bytes());
    }

    pub fn rdata(&mut self, offset: u32, data: &[u8]) {
        self.put(Self::RDATA + offset, data);
    }

    pub fn code(&mut self, code: Code) {
        self.put(code.start, &code.bytes);
    }

    /// A 64-bit virtual address for an RVA.
    pub fn va(rva: u32) -> u64 {
        BASE + u64::from(rva)
    }

    fn unwind(&mut self, info: &[u8]) -> u32 {
        let at = self.unwind_next;
        self.put(at, info);
        self.unwind_next += (info.len() as u32 + 3) & !3;
        at
    }

    /// An exception-table entry with plain unwind data.
    pub fn function(&mut self, begin: u32, end: u32) {
        let unwind = self.unwind(&[1, 0, 0, 0]);
        self.functions.push((begin, end, unwind));
    }

    /// An entry whose unwind data chains to the function `parent`.
    pub fn chained(&mut self, begin: u32, end: u32, parent: (u32, u32)) {
        let parent_unwind = self
            .functions
            .iter()
            .find(|f| f.0 == parent.0)
            .expect("parent first")
            .2;
        let mut info = vec![1 | (4 << 3), 0, 0, 0];
        for value in [parent.0, parent.1, parent_unwind] {
            info.extend(value.to_le_bytes());
        }
        let unwind = self.unwind(&info);
        self.functions.push((begin, end, unwind));
    }

    /// One imported DLL; returns each function's IAT slot RVA.
    pub fn import(&mut self, dll: &str, names: &[&str]) -> Vec<u32> {
        let (table, lookup, iat, strings) = (
            Self::RDATA,
            Self::RDATA + 0x100,
            Self::RDATA + 0x200,
            Self::RDATA + 0x300,
        );
        self.put32(table, lookup);
        self.put32(table + 12, strings);
        self.put32(table + 16, iat);
        let mut at = strings + 0x40;
        let mut slots = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let entry = u64::from(at);
            self.put64(lookup + 8 * index as u32, entry);
            self.put64(iat + 8 * index as u32, entry);
            self.put(at + 2, name.as_bytes());
            at += 2 + name.len() as u32 + 1;
            slots.push(iat + 8 * index as u32);
        }
        self.put(strings, dll.as_bytes());
        self.directories[DIRECTORY_IMPORT] = (table, 40);
        slots
    }

    pub fn export(&mut self, exports: &[(&str, u32)]) {
        let table = Self::RDATA + 0x400;
        let (functions, names, ordinals, strings) =
            (table + 0x40, table + 0x80, table + 0xC0, table + 0x100);
        self.put32(table + 16, 1);
        self.put32(table + 20, exports.len() as u32);
        self.put32(table + 24, exports.len() as u32);
        self.put32(table + 28, functions);
        self.put32(table + 32, names);
        self.put32(table + 36, ordinals);
        let mut at = strings;
        for (index, (name, rva)) in exports.iter().enumerate() {
            let index = index as u32;
            self.put32(functions + 4 * index, *rva);
            self.put32(names + 4 * index, at);
            self.put(ordinals + 2 * index, &(index as u16).to_le_bytes());
            self.put(at, name.as_bytes());
            at += name.len() as u32 + 1;
        }
        self.directories[DIRECTORY_EXPORT] = (table, 0x200);
    }

    pub fn finish(mut self) -> Vec<u8> {
        let mut functions = self.functions.clone();
        functions.sort();
        for (index, (begin, end, unwind)) in functions.iter().enumerate() {
            let at = Self::PDATA + 12 * index as u32;
            self.put32(at, *begin);
            self.put32(at + 4, *end);
            self.put32(at + 8, *unwind);
        }
        if !functions.is_empty() {
            self.directories[DIRECTORY_EXCEPTION] = (Self::PDATA, 12 * functions.len() as u32);
        }
        self.put(0, b"MZ");
        self.put32(0x3C, 0x80);
        self.put(0x80, b"PE\0\0");
        let file = 0x84;
        self.put(file, &0x8664u16.to_le_bytes());
        self.put(file + 2, &4u16.to_le_bytes());
        self.put(file + 16, &240u16.to_le_bytes());
        let optional = file + 20;
        self.put(optional, &0x20Bu16.to_le_bytes());
        self.put32(optional + 16, Self::TEXT);
        self.put64(optional + 24, BASE);
        self.put32(optional + 56, Self::SIZE);
        self.put32(optional + 60, 0x400);
        self.put32(optional + 108, 16);
        for (index, (rva, size)) in self.directories.into_iter().enumerate() {
            self.put32(optional + 112 + 8 * index as u32, rva);
            self.put32(optional + 116 + 8 * index as u32, size);
        }
        let sections = [
            (".text", Self::TEXT, 0x2000, 0x6000_0020),
            (".rdata", Self::RDATA, 0x3000, 0x4000_0040),
            (".data", Self::DATA, 0x1000, 0xC000_0040),
            (".pdata", Self::PDATA, 0x1000, 0x4000_0040),
        ];
        for (index, (name, rva, size, flags)) in sections.into_iter().enumerate() {
            let at = optional + 240 + 40 * index as u32;
            self.put(at, name.as_bytes());
            self.put32(at + 8, size);
            self.put32(at + 12, rva);
            self.put32(at + 16, size);
            self.put32(at + 20, rva);
            self.put32(at + 36, flags);
        }
        self.bytes
    }
}

/// Machine code assembled at a fixed RVA, with RIP-relative operands computed from where each
/// instruction lands.
pub struct Code {
    pub start: u32,
    pub bytes: Vec<u8>,
}

impl Code {
    pub fn at(start: u32) -> Self {
        Code {
            start,
            bytes: Vec::new(),
        }
    }

    pub fn here(&self) -> u32 {
        self.start + self.bytes.len() as u32
    }

    fn relative(&mut self, opcode: &[u8], target: u32) -> &mut Self {
        self.bytes.extend(opcode);
        let next = self.here() + 4;
        self.bytes.extend((target.wrapping_sub(next)).to_le_bytes());
        self
    }

    pub fn call(&mut self, target: u32) -> &mut Self {
        self.relative(&[0xE8], target)
    }

    pub fn jmp(&mut self, target: u32) -> &mut Self {
        self.relative(&[0xE9], target)
    }

    pub fn jz(&mut self, target: u32) -> &mut Self {
        self.relative(&[0x0F, 0x84], target)
    }

    pub fn call_slot(&mut self, slot: u32) -> &mut Self {
        self.relative(&[0xFF, 0x15], slot)
    }

    pub fn jmp_slot(&mut self, slot: u32) -> &mut Self {
        self.relative(&[0xFF, 0x25], slot)
    }

    /// `lea rax, [rip+target]`
    pub fn lea(&mut self, target: u32) -> &mut Self {
        self.relative(&[0x48, 0x8D, 0x05], target)
    }

    /// `mov [rip+target], rax`
    pub fn store(&mut self, target: u32) -> &mut Self {
        self.relative(&[0x48, 0x89, 0x05], target)
    }

    /// `mov rax, [rip+target]`
    pub fn load(&mut self, target: u32) -> &mut Self {
        self.relative(&[0x48, 0x8B, 0x05], target)
    }

    pub fn ret(&mut self) -> &mut Self {
        self.bytes.push(0xC3);
        self
    }
}
