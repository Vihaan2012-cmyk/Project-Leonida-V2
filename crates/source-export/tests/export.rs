mod support;

use std::fs;
use std::path::Path;

use pe_image::Image;
use source_export::{ClassInfo, ImageSymbols, MARKER, Options, Symbols, default_name, export};
use support::{F1, F2, F3};

struct Named;

impl Symbols for Named {
    fn function_name(&self, rva: u32) -> String {
        match rva {
            F1 => "Ped_Process".to_string(),
            F2 => "Ped_Update".to_string(),
            _ => default_name(rva),
        }
    }

    fn class_of(&self, rva: u32) -> Option<String> {
        (rva == F2).then(|| "rage::Ped".to_string())
    }

    fn source_file(&self, rva: u32) -> Option<String> {
        (rva == F1).then(|| r"x:\game\src\Ped.cpp".to_string())
    }

    fn notes(&self, rva: u32) -> Vec<String> {
        if rva == F2 {
            vec!["updates the ped each frame".to_string()]
        } else {
            Vec::new()
        }
    }

    fn global_name(&self, _: u32) -> Option<String> {
        None
    }

    fn classes(&self) -> Vec<ClassInfo> {
        vec![ClassInfo {
            name: "rage::Ped".to_string(),
            bases: vec!["rage::Entity".to_string()],
            vtables: vec![0x2500],
            methods: vec![(0, F2), (2, F3)],
        }]
    }
}

fn options(out: &Path) -> Options {
    Options {
        out: out.to_path_buf(),
        jobs: 2,
        progress_every: 0,
        ..Options::default()
    }
}

fn read(out: &Path, relative: &str) -> String {
    fs::read_to_string(out.join(relative)).unwrap_or_else(|e| panic!("{relative}: {e}"))
}

#[test]
fn exports_the_expected_layout_with_headers() {
    let bytes = support::image();
    let image = Image::parse(&bytes).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let summary = export(&image, &Named, &options(dir.path())).unwrap();
    assert_eq!(summary.functions, 4);
    assert_eq!(summary.failed, 0, "{summary:?}");
    assert_eq!(summary.skipped_files, 0);

    for file in [
        "sources/game/src/Ped.c",
        "classes/rage/Ped.c",
        "classes/rage/Ped.h",
        "unsorted/0x00001080.c",
        "index.md",
        "index/functions.md",
        "index/imports.md",
        "index/strings.md",
        "imports/kernel32.dll.md",
    ] {
        let text = read(dir.path(), file);
        assert!(text.starts_with(MARKER), "{file} lacks the marker");
    }

    let ped = read(dir.path(), "sources/game/src/Ped.c");
    assert!(ped.contains("// ---- Ped_Process  rva 0x00001000  size 0x20 (32 bytes)"));
    assert!(ped.contains("// source: x:\\game\\src\\Ped.cpp"));
    assert!(ped.contains("// callees (1): Ped_Update"));
    assert!(ped.contains("// imports (1): kernel32.dll!CreateThing"));
    assert!(ped.contains("// strings (1): \"hello world\""));
    assert!(ped.contains("// end: functions 1, stubbed 0, failed 0"));
    assert!(!ped.contains("sub_00001040"), "calls use the symbol name");

    let update = read(dir.path(), "classes/rage/Ped.c");
    assert!(update.contains("// class: rage::Ped"));
    assert!(update.contains("// callers (2): Ped_Process, sub_00001080"));
    assert!(update.contains("// imports (1): kernel32.dll!OtherThing"));
    assert!(update.contains("// note: updates the ped each frame"));

    let unsorted = read(dir.path(), "unsorted/0x00001080.c");
    assert!(unsorted.contains("// ---- sub_000010c0  rva 0x000010c0  size unknown"));
    assert!(unsorted.contains("// callees (2): Ped_Update, sub_000010c0"));
    assert!(unsorted.contains("address range: 2 functions"));

    let header = read(dir.path(), "classes/rage/Ped.h");
    assert!(header.contains("// bases: rage::Entity"));
    assert!(header.contains("struct rage__Ped_vtbl {"));
    assert!(header.contains("void (*slot_0_Ped_Update)(struct rage__Ped *self);"));
    assert!(header.contains("void *slot_1; // unknown"));
    assert!(header.contains("void (*slot_2_sub_00001080)(struct rage__Ped *self);"));

    let index = read(dir.path(), "index.md");
    assert!(index.contains(
        "| [sources/game/src/Ped.c](sources/game/src/Ped.c) | source file | 1 | 0 | 0 |"
    ));
    assert!(
        index.contains("| [unsorted/0x00001080.c](unsorted/0x00001080.c) | address range | 2 |")
    );
    assert!(index.contains("[classes/rage/Ped.h](classes/rage/Ped.h)"));

    let by_name = read(dir.path(), "index/functions.md");
    let first = by_name.find("Ped_Process").unwrap();
    assert!(first < by_name.find("Ped_Update").unwrap());
    assert!(by_name.contains("- Ped_Update  0x00001040  classes/rage/Ped.c"));

    let imports = read(dir.path(), "imports/kernel32.dll.md");
    assert!(
        imports.contains("## CreateThing\n\n- Ped_Process  0x00001000  sources/game/src/Ped.c")
    );
    assert!(imports.contains("## OtherThing\n\n- Ped_Update  0x00001040  classes/rage/Ped.c"));
    let by_caller = read(dir.path(), "index/imports.md");
    assert!(by_caller.contains("- Ped_Process  0x00001000: kernel32.dll!CreateThing"));
    let strings = read(dir.path(), "index/strings.md");
    assert!(strings.contains("- \"hello world\"  0x00002100: Ped_Process"));
}

