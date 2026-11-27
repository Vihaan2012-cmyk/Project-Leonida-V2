use shader_census::{
    Census, Program, SFI0_RESOURCE_HEAP, SFI0_SAMPLER_HEAP, Selection, Use, census_archive,
    census_blob, census_container,
};
use shader_container::Stage;
use shader_container::testutil::{dxbc, dxil, fssm, rpf_entry, xbch};

fn words(source: &str) -> Vec<u32> {
    rdna::assemble(source, false)
        .expect("assembles")
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
        .collect()
}

/// A wave64 FSSM blob for `source` whose header says `user` user SGPRs.
fn blob(source: &str, user: u32) -> Vec<u8> {
    let mut out = fssm(false, &words(source));
    out[0xD4..0xD8].copy_from_slice(&(user << 16).to_le_bytes());
    out
}

const PIXEL: &str = "s_mov_b32 s20, s8\ns_mov_b32 s21, 0\ns_load_dwordx4 s[4:7], s[20:21], 0x0\n\
                     s_add_u32 s22, s10, s11\ns_mov_b32 m0, s12";

fn program(name: &str, stage: &str, code: &str, user: u32) -> Program {
    census_blob(name, stage, false, &blob(code, user)).expect("a program")
}

#[test]
fn a_program_blob_reports_its_reads_uses_and_header_count() {
    let p = program("PS_Test_01234567_Wrapped", "ps", PIXEL, 12);
    assert_eq!(p.header_user_sgprs, 12);
    assert!(p.wave64);
    assert_eq!(p.reads.iter().collect::<Vec<_>>(), vec![8, 10, 11, 12]);
    assert_eq!(p.use_of(8), Use::Pointer32);
    assert_eq!(p.use_of(10), Use::Plain);
    assert_eq!(p.pointers().iter().collect::<Vec<_>>(), vec![8]);
    assert_eq!(p.plain().iter().collect::<Vec<_>>(), vec![10, 11, 12]);
    assert_eq!(p.reads.lowest(), Some(8));
    assert_eq!(p.reads.highest(), Some(12));
    assert_eq!(p.highest_system_read, None);
}

#[test]
fn reads_past_the_user_data_are_system_reads() {
    let p = program("PS_A_01234567_Wrapped", "ps", "s_add_u32 s20, s40, s9", 10);
    assert_eq!(p.reads.iter().collect::<Vec<_>>(), vec![9]);
    assert_eq!(p.highest_system_read, Some(40));
}

#[test]
fn a_blob_without_code_is_skipped() {
    let mut bytes = vec![0u8; 0xD8];
    bytes[..4].copy_from_slice(b"FSSM");
    assert!(census_blob("x", "ps", false, &bytes).is_none());
}

fn container(kind: u16, blobs: &[Vec<u8>], extra: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let entries: Vec<(u32, Vec<u8>)> = blobs
        .iter()
        .map(|b| (shader_container::XBCH_TAG, xbch(b)))
        .collect();
    let dxil = dxil(kind, &entries);
    let mut parts: Vec<(&[u8; 4], &[u8])> = vec![(b"DXIL", &dxil[..])];
    for (tag, data) in extra {
        parts.push((tag, &data[..]));
    }
    dxbc(&parts)
}

#[test]
fn container_parts_give_signature_and_heap_flags() {
    let flags = (SFI0_RESOURCE_HEAP | SFI0_SAMPLER_HEAP)
        .to_le_bytes()
        .to_vec();
    let dxbc = container(
        0,
        &[blob(PIXEL, 12)],
        &[(b"SFI0", flags), (b"RTS0", vec![0; 24])],
    );
    let found = census_container("PS_A_01234567_Wrapped", Some(Stage::Pixel), &dxbc);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].stage, "ps");
    assert!(found[0].has_rts0);
    assert!(found[0].heap_resource() && found[0].heap_sampler());

    let plain = container(0, &[blob(PIXEL, 12)], &[]);
    let found = census_container("PS_A_01234567_Wrapped", Some(Stage::Pixel), &plain);
    assert!(!found[0].has_rts0);
    assert!(!found[0].heap_resource());
    assert_eq!(found[0].sfi0, None);
}

#[test]
fn a_vertex_container_with_two_programs_has_a_first_half() {
    let first = blob("s_add_u32 s20, s17, s3", 15);
    let main = blob("s_add_u32 s20, s9, s3", 12);
    let dxbc = container(1, &[first, main], &[]);
    let found = census_container("VS_A_01234567_Wrapped", Some(Stage::Vertex), &dxbc);
    assert_eq!(found.len(), 2);
    assert!(found[0].first_half && !found[1].first_half);
    assert_eq!(found[0].reads.iter().collect::<Vec<_>>(), vec![3, 17]);
    assert_eq!(found[1].reads.iter().collect::<Vec<_>>(), vec![3, 9]);

    let census = Census::of(found);
    assert!(census.stage("vs").is_some());
    assert!(census.stage("vs-first-half").is_some());
}

