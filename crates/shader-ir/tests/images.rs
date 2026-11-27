use std::collections::HashMap;

use shader_ir::{
    Bin, Expr, Function, Image, ImageDim, ImageOp, Machine, Reg, RunError, Stmt, Un, lift,
};

fn lifted(source: &str) -> Function {
    let code = rdna::assemble(source, true).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
    let function = lift(&code, false).unwrap_or_else(|e| panic!("{e}\n{source}"));
    function.verify().unwrap();
    function
}

/// The image read written to each VGPR, and the register each address operand came from.
fn image_writes(function: &Function) -> (Vec<(u8, Image)>, HashMap<u32, Reg>) {
    let mut defined = HashMap::new();
    let mut registers = HashMap::new();
    let mut out = Vec::new();
    for stmt in function.blocks.iter().flat_map(|b| &b.stmts) {
        match stmt {
            Stmt::Let(tmp, Expr::Get(reg)) => {
                registers.insert(*tmp, *reg);
            }
            Stmt::Let(tmp, Expr::Image(image)) => {
                defined.insert(*tmp, (**image).clone());
            }
            Stmt::SetLane(reg, tmp) => {
                if let Some(image) = defined.get(tmp) {
                    out.push((*reg, image.clone()));
                }
            }
            _ => {}
        }
    }
    (out, registers)
}

fn regs(tmps: &[u32], registers: &HashMap<u32, Reg>) -> Vec<Reg> {
    tmps.iter().map(|t| registers[t]).collect()
}

#[test]
fn sample_lz_writes_each_enabled_component_in_order() {
    let function = lifted(
        "image_sample_lz v[0:1], v[4:5], s[0:7], s[8:11] dmask:0x9 dim:SQ_RSRC_IMG_2D\ns_endpgm",
    );
    let (writes, registers) = image_writes(&function);
    assert_eq!(writes.len(), 2);
    assert_eq!((writes[0].0, writes[0].1.component), (0, 0));
    assert_eq!((writes[1].0, writes[1].1.component), (1, 3));
    let image = &writes[0].1;
    assert_eq!(image.op, ImageOp::SampleLodZero);
    assert_eq!(image.dim, ImageDim::D2);
    assert_eq!(regs(&image.coordinates, &registers), [Reg::V(4), Reg::V(5)]);
    assert_eq!(image.lod, None);
    assert_eq!(
        regs(&image.resource, &registers),
        (0..8).map(Reg::S).collect::<Vec<_>>()
    );
    assert_eq!(
        regs(&image.sampler.unwrap(), &registers),
        (8..12).map(Reg::S).collect::<Vec<_>>()
    );
    assert!(writes[1].1.same_instruction(image));
}

#[test]
fn sample_l_reads_the_lod_after_the_coordinates() {
    let function = lifted(
        "image_sample_l v[0:3], v[4:7], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D_ARRAY\ns_endpgm",
    );
    let (writes, registers) = image_writes(&function);
    assert_eq!(writes.len(), 4);
    let image = &writes[3].1;
    assert_eq!(image.op, ImageOp::SampleLod);
    assert_eq!(image.dim, ImageDim::D2Array);
    assert_eq!(
        regs(&image.coordinates, &registers),
        [Reg::V(4), Reg::V(5), Reg::V(6)]
    );
    assert_eq!(registers[&image.lod.unwrap()], Reg::V(7));
}

#[test]
fn gather_returns_four_texels_of_the_masked_channel() {
    let function = lifted(
        "image_gather4_lz v[0:3], v[4:5], s[0:7], s[8:11] dmask:0x2 dim:SQ_RSRC_IMG_2D\ns_endpgm",
    );
    let (writes, _) = image_writes(&function);
    let found: Vec<(u8, u8)> = writes.iter().map(|(r, i)| (*r, i.component)).collect();
    assert_eq!(found, [(0, 0), (1, 1), (2, 2), (3, 3)]);
    assert!(
        writes
            .iter()
            .all(|(_, i)| i.op == ImageOp::Gather { channel: 1 })
    );
}

#[test]
fn loads_take_integer_coordinates_and_an_optional_mip() {
    let function =
        lifted("image_load v[0:3], v[4:5], s[0:7] dmask:0xf dim:SQ_RSRC_IMG_2D unorm\ns_endpgm");
    let (writes, _) = image_writes(&function);
    assert_eq!(writes.len(), 4);
    assert_eq!(writes[0].1.op, ImageOp::Load);
    assert_eq!(writes[0].1.sampler, None);
    assert!(writes[0].1.unnormalized);

    let function =
        lifted("image_load_mip v2, v[4:6], s[0:7] dmask:0x4 dim:SQ_RSRC_IMG_2D\ns_endpgm");
    let (writes, registers) = image_writes(&function);
    assert_eq!(writes.len(), 1);
    assert_eq!((writes[0].0, writes[0].1.component), (2, 2));
    assert_eq!(writes[0].1.op, ImageOp::LoadMip);
    assert_eq!(registers[&writes[0].1.lod.unwrap()], Reg::V(6));
}

