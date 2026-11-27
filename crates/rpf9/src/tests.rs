use sha2::{Digest, Sha256};
use shader_container::testutil::{container, fssm, rpf_entry};

use crate::archive::testutil::ArchiveBuilder;
use crate::walk::testutil::{awc, bink2};
use crate::*;

const CHUNK: usize = 0x10000;

/// A `0DRP` system page of `len` bytes: the header, a `q38` table of `records` `(hash, target, size)` and filler.
fn page_directory(len: usize, table_at: usize, records: &[(u32, u32, u32)]) -> Vec<u8> {
    let mut page = vec![0u8; len];
    page[8..12].copy_from_slice(&0x5000_0000u32.to_le_bytes());
    page[0x10..0x14].copy_from_slice(b"0DRP");
    page[0x14..0x18].copy_from_slice(&122u32.to_le_bytes());
    page[0x38..0x40].copy_from_slice(&(0x5000_0000u64 + table_at as u64).to_le_bytes());
    page[0x4C..0x4E].copy_from_slice(&(records.len() as u16).to_le_bytes());
    for (index, &(hash, target, size)) in records.iter().enumerate() {
        let at = table_at + index * 16;
        if at + 16 <= len {
            page[at..at + 4].copy_from_slice(&hash.to_le_bytes());
            page[at + 4..at + 8].copy_from_slice(&size.to_le_bytes());
            page[at + 8..at + 16]
                .copy_from_slice(&(0x5000_0000u64 + u64::from(target)).to_le_bytes());
        }
    }
    for (index, byte) in page[0x60..0x100].iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(7) | 1;
    }
    page
}

/// A system page: zero head, a `0x5xxxxxxx` pointer, names after it.
fn system_page(len: usize, names: &[&str]) -> Vec<u8> {
    let mut page = vec![0u8; len];
    page[8..12].copy_from_slice(&0x5000_0040u32.to_le_bytes());
    let mut at = 0x40;
    for name in names {
        page[at..at + name.len()].copy_from_slice(name.as_bytes());
        at += name.len() + 1;
    }
    page
}

fn graphics_page(len: usize, seed: u8) -> Vec<u8> {
    let mut page: Vec<u8> = (0..len)
        .map(|index| (index as u8).wrapping_mul(seed) ^ 0x5A)
        .collect();
    page[..4].copy_from_slice(&[0x42, 0x40, 0, 0]);
    page
}

fn scan(archive: &Archive) -> Vec<Resource> {
    let mut found = Vec::new();
    archive.scan(|resource| found.push(resource));
    found
}

#[test]
fn a_page_directory_joins_its_continuation() {
    let head = page_directory(
        2 * CHUNK,
        0x300,
        &[
            (0x101, 0x60, 0x200),
            (0x10, 0x2_0000, 100),
            (0x7, 0x400, 0x100),
        ],
    );
    let tail: Vec<u8> = (0..110u8).collect();
    let mut builder = ArchiveBuilder::new();
    builder.start_resource();
    let first = builder.stream(&head);
    let second = builder.stream(&tail);
    let archive = builder.finish(64);
    let found = scan(&archive);
    assert_eq!(found.len(), 1);
    let resource = &found[0];
    assert_eq!(resource.kind, Kind::PageDir);
    assert_eq!(&resource.magic, b"0DRP");
    assert_eq!(resource.version, 122);
    assert_eq!(resource.offset, first);
    let last = resource.layout.segments[1];
    assert_eq!(last.offset, second);
    assert_eq!(resource.stored, second + u64::from(last.stored) - first);
    assert_eq!(resource.size, (head.len() + tail.len()) as u64);
    assert!(resource.layout.complete);
    assert_eq!(resource.layout.segments.len(), 2);
    assert_eq!(resource.layout.virtual_size, resource.size);
    let data = archive.read(resource).unwrap();
    assert_eq!(data.virtual_data.len(), head.len() + tail.len());
    assert_eq!(&data.virtual_data[..head.len()], &head[..]);
    assert_eq!(data.at_pointer(0x5002_0000).unwrap(), &tail[..]);
    assert_eq!(data.at_pointer(0x6000_0000), Some(&[][..]));
    assert!(data.at_pointer(0x7000_0000).is_none());
}

