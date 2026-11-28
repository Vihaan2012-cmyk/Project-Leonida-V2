//! Read-only parsing of PE32+ images: headers, sections, imports, exports and the x64 function table.

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeError {
    #[error("not a PE image: {0}")]
    NotPe(&'static str),
    #[error("truncated {0}")]
    Truncated(&'static str),
    #[error("only 64-bit (PE32+) images are supported")]
    Not64,
}

pub mod authenticode;

type Result<T> = std::result::Result<T, PeError>;

pub const DIRECTORY_EXPORT: usize = 0;
pub const DIRECTORY_IMPORT: usize = 1;
pub const DIRECTORY_RESOURCE: usize = 2;
pub const DIRECTORY_EXCEPTION: usize = 3;
pub const DIRECTORY_SECURITY: usize = 4;
pub const DIRECTORY_RELOCATION: usize = 5;
pub const DIRECTORY_TLS: usize = 9;
pub const DIRECTORY_DELAY_IMPORT: usize = 13;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub virtual_address: u32,
    pub virtual_size: u32,
    pub raw_offset: u32,
    pub raw_size: u32,
    pub characteristics: u32,
}

impl Section {
    pub fn executable(&self) -> bool {
        self.characteristics & 0x2000_0000 != 0
    }

    pub fn writable(&self) -> bool {
        self.characteristics & 0x8000_0000 != 0
    }

    fn contains(&self, rva: u32) -> bool {
        rva >= self.virtual_address
            && rva - self.virtual_address < self.virtual_size.max(self.raw_size)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub name: Option<String>,
    pub ordinal: Option<u16>,
    pub iat_rva: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedDll {
    pub name: String,
    pub delayed: bool,
    pub entries: Vec<Import>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    pub name: Option<String>,
    pub ordinal: u32,
    pub rva: u32,
    pub forwarder: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Function {
    pub begin: u32,
    pub end: u32,
    pub unwind: u32,
}

#[derive(Debug, Clone)]
pub struct Image<'a> {
    bytes: &'a [u8],
    optional: usize,
    pub machine: u16,
    pub timestamp: u32,
    pub characteristics: u16,
    pub image_base: u64,
    pub entry: u32,
    pub image_size: u32,
    pub headers_size: u32,
    pub subsystem: u16,
    pub dll_characteristics: u16,
    pub sections: Vec<Section>,
    pub directories: Vec<(u32, u32)>,
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

impl<'a> Image<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Image<'a>> {
        if bytes.get(..2) != Some(b"MZ") {
            return Err(PeError::NotPe("no MZ header"));
        }
        let pe = u32_at(bytes, 0x3C).ok_or(PeError::Truncated("DOS header"))? as usize;
        if bytes.get(pe..pe + 4) != Some(b"PE\0\0") {
            return Err(PeError::NotPe("no PE signature"));
        }
        let file = pe + 4;
        let field16 = |at: usize, what| u16_at(bytes, at).ok_or(PeError::Truncated(what));
        let field32 = |at: usize, what| u32_at(bytes, at).ok_or(PeError::Truncated(what));
        let machine = field16(file, "file header")?;
        let section_count = field16(file + 2, "file header")? as usize;
        let timestamp = field32(file + 4, "file header")?;
        let optional_size = field16(file + 16, "file header")? as usize;
        let characteristics = field16(file + 18, "file header")?;
        let optional = file + 20;
        if field16(optional, "optional header")? != 0x20B {
            return Err(PeError::Not64);
        }
        let entry = field32(optional + 16, "optional header")?;
        let image_base =
            u64_at(bytes, optional + 24).ok_or(PeError::Truncated("optional header"))?;
        let image_size = field32(optional + 56, "optional header")?;
        let headers_size = field32(optional + 60, "optional header")?;
        let subsystem = field16(optional + 68, "optional header")?;
        let dll_characteristics = field16(optional + 70, "optional header")?;
        let directory_count = (field32(optional + 108, "optional header")? as usize).min(16);
        let mut directories = Vec::with_capacity(directory_count);
        for index in 0..directory_count {
            let at = optional + 112 + 8 * index;
            directories.push((
                field32(at, "data directories")?,
                field32(at + 4, "data directories")?,
            ));
        }
        let table = optional + optional_size;
        let mut sections = Vec::with_capacity(section_count);
        for index in 0..section_count {
            let at = table + 40 * index;
            let raw = bytes
                .get(at..at + 40)
                .ok_or(PeError::Truncated("section table"))?;
            let name_end = raw[..8].iter().position(|b| *b == 0).unwrap_or(8);
            sections.push(Section {
                name: String::from_utf8_lossy(&raw[..name_end]).into_owned(),
                virtual_size: u32_at(raw, 8).unwrap_or(0),
                virtual_address: u32_at(raw, 12).unwrap_or(0),
                raw_size: u32_at(raw, 16).unwrap_or(0),
                raw_offset: u32_at(raw, 20).unwrap_or(0),
                characteristics: u32_at(raw, 36).unwrap_or(0),
            });
        }
        Ok(Image {
            bytes,
            optional,
            machine,
            timestamp,
            characteristics,
            image_base,
            entry,
            image_size,
            headers_size,
            subsystem,
            dll_characteristics,
            sections,
            directories,
        })
    }

    /// The whole file the image was parsed from.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// File offset of the 4-byte checksum field in the optional header.
    pub fn checksum_offset(&self) -> usize {
        self.optional + 64
    }

    /// File offset of the 8-byte entry for data directory `index` in the optional header.
    pub fn directory_entry_offset(&self, index: usize) -> usize {
        self.optional + 112 + 8 * index
    }

    pub fn directory(&self, index: usize) -> Option<(u32, u32)> {
        self.directories
            .get(index)
            .copied()
            .filter(|(rva, size)| *rva != 0 && *size != 0)
    }

    pub fn section_of(&self, rva: u32) -> Option<&Section> {
        self.sections.iter().find(|s| s.contains(rva))
    }

    pub fn offset_of(&self, rva: u32) -> Option<usize> {
        let section = self.section_of(rva)?;
        let delta = rva - section.virtual_address;
        (delta < section.raw_size).then(|| (section.raw_offset + delta) as usize)
    }

    /// Bytes stored in the file from `rva` to the end of its section's raw data.
    pub fn bytes_at(&self, rva: u32) -> Option<&'a [u8]> {
        let section = self.section_of(rva)?;
        let start = self.offset_of(rva)?;
        let end = (section.raw_offset + section.raw_size) as usize;
        self.bytes.get(start..end.min(self.bytes.len()))
    }

    pub fn read(&self, rva: u32, len: usize) -> Option<&'a [u8]> {
        self.bytes_at(rva)?.get(..len)
    }

    pub fn string_at(&self, rva: u32) -> Option<String> {
        let bytes = self.bytes_at(rva)?;
        let end = bytes.iter().position(|b| *b == 0)?;
        Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }

    fn u32_rva(&self, rva: u32) -> Option<u32> {
        u32_at(self.read(rva, 4)?, 0)
    }

    fn thunks(&self, lookup: u32, iat: u32) -> Vec<Import> {
        let mut entries = Vec::new();
        let start = if lookup != 0 { lookup } else { iat };
        for index in 0.. {
            let Some(raw) = self.read(start + 8 * index, 8).and_then(|b| u64_at(b, 0)) else {
                break;
            };
            if raw == 0 {
                break;
            }
            let iat_rva = iat + 8 * index;
            if raw & (1 << 63) != 0 {
                entries.push(Import {
                    name: None,
                    ordinal: Some(raw as u16),
                    iat_rva,
                });
            } else {
                entries.push(Import {
                    name: self.string_at(raw as u32 + 2),
                    ordinal: None,
                    iat_rva,
                });
            }
        }
        entries
    }

    pub fn imports(&self) -> Vec<ImportedDll> {
        let mut dlls = Vec::new();
        if let Some((rva, _)) = self.directory(DIRECTORY_IMPORT) {
            for index in 0.. {
                let at = rva + 20 * index;
                let (Some(lookup), Some(name), Some(iat)) = (
                    self.u32_rva(at),
                    self.u32_rva(at + 12),
                    self.u32_rva(at + 16),
                ) else {
                    break;
                };
                if lookup == 0 && name == 0 && iat == 0 {
                    break;
                }
                dlls.push(ImportedDll {
                    name: self.string_at(name).unwrap_or_default(),
                    delayed: false,
                    entries: self.thunks(lookup, iat),
                });
            }
        }
        if let Some((rva, _)) = self.directory(DIRECTORY_DELAY_IMPORT) {
            for index in 0.. {
                let at = rva + 32 * index;
                let (Some(name), Some(iat), Some(lookup)) = (
                    self.u32_rva(at + 4),
                    self.u32_rva(at + 12),
                    self.u32_rva(at + 16),
                ) else {
                    break;
                };
                if name == 0 {
                    break;
                }
                dlls.push(ImportedDll {
                    name: self.string_at(name).unwrap_or_default(),
                    delayed: true,
                    entries: self.thunks(lookup, iat),
                });
            }
        }
        for dll in &mut dlls {
            let sockets = ["ws2_32.dll", "wsock32.dll"]
                .iter()
                .any(|n| dll.name.eq_ignore_ascii_case(n));
            for entry in dll
                .entries
                .iter_mut()
                .filter(|e| sockets && e.name.is_none())
            {
                entry.name = entry.ordinal.and_then(socket_ordinal).map(str::to_string);
            }
        }
        dlls
    }

    pub fn exports(&self) -> Vec<Export> {
        let Some((rva, size)) = self.directory(DIRECTORY_EXPORT) else {
            return Vec::new();
        };
        let field = |offset: u32| self.u32_rva(rva + offset).unwrap_or(0);
        let (base, functions, names) = (field(16), field(20), field(24));
        let (function_table, name_table, ordinal_table) = (field(28), field(32), field(36));
        let mut named = std::collections::HashMap::new();
        for index in 0..names.min(1 << 20) {
            let name = self
                .u32_rva(name_table + 4 * index)
                .and_then(|n| self.string_at(n));
            let ordinal = self
                .read(ordinal_table + 2 * index, 2)
                .and_then(|b| u16_at(b, 0));
            if let (Some(name), Some(ordinal)) = (name, ordinal) {
                named.insert(u32::from(ordinal), name);
            }
        }
        (0..functions.min(1 << 20))
            .filter_map(|index| {
                let target = self.u32_rva(function_table + 4 * index)?;
                if target == 0 {
                    return None;
                }
                let forwarder = (target >= rva && target < rva + size)
                    .then(|| self.string_at(target))
                    .flatten();
                Some(Export {
                    name: named.get(&index).cloned(),
                    ordinal: base + index,
                    rva: target,
                    forwarder,
                })
            })
            .collect()
    }

    /// The x64 exception table: every function with unwind data, sorted by start address.
    pub fn functions(&self) -> Vec<Function> {
        let Some((rva, size)) = self.directory(DIRECTORY_EXCEPTION) else {
            return Vec::new();
        };
        let Some(table) = self.read(rva, size as usize) else {
            return Vec::new();
        };
        let mut functions: Vec<Function> = table
            .chunks_exact(12)
            .filter_map(|entry| {
                let begin = u32_at(entry, 0)?;
                let end = u32_at(entry, 4)?;
                (begin != 0 && end > begin).then(|| Function {
                    begin,
                    end,
                    unwind: u32_at(entry, 8).unwrap_or(0),
                })
            })
            .collect();
        functions.sort_by_key(|f| f.begin);
        functions
    }

    /// The first piece of a function the compiler split into parts with chained unwind data.
    pub fn primary(&self, mut function: Function) -> Function {
        for _ in 0..32 {
            let Some(info) = self.read(function.unwind & !1, 4) else {
                break;
            };
            if (info[0] >> 3) & 0x4 == 0 {
                break;
            }
            let codes = (u32::from(info[2]) + 1) & !1;
            let Some(entry) = self.read((function.unwind & !1) + 4 + 2 * codes, 12) else {
                break;
            };
            let (Some(begin), Some(end), Some(unwind)) =
                (u32_at(entry, 0), u32_at(entry, 4), u32_at(entry, 8))
            else {
                break;
            };
            function = Function { begin, end, unwind };
        }
        function
    }

    pub fn function_containing(&self, rva: u32) -> Option<Function> {
        let functions = self.functions();
        let index = functions.partition_point(|f| f.begin <= rva);
        functions[..index].last().copied().filter(|f| rva < f.end)
    }

    /// Each base relocation as the RVA it patches and its type (10 is a 64-bit address, 0 is padding).
    pub fn relocations(&self) -> Vec<(u32, u8)> {
        let mut out = Vec::new();
        let Some((rva, size)) = self.directory(DIRECTORY_RELOCATION) else {
            return out;
        };
        let mut at = 0;
        while at + 8 <= size {
            let page = self.u32_rva(rva + at).unwrap_or(0);
            let block = self.u32_rva(rva + at + 4).unwrap_or(0);
            if block < 8 {
                break;
            }
            let Some(entries) = self.read(rva + at + 8, (block - 8) as usize) else {
                break;
            };
            for pair in entries.chunks_exact(2) {
                let entry = u16::from_le_bytes([pair[0], pair[1]]);
                out.push((page + u32::from(entry & 0xFFF), (entry >> 12) as u8));
            }
            at += block;
        }
        out
    }

    pub fn relocation_count(&self) -> usize {
        let Some((rva, size)) = self.directory(DIRECTORY_RELOCATION) else {
            return 0;
        };
        let mut count = 0;
        let mut at = 0;
        while at + 8 <= size {
            let block = self.u32_rva(rva + at + 4).unwrap_or(0);
            if block < 8 {
                break;
            }
            count += ((block - 8) / 2) as usize;
            at += block;
        }
        count
    }
}

fn socket_ordinal(ordinal: u16) -> Option<&'static str> {
    const LOW: [&str; 23] = [
        "accept",
        "bind",
        "closesocket",
        "connect",
        "getpeername",
        "getsockname",
        "getsockopt",
        "htonl",
        "htons",
        "ioctlsocket",
        "inet_addr",
        "inet_ntoa",
        "listen",
        "ntohl",
        "ntohs",
        "recv",
        "recvfrom",
        "select",
        "send",
        "sendto",
        "setsockopt",
        "shutdown",
        "socket",
    ];
    const DATABASE: [&str; 7] = [
        "gethostbyaddr",
        "gethostbyname",
        "getprotobyname",
        "getprotobynumber",
        "getservbyname",
        "getservbyport",
        "gethostname",
    ];
    const ASYNC: [&str; 16] = [
        "WSAAsyncSelect",
        "WSAAsyncGetHostByAddr",
        "WSAAsyncGetHostByName",
        "WSAAsyncGetProtoByNumber",
        "WSAAsyncGetProtoByName",
        "WSAAsyncGetServByPort",
        "WSAAsyncGetServByName",
        "WSACancelAsyncRequest",
        "WSASetBlockingHook",
        "WSAUnhookBlockingHook",
        "WSAGetLastError",
        "WSASetLastError",
        "WSACancelBlockingCall",
        "WSAIsBlocking",
        "WSAStartup",
        "WSACleanup",
    ];
    let index = usize::from(ordinal);
    match ordinal {
        1..=23 => Some(LOW[index - 1]),
        51..=57 => Some(DATABASE[index - 51]),
        101..=116 => Some(ASYNC[index - 101]),
        151 => Some("__WSAFDIsSet"),
        _ => None,
    }
}

