//! The PE64 writer: lays out the sections, writes the import table, the TLS directory with its callback, the
//! base relocations and the exception table around the code the emitter produced.

use std::collections::HashMap;

use crate::emit::{
    Assembled, BuildError, Decls, Emitter, ImportName, Layout, Res, StepInfo, Target,
};

pub use crate::emit::BASE;

const SECTION_ALIGN: u32 = 0x1000;
const FILE_ALIGN: u32 = 0x200;
const HEADERS: u32 = 0x400;
const FIRST_SECTION: u32 = 0x1000;

/// The TLS template: two 32-bit variables, then zero-filled bytes.
pub const TLS_FIRST: u32 = 0x1357_9BDF;
pub const TLS_SECOND: u32 = 0x2468_ACE0;
const TLS_ZERO_FILL: u32 = 0x10;

const READ: u32 = 0x4000_0000;
const WRITE: u32 = 0x8000_0000;
const EXEC: u32 = 0x2000_0000;
const CODE: u32 = 0x20;
const INITIALIZED: u32 = 0x40;
const DISCARDABLE: u32 = 0x0200_0000;

fn align(value: u32, to: u32) -> u32 {
    value.next_multiple_of(to)
}

fn put16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// A finished image and what the harness needs to read back from it.
#[derive(Debug, Clone)]
pub struct Built {
    pub bytes: Vec<u8>,
    pub steps: Vec<StepInfo>,
    /// Offsets from the image base of every declared data item.
    symbols: HashMap<String, u32>,
    /// Offsets of the eight-byte fields that carry an absolute address and so a base relocation.
    pub relocated: Vec<u32>,
    pub entry_rva: u32,
    /// Where `.data` lies: its offset from the base and its size.
    pub data_span: (u32, u32),
    /// `(name, begin, end)` of every function, as offsets from the base.
    pub functions: Vec<(String, u32, u32)>,
}

impl Built {
    /// The offset from the image base of a declared data item.
    pub fn symbol(&self, name: &str) -> Option<u32> {
        self.symbols.get(name).copied()
    }

    pub fn symbol_names(&self) -> impl Iterator<Item = &str> {
        self.symbols.keys().map(String::as_str)
    }

    pub fn step(&self, id: u32) -> Option<&StepInfo> {
        self.steps.iter().find(|step| step.id == id)
    }

    pub fn function(&self, name: &str) -> Option<(u32, u32)> {
        self.functions
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, begin, end)| (*begin, *end))
    }
}

/// The import table's shape: each DLL's imports, in order of first use.
struct Imports {
    /// `(dll, indices into decls.imports)`.
    groups: Vec<(String, Vec<usize>)>,
}

fn group_imports(decls: &Decls) -> Imports {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, import) in decls.imports.iter().enumerate() {
        match groups
            .iter_mut()
            .find(|(dll, _)| dll.eq_ignore_ascii_case(&import.dll))
        {
            Some((_, list)) => list.push(index),
            None => groups.push((import.dll.clone(), vec![index])),
        }
    }
    Imports { groups }
}

/// Where everything in `.rdata` lies, as offsets from the section start.
struct Rdata {
    descriptors: u32,
    lookups: Vec<u32>,
    tables: Vec<u32>,
    hints: u32,
    names: u32,
    tls: u32,
    callbacks: u32,
    unwind: u32,
    size: u32,
}

fn plan_rdata(decls: &Decls, imports: &Imports, unwound: usize) -> Rdata {
    let mut at = 0u32;
    let descriptors = at;
    at += 20 * (imports.groups.len() as u32 + 1);
    let mut lookups = Vec::new();
    for (_, list) in &imports.groups {
        lookups.push(at);
        at += 8 * (list.len() as u32 + 1);
    }
    let mut tables = Vec::new();
    for (_, list) in &imports.groups {
        tables.push(at);
        at += 8 * (list.len() as u32 + 1);
    }
    let hints = at;
    for import in &decls.imports {
        if let ImportName::Name(name) = &import.name {
            at += align(2 + name.len() as u32 + 1, 2);
        }
    }
    let names = at;
    for (dll, _) in &imports.groups {
        at += dll.len() as u32 + 1;
    }
    at = align(at, 16);
    let tls = at;
    at += 0x28;
    at = align(at, 16);
    let callbacks = at;
    at += 16;
    let unwind = at;
    at += 8 * unwound as u32;
    Rdata {
        descriptors,
        lookups,
        tables,
        hints,
        names,
        tls,
        callbacks,
        unwind,
        size: align(at, 16),
    }
}

