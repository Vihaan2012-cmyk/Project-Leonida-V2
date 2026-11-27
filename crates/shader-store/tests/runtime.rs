use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use shader_container::Stage;
use shader_container::testutil::{container, dxbc, dxil, xbch};
use shader_store::{Role, Source, SourceStats, Store, StoreSource, key_hex, key_of};
use shader_translate::FloatMode;
use xbox_d3d12::pipeline::{DirectTranslation, Link, PipelineError, ShaderSource, TranslatedStage};

fn folder() -> tempfile::TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap()
}

fn open(folder: &std::path::Path) -> Arc<Store> {
    Arc::new(Store::open(&folder.join("Output").join("leonida.db")).unwrap())
}

fn direct() -> Arc<dyn ShaderSource> {
    Arc::new(DirectTranslation::new(FloatMode::default()))
}

/// A source that answers every ask with an error and counts them.
#[derive(Default)]
struct Refusing(AtomicUsize);

impl ShaderSource for Refusing {
    fn translate(
        &self,
        _: &[u8],
        _: Stage,
        _: &Link,
    ) -> Result<Arc<TranslatedStage>, PipelineError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(PipelineError::Unsupported("the fallback was asked"))
    }
}

/// An FSSM blob with room before its code for the header fields the stages read.
fn blob(patches: &[(usize, u32)], assembly: &str) -> Vec<u8> {
    const CODE_OFFSET: usize = 0x300;
    let mut blob = vec![0u8; CODE_OFFSET + 0x28];
    blob[..4].copy_from_slice(b"FSSM");
    blob[4..6].copy_from_slice(&1u16.to_le_bytes());
    blob[0xC0..0xC4].copy_from_slice(&(CODE_OFFSET as u32).to_le_bytes());
    blob[0xD4..0xD8].copy_from_slice(&0x20u32.to_le_bytes());
    for (at, value) in patches {
        blob[*at..*at + 4].copy_from_slice(&value.to_le_bytes());
    }
    blob.extend_from_slice(&rdna::assemble(assembly, true).unwrap());
    blob
}

fn compute_blob(assembly: &str) -> Vec<u8> {
    blob(
        &[(0x138, 4 << 1), (0x13C, 32), (0x140, 2), (0x144, 1)],
        assembly,
    )
}

const VERTEX: &str = "v_mov_b32_e32 v1, 0
    exp pos0 v1, v1, v1, v1 done
    exp param0 v1, v1, v1, v1
    s_endpgm";

fn vertex_blob() -> Vec<u8> {
    let mut patches = vec![(0x29C, 1u32)];
    patches.push((0x1DC, 6 | (1 << 8) | (3 << 16)));
    blob(&patches, VERTEX)
}

fn pixel_blob(assembly: &str) -> Vec<u8> {
    blob(
        &[(0x138, 0xF << 16), (0x13C, 2 << 24), (0x248, 0x2)],
        assembly,
    )
}

/// A signature part of `ISG1` or `OSG1` shape: one float element per `(name, index, register)`.
fn signature(elements: &[(&str, u32, u32)]) -> Vec<u8> {
    let mut part = Vec::new();
    part.extend((elements.len() as u32).to_le_bytes());
    part.extend(8u32.to_le_bytes());
    let names_at = 8 + 32 * elements.len();
    let mut names = Vec::new();
    for (name, index, register) in elements {
        let offset = names_at + names.len();
        for word in [0, offset as u32, *index, 0, 3, *register] {
            part.extend(word.to_le_bytes());
        }
        part.extend([0xF, 0xF, 0, 0]);
        part.extend(0u32.to_le_bytes());
        names.extend(name.as_bytes());
        names.push(0);
    }
    part.extend(names);
    part
}

fn with_signatures(kind: u16, blobs: &[Vec<u8>], input: &[u8], output: &[u8]) -> Vec<u8> {
    let entries: Vec<(u32, Vec<u8>)> = blobs
        .iter()
        .map(|b| (shader_container::XBCH_TAG, xbch(b)))
        .collect();
    let program = dxil(kind, &entries);
    dxbc(&[
        (b"DXIL", &program[..]),
        (b"ISG1", input),
        (b"OSG1", output),
        (b"PRIV", b"private"),
    ])
}

/// Equal in everything but the order of the SPIR-V's words: the translator does not emit the same order each time.
fn same(a: &TranslatedStage, b: &TranslatedStage) {
    let normal = |stage: &TranslatedStage| {
        let mut stage = stage.clone();
        stage.spirv.sort_unstable();
        format!("{stage:?}")
    };
    assert_eq!(normal(a), normal(b));
}