#[test]
fn a_page_directory_table_in_the_continuation_is_found() {
    let head = page_directory(CHUNK, CHUNK + 0x20, &[(1, 0x60, 0x80), (2, 0x1_0020, 0x20)]);
    let mut tail = vec![0u8; 0x100];
    let table = 0x20;
    tail[table..table + 4].copy_from_slice(&1u32.to_le_bytes());
    tail[table + 4..table + 8].copy_from_slice(&0x80u32.to_le_bytes());
    tail[table + 8..table + 16].copy_from_slice(&0x5000_0060u64.to_le_bytes());
    tail[table + 16..table + 20].copy_from_slice(&2u32.to_le_bytes());
    tail[table + 20..table + 24].copy_from_slice(&0x20u32.to_le_bytes());
    tail[table + 24..table + 32].copy_from_slice(&0x5001_0020u64.to_le_bytes());
    let mut builder = ArchiveBuilder::new();
    builder.start_resource();
    builder.stream(&head);
    builder.stream(&tail);
    let found = scan(&builder.finish(0));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, Kind::PageDir);
    assert_eq!(found[0].layout.segments.len(), 2);
    assert!(found[0].layout.complete);
}

#[test]
fn a_page_directory_with_overlapping_records_is_only_a_system_page() {
    let head = page_directory(0x1000, 0x300, &[(1, 0x60, 0x200), (2, 0x100, 0x200)]);
    let mut builder = ArchiveBuilder::new();
    builder.start_resource();
    builder.stream(&head);
    let found = scan(&builder.finish(0));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, Kind::SystemPage);
}

#[test]
fn a_page_directory_missing_its_continuation_is_incomplete() {
    let head = page_directory(0x1000, 0x300, &[(1, 0x60, 0x200), (2, 0x2000, 0x200)]);
    let mut builder = ArchiveBuilder::new();
    builder.start_resource();
    builder.stream(&head);
    let found = scan(&builder.finish(0));
    assert_eq!(found[0].kind, Kind::PageDir);
    assert!(!found[0].layout.complete);
}

#[test]
fn a_model_page_has_a_system_part_a_spill_and_graphics_pages() {
    let system = system_page(CHUNK, &["prop_crate_01.#dr", "ignored"]);
    let spill = vec![0x33u8; 2048];
    let gfx_a = graphics_page(CHUNK, 3);
    let gfx_b = graphics_page(5000, 5);
    let mut builder = ArchiveBuilder::new();
    builder.start_resource();
    builder.stream(&system);
    builder.stream(&spill);
    builder.stream(&gfx_a);
    builder.stream(&gfx_b);
    builder.start_resource();
    let next = builder.stream(&system_page(0x400, &["car_door.#ft"]));
    builder.stream(&graphics_page(0x8000, 9));
    let archive = builder.finish(32);
    let found = scan(&archive);
    assert_eq!(found.len(), 2);
    let model = &found[0];
    assert_eq!(model.kind, Kind::Drawable);
    assert_eq!(model.name.as_deref(), Some("prop_crate_01.#dr"));
    assert_eq!(model.name_hash, Some(joaat(b"prop_crate_01")));
    assert_eq!(model.layout.virtual_size, (CHUNK + 2048) as u64);
    assert_eq!(model.layout.physical_size, (CHUNK + 5000) as u64);
    assert_eq!(model.layout.segments.len(), 4);
    let data = archive.read(model).unwrap();
    assert_eq!(data.virtual_data.len(), CHUNK + 2048);
    assert_eq!(data.physical_data[..4], [0x42, 0x40, 0, 0]);
    assert_eq!(
        data.at_pointer(0x6000_0000 + CHUNK as u64).unwrap(),
        &gfx_b[..]
    );
    assert_eq!(&data.at_pointer(0x5001_0000).unwrap()[..2048], &spill[..]);
    let small = &found[1];
    assert_eq!(small.offset, next);
    assert_eq!(small.kind, Kind::Fragment);
    assert_eq!(small.layout.virtual_size, 0x400);
    assert_eq!(small.layout.physical_size, 0x8000);
}

