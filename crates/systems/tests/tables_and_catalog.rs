use systems::catalog::{self, Reference};
use systems::convert::{self, Kind};
use systems::text::{self, TextFile};
use systems::{Error, rbf0, rbf0_xml};

const DESCRIPTOR: usize = 0x100;
const INDEX: usize = 0x200;
const HASHES: usize = 0x400;
const BLOB: usize = 0x800;

fn put_ptr(buf: &mut [u8], at: usize, offset: usize) {
    buf[at..at + 4].copy_from_slice(&(0x5000_0000u32 | offset as u32).to_le_bytes());
}

/// A resource with one string table. `entries` are `(hash, index slot, blob offset)`; the blob holds `blob`.
fn resource(entries: &[(u32, usize, usize)], slots: usize, blob: &[u8]) -> Vec<u8> {
    let mut buf = vec![0u8; 0x1000];
    buf[0x10..0x14].copy_from_slice(b"0DRP");
    let d = DESCRIPTOR;
    put_ptr(&mut buf, d, INDEX);
    buf[d + 12] = 0x10;
    put_ptr(&mut buf, d + 16, 0x300);
    buf[d + 24..d + 28].copy_from_slice(&[1, 1, 0, 0]);
    buf[d + 28..d + 32].copy_from_slice(&((entries.len() * 16) as u32).to_le_bytes());
    put_ptr(&mut buf, d + 32, HASHES);
    buf[d + 40] = 0x10;
    buf[d + 44..d + 48].copy_from_slice(&(blob.len() as u32).to_le_bytes());
    put_ptr(&mut buf, d + 48, BLOB);
    buf[BLOB..BLOB + blob.len()].copy_from_slice(blob);
    for slot in 0..slots {
        let at = INDEX + 16 * slot;
        let offset = entries.iter().find(|e| e.1 == slot).map_or(0, |e| e.2);
        buf[at + 8..at + 16].copy_from_slice(&(((offset as u64) << 20) | 0b100).to_le_bytes());
    }
    for (n, &(hash, slot, _)) in entries.iter().enumerate() {
        let at = HASHES + 16 * n;
        buf[at..at + 4].copy_from_slice(&hash.to_le_bytes());
        buf[at + 8] = 1;
        buf[at + 11..at + 14].copy_from_slice(&(slot as u32).to_le_bytes()[..3]);
    }
    buf
}

fn english() -> Vec<u8> {
    let blob = b"Hello, what is this and you~n~not\0Bye\0Longer text here\0";
    resource(
        &[
            (0x1111_1111, 0, 0),
            (0x2222_2222, 1, 34),
            (0x3333_3333, 2, 38),
        ],
        3,
        blob,
    )
}

#[test]
fn reads_a_string_table_to_json() {
    let file = text::read_tables(&english()).unwrap();
    assert_eq!(file.language, "en");
    assert_eq!(
        file.strings["0x11111111"],
        "Hello, what is this and you~n~not"
    );
    assert_eq!(file.strings["0x22222222"], "Bye");
    assert_eq!(file.strings.len(), 3);
    assert_eq!(TextFile::from_json(&file.to_json()).unwrap(), file);
}

#[test]
fn patches_a_text_in_place_when_it_fits() {
    let base = english();
    let mut file = text::read_tables(&base).unwrap();
    file.strings.insert("0x33333333".into(), "Shorter".into());
    let patched = text::patch_tables(&base, &file).unwrap();
    assert_eq!(patched.len(), base.len());
    assert_eq!(text::read_tables(&patched).unwrap(), file);
    let untouched = text::patch_tables(&base, &text::read_tables(&base).unwrap()).unwrap();
    assert_eq!(untouched, base);
}

#[test]
fn refuses_text_that_cannot_fit_or_keys_that_do_not_exist() {
    let base = english();
    let mut file = text::read_tables(&base).unwrap();
    file.strings
        .insert("0x22222222".into(), "Much too long".into());
    assert!(matches!(
        text::patch_tables(&base, &file),
        Err(Error::TextDoesNotFit { .. })
    ));
    let mut file = text::read_tables(&base).unwrap();
    file.strings.insert("0x44444444".into(), "new".into());
    assert!(matches!(
        text::patch_tables(&base, &file),
        Err(Error::Json(_))
    ));
    assert!(matches!(
        text::read_tables(&[0u8; 0x1000]),
        Err(Error::NotATable(_))
    ));
}

#[test]
fn shared_storage_must_agree() {
    let blob = b"Shared\0";
    let base = resource(&[(0xAAAA_0001, 0, 0), (0xAAAA_0002, 1, 0)], 2, blob);
    let mut file = text::read_tables(&base).unwrap();
    file.strings.insert("0xaaaa0001".into(), "Edit".into());
    assert!(matches!(
        text::patch_tables(&base, &file),
        Err(Error::SharedText(_))
    ));
    file.strings.insert("0xaaaa0002".into(), "Edit".into());
    let patched = text::patch_tables(&base, &file).unwrap();
    assert_eq!(
        text::read_tables(&patched).unwrap().strings["0xaaaa0002"],
        "Edit"
    );
}