#[test]
fn addresses_are_read_before_results_overwrite_them() {
    let function = lifted(
        "image_sample_lz v[0:3], v[0:1], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D\ns_endpgm",
    );
    let stmts: Vec<&Stmt> = function.blocks.iter().flat_map(|b| &b.stmts).collect();
    let first_write = stmts
        .iter()
        .position(|s| matches!(s, Stmt::SetLane(..)))
        .unwrap();
    let last_read = stmts
        .iter()
        .rposition(|s| matches!(s, Stmt::Let(_, Expr::Get(Reg::V(_)))))
        .unwrap();
    assert!(last_read < first_write);
}

#[test]
fn unsupported_forms_are_named_in_coverage() {
    for (source, name) in [
        (
            "image_load v[0:3], v[4:6], s[0:7] dmask:0xf dim:SQ_RSRC_IMG_2D_MSAA",
            "image_load msaa",
        ),
        (
            "image_sample_o v[0:3], v[4:6], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D",
            "image_sample_o",
        ),
        (
            "image_gather4_l v[0:3], v[4:6], s[0:7], s[8:11] dmask:0x1 dim:SQ_RSRC_IMG_2D",
            "image_gather4_l",
        ),
        (
            "image_load v[0:4], v[4:5], s[0:7] dmask:0xf dim:SQ_RSRC_IMG_2D unorm tfe",
            "image_load tfe",
        ),
    ] {
        let code = rdna::assemble(source, true).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
        let coverage = shader_ir::coverage(&code, false);
        assert_eq!(coverage.unsupported, [name], "{source}");
    }
}

#[test]
fn the_interpreter_refuses_images() {
    let function = lifted("image_load v0, v[4:5], s[0:7] dmask:0x1 dim:SQ_RSRC_IMG_2D\ns_endpgm");
    let mut machine = Machine::new(false, 64);
    assert_eq!(
        shader_ir::run(&function, &mut machine),
        Err(RunError::Image)
    );
}

/// Where a value came from, looking through half unpacking.
#[derive(Debug, PartialEq)]
enum Source {
    Whole(Reg),
    Half(Reg, bool),
}