#[test]
fn a_dictionary_name_selects_the_kind_and_every_name_is_kept() {
    let mut builder = ArchiveBuilder::new();
    builder.start_resource();
    builder.stream(&system_page(
        0x800,
        &["dict_a.#dd", "child_one.#dr", "child_two.#dr"],
    ));
    let found = scan(&builder.finish(0));
    assert_eq!(found[0].kind, Kind::DrawableDictionary);
    assert_eq!(
        found[0].names,
        ["dict_a.#dd", "child_one.#dr", "child_two.#dr"]
    );
}

#[test]
fn chunked_resources_continue_while_streams_are_64_kib() {
    let mut psin = vec![0x11u8; CHUNK];
    psin[..4].copy_from_slice(b"PSIN");
    psin[8..12].copy_from_slice(&261u32.to_le_bytes());
    let middle = vec![0x22u8; CHUNK];
    let last = vec![0x33u8; 777];
    let mut rbf = b"RBF0\xfb\xff\x01\x00record bytes".to_vec();
    rbf.resize(300, 1);
    let xml = b"<?xml version=\"1.0\"?><root/>".to_vec();
    let mut builder = ArchiveBuilder::new();
    builder.start_resource();
    builder.stream(&psin);
    builder.stream(&middle);
    builder.stream(&last);
    builder.start_resource();
    builder.stream(&rbf);
    builder.stream(&xml);
    let found = scan(&builder.finish(0));
    let kinds: Vec<Kind> = found.iter().map(|resource| resource.kind).collect();
    assert_eq!(kinds, [Kind::Psin, Kind::Rbf0, Kind::Xml]);
    assert_eq!(found[0].version, 261);
    assert_eq!(found[0].layout.segments.len(), 3);
    assert_eq!(found[0].size, (2 * CHUNK + 777) as u64);
    assert_eq!(&found[0].magic, b"PSIN");
}

#[test]
fn a_gap_wider_than_one_unit_ends_a_resource() {
    let mut psin = vec![0x11u8; CHUNK];
    psin[..4].copy_from_slice(b"PSIN");
    let mut builder = ArchiveBuilder::new();
    builder.start_resource();
    builder.stream(&psin);
    builder.raw(&[0u8; 32]);
    builder.stream(&[0xC4u8; 90]);
    let found = scan(&builder.finish(0));
    assert_eq!(found.len(), 2);
    assert_eq!(found[1].kind, Kind::Raw);
}

#[test]
fn lone_streams_are_classified_by_their_start() {
    let mut builder = ArchiveBuilder::new();
    builder.start_resource();
    builder.stream(&graphics_page(0x900, 3));
    builder.stream(&[0xC3u8; 40]);
    builder.stream(
        b"readable text that goes on for a good while, longer than sixty-four bytes in all.",
    );
    builder.stream(b"RSC8\x41\x1f\x00\x00\x10\x00\x00\x00\x01\x00\x00\x00");
    let found = scan(&builder.finish(0));
    let kinds: Vec<Kind> = found.iter().map(|resource| resource.kind).collect();
    assert_eq!(
        kinds,
        [Kind::GraphicsPage, Kind::Raw, Kind::Text, Kind::Rsc]
    );
    assert_eq!(found[3].version, 8001);
}

#[test]
fn bink_and_audio_banks_are_resources_of_their_own() {
    let video = bink2(24, 500);
    let bank = awc(3000);
    let mut builder = ArchiveBuilder::new();
    builder.start_resource();
    builder.stream(&[0xC3u8; 40]);
    let video_at = builder.raw(&video);
    let bank_at = builder.raw(&bank);
    let archive = builder.finish(16);
    let found = scan(&archive);
    assert_eq!(found.len(), 3);
    assert_eq!(found[1].kind, Kind::Bink2);
    assert_eq!(found[1].offset, video_at);
    assert_eq!(&found[1].magic, b"KB2n");
    assert_eq!(
        found[1].content_hash,
        <[u8; 32]>::from(Sha256::digest(&video))
    );
    assert_eq!(found[2].kind, Kind::Awc);
    assert_eq!(found[2].offset, bank_at);
    assert_eq!(archive.read(&found[2]).unwrap().into_bytes(), bank);
    assert_eq!(found[2].layout.segments[0].codec, Codec::Raw);
}