fn compute_layout(
    decls: &Decls,
    imports: &Imports,
    rdata: &Rdata,
    text_size: u32,
    unwound: usize,
) -> (Layout, Sections) {
    let rdata_rva = FIRST_SECTION;
    let data_rva = rdata_rva + align(rdata.size, SECTION_ALIGN);
    let mut data_rvas = Vec::new();
    let mut at = 0u32;
    for decl in &decls.data {
        at = align(at, decl.align as u32);
        data_rvas.push(data_rva + at);
        at += decl.init.len() as u32;
    }
    let data_size = align(at, 16);
    let tls_rva = data_rva + align(data_size, SECTION_ALIGN);
    let text_rva = tls_rva + SECTION_ALIGN;
    let pdata_rva = text_rva + align(text_size, SECTION_ALIGN);
    let pdata_size = 12 * unwound as u32;
    let reloc_rva = pdata_rva + align(pdata_size.max(1), SECTION_ALIGN);
    let mut iat_rva = vec![0; decls.imports.len()];
    for (group, (_, list)) in imports.groups.iter().enumerate() {
        for (slot, import) in list.iter().enumerate() {
            iat_rva[*import] = rdata_rva + rdata.tables[group] + 8 * slot as u32;
        }
    }
    (
        Layout {
            data_rva: data_rvas,
            iat_rva,
        },
        Sections {
            rdata_rva,
            data_rva,
            data_size,
            tls_rva,
            text_rva,
            text_size,
            pdata_rva,
            pdata_size,
            reloc_rva,
        },
    )
}

struct Sections {
    rdata_rva: u32,
    data_rva: u32,
    data_size: u32,
    tls_rva: u32,
    text_rva: u32,
    text_size: u32,
    pdata_rva: u32,
    pdata_size: u32,
    reloc_rva: u32,
}

struct Section {
    name: &'static str,
    rva: u32,
    flags: u32,
    data: Vec<u8>,
}

/// Runs `emit` twice and writes the image.
pub(crate) fn build(emit: &dyn Fn(&mut Emitter) -> Res) -> Res<Built> {
    let mut probe = Emitter::new(None, None, FIRST_SECTION)?;
    emit(&mut probe)?;
    let decls = probe.decls().clone();
    let first = probe.finish(FIRST_SECTION)?;
    let unwound = first.functions.iter().filter(|f| f.3 != 0).count();
    let imports = group_imports(&decls);
    let rdata = plan_rdata(&decls, &imports, unwound);
    let (layout, sections) =
        compute_layout(&decls, &imports, &rdata, first.code.len() as u32, unwound);

    let mut emitter = Emitter::new(Some(decls.clone()), Some(layout.clone()), sections.text_rva)?;
    emit(&mut emitter)?;
    let last = emitter.finish(sections.text_rva)?;
    if last.code.len() != first.code.len() {
        return Err(BuildError::Layout(format!(
            "the two assembly passes differ: {} and {} bytes",
            first.code.len(),
            last.code.len()
        )));
    }
    write_image(&decls, &imports, &rdata, &layout, &sections, last)
}