fn sources(function: &Function) -> HashMap<u32, Source> {
    let mut values: HashMap<u32, &Expr> = HashMap::new();
    let mut out = HashMap::new();
    for stmt in function.blocks.iter().flat_map(|b| &b.stmts) {
        if let Stmt::Let(tmp, expr) = stmt {
            values.insert(*tmp, expr);
            let resolve = |t: &u32| values.get(t).copied();
            let found = match expr {
                Expr::Get(reg) => Some(Source::Whole(*reg)),
                Expr::Un(Un::F16ToF32, half) => match resolve(half) {
                    Some(Expr::Bin(Bin::And, moved, _)) => match resolve(moved) {
                        Some(Expr::Bin(Bin::ShrU, word, shift)) => {
                            match (resolve(word), resolve(shift)) {
                                (Some(Expr::Get(reg)), Some(Expr::Const(by))) => {
                                    Some(Source::Half(*reg, *by == 16))
                                }
                                _ => None,
                            }
                        }
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            };
            if let Some(found) = found {
                out.insert(*tmp, found);
            }
        }
    }
    out
}

fn first_image(function: &Function) -> Image {
    function
        .blocks
        .iter()
        .flat_map(|b| &b.stmts)
        .find_map(|s| match s {
            Stmt::Let(_, Expr::Image(image)) => Some((**image).clone()),
            Stmt::ImageStore { image, .. } => Some((**image).clone()),
            _ => None,
        })
        .unwrap()
}

#[test]
fn compare_sampling_reads_the_reference_before_the_coordinates() {
    let function = lifted(
        "image_sample_c_lz v0, [v7,v5,v9], s[0:7], s[8:11] dmask:0x1 dim:SQ_RSRC_IMG_2D\ns_endpgm",
    );
    let image = first_image(&function);
    let from = sources(&function);
    assert_eq!(image.op, ImageOp::SampleLodZero);
    assert_eq!(from[&image.compare.unwrap()], Source::Whole(Reg::V(7)));
    assert_eq!(from[&image.coordinates[0]], Source::Whole(Reg::V(5)));
    assert_eq!(from[&image.coordinates[1]], Source::Whole(Reg::V(9)));
}

#[test]
fn gradient_sampling_takes_both_directions_then_the_coordinates_and_clamp() {
    let function = lifted(
        "image_sample_d_cl v0, v[1:7], s[0:7], s[8:11] dmask:0x1 dim:SQ_RSRC_IMG_2D\ns_endpgm",
    );
    let image = first_image(&function);
    let from = sources(&function);
    assert_eq!(image.op, ImageOp::SampleGrad);
    let gradients: Vec<&Source> = image.gradients.iter().map(|t| &from[t]).collect();
    assert_eq!(
        gradients,
        (1..5)
            .map(|r| Source::Whole(Reg::V(r)))
            .collect::<Vec<_>>()
            .iter()
            .collect::<Vec<_>>()
    );
    assert_eq!(from[&image.coordinates[1]], Source::Whole(Reg::V(6)));
    assert_eq!(from[&image.clamp.unwrap()], Source::Whole(Reg::V(7)));
}

#[test]
fn sixteen_bit_addresses_pack_coordinates_but_keep_the_bias_word() {
    let function = lifted(
        "image_sample_b v0, [v3,v4], s[0:7], s[8:11] dmask:0x1 dim:SQ_RSRC_IMG_2D a16\ns_endpgm",
    );
    let image = first_image(&function);
    let from = sources(&function);
    assert_eq!(image.op, ImageOp::SampleBias);
    assert_eq!(from[&image.lod.unwrap()], Source::Half(Reg::V(3), false));
    assert_eq!(from[&image.coordinates[0]], Source::Half(Reg::V(4), false));
    assert_eq!(from[&image.coordinates[1]], Source::Half(Reg::V(4), true));

    let function = lifted(
        "image_sample_lz v[0:2], [v3,v4], s[0:7], s[8:11] dmask:0x7 dim:SQ_RSRC_IMG_2D_ARRAY a16\ns_endpgm",
    );
    let image = first_image(&function);
    let from = sources(&function);
    assert_eq!(from[&image.coordinates[2]], Source::Half(Reg::V(4), false));
}

#[test]
fn half_results_pack_two_components_per_register() {
    let function = lifted(
        "image_load v[0:1], v[4:5], s[0:7] dmask:0xe dim:SQ_RSRC_IMG_2D unorm d16\ns_endpgm",
    );
    let stmts: Vec<&Stmt> = function.blocks.iter().flat_map(|b| &b.stmts).collect();
    let written: Vec<u8> = stmts
        .iter()
        .filter_map(|s| match s {
            Stmt::SetLane(reg, _) => Some(*reg),
            _ => None,
        })
        .collect();
    assert_eq!(written, [0, 1]);
    assert!(first_image(&function).d16);
}

#[test]
fn stores_give_each_masked_component_its_register() {
    let function =
        lifted("image_store v[6:7], v[4:5], s[0:7] dmask:0x5 dim:SQ_RSRC_IMG_2D unorm\ns_endpgm");
    let stmts: Vec<&Stmt> = function.blocks.iter().flat_map(|b| &b.stmts).collect();
    let from = sources(&function);
    let Some(Stmt::ImageStore { image, values }) =
        stmts.iter().find(|s| matches!(s, Stmt::ImageStore { .. }))
    else {
        panic!("no store\n{function}");
    };
    assert_eq!(image.op, ImageOp::Store);
    assert_eq!(from[&values[0].unwrap()], Source::Whole(Reg::V(6)));
    assert_eq!(values[1], None);
    assert_eq!(from[&values[2].unwrap()], Source::Whole(Reg::V(7)));
    assert_eq!(values[3], None);
}

#[test]
fn size_queries_read_only_the_mip() {
    let function =
        lifted("image_get_resinfo v[0:3], v9, s[0:7] dmask:0xf dim:SQ_RSRC_IMG_2D unorm\ns_endpgm");
    let image = first_image(&function);
    let from = sources(&function);
    assert_eq!(image.op, ImageOp::Size);
    assert!(image.coordinates.is_empty());
    assert_eq!(from[&image.lod.unwrap()], Source::Whole(Reg::V(9)));
}

#[test]
fn gather_offsets_come_first() {
    let function = lifted(
        "image_gather4_c_lz_o v[0:3], v[4:7], s[0:7], s[8:11] dmask:0x1 dim:SQ_RSRC_IMG_2D\ns_endpgm",
    );
    let image = first_image(&function);
    let from = sources(&function);
    assert_eq!(from[&image.offset.unwrap()], Source::Whole(Reg::V(4)));
    assert_eq!(from[&image.compare.unwrap()], Source::Whole(Reg::V(5)));
    assert_eq!(from[&image.coordinates[0]], Source::Whole(Reg::V(6)));
}
