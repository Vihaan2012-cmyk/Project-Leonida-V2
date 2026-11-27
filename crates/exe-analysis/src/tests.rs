use pe_image::Image;

use crate::rtti::tests::write_animals;
use crate::testimage::{Builder, Code};
use crate::{BuildOptions, Error, NameEvidence, ProgramDb};

const STR_NAME: u32 = 0x4000;
const STR_PATH: u32 = 0x4040;
const STR_SHARED: u32 = 0x4100;
const STR_ONE: u32 = 0x4140;
const STR_TWO: u32 = 0x4180;
const STR_HEADER: u32 = 0x41C0;
const G1: u32 = 0x6000;
const G2: u32 = 0x6008;
const G3: u32 = 0x6010;
const G4: u32 = 0x6018;

fn function(b: &mut Builder, rva: u32, unwind: bool, body: impl FnOnce(&mut Code)) -> u32 {
    let mut code = Code::at(rva);
    body(&mut code);
    let end = code.here();
    b.code(code);
    if unwind {
        b.function(rva, end);
    }
    end
}

fn program() -> Vec<u8> {
    let mut b = Builder::new();
    let slots = b.import("kernel.dll", &["ExitThing", "OtherThing"]);
    b.put(STR_NAME, b"CFoo::Bar: failed\0");
    b.put(STR_PATH, b"x:\\game\\src\\foo.cpp\0");
    b.put(STR_SHARED, b"Shared failure message\0");
    b.put(STR_ONE, b"A::One: x\0");
    b.put(STR_TWO, b"B::Two: y\0");
    b.put(STR_HEADER, b"x:\\game\\src\\shared.h\0");
    write_animals(&mut b);

    function(&mut b, 0x1000, true, |c| {
        c.call(0x1040).call_slot(slots[0]).call(0x1100).ret();
    });
    function(&mut b, 0x1040, true, |c| {
        c.lea(STR_NAME)
            .lea(STR_PATH)
            .store(G1)
            .store(G3)
            .call(0x1080)
            .ret();
    });
    function(&mut b, 0x1080, true, |c| {
        c.load(G1).ret();
    });
    function(&mut b, 0x10C0, true, |c| {
        c.jmp(0x1080);
    });
    function(&mut b, 0x1100, false, |c| {
        c.jmp_slot(slots[1]);
    });
    function(&mut b, 0x1140, true, |c| {
        c.call(0x1180).ret();
    });
    function(&mut b, 0x1180, false, |c| {
        c.ret();
    });
    let primary_end = function(&mut b, 0x11C0, true, |c| {
        c.jz(0x1200).ret();
    });
    let cold_end = function(&mut b, 0x1200, false, |c| {
        c.call(0x1080).ret();
    });
    b.chained(0x1200, cold_end, (0x11C0, primary_end));
    function(&mut b, 0x1240, true, |c| {
        c.store(G4).ret();
    });
    function(&mut b, 0x1280, true, |c| {
        c.lea(STR_SHARED).lea(STR_HEADER).load(G4).ret();
    });
    function(&mut b, 0x12C0, true, |c| {
        c.lea(STR_SHARED).lea(STR_HEADER).store(G2).ret();
    });
    function(&mut b, 0x1300, true, |c| {
        c.lea(STR_ONE).lea(STR_TWO).store(G2).ret();
    });
    for rva in [0x1400, 0x1440, 0x1480] {
        function(&mut b, rva, true, |c| {
            c.ret();
        });
    }
    for rva in [0x14C0, 0x1500] {
        function(&mut b, rva, false, |c| {
            c.ret();
        });
    }
    b.export(&[("ExportedThing", 0x1240)]);
    b.finish()
}

fn analysed() -> ProgramDb {
    let bytes = program();
    let image = Image::parse(&bytes).unwrap();
    ProgramDb::build_with(&image, &BuildOptions { threads: 2 })
}

#[test]
fn finds_known_and_discovered_functions() {
    let db = analysed();
    let rvas: Vec<u32> = db.functions().iter().map(|f| f.rva).collect();
    assert_eq!(
        rvas,
        [
            0x1000, 0x1040, 0x1080, 0x10C0, 0x1100, 0x1140, 0x1180, 0x11C0, 0x1240, 0x1280, 0x12C0,
            0x1300, 0x1400, 0x1440, 0x1480, 0x14C0, 0x1500
        ]
    );
    let discovered: Vec<u32> = db
        .functions()
        .iter()
        .filter(|f| f.discovered)
        .map(|f| f.rva)
        .collect();
    assert_eq!(discovered, [0x1100, 0x1180, 0x14C0, 0x1500]);
    assert_eq!(db.function(0x1100).unwrap().size, 6);
}

#[test]
fn chained_unwind_entries_form_one_function() {
    let db = analysed();
    assert!(db.function(0x1200).is_none());
    let function = db.function(0x11C0).unwrap();
    assert_eq!(function.chunks.len(), 2);
    assert_eq!(function.chunks[0].0, 0x11C0);
    assert_eq!(function.chunks[1].0, 0x1200);
    assert_eq!(
        function.size,
        function.chunks.iter().map(|(b, e)| e - b).sum::<u32>()
    );
    assert_eq!(db.function_containing(0x1201).map(|f| f.rva), Some(0x11C0));
    assert_eq!(function.callees, [0x1080]);
}