fn ask(source: &dyn ShaderSource, bytes: &[u8], stage: Stage, link: &Link) -> Arc<TranslatedStage> {
    source.translate(bytes, stage, link).unwrap()
}

fn stats(source: &StoreSource) -> (u64, u64, u64, u64) {
    let SourceStats {
        from_store,
        translated,
        fallback,
        store_errors,
        ..
    } = source.stats();
    (from_store, translated, fallback, store_errors)
}

#[test]
fn a_stage_from_the_store_is_the_stage_translating_directly_gives() {
    let dir = folder();
    let bare = compute_blob("s_mov_b32 s20, 1\ns_endpgm");
    let wrapped = container(5, &[compute_blob("s_mov_b32 s20, 2\ns_endpgm")]);
    let vertex = with_signatures(
        1,
        &[vertex_blob()],
        &signature(&[("POSITION", 0, 0)]),
        &signature(&[("TEXCOORD", 0, 1)]),
    );
    let pixel = with_signatures(
        0,
        &[pixel_blob("s_endpgm")],
        &signature(&[("TEXCOORD", 0, 0)]),
        &signature(&[]),
    );
    let cases = [
        (&bare, Stage::Compute),
        (&wrapped, Stage::Compute),
        (&vertex, Stage::Vertex),
        (&pixel, Stage::Pixel),
    ];
    let link = Link::default();
    let reference = direct();
    {
        let source = StoreSource::new(open(dir.path()), direct());
        for (bytes, stage) in cases {
            same(
                &ask(&source, bytes, stage, &link),
                &ask(&*reference, bytes, stage, &link),
            );
        }
        assert_eq!(stats(&source), (0, 4, 0, 0));
    }
    let source = StoreSource::new(open(dir.path()), Arc::new(Refusing::default()));
    for (bytes, stage) in cases {
        same(
            &ask(&source, bytes, stage, &link),
            &ask(&*reference, bytes, stage, &link),
        );
    }
    assert_eq!(stats(&source), (4, 0, 0, 0));
    assert_eq!(source.store().len().unwrap(), 4);
}

#[test]
fn a_stage_still_in_use_is_not_read_again() {
    let dir = folder();
    let source = StoreSource::new(open(dir.path()), direct());
    let bytes = compute_blob("s_endpgm");
    let first = ask(&source, &bytes, Stage::Compute, &Link::default());
    let second = ask(&source, &bytes, Stage::Compute, &Link::default());
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(source.stats().reused, 1);
    drop((first, second));
    ask(&source, &bytes, Stage::Compute, &Link::default());
    assert_eq!(source.stats().from_store, 1);
}

#[test]
fn a_pixel_shader_linked_to_its_vertex_stage_is_kept_apart_from_the_unlinked_one() {
    let dir = folder();
    let vertex = with_signatures(
        1,
        &[vertex_blob()],
        &signature(&[("POSITION", 0, 0)]),
        &signature(&[("TEXCOORD", 0, 1)]),
    );
    let pixel = with_signatures(
        0,
        &[pixel_blob("s_endpgm")],
        &signature(&[("TEXCOORD", 0, 0)]),
        &signature(&[]),
    );
    let reference = direct();
    let previous = ask(&*reference, &vertex, Stage::Vertex, &Link::default());
    assert_eq!(previous.exports.len(), 1);
    let linked = Link {
        previous: Some(previous),
        first_half: false,
    };
    let source = StoreSource::new(open(dir.path()), Arc::new(Refusing::default()));
    let wired = ask(&source, &pixel, Stage::Pixel, &linked);
    same(&wired, &ask(&*reference, &pixel, Stage::Pixel, &linked));
    let loose = ask(&source, &pixel, Stage::Pixel, &Link::default());
    same(
        &loose,
        &ask(&*reference, &pixel, Stage::Pixel, &Link::default()),
    );
    assert_eq!(source.store().len().unwrap(), 2);
    drop((wired, loose));
    let again = ask(&source, &pixel, Stage::Pixel, &linked);
    same(&again, &ask(&*reference, &pixel, Stage::Pixel, &linked));
    assert_eq!(source.stats().from_store, 1);
}

#[test]
fn what_the_store_cannot_hold_goes_to_the_fallback() {
    let dir = folder();
    let refusing = Arc::new(Refusing::default());
    let source = StoreSource::new(open(dir.path()), refusing.clone());
    let mut payload = b"FSPX".to_vec();
    payload.extend([0u8; 60]);
    let error = source
        .translate(&payload, Stage::Compute, &Link::default())
        .unwrap_err();
    assert!(matches!(error, PipelineError::Unsupported(_)));
    let pixel_in_vertex_slot = container(0, &[pixel_blob("s_endpgm")]);
    assert!(
        source
            .translate(&pixel_in_vertex_slot, Stage::Vertex, &Link::default())
            .is_err()
    );
    assert_eq!(refusing.0.load(Ordering::SeqCst), 2);
    assert_eq!(source.store().len().unwrap(), 0);
}