#[test]
fn the_archive_scan_honours_stage_limit_and_stride() {
    let mut archive = Vec::new();
    for (i, (name, kind)) in [("A", 0u16), ("B", 1), ("C", 0), ("D", 0)]
        .iter()
        .enumerate()
    {
        let code = format!("s_add_u32 s20, s{}, s9", 8 + i);
        let dxbc = container(*kind, &[blob(&code, 12)], &[]);
        archive.extend(rpf_entry(
            &format!("{name}_Shader_0000000{i}"),
            1,
            i as u32,
            &dxbc,
        ));
    }
    let all = census_archive(&archive, &Selection::default());
    assert_eq!(all.len(), 4);
    let pixel = |sel: Selection| census_archive(&archive, &sel);
    let only_ps = pixel(Selection {
        stages: vec!["ps".into()],
        ..Selection::default()
    });
    assert_eq!(only_ps.len(), 3);
    let limited = pixel(Selection {
        stages: vec!["ps".into()],
        limit: Some(2),
        every: 1,
    });
    assert_eq!(limited.len(), 2);
    let strided = pixel(Selection {
        stages: vec!["ps".into()],
        limit: None,
        every: 2,
    });
    assert_eq!(strided.len(), 2);
}

#[test]
fn the_summary_counts_programs_per_register() {
    let programs = vec![
        program("PS_A_aaaaaaaa_Wrapped", "ps", PIXEL, 12),
        program("PS_B_bbbbbbbb_Wrapped", "ps", PIXEL, 13),
        program("PS_C_cccccccc_Wrapped", "ps", "s_add_u32 s20, s9, s9", 10),
    ];
    let census = Census::of(programs);
    let ps = census.stage("ps").unwrap();
    assert_eq!(ps.programs, 3);
    assert_eq!(ps.rows[8].read, 2);
    assert_eq!(ps.rows[8].load_ptr32, 2);
    assert_eq!(ps.rows[9].read, 1);
    assert_eq!(ps.rows[9].plain, 1);
    assert_eq!(ps.lowest_read[&8], 2);
    assert_eq!(ps.lowest_read[&9], 1);
    assert_eq!(ps.lowest_pointer[&8], 2);
    assert_eq!(ps.header_user_sgprs[&12], 1);
    assert_eq!(ps.header_minus_highest_read[&0], 2);
    assert_eq!(ps.header_minus_highest_read[&-1], 1);
    assert_eq!(ps.reads_at_or_above_header, 1);
    assert_eq!(ps.pointer_at_s0, 0);
    let text = census.report();
    assert!(text.contains("== ps == 3 programs"), "{text}");
    assert!(text.contains("lowest user SGPR read:    8:2 9:1"), "{text}");
}

#[test]
fn programs_sharing_a_hash_show_a_window_shift() {
    let pixel = program(
        "PS_A_0badf00d_Wrapped",
        "ps",
        "s_mov_b32 s20, s8\ns_mov_b32 s21, 0\ns_load_dwordx4 s[4:7], s[20:21], 0x0\n\
         s_add_u32 s22, s9, s9",
        11,
    );
    let mesh = program(
        "MS_A_0badf00d_Wrapped",
        "ms",
        "s_mov_b32 s20, s16\ns_mov_b32 s21, 0\ns_load_dwordx4 s[4:7], s[20:21], 0x0\n\
         s_add_u32 s22, s17, s17",
        11,
    );
    let other = program("PS_B_00000001_Wrapped", "ps", "s_add_u32 s20, s8, s9", 10);
    let census = Census::of(vec![pixel, mesh, other]);
    let t = &census.techniques;
    assert_eq!(t.distinct_hashes, 2);
    let pair = t.pair("ps", "ms").unwrap();
    assert_eq!(
        (pair.hashes, pair.equal_header, pair.unequal_header),
        (1, 1, 0)
    );
    let best = pair.best().unwrap();
    assert_eq!((best.shift, best.agree, best.differ), (8, 2, 0));
    assert_eq!(pair.at(0).unwrap().agree, 0);
    assert!(census.report().contains("ps -> ms: 1 hashes"));
}

/// The real archive, when the game folder is given: invariants the census measured on the whole of it.
#[test]
fn real_sample_reads_start_where_root_rs_says() {
    let Ok(game) = std::env::var("LEONIDA_GAME_DIR") else {
        return;
    };
    let path = game_package::package_path(std::path::Path::new(&game), "shaders.rpf");
    let Ok(file) = std::fs::File::open(&path) else {
        return;
    };
    let archive = game_package::map_file(&file).expect("maps");
    let programs = census_archive(
        &archive,
        &Selection {
            stages: vec![],
            limit: Some(300),
            every: 90,
        },
    );
    assert!(programs.len() > 100, "{} programs", programs.len());
    let census = Census::of(programs);
    for stage in &census.stages {
        let name = stage.stage.as_str();
        assert_eq!(stage.with_rts0, 0, "{name}");
        match name {
            "cs" => {
                assert_eq!(stage.header_equals_rsrc2, stage.with_rsrc2);
                assert!(stage.lowest_pointer.keys().all(|&r| r >= 4), "{name}");
            }
            _ => assert!(stage.lowest_pointer.keys().all(|&r| r >= 8), "{name}"),
        }
        assert_eq!(stage.pointer_at_s0, 0, "{name}");
    }
    if let Some(ms) = census.stage("ms") {
        assert!(ms.lowest_pointer.keys().all(|&r| r >= 16));
    }
}