#[test]
fn guesses_languages() {
    assert_eq!(text::guess_language(["こんにちは"]), "ja");
    assert_eq!(text::guess_language(["안녕하세요"]), "ko");
    assert_eq!(text::guess_language(["Привет"]), "ru");
    assert_eq!(text::guess_language(["这是我们的车"]), "zh-Hans");
    assert_eq!(text::guess_language(["這是我們說的車"]), "zh-Hant");
    assert_eq!(
        text::guess_language(["Das ist nicht der Wagen, ich weiß"]),
        "de"
    );
    assert_eq!(text::guess_language(["qqq"]), "xx");
}

#[test]
fn categories_follow_the_measured_roots() {
    assert_eq!(
        catalog::directory_for_root("CContentsOfWeatherXmlFile"),
        "World/Weather"
    );
    assert_eq!(
        catalog::directory_for_root("timecycle_keyframe_data"),
        "World/Timecycle"
    );
    assert_eq!(catalog::directory_for_root("CVehiclePoints"), "Vehicles");
    assert_eq!(catalog::directory_for_root("CTyreDataSet"), "Vehicles");
    assert_eq!(catalog::directory_for_root("rage__fwuiPrefab"), "UI");
    assert_eq!(catalog::directory_for_root("SomethingNew"), "Misc");
    assert_eq!(
        catalog::systems_path("CVehiclePoints", None, "xml"),
        "Systems/Vehicles/CVehiclePoints.xml"
    );
    assert_eq!(
        catalog::systems_path("CVehiclePoints", Some("a b/c"), "xml"),
        "Systems/Vehicles/a_b_c.xml"
    );
    assert_eq!(catalog::text_path("es-419"), "Systems/Text/es-419.json");
}

#[test]
fn every_undecoded_system_gets_a_note() {
    let notes = catalog::note_files();
    assert_eq!(notes.len(), catalog::UNDECODED.len());
    assert!(
        notes.iter().all(
            |(path, text)| path.ends_with("NOT_DECODED.md") && text.contains("What is missing")
        )
    );
}

#[test]
fn references_and_parent_paths() {
    assert_eq!(
        catalog::classify_reference("$(common)/a/b"),
        Reference::Common("a/b")
    );
    assert_eq!(
        catalog::classify_reference("platform:/levels/x"),
        Reference::Levels("x")
    );
    assert_eq!(
        catalog::classify_reference("platform:/other"),
        Reference::Platform("other")
    );
    assert_eq!(
        catalog::classify_reference("common:/x"),
        Reference::CommonScheme("x")
    );
    assert_eq!(
        catalog::classify_reference("dir/file.meta"),
        Reference::Relative("dir/file.meta")
    );
    assert_eq!(
        catalog::parent_path_key("../Item[@key='Door_A']"),
        Some("Door_A")
    );
    assert_eq!(catalog::parent_path_key("Item[@key='x']"), None);
}

fn rbf0_bytes() -> Vec<u8> {
    let mut b = b"RBF0\xFB\xFF\x01\x00".to_vec();
    b.extend([0, 0, 4, 0]);
    b.extend(b"Root");
    b.extend([0, 0, 0, 0, 0, 0, 0xFF, 0xFF]);
    b
}

#[test]
fn export_and_import_loose_files() {
    let bytes = rbf0_bytes();
    assert_eq!(convert::sniff(&bytes), Kind::Rbf0);
    let out = convert::export(&bytes, None).unwrap();
    assert_eq!(out.path, "Systems/Misc/Root.xml");
    assert_eq!(convert::import(&out.data, None).unwrap(), bytes);

    let plain =
        b"\xEF\xBB\xBF<?xml version=\"1.0\"?>\n<!-- c -->\n<CVehiclePoints><a/></CVehiclePoints>";
    assert_eq!(
        convert::sniff(plain),
        Kind::Xml(Some("CVehiclePoints".into()))
    );
    let out = convert::export(plain, Some("points")).unwrap();
    assert_eq!(out.path, "Systems/Vehicles/points.xml");
    assert_eq!(out.data, plain);
    assert_eq!(convert::import(plain, None).unwrap(), plain);

    let table = english();
    assert_eq!(convert::sniff(&table), Kind::TextTable);
    let out = convert::export(&table, None).unwrap();
    assert_eq!(out.path, "Systems/Text/en.json");
    assert_eq!(convert::import(&out.data, Some(&table)).unwrap(), table);
    assert!(convert::import(&out.data, None).is_err());

    assert!(matches!(
        convert::export(&[1, 2, 3], None),
        Err(Error::Unsupported(_))
    ));
    assert!(convert::export(b"RBF0 junk", None).is_err());
}

#[test]
fn xml_text_is_exactly_what_the_tree_says() {
    let doc = rbf0::parse(&rbf0_bytes()).unwrap();
    assert_eq!(
        rbf0_xml::to_xml(&doc),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rbf0 width=\"wide\">\n  <Root/>\n</rbf0>\n"
    );
}