#[test]
fn content_hash_covers_the_inflated_bytes_virtual_first() {
    let system = system_page(0x400, &["same.#dr"]);
    let gfx = graphics_page(0x500, 3);
    let mut builder = ArchiveBuilder::new();
    builder.start_resource();
    builder.stream(&system);
    builder.stream(&gfx);
    builder.start_resource();
    builder.stream(&system);
    builder.stream(&gfx);
    let found = scan(&builder.finish(0));
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].content_hash, found[1].content_hash);
    let mut whole = system.clone();
    whole.extend_from_slice(&gfx);
    assert_eq!(
        found[0].content_hash,
        <[u8; 32]>::from(Sha256::digest(&whole))
    );
}

#[test]
fn the_trailer_and_header_are_not_scanned() {
    let mut builder = ArchiveBuilder::new();
    builder.start_resource();
    builder.stream(&[0xC3u8; 40]);
    let archive = builder.finish(4096);
    assert_eq!(archive.header().data_start, 0x40);
    assert_eq!(archive.header().trailer, 4096);
    assert_eq!(scan(&archive).len(), 1);
}

#[test]
fn headers_that_are_not_archives_are_refused() {
    assert!(Archive::from_bytes(b"MZ\x90\x00 not an archive at all".to_vec()).is_err());
    assert!(Archive::from_bytes(b"9FPR".to_vec()).is_err());
    let mut bytes = b"9FPR".to_vec();
    bytes.extend_from_slice(&1_000_000u32.to_le_bytes());
    bytes.resize(64, 0);
    assert!(Archive::from_bytes(bytes).is_err());
}

#[test]
fn reading_detects_corrupt_and_misplaced_segments() {
    let mut builder = ArchiveBuilder::new();
    builder.start_resource();
    builder.stream(&[0xC3u8; 40]);
    let archive = builder.finish(0);
    let mut resource = scan(&archive).remove(0);
    resource.layout.segments[0].size += 1;
    assert!(matches!(
        archive.read(&resource),
        Err(Rpf9Error::Corrupt { .. } | Rpf9Error::SizeMismatch { .. })
    ));
    resource.layout.segments[0].offset = archive.len();
    assert!(matches!(
        archive.read(&resource),
        Err(Rpf9Error::OutOfRange { .. })
    ));
}

#[test]
fn zstd_segments_are_read() {
    use ruzstd::encoding::{CompressionLevel, compress_to_vec};
    let plain: Vec<u8> = (0..5000u32).map(|n| (n % 251) as u8).collect();
    let packed = compress_to_vec(&plain[..], CompressionLevel::Fastest);
    let mut builder = ArchiveBuilder::new();
    builder.start_resource();
    let at = builder.raw(&packed);
    let archive = builder.finish(0);
    let resource = Resource {
        archive: 0,
        offset: at,
        stored: packed.len() as u64,
        size: plain.len() as u64,
        kind: Kind::Raw,
        magic: [0; 4],
        version: 0,
        layout: Layout {
            virtual_size: plain.len() as u64,
            physical_size: 0,
            segments: vec![Segment {
                offset: at,
                stored: packed.len() as u32,
                size: plain.len() as u32,
                codec: Codec::Zstd,
                space: Space::Virtual,
            }],
            complete: true,
        },
        name_hash: None,
        name: None,
        names: Vec::new(),
        content_hash: [0; 32],
    };
    assert_eq!(archive.read(&resource).unwrap().virtual_data, plain);
}

