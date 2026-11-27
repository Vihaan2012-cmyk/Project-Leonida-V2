//! The built image as a file: it parses, names the imports the game names, and every import is one the runtime
//! binds (the same classification `leonida run --dry-run` prints). Nothing is loaded here.
#![cfg(windows)]

use pe_image::Image;
use pe_loader::ImportRef;
use shims::{Binder, Binding};
use standin::steps;

#[test]
fn every_import_is_provided_by_leonida_or_windows() {
    let built = steps::boot().build().expect("assembles");
    let image = Image::parse(&built.bytes).expect("parses");
    let binder = Binder::new(&[]);
    let mut shims = 0;
    let mut dlls = Vec::new();
    for dll in image.imports() {
        dlls.push(dll.name.clone());
        for entry in &dll.entries {
            let import = ImportRef {
                dll: &dll.name,
                name: entry.name.as_deref(),
                ordinal: entry.ordinal,
            };
            match binder.classify(&import) {
                Binding::Shim => shims += 1,
                Binding::Host => {}
                other => panic!("{}!{:?} is {other:?}", dll.name, entry.name),
            }
        }
    }
    for console in [
        "xmem.dll",
        "d3d12_xs.dll",
        "xapu.dll",
        "AcpHal.dll",
        "chad.dll",
        "GameInput.dll",
        "XCRDAPI.dll",
    ] {
        assert!(dlls.iter().any(|d| d == console), "{console} is imported");
    }
    assert!(shims >= 30, "{shims} imports are Leonida's own");
}

#[test]
fn the_image_has_a_tls_callback_relocations_and_an_exception_table() {
    let built = steps::boot().build().expect("assembles");
    let image = Image::parse(&built.bytes).expect("parses");
    assert!(image.directory(pe_image::DIRECTORY_TLS).is_some());
    assert!(image.directory(pe_image::DIRECTORY_EXCEPTION).is_some());
    let relocations = image.relocations();
    assert!(!relocations.is_empty());
    for rva in &built.relocated {
        assert!(
            relocations
                .iter()
                .any(|(at, kind)| at == rva && *kind == 10),
            "{rva:#x} is relocated"
        );
    }
    let (entry, _) = built.function("entry").expect("entry");
    assert_eq!(image.entry, entry);
    assert_eq!(image.image_base, standin::BASE);
}
