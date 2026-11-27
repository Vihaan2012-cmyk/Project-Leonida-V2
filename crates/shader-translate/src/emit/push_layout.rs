use super::*;
use rspirv::dr::load_words;
use rspirv::spirv::Op;
use shader_ir::{Block, Function, Terminator};

/// A program that takes its own address and keeps it in s0:s1.
fn reads_own_address() -> Function {
    Function {
        wave64: false,
        blocks: vec![Block {
            pc: 0,
            stmts: vec![Stmt::Let(0, Expr::CodeBase), Stmt::SetPair(0, 0)],
            terminator: Terminator::End,
        }],
        temps: 1,
        ordered: false,
        upper_lanes: false,
    }
}

/// The push block's length in words and the word indexes the program reads, from the module itself.
fn push_block(spirv: &[u32]) -> (u32, Vec<u32>) {
    let module = load_words(spirv).expect("a well-formed module");
    let constant = |id: Word| {
        module
            .types_global_values
            .iter()
            .find(|i| i.result_id == Some(id) && i.class.opcode == Op::Constant)
            .map(|i| match i.operands[0] {
                Operand::LiteralBit32(v) => v,
                ref other => panic!("{other:?}"),
            })
    };
    let variable = module
        .types_global_values
        .iter()
        .find(|i| {
            i.class.opcode == Op::Variable
                && i.operands[0] == Operand::StorageClass(StorageClass::PushConstant)
        })
        .expect("a push block");
    let pointer = variable.result_type.unwrap();
    let block = module
        .types_global_values
        .iter()
        .find(|i| i.result_id == Some(pointer))
        .map(|i| i.operands[1].unwrap_id_ref())
        .unwrap();
    let array = module
        .types_global_values
        .iter()
        .find(|i| i.result_id == Some(block))
        .map(|i| i.operands[0].unwrap_id_ref())
        .unwrap();
    let length = module
        .types_global_values
        .iter()
        .find(|i| i.result_id == Some(array))
        .map(|i| constant(i.operands[1].unwrap_id_ref()).unwrap())
        .unwrap();
    let push = variable.result_id.unwrap();
    let mut reads = Vec::new();
    for function in &module.functions {
        for block in &function.blocks {
            for inst in &block.instructions {
                if inst.class.opcode == Op::AccessChain && inst.operands[0].unwrap_id_ref() == push
                {
                    reads.push(constant(inst.operands[2].unwrap_id_ref()).unwrap());
                }
            }
        }
    }
    (length, reads)
}

fn code_words(spirv: &[u32]) -> (u32, Vec<u32>) {
    let (length, reads) = push_block(spirv);
    (
        length,
        reads.into_iter().filter(|&w| w >= PUSH_CODE).collect(),
    )
}

#[test]
fn each_stage_kind_reads_the_code_base_words_of_its_own() {
    let function = reads_own_address();
    let float = FloatMode::default();
    let compute = compute(
        &function,
        &ComputeOptions {
            group_size: [32, 1, 1],
            user_sgprs: 2,
            workgroup_ids: [false; 3],
            thread_ids: 0,
            lds_bytes: 0,
            float,
        },
    )
    .unwrap();
    let vertex = vertex(
        &function,
        &VertexOptions {
            user_sgprs: 2,
            inputs: Vec::new(),
            float,
        },
    )
    .unwrap();
    let pixel = pixel(
        &function,
        &PixelOptions {
            user_sgprs: 2,
            inputs: 1 << 1,
            outputs: Vec::new(),
            depth: None,
            attribute_locations: Vec::new(),
            float,
        },
    )
    .unwrap();
    let geometry = gs(
        &function,
        &GsOptions {
            user_sgprs: 2,
            input: GsInput::Points,
            item_bytes: 16,
            output: GsOutput::Points,
            max_vertices: 1,
            vertex_bytes: 16,
            position: None,
            float,
        },
    )
    .unwrap();
    assert_eq!(
        code_words(&compute),
        (PUSH_CODE + 2, vec![PUSH_CODE, PUSH_CODE + 1])
    );
    assert_eq!(
        code_words(&vertex),
        (PUSH_CODE + 2, vec![PUSH_CODE, PUSH_CODE + 1])
    );
    assert_eq!(
        code_words(&geometry),
        (
            PUSH_CODE_GEOMETRY + 2,
            vec![PUSH_CODE_GEOMETRY, PUSH_CODE_GEOMETRY + 1]
        )
    );
    assert_eq!(
        code_words(&pixel),
        (
            PUSH_CODE_PIXEL + 2,
            vec![PUSH_CODE_PIXEL, PUSH_CODE_PIXEL + 1]
        )
    );
}

#[test]
fn the_push_words_do_not_overlap_and_fit_the_block() {
    let bases = [PUSH_HEAP, PUSH_CODE, PUSH_CODE_GEOMETRY, PUSH_CODE_PIXEL];
    assert_eq!(bases[0], 32);
    for pair in bases.windows(2) {
        assert_eq!(pair[1], pair[0] + 2);
    }
    assert_eq!(PUSH_WORDS, PUSH_CODE_PIXEL + 2);
}