#[test]
fn call_graph_has_calls_tail_jumps_and_imports() {
    let db = analysed();
    let entry = db.function(0x1000).unwrap();
    assert_eq!(entry.callees, [0x1040, 0x1100]);
    assert_eq!(entry.imports, ["ExitThing", "OtherThing"]);
    assert_eq!(db.function(0x1040).unwrap().callers, [0x1000]);
    assert_eq!(db.function(0x10C0).unwrap().callees, [0x1080]);
    assert_eq!(
        db.function(0x1080).unwrap().callers,
        [0x1040, 0x10C0, 0x11C0]
    );
    assert_eq!(db.function(0x1140).unwrap().callees, [0x1180]);
    assert_eq!(db.function(0x1100).unwrap().imports, ["OtherThing"]);
    assert!(db.function(0x1080).unwrap().callees.is_empty());
}

#[test]
fn records_string_references() {
    let db = analysed();
    let function = db.function(0x1040).unwrap();
    assert_eq!(function.strings, [STR_NAME, STR_PATH]);
    assert_eq!(db.string(STR_NAME), Some("CFoo::Bar: failed"));
    assert_eq!(db.string(0x4001), None);
}

#[test]
fn names_from_strings_need_a_single_referencing_function() {
    let db = analysed();
    let named = db.function(0x1040).unwrap();
    assert_eq!(named.name.as_deref(), Some("CFoo::Bar"));
    assert_eq!(named.class.as_deref(), Some("CFoo"));
    assert_eq!(
        named.name_evidence,
        Some(NameEvidence::String { string: STR_NAME })
    );
    assert_eq!(db.name_of(0x1040), Some("CFoo::Bar"));
    for rva in [0x1280, 0x12C0] {
        assert_eq!(db.name_of(rva), None, "shared text names nothing");
    }
    assert_eq!(
        db.name_of(0x1300),
        None,
        "two different names are ambiguous"
    );
    assert_eq!(db.display_name(0x1300), "sub_00001300");
}

#[test]
fn source_files_from_path_strings() {
    let db = analysed();
    let owner = db.function(0x1040).unwrap();
    assert_eq!(owner.source_file.as_deref(), Some("x:\\game\\src\\foo.cpp"));
    assert!(owner.source_evidence.unwrap().sole_reference);
    let shared = db.function(0x1280).unwrap();
    assert_eq!(
        shared.source_file.as_deref(),
        Some("x:\\game\\src\\shared.h")
    );
    assert!(!shared.source_evidence.unwrap().sole_reference);
    assert_eq!(
        db.source_files(),
        [
            ("x:\\game\\src\\foo.cpp".to_string(), vec![0x1040]),
            ("x:\\game\\src\\shared.h".to_string(), vec![0x1280, 0x12C0]),
        ]
    );
}

#[test]
fn thunks_and_exports_are_named_after_their_source() {
    let db = analysed();
    let thunk = db.function(0x1100).unwrap();
    assert_eq!(thunk.name.as_deref(), Some("OtherThing"));
    assert_eq!(
        thunk.name_evidence,
        Some(NameEvidence::ImportThunk {
            import: "OtherThing".into()
        })
    );
    let export = db.function(0x1240).unwrap();
    assert_eq!(export.name.as_deref(), Some("ExportedThing"));
    assert_eq!(
        export.name_evidence,
        Some(NameEvidence::Export { ordinal: 1 })
    );
}

#[test]
fn globals_written_by_one_named_function_get_its_name() {
    let db = analysed();
    let g1 = db.global(G1).unwrap();
    assert_eq!(g1.name.as_deref(), Some("g_CFoo_Bar"));
    assert_eq!(
        g1.name_evidence,
        Some(NameEvidence::GlobalWriter { function: 0x1040 })
    );
    assert_eq!(
        (g1.writers.as_slice(), g1.readers.as_slice()),
        (&[0x1040][..], &[0x1080][..])
    );
    assert_eq!(db.name_of(G1), Some("g_CFoo_Bar"));
    assert_eq!(
        db.global(G4).unwrap().name.as_deref(),
        Some("g_ExportedThing")
    );
    assert_eq!(db.global(G2).unwrap().writers, [0x12C0, 0x1300]);
    assert_eq!(db.global(G2).unwrap().name, None, "two writers");
    assert_eq!(db.global(G3).unwrap().name, None, "nothing else reads it");
}

