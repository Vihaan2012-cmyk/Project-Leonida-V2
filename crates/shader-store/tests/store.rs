use std::path::{Path, PathBuf};

use shader_container::Stage;
use shader_container::testutil::{container, fssm};
use shader_store::{
    Built, Record, Role, SCHEMA_VERSION, Source, Store, StoreError, container_programs, key_hex,
    translator_version,
};

const NOP: u32 = 0xBF80_0000;
const ENDPGM: u32 = 0xBF81_0000;

fn folder() -> tempfile::TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap()
}

/// A compute program of `nops` no-ops, so each count is a different shader.
fn compute(nops: usize) -> Source {
    const CODE_OFFSET: usize = 0x200;
    let mut blob = vec![0u8; CODE_OFFSET + 0x28];
    blob[..4].copy_from_slice(b"FSSM");
    blob[4..6].copy_from_slice(&1u16.to_le_bytes());
    blob[0xC0..0xC4].copy_from_slice(&(CODE_OFFSET as u32).to_le_bytes());
    blob[0xD4..0xD8].copy_from_slice(&0x20u32.to_le_bytes());
    for (at, value) in [(0x138, 0u32), (0x13C, 8), (0x140, 1), (0x144, 1)] {
        blob[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    for word in std::iter::repeat_n(NOP, nops).chain([ENDPGM]) {
        blob.extend_from_slice(&word.to_le_bytes());
    }
    Source {
        stage: Some(Stage::Compute),
        role: Role::Main,
        fssm: blob,
        interface: None,
    }
}

fn open(folder: &Path) -> Store {
    Store::open(&folder.join("Output").join("leonida.db")).unwrap()
}

fn words(seed: u32, count: usize) -> Vec<u8> {
    let mut words = vec![0x0723_0203, seed];
    words.resize(count, seed);
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn put_override(folder: &Path, source: &Source, bytes: &[u8]) -> PathBuf {
    std::fs::create_dir_all(folder).unwrap();
    let path = folder.join(format!("{}.spv", key_hex(&source.key())));
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn a_program_is_translated_once_and_read_back_after_reopening() {
    let dir = folder();
    let source = compute(3);
    let first = {
        let store = open(dir.path());
        assert!(store.current(&source).unwrap().is_none());
        let program = store.program(&source).unwrap();
        assert!(
            matches!(program.built, Ok(Built::Compute(_))),
            "{:?}",
            program.record.error
        );
        assert_eq!(store.len().unwrap(), 1);
        program
    };

    let store = open(dir.path());
    let stored = store.current(&source).unwrap().expect("stored");
    assert_eq!(stored, first.record);
    assert_eq!(stored.translator, translator_version());
    assert_eq!(stored.wave64, Some(false));
    let again = store.program(&source).unwrap();
    assert_eq!(again.built.unwrap().spirv(), first.built.unwrap().spirv());
    assert_eq!(store.len().unwrap(), 1);
}

#[test]
fn a_batch_lands_in_one_transaction_and_keeps_every_record() {
    let dir = folder();
    let store = open(dir.path());
    let batch: Vec<_> = (1..=5)
        .map(|n| {
            let source = compute(n);
            (source.key(), shader_store::translate_program(&source).0)
        })
        .collect();
    store.put_batch(&batch).unwrap();
    assert_eq!(store.len().unwrap(), 5);
    let mut seen = 0;
    store
        .for_each(|key, record| {
            let record = record.unwrap();
            assert!(batch.iter().any(|(k, r)| *k == key && *r == record));
            seen += 1;
        })
        .unwrap();
    assert_eq!(seen, 5);
}

#[test]
fn a_program_that_cannot_be_translated_is_stored_with_its_error() {
    let dir = folder();
    let store = open(dir.path());
    let bad = Source {
        stage: Some(Stage::Compute),
        role: Role::Main,
        fssm: b"not a shader".to_vec(),
        interface: None,
    };
    let program = store.program(&bad).unwrap();
    let error = program.built.unwrap_err();
    assert!(!error.is_empty());
    let stored = store.current(&bad).unwrap().unwrap();
    assert_eq!(stored.error.as_deref(), Some(error.as_str()));
    assert!(stored.spirv.is_none() && stored.options.is_none());
}

#[test]
fn a_record_of_another_layout_is_an_error_to_read_and_a_miss_to_look_up() {
    let dir = folder();
    let store = open(dir.path());
    let source = compute(2);
    let mut bytes = shader_store::translate_program(&source).0.encode();
    bytes[..2].copy_from_slice(&(SCHEMA_VERSION + 1).to_le_bytes());
    store.put_raw(&source.key(), &bytes).unwrap();

    match store.get(&source.key()) {
        Err(StoreError::Schema { found, expected }) => {
            assert_eq!((found, expected), (SCHEMA_VERSION + 1, SCHEMA_VERSION));
        }
        other => panic!("{other:?}"),
    }
    assert!(store.current(&source).unwrap().is_none());
    let program = store.program(&source).unwrap();
    assert!(program.built.is_ok());
    assert_eq!(store.get(&source.key()).unwrap().unwrap(), program.record);
}

#[test]
fn a_record_of_another_translator_or_context_is_retranslated() {
    let dir = folder();
    let store = open(dir.path());
    let source = compute(2);
    let mut old: Record = shader_store::translate_program(&source).0;
    old.translator ^= 1;
    store.put_batch(&[(source.key(), old)]).unwrap();
    assert!(store.current(&source).unwrap().is_none());

    let fresh = store.program(&source).unwrap().record;
    assert_eq!(fresh.translator, translator_version());
    let mut other = source.clone();
    other.stage = Some(Stage::Mesh);
    assert!(store.current(&other).unwrap().is_none());
}

#[test]
fn a_damaged_record_is_an_error() {
    let dir = folder();
    let store = open(dir.path());
    let source = compute(2);
    let mut bytes = SCHEMA_VERSION.to_le_bytes().to_vec();
    bytes.extend_from_slice(&[0xFF; 3]);
    store.put_raw(&source.key(), &bytes).unwrap();
    assert!(matches!(
        store.get(&source.key()),
        Err(StoreError::Corrupt(_))
    ));
    assert!(matches!(
        store.current(&source),
        Err(StoreError::Corrupt(_))
    ));
}

#[test]
fn an_override_replaces_the_stored_spirv_and_the_first_folder_wins() {
    let dir = folder();
    let source = compute(4);
    let (first, second) = (dir.path().join("a"), dir.path().join("b"));
    let store = open(dir.path()).with_overrides(vec![first.clone(), second.clone()]);
    let generated = store.program(&source).unwrap();
    assert!(generated.overridden_by.is_none());

    let later = put_override(&second, &source, &words(7, 10));
    let program = store.program(&source).unwrap();
    assert_eq!(program.overridden_by.as_deref(), Some(later.as_path()));
    let built = program.built.unwrap();
    assert!(matches!(built, Built::Compute(_)));
    assert_eq!(built.spirv(), words_of(&words(7, 10)));

    let earlier = put_override(&first, &source, &words(9, 12));
    let program = store.program(&source).unwrap();
    assert_eq!(program.overridden_by.as_deref(), Some(earlier.as_path()));
    assert_eq!(program.built.unwrap().spirv(), words_of(&words(9, 12)));

    // The database still holds the generated program.
    let stored = store.get(&source.key()).unwrap().unwrap();
    assert_eq!(stored, generated.record);
}

fn words_of(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
        .collect()
}

#[test]
fn an_unusable_override_is_reported_and_the_generated_program_is_used() {
    let dir = folder();
    let source = compute(5);
    let (first, second) = (dir.path().join("a"), dir.path().join("b"));
    let store = open(dir.path()).with_overrides(vec![first.clone(), second.clone()]);
    let generated = store.program(&source).unwrap().built.unwrap();

    put_override(&second, &source, &words(1, 8));
    put_override(&first, &source, b"\x01\x02\x03");
    let program = store.program(&source).unwrap();
    assert!(program.overridden_by.is_none());
    assert!(program.override_error.is_some());
    assert_eq!(program.built.unwrap().spirv(), generated.spirv());

    let mut no_magic = words(1, 8);
    no_magic[0] = 0;
    put_override(&first, &source, &no_magic);
    assert!(store.program(&source).unwrap().override_error.is_some());
}

#[test]
fn a_container_yields_its_vertex_half_and_main_program() {
    let blobs = vec![fssm(true, &[NOP]), fssm(true, &[NOP, NOP])];
    let dxbc = container(1, &blobs);
    let sources = container_programs(Some(Stage::Vertex), &dxbc).unwrap();
    let roles: Vec<_> = sources.iter().map(|s| s.role).collect();
    assert_eq!(roles, [Role::FirstHalf, Role::Main]);
    assert_eq!(sources[0].fssm, blobs[0]);
    assert_eq!(sources[1].fssm, blobs[1]);

    let single = container(5, &blobs[1..]);
    let sources = container_programs(Some(Stage::Compute), &single).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].role, Role::Main);
}