/// Printable ASCII and UTF-16LE runs of at least `min` characters in `bytes`, with their offsets.
pub fn strings(bytes: &[u8], min: usize) -> Vec<(usize, String)> {
    let mut found = runs(bytes, 1, min);
    found.extend(runs(bytes, 2, min));
    found.sort_by_key(|(offset, _)| *offset);
    found
}

fn runs(bytes: &[u8], width: usize, min: usize) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut text = String::new();
    let mut start = 0;
    for (index, unit) in bytes.chunks_exact(width).enumerate() {
        let printable = (0x20..0x7F).contains(&unit[0]) && unit[1..].iter().all(|b| *b == 0);
        if printable {
            if text.is_empty() {
                start = index * width;
            }
            text.push(char::from(unit[0]));
            continue;
        }
        if text.len() >= min {
            found.push((start, std::mem::take(&mut text)));
        }
        text.clear();
    }
    if text.len() >= min {
        found.push((start, text));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn synthetic() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x2000];
        bytes[..2].copy_from_slice(b"MZ");
        put32(&mut bytes, 0x3C, 0x80);
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        let file = 0x84;
        bytes[file..file + 2].copy_from_slice(&0x8664u16.to_le_bytes());
        bytes[file + 2..file + 4].copy_from_slice(&1u16.to_le_bytes());
        bytes[file + 16..file + 18].copy_from_slice(&240u16.to_le_bytes());
        let optional = file + 20;
        bytes[optional..optional + 2].copy_from_slice(&0x20Bu16.to_le_bytes());
        put32(&mut bytes, optional + 16, 0x1010);
        bytes[optional + 24..optional + 32].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
        put32(&mut bytes, optional + 56, 0x3000);
        put32(&mut bytes, optional + 108, 16);
        let directory = |index: usize| optional + 112 + 8 * index;
        put32(&mut bytes, directory(DIRECTORY_IMPORT), 0x1100);
        put32(&mut bytes, directory(DIRECTORY_IMPORT) + 4, 40);
        put32(&mut bytes, directory(DIRECTORY_EXCEPTION), 0x1200);
        put32(&mut bytes, directory(DIRECTORY_EXCEPTION) + 4, 24);
        let section = optional + 240;
        bytes[section..section + 5].copy_from_slice(b".text");
        put32(&mut bytes, section + 8, 0x1000);
        put32(&mut bytes, section + 12, 0x1000);
        put32(&mut bytes, section + 16, 0x1000);
        put32(&mut bytes, section + 20, 0x1000);
        put32(&mut bytes, section + 36, 0x6000_0020);
        let at = |rva: usize| rva - 0x1000 + 0x1000;
        put32(&mut bytes, at(0x1100), 0x1140);
        put32(&mut bytes, at(0x1100) + 12, 0x1180);
        put32(&mut bytes, at(0x1100) + 16, 0x1160);
        bytes[at(0x1140)..at(0x1140) + 8].copy_from_slice(&0x11A0u64.to_le_bytes());
        bytes[at(0x1148)..at(0x1148) + 8]
            .copy_from_slice(&(0x8000_0000_0000_0007u64).to_le_bytes());
        bytes[at(0x1180)..at(0x1180) + 11].copy_from_slice(b"kernel.dll\0");
        bytes[at(0x11A2)..at(0x11A2) + 10].copy_from_slice(b"ExitThing\0");
        put32(&mut bytes, at(0x1200), 0x1030);
        put32(&mut bytes, at(0x1200) + 4, 0x1040);
        put32(&mut bytes, at(0x1200) + 12, 0x1010);
        put32(&mut bytes, at(0x1200) + 16, 0x1020);
        bytes[at(0x1300)..at(0x1300) + 12].copy_from_slice(b"hello world\0");
        bytes
    }

    #[test]
    fn headers_sections_imports_and_functions() {
        let bytes = synthetic();
        let image = Image::parse(&bytes).unwrap();
        assert_eq!(
            (image.machine, image.entry, image.image_base),
            (0x8664, 0x1010, 0x1_4000_0000)
        );
        assert_eq!(image.sections[0].name, ".text");
        assert!(image.sections[0].executable() && !image.sections[0].writable());
        let imports = image.imports();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].name, "kernel.dll");
        assert_eq!(imports[0].entries[0].name.as_deref(), Some("ExitThing"));
        assert_eq!(imports[0].entries[1].ordinal, Some(7));
        assert_eq!(imports[0].entries[1].iat_rva, 0x1168);
        let functions = image.functions();
        assert_eq!(
            functions.iter().map(|f| f.begin).collect::<Vec<_>>(),
            vec![0x1010, 0x1030]
        );
        assert_eq!(
            image.function_containing(0x1015).map(|f| f.begin),
            Some(0x1010)
        );
        assert_eq!(image.function_containing(0x1025), None);
        assert!(strings(&bytes, 8).iter().any(|(_, s)| s == "hello world"));
    }

    #[test]
    fn names_socket_ordinals() {
        assert_eq!(socket_ordinal(23), Some("socket"));
        assert_eq!(socket_ordinal(57), Some("gethostname"));
        assert_eq!(socket_ordinal(115), Some("WSAStartup"));
        assert_eq!(socket_ordinal(151), Some("__WSAFDIsSet"));
        assert_eq!(socket_ordinal(24), None);
    }

    #[test]
    fn finds_wide_strings() {
        let mut bytes = vec![0u8; 8];
        bytes.extend(
            "https://a.example"
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        );
        bytes.extend([0, 0, b'x', b'y', b'z', b'w', b'v', 0]);
        let found = strings(&bytes, 5);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], (8, "https://a.example".to_string()));
        assert_eq!(found[1], (44, "xyzwv".to_string()));
    }

    #[test]
    fn rejects_non_pe_and_32_bit() {
        assert!(matches!(Image::parse(b"nope"), Err(PeError::NotPe(_))));
        let mut bytes = synthetic();
        bytes[0x84 + 20] = 0x0B;
        bytes[0x84 + 21] = 0x01;
        assert_eq!(Image::parse(&bytes).unwrap_err(), PeError::Not64);
    }
}