#[test]
fn classes_vtables_and_virtual_method_names() {
    let db = analysed();
    let r = Builder::FREE_RDATA;
    let dog = db.class("Zoo::Dog").unwrap();
    assert_eq!(dog.bases, ["Animal"]);
    assert_eq!(dog.vtables, [r + 0x330, r + 0x360]);
    assert_eq!(db.vtable(r + 0x330).unwrap().class, "Zoo::Dog");
    assert_eq!(
        db.function(0x1400).unwrap().name.as_deref(),
        Some("Animal::vf0")
    );
    assert_eq!(
        db.function(0x1440).unwrap().name.as_deref(),
        Some("Animal::vf1")
    );
    assert_eq!(
        db.function(0x1480).unwrap().name.as_deref(),
        Some("Zoo::Dog::vf0")
    );
    assert_eq!(
        db.function(0x14C0).unwrap().name.as_deref(),
        Some("Zoo::Dog::vf2")
    );
    assert_eq!(
        db.function(0x1500).unwrap().name,
        None,
        "secondary vtables are not named"
    );
    let shared = db.function(0x1440).unwrap();
    assert_eq!(shared.class.as_deref(), Some("Animal"));
    assert_eq!(shared.vtable_slots, [(r + 0x308, 1), (r + 0x330, 1)]);
    assert_eq!(
        shared.name_evidence,
        Some(NameEvidence::VirtualSlot {
            vtable: r + 0x308,
            slot: 1
        })
    );
}

#[test]
fn a_better_name_wins_over_the_virtual_slot_name() {
    let mut b = Builder::new();
    write_animals(&mut b);
    b.put(STR_NAME, b"Animal::speak: called\0");
    let mut code = Code::at(0x1400);
    code.lea(STR_NAME).ret();
    b.code(code);
    b.function(0x1400, 0x1408);
    let bytes = b.finish();
    let image = Image::parse(&bytes).unwrap();
    let db = ProgramDb::build(&image);
    let function = db.function(0x1400).unwrap();
    assert_eq!(function.name.as_deref(), Some("Animal::speak"));
    assert_eq!(function.class.as_deref(), Some("Animal"));
}

#[test]
fn summary_counts_each_kind_of_evidence() {
    let summary = analysed().summary();
    assert_eq!(summary.functions, 17);
    assert_eq!(summary.discovered, 4);
    assert_eq!(summary.named_by_export, 1);
    assert_eq!(summary.named_by_import_thunk, 1);
    assert_eq!(summary.named_by_string, 1);
    assert_eq!(summary.named_by_vtable_slot, 4);
    assert_eq!(summary.named, 7);
    assert_eq!(summary.classes, 2);
    assert_eq!(summary.vtables, 3);
    assert_eq!(summary.source_files, 2);
    assert_eq!(summary.named_globals, 2);
    assert!(summary.to_string().contains("17 functions"));
}

#[test]
fn saves_and_loads_the_same_database() {
    let db = analysed();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("program.db");
    db.save(&path).unwrap();
    let loaded = ProgramDb::load(&path).unwrap();
    assert_eq!(loaded, db);
    assert_eq!(
        loaded.function_containing(0x1201).map(|f| f.rva),
        Some(0x11C0)
    );
    assert_eq!(loaded.name_of(0x1040), Some("CFoo::Bar"));
}

#[test]
fn rejects_files_that_are_not_databases() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.db");
    std::fs::write(&path, b"not a database").unwrap();
    assert!(matches!(ProgramDb::load(&path), Err(Error::BadHeader)));
    let db = analysed();
    db.save(&path).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.truncate(bytes.len() / 2);
    std::fs::write(&path, bytes).unwrap();
    assert!(matches!(ProgramDb::load(&path), Err(Error::Corrupt(_))));
    assert!(matches!(
        ProgramDb::load(&dir.path().join("missing.db")),
        Err(Error::Io(_))
    ));
}

#[test]
fn an_image_without_a_function_table_still_builds() {
    let image_bytes = Builder::new().finish();
    let image = Image::parse(&image_bytes).unwrap();
    let db = ProgramDb::build(&image);
    assert!(db.classes().is_empty() && db.vtables().is_empty());
    assert!(db.functions().iter().all(|f| f.discovered));
}

/// Builds the real executable's database when `LEONIDA_GAME_DIR` names the game folder.
#[test]
fn real_game_has_plausible_counts() {
    let Some(dir) = std::env::var_os("LEONIDA_GAME_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let config = std::fs::read_to_string(dir.join("MicrosoftGame.Config")).unwrap();
    let name = config
        .split("<Executable ")
        .nth(1)
        .and_then(|rest| rest.split("Name=\"").nth(1))
        .and_then(|rest| rest.split('"').next())
        .unwrap();
    let bytes = std::fs::read(dir.join(name)).unwrap();
    let image = Image::parse(&bytes).unwrap();
    let db = ProgramDb::build(&image);
    let summary = db.summary();
    println!("{summary}");
    assert!(summary.functions > 10_000, "{summary}");
    assert!(summary.call_edges > summary.functions, "{summary}");
    assert!(
        summary.classes > 100 && summary.vtables >= summary.classes,
        "{summary}"
    );
    assert!(summary.named > 0, "{summary}");
    let sorted = db.functions().windows(2).all(|w| w[0].rva < w[1].rva);
    assert!(sorted);
}