#[test]
fn only_exports_the_files_it_names() {
    let bytes = support::image();
    let image = Image::parse(&bytes).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut only = options(dir.path());
    only.only = vec!["0x1040".to_string()];
    let summary = export(&image, &Named, &only).unwrap();
    assert_eq!(summary.functions, 1);
    assert!(dir.path().join("classes/rage/Ped.c").exists());
    assert!(!dir.path().join("sources/game/src/Ped.c").exists());
    assert!(!dir.path().join("index/functions.md").exists());
    let index = read(dir.path(), "index.md");
    assert!(index.contains("| sources/game/src/Ped.c | source file | 1 | not exported |"));

    only.only = vec!["ped.c".to_string(), "Ped_Update".to_string()];
    let summary = export(&image, &Named, &only).unwrap();
    assert_eq!(summary.functions, 2);
    assert!(dir.path().join("sources/game/src/Ped.c").exists());
    assert!(!dir.path().join("unsorted/0x00001080.c").exists());

    only.only = vec!["unsorted".to_string()];
    let summary = export(&image, &Named, &only).unwrap();
    assert_eq!(summary.functions, 2);
}

#[test]
fn resume_skips_finished_files_and_redoes_cut_ones() {
    let bytes = support::image();
    let image = Image::parse(&bytes).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut resume = options(dir.path());
    let first = export(&image, &Named, &resume).unwrap();
    assert_eq!(first.files, 4);

    resume.resume = true;
    let again = export(&image, &Named, &resume).unwrap();
    assert_eq!(
        (again.functions, again.files, again.skipped_files),
        (0, 0, 4)
    );

    let path = dir.path().join("classes/rage/Ped.c");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, &text[..text.len() - 40]).unwrap();
    let repaired = export(&image, &Named, &resume).unwrap();
    assert_eq!(
        (repaired.functions, repaired.files, repaired.skipped_files),
        (1, 1, 3)
    );
    assert_eq!(read(dir.path(), "classes/rage/Ped.c"), text);
    assert!(read(dir.path(), "index.md").contains("| [classes/rage/Ped.c]"));
}

#[test]
fn functions_over_the_limit_are_stubbed() {
    let bytes = support::image();
    let image = Image::parse(&bytes).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut limited = options(dir.path());
    limited.max_function_bytes = 0x10;
    let summary = export(&image, &Named, &limited).unwrap();
    assert_eq!(summary.stubbed, 3);
    let ped = read(dir.path(), "sources/game/src/Ped.c");
    assert!(ped.contains("// stub: the function is 32 bytes, over the limit of 16"));
    assert!(ped.contains("void Ped_Process(void);"));
    assert!(ped.contains("// end: functions 1, stubbed 1, failed 0"));
    assert!(read(dir.path(), "index.md").contains("| source file | 1 | 1 | 0 |"));
}

#[test]
fn image_symbols_name_thunks_and_exports() {
    let bytes = support::image();
    let image = Image::parse(&bytes).unwrap();
    let symbols = ImageSymbols::new(&image);
    assert_eq!(symbols.function_name(F1), "ExportedStart");
    assert_eq!(symbols.function_name(support::THUNK), "j_OtherThing");
    assert_eq!(symbols.function_name(F3), "sub_00001080");
    assert_eq!(symbols.class_of(F1), None);
    assert_eq!(symbols.source_file(F1), None);
    assert_eq!(
        symbols.notes(support::THUNK),
        vec!["import thunk for kernel32.dll!OtherThing".to_string()]
    );
    assert!(symbols.notes(F1).contains(&"image entry point".to_string()));
    assert!(symbols.notes(F1).contains(&"exported".to_string()));

    let dir = tempfile::tempdir().unwrap();
    let summary = export(&image, &symbols, &options(dir.path())).unwrap();
    assert_eq!(summary.functions, 4);
    let unsorted = read(dir.path(), "unsorted/0x00001000.c");
    assert!(unsorted.contains("// ---- ExportedStart  rva 0x00001000"));
    assert!(unsorted.contains("// callees (1): sub_00001040"));
    assert!(unsorted.contains("// callees (2): sub_00001040, sub_000010c0"));
}

#[test]
fn program_database_names_and_notes_reach_the_files() {
    let bytes = support::image();
    let image = Image::parse(&bytes).unwrap();
    let db = exe_analysis::ProgramDb::build(&image);
    let symbols = source_export::DbSymbols::new(&db);
    assert_eq!(symbols.function_name(F1), "ExportedStart");
    let dir = tempfile::tempdir().unwrap();
    let summary = export(&image, &symbols, &options(dir.path())).unwrap();
    assert_eq!(summary.failed, 0, "{summary:?}");
    let all = fs::read_dir(dir.path().join("unsorted"))
        .unwrap()
        .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(all.contains("// ---- ExportedStart  rva 0x00001000"));
    assert!(all.contains("// note: named from export ordinal 1"));
}