fn write_image(
    decls: &Decls,
    imports: &Imports,
    rdata_plan: &Rdata,
    layout: &Layout,
    sections: &Sections,
    assembled: Assembled,
) -> Res<Built> {
    let function_rva = |name: &str| -> Res<u32> {
        assembled
            .functions
            .iter()
            .find(|f| f.0 == name)
            .map(|f| f.1)
            .ok_or_else(|| BuildError::Layout(format!("no function named {name}")))
    };
    let mut relocated: Vec<u32> = Vec::new();

    // .data
    let mut data = vec![0u8; sections.data_size as usize];
    for (index, decl) in decls.data.iter().enumerate() {
        let at = (layout.data_rva[index] - sections.data_rva) as usize;
        data[at..at + decl.init.len()].copy_from_slice(&decl.init);
        for (offset, target) in &decl.pointers {
            let rva = match target {
                Target::Place(place) => {
                    let base = match place.sym {
                        crate::Sym::Data(i) => layout.data_rva[i],
                        crate::Sym::Import(i) => layout.iat_rva[i],
                        crate::Sym::Image => 0,
                    };
                    base + place.offset
                }
                Target::Function(name) => function_rva(name)?,
            };
            put64(&mut data, at + offset, BASE + u64::from(rva));
            relocated.push(layout.data_rva[index] + *offset as u32);
        }
    }

    // .rdata
    let rd = |offset: u32| sections.rdata_rva + offset;
    let mut rdata = vec![0u8; rdata_plan.size as usize];
    let mut hint_at = rdata_plan.hints;
    let mut name_at = rdata_plan.names;
    for (group, (dll, list)) in imports.groups.iter().enumerate() {
        let descriptor = (rdata_plan.descriptors + 20 * group as u32) as usize;
        put32(&mut rdata, descriptor, rd(rdata_plan.lookups[group]));
        put32(&mut rdata, descriptor + 12, rd(name_at));
        put32(&mut rdata, descriptor + 16, rd(rdata_plan.tables[group]));
        rdata[name_at as usize..name_at as usize + dll.len()].copy_from_slice(dll.as_bytes());
        name_at += dll.len() as u32 + 1;
        for (slot, import) in list.iter().enumerate() {
            let entry = match &decls.imports[*import].name {
                ImportName::Name(name) => {
                    let at = hint_at as usize;
                    rdata[at + 2..at + 2 + name.len()].copy_from_slice(name.as_bytes());
                    hint_at += align(2 + name.len() as u32 + 1, 2);
                    u64::from(rd(at as u32))
                }
                ImportName::Ordinal(ordinal) => (1 << 63) | u64::from(*ordinal),
            };
            put64(
                &mut rdata,
                (rdata_plan.lookups[group] + 8 * slot as u32) as usize,
                entry,
            );
            put64(
                &mut rdata,
                (rdata_plan.tables[group] + 8 * slot as u32) as usize,
                entry,
            );
        }
    }

    // The TLS directory and its callback list; the index variable is `tls_index`, in `.data`.
    let tls_dir = rdata_plan.tls as usize;
    let index_rva = layout.data_rva[decls
        .data_named("tls_index")
        .ok_or_else(|| BuildError::Layout("the emitter always declares tls_index".into()))?];
    let template_len = 8u32;
    put64(&mut rdata, tls_dir, BASE + u64::from(sections.tls_rva));
    put64(
        &mut rdata,
        tls_dir + 8,
        BASE + u64::from(sections.tls_rva + template_len),
    );
    put64(&mut rdata, tls_dir + 16, BASE + u64::from(index_rva));
    put64(
        &mut rdata,
        tls_dir + 24,
        BASE + u64::from(rd(rdata_plan.callbacks)),
    );
    put32(&mut rdata, tls_dir + 32, TLS_ZERO_FILL);
    put64(
        &mut rdata,
        rdata_plan.callbacks as usize,
        BASE + u64::from(function_rva("tls_callback")?),
    );
    for field in 0..4 {
        relocated.push(rd(rdata_plan.tls + 8 * field));
    }
    relocated.push(rd(rdata_plan.callbacks));

    // Unwind information and the exception table: each framed function allocates its frame in one `sub rsp`.
    let mut unwind_at = rdata_plan.unwind;
    let mut pdata = Vec::new();
    let mut framed: Vec<&(String, u32, u32, u32)> =
        assembled.functions.iter().filter(|f| f.3 != 0).collect();
    framed.sort_by_key(|f| f.1);
    for (_, begin, end, frame) in framed {
        let at = unwind_at as usize;
        let prolog = 4u8;
        rdata[at] = 1;
        rdata[at + 1] = prolog;
        rdata[at + 2] = 1;
        rdata[at + 4] = prolog;
        rdata[at + 5] = (((frame - 8) / 8) as u8) << 4 | 2;
        pdata.extend_from_slice(&begin.to_le_bytes());
        pdata.extend_from_slice(&end.to_le_bytes());
        pdata.extend_from_slice(&rd(unwind_at).to_le_bytes());
        unwind_at += 8;
    }

    // .tls
    let mut tls = vec![0u8; template_len as usize];
    put32(&mut tls, 0, TLS_FIRST);
    put32(&mut tls, 4, TLS_SECOND);

    // .reloc
    relocated.sort_unstable();
    let mut reloc = Vec::new();
    let mut at = 0;
    while at < relocated.len() {
        let page = relocated[at] & !0xFFF;
        let mut entries: Vec<u16> = Vec::new();
        while at < relocated.len() && relocated[at] & !0xFFF == page {
            entries.push(0xA000 | (relocated[at] & 0xFFF) as u16);
            at += 1;
        }
        if entries.len() % 2 == 1 {
            entries.push(0);
        }
        reloc.extend_from_slice(&page.to_le_bytes());
        reloc.extend_from_slice(&(8 + 2 * entries.len() as u32).to_le_bytes());
        for entry in entries {
            reloc.extend_from_slice(&entry.to_le_bytes());
        }
    }

    let entry_rva = function_rva("entry")?;
    let table = [
        Section {
            name: ".rdata",
            rva: sections.rdata_rva,
            flags: READ | INITIALIZED,
            data: rdata,
        },
        Section {
            name: ".data",
            rva: sections.data_rva,
            flags: READ | WRITE | INITIALIZED,
            data,
        },
        Section {
            name: ".tls",
            rva: sections.tls_rva,
            flags: READ | WRITE | INITIALIZED,
            data: tls,
        },
        Section {
            name: ".text",
            rva: sections.text_rva,
            flags: READ | EXEC | CODE,
            data: assembled.code.clone(),
        },
        Section {
            name: ".pdata",
            rva: sections.pdata_rva,
            flags: READ | INITIALIZED,
            data: pdata,
        },
        Section {
            name: ".reloc",
            rva: sections.reloc_rva,
            flags: READ | INITIALIZED | DISCARDABLE,
            data: reloc.clone(),
        },
    ];
    debug_assert_eq!(sections.text_size as usize, assembled.code.len());

    let image_size = align(
        sections.reloc_rva + align(reloc.len() as u32, SECTION_ALIGN).max(SECTION_ALIGN),
        SECTION_ALIGN,
    );
    let mut raw_end = HEADERS;
    let mut offsets = Vec::new();
    for section in &table {
        offsets.push(raw_end);
        raw_end += align(section.data.len() as u32, FILE_ALIGN);
    }
    let mut file = vec![0u8; raw_end as usize];
    file[0..2].copy_from_slice(b"MZ");
    put32(&mut file, 0x3C, 0x40);
    file[0x40..0x44].copy_from_slice(b"PE\0\0");
    put16(&mut file, 0x44, 0x8664);
    put16(&mut file, 0x46, table.len() as u16);
    put16(&mut file, 0x54, 0xF0);
    put16(&mut file, 0x56, 0x22);
    let optional = 0x58;
    put16(&mut file, optional, 0x20B);
    put32(&mut file, optional + 4, sections.text_size);
    put32(&mut file, optional + 16, entry_rva);
    put32(&mut file, optional + 20, sections.text_rva);
    put64(&mut file, optional + 24, BASE);
    put32(&mut file, optional + 32, SECTION_ALIGN);
    put32(&mut file, optional + 36, FILE_ALIGN);
    put16(&mut file, optional + 40, 6);
    put16(&mut file, optional + 48, 6);
    put32(&mut file, optional + 56, image_size);
    put32(&mut file, optional + 60, HEADERS);
    put16(&mut file, optional + 68, 3);
    put16(&mut file, optional + 70, 0x160);
    put64(&mut file, optional + 72, 0x10_0000);
    put64(&mut file, optional + 80, 0x1000);
    put64(&mut file, optional + 88, 0x10_0000);
    put64(&mut file, optional + 96, 0x1000);
    put32(&mut file, optional + 108, 16);
    let directory = |file: &mut [u8], index: usize, rva: u32, size: u32| {
        put32(file, optional + 112 + 8 * index, rva);
        put32(file, optional + 116 + 8 * index, size);
    };
    let import_size = 20 * (imports.groups.len() as u32 + 1);
    directory(&mut file, 1, rd(rdata_plan.descriptors), import_size);
    directory(&mut file, 3, sections.pdata_rva, sections.pdata_size);
    directory(&mut file, 5, sections.reloc_rva, reloc.len() as u32);
    directory(&mut file, 9, rd(rdata_plan.tls), 0x28);
    for (index, section) in table.iter().enumerate() {
        let at = 0x148 + 40 * index;
        file[at..at + section.name.len()].copy_from_slice(section.name.as_bytes());
        put32(&mut file, at + 8, section.data.len() as u32);
        put32(&mut file, at + 12, section.rva);
        put32(
            &mut file,
            at + 16,
            align(section.data.len() as u32, FILE_ALIGN),
        );
        put32(&mut file, at + 20, offsets[index]);
        put32(&mut file, at + 36, section.flags);
        file[offsets[index] as usize..offsets[index] as usize + section.data.len()]
            .copy_from_slice(&section.data);
    }

    let symbols = decls
        .data
        .iter()
        .enumerate()
        .map(|(index, decl)| (decl.name.clone(), layout.data_rva[index]))
        .collect();
    Ok(Built {
        bytes: file,
        steps: assembled.steps,
        symbols,
        relocated,
        entry_rva,
        data_span: (sections.data_rva, sections.data_size),
        functions: assembled
            .functions
            .into_iter()
            .map(|(name, begin, end, _)| (name, begin, end))
            .collect(),
    })
}