#[test]
fn shader_records_are_resources_with_names() {
    let ps = container(0, &[fssm(false, &[0xBF80_0000])]);
    let vs = container(1, &[fssm(true, &[0xBF80_0000])]);
    let mut bytes = b"9FPR".to_vec();
    bytes.extend_from_slice(&0x40u32.to_le_bytes());
    bytes.resize(0x40, 0);
    bytes.push(0);
    let first = bytes.len() as u64;
    bytes.extend(rpf_entry("PS_Opaque_f00d", 3, 7, &ps));
    bytes.extend_from_slice(b"\0\0padding\0");
    bytes.extend(rpf_entry("VS_Quad_beef", 3, 8, &vs));
    let archive = Archive::from_bytes(bytes).unwrap();
    let mut found = Vec::new();
    let report = archive.scan_shaders(|resource| found.push(resource));
    assert_eq!(report.resources, 2);
    assert_eq!(found[0].kind, Kind::Shader);
    assert_eq!(found[0].offset, first);
    assert_eq!(found[0].name.as_deref(), Some("PS_Opaque_f00d"));
    assert_eq!(found[0].name_hash, Some(joaat(b"ps_opaque_f00d")));
    assert_eq!(&found[0].magic, b"DXBC");
    assert_eq!(archive.read(&found[0]).unwrap().into_bytes(), ps);
    assert_eq!(archive.read(&found[1]).unwrap().into_bytes(), vs);
    assert_eq!(found[1].content_hash, <[u8; 32]>::from(Sha256::digest(&vs)));
}

fn indexed(builder: ArchiveBuilder, name: &str, index: &Index) -> (u32, Archive) {
    let archive = builder.finish(0);
    let id = index.begin_archive(name, archive.len()).unwrap();
    let mut found = Vec::new();
    let report = archive.scan(|mut resource| {
        resource.archive = id;
        found.push(resource);
    });
    index.add(&found).unwrap();
    index
        .finish_archive(id, name, archive.len(), &report)
        .unwrap();
    (id, archive)
}

fn sample_archive(extra: u8) -> ArchiveBuilder {
    let mut builder = ArchiveBuilder::new();
    builder.start_resource();
    builder.stream(&system_page(0x400, &["Prop_Crate_01.#dr"]));
    builder.stream(&graphics_page(0x500, 3));
    builder.start_resource();
    builder.stream(&system_page(0x400, &["car_door.#ft"]));
    builder.stream(&graphics_page(0x500, 5));
    builder.start_resource();
    builder.stream(&[extra; 100]);
    builder.start_resource();
    builder.stream(b"<?xml version=\"1.0\"?><doc>hello</doc>");
    builder
}

#[test]
fn the_index_answers_every_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let index = Index::open(&dir.path().join("sub").join("leonida.db")).unwrap();
    let (first, _) = indexed(sample_archive(0xC3), "pack/one.rpf", &index);
    let (second, _) = indexed(sample_archive(0xC3), "two.rpf", &index);
    assert_eq!((first, second), (1, 2));

    let counts = index.counts().unwrap();
    assert_eq!(counts[&Kind::Drawable], 2);
    assert_eq!(counts[&Kind::Fragment], 2);
    assert_eq!(counts[&Kind::Raw], 2);
    assert_eq!(counts[&Kind::Xml], 2);

    let drawables = index.by_kind(Kind::Drawable, 10).unwrap();
    assert_eq!(drawables.len(), 2);
    assert_eq!((drawables[0].archive, drawables[1].archive), (1, 2));
    assert_eq!(index.by_kind(Kind::Drawable, 1).unwrap().len(), 1);

    let hash = joaat(b"prop_crate_01");
    assert_eq!(index.by_name_hash(hash).unwrap().len(), 2);
    assert_eq!(index.by_name("PROP_CRATE_01.#DR").unwrap().len(), 2);
    assert_eq!(index.by_name("prop_crate_01").unwrap().len(), 2);
    assert!(index.by_name("prop_crate").unwrap().is_empty());

    let same = index.by_content_hash(&drawables[0].content_hash).unwrap();
    assert_eq!(same.len(), 2);
    assert_ne!(same[0].archive, same[1].archive);
    let again = index
        .get(drawables[1].archive, drawables[1].offset)
        .unwrap();
    assert_eq!(again.as_ref(), Some(&drawables[1]));
    assert!(index.get(9, 9).unwrap().is_none());

    let found = |find: &str, kind: Option<Kind>| {
        index
            .list(&Query {
                kind,
                find: Some(find.to_string()),
                limit: None,
            })
            .unwrap()
    };
    assert_eq!(found("crate", None).len(), 2);
    assert_eq!(found("DOOR", Some(Kind::Fragment)).len(), 2);
    assert!(found("door", Some(Kind::Drawable)).is_empty());
    assert_eq!(found("pack/one", None).len(), 4);
    assert_eq!(found(&format!("{hash:08x}"), None).len(), 2);
    assert_eq!(found(&hex(&drawables[0].content_hash), None).len(), 2);
    let limited = index
        .list(&Query {
            kind: None,
            find: None,
            limit: Some(3),
        })
        .unwrap();
    assert_eq!(limited.len(), 3);

    let archives = index.archives().unwrap();
    assert_eq!(archives.len(), 2);
    assert!(archives.iter().all(|info| info.done && info.resources == 4));
    assert_eq!(archives[0].name, "pack/one.rpf");
}