#[test]
fn a_program_that_does_not_translate_is_an_error_each_time_and_is_translated_once() {
    let dir = folder();
    let source = StoreSource::new(open(dir.path()), Arc::new(Refusing::default()));
    let bad = compute_blob("s_endpgm")[..0x328].to_vec();
    for _ in 0..2 {
        let error = source
            .translate(&bad, Stage::Compute, &Link::default())
            .unwrap_err();
        assert!(matches!(
            error,
            PipelineError::Translate(_) | PipelineError::Bytecode(_)
        ));
    }
    assert_eq!(source.stats().translated, 0);
    assert_eq!(source.stats().fallback, 0);
    let stored = source
        .store()
        .current(&Source {
            stage: Some(Stage::Compute),
            role: Role::Main,
            fssm: bad,
            interface: None,
        })
        .unwrap();
    assert!(stored.is_some_and(|r| r.error.is_some()));
}

#[test]
fn an_override_replaces_the_translation_the_pipeline_gets() {
    let dir = folder();
    let overrides = dir.path().join("overrides");
    std::fs::create_dir_all(&overrides).unwrap();
    let bytes = compute_blob("s_endpgm");
    let words: Vec<u32> = vec![0x0723_0203, 7, 7, 7];
    let packed: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    std::fs::write(
        overrides.join(format!("{}.spv", key_hex(&key_of(&bytes)))),
        packed,
    )
    .unwrap();
    let store = Arc::new(
        Store::open(&dir.path().join("Output").join("leonida.db"))
            .unwrap()
            .with_overrides(vec![overrides]),
    );
    let source = StoreSource::new(store, direct());
    let stage = ask(&source, &bytes, Stage::Compute, &Link::default());
    assert_eq!(stage.spirv, words);
}

#[test]
fn whether_a_program_reads_its_own_address_is_kept_beside_its_record() {
    let dir = folder();
    let plain = compute_blob("s_endpgm");
    let reading = compute_blob("s_getpc_b64 s[4:5]\ns_endpgm");
    let source = StoreSource::new(open(dir.path()), direct());
    let reference = direct();
    for bytes in [&plain, &reading] {
        let stage = ask(&source, bytes, Stage::Compute, &Link::default());
        let wanted = ask(&*reference, bytes, Stage::Compute, &Link::default());
        assert_eq!(stage.uses_code_base, wanted.uses_code_base);
    }
    assert!(!ask(&source, &plain, Stage::Compute, &Link::default()).uses_code_base);
    assert!(ask(&source, &reading, Stage::Compute, &Link::default()).uses_code_base);
    assert_eq!(
        source.store().uses_code_base(&key_of(&reading)).unwrap(),
        Some(true)
    );
    assert_eq!(
        source.store().uses_code_base(&key_of(&plain)).unwrap(),
        Some(false)
    );
}

#[test]
fn a_program_stored_without_the_fact_gets_it_worked_out_once() {
    let dir = folder();
    let store = open(dir.path());
    let reading = compute_blob("s_getpc_b64 s[4:5]\ns_endpgm");
    let program = Source {
        stage: Some(Stage::Compute),
        role: Role::Main,
        fssm: reading.clone(),
        interface: None,
    };
    store.program(&program).unwrap();
    assert_eq!(store.uses_code_base(&program.key()).unwrap(), None);
    let source = StoreSource::new(store.clone(), Arc::new(Refusing::default()));
    assert!(ask(&source, &reading, Stage::Compute, &Link::default()).uses_code_base);
    assert_eq!(source.stats().from_store, 1);
    assert_eq!(store.uses_code_base(&program.key()).unwrap(), Some(true));
}

#[test]
fn the_vertex_half_of_a_merged_shader_is_a_program_of_its_own() {
    let dir = folder();
    let bytes = container(1, &[blob(&[], "s_endpgm"), vertex_blob()]);
    let half = Link {
        previous: None,
        first_half: true,
    };
    let reference = direct();
    let source = StoreSource::new(open(dir.path()), direct());
    for link in [&half, &Link::default()] {
        let stage = ask(&source, &bytes, Stage::Vertex, link);
        same(&stage, &ask(&*reference, &bytes, Stage::Vertex, link));
        assert_eq!(stage.first_half, link.first_half);
    }
    assert_eq!(source.store().len().unwrap(), 2);
    let missing = container(1, &[vertex_blob()]);
    assert!(source.translate(&missing, Stage::Vertex, &half).is_err());
}
