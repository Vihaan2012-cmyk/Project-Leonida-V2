use std::collections::HashMap;

use shader_ir::{Expr, Function, Image, ImageDim, ImageOp, Machine, Reg, RunError, Stmt, lift};

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
            "image_sample_lz v[0:1], v4, s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D a16 d16",
            "image_sample_lz a16 d16",
        ),
        (
            "image_load v[0:3], v[4:6], s[0:7] dmask:0xf dim:SQ_RSRC_IMG_2D_MSAA",
            "image_load msaa",
        ),
        (
            "image_sample_b v[0:3], v[4:6], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D",
            "image_sample_b",
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