#[test]
fn reindexing_an_archive_replaces_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("leonida.db");
    {
        let index = Index::open(&path).unwrap();
        let (id, _) = indexed(sample_archive(0xC3), "one.rpf", &index);
        assert_eq!(id, 1);
    }
    let index = Index::open(&path).unwrap();
    let (id, _) = indexed(sample_archive(0xC4), "one.rpf", &index);
    assert_eq!(id, 1);
    assert_eq!(index.archives().unwrap().len(), 1);
    assert_eq!(index.counts().unwrap()[&Kind::Raw], 1);
    let raw = index.by_kind(Kind::Raw, 10).unwrap();
    let again = index.by_content_hash(&raw[0].content_hash).unwrap();
    assert_eq!(again.len(), 1);
    let old = Sha256::digest(vec![0xC3u8; 100]);
    assert!(index.by_content_hash(&old.into()).unwrap().is_empty());
}

#[test]
fn kinds_parse_by_name_and_magic() {
    assert_eq!(Kind::parse("PageDir"), Some(Kind::PageDir));
    assert_eq!(Kind::parse("0DRP"), Some(Kind::PageDir));
    assert_eq!(Kind::parse("kb2n"), Some(Kind::Bink2));
    assert_eq!(Kind::parse("#dr"), Some(Kind::Drawable));
    assert_eq!(Kind::parse("nonsense"), None);
    for kind in Kind::ALL {
        assert_eq!(Kind::from_code(kind.code()), Some(kind));
        assert_eq!(Kind::parse(kind.name()), Some(kind));
    }
}

/// One small real archive, when `LEONIDA_GAME_DIR` points at the game package.
#[test]
fn a_real_archive_scans_into_whole_readable_resources() {
    let Some(dir) = std::env::var_os("LEONIDA_GAME_DIR") else {
        return;
    };
    let path = game_package::package_path(std::path::Path::new(&dir), "script.rpf");
    if !path.is_file() {
        return;
    }
    let archive = Archive::open(&path).unwrap();
    let mut found = Vec::new();
    let report = archive.scan(|resource| found.push(resource));
    assert!(report.walk.streams > 0, "no streams found");
    assert_eq!(report.resources as usize, found.len());
    assert!(
        report.walk.stored_bytes + report.walk.zero_padding + report.walk.unread_bytes
            <= archive.len()
    );
    let mut previous_end = 0;
    for resource in &found {
        assert!(resource.offset >= previous_end, "resources overlap");
        previous_end = resource.offset + resource.stored;
        assert!(previous_end <= archive.len());
        assert_eq!(
            resource.size,
            resource
                .layout
                .segments
                .iter()
                .map(|segment| u64::from(segment.size))
                .sum::<u64>()
        );
    }
    for resource in found.iter().step_by(1 + found.len() / 40) {
        let data = archive.read(resource).unwrap();
        assert_eq!(
            data.virtual_data.len() as u64 + data.physical_data.len() as u64,
            resource.size
        );
        let mut whole = data.virtual_data.clone();
        whole.extend_from_slice(&data.physical_data);
        let hash: [u8; 32] = Sha256::digest(&whole).into();
        assert_eq!(hash, resource.content_hash);
    }
}
