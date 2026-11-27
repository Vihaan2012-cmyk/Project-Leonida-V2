//! GameInput: the title's one import, ordinal 1, and a reading of the keyboard pad.

use iced_x86::Register;
use iced_x86::code_asm::{al, eax};

use crate::emit::{Arg, Emitter, Res};
use crate::script::{Script, Stage};

const GAME_INPUT: &str = "GameInput.dll";
const GAMEPAD: u64 = 0x4_0000;
/// `GameInputGamepadA`.
const BUTTON_A: u32 = 0x4;

pub fn add(script: Script) -> Script {
    script
        .step(
            Stage::Input,
            "GameInputCreate (ordinal 1) gives the input object",
            create,
        )
        .step(
            Stage::Input,
            "a current gamepad reading is available",
            reading,
        )
        .step(
            Stage::Input,
            "the reading carries the held button",
            gamepad_state,
        )
}

fn create(e: &mut Emitter) -> Res {
    let create = e.import_ordinal(GAME_INPUT, 1);
    let input = e.buffer("game_input", 8);
    e.call(create, &[Arg::addr(input)])?;
    e.fail_if_hresult_failed()?;
    e.load64(Register::RAX, input)?;
    e.fail_if_zero64()?;
    e.vcall(input, 3, &[])?;
    e.fail_if_zero64()
}

fn reading(e: &mut Emitter) -> Res {
    let input = e.cell("game_input");
    let reading = e.buffer("game_reading", 8);
    e.vcall(
        input,
        4,
        &[Arg::Imm(GAMEPAD), Arg::Imm(0), Arg::addr(reading)],
    )?;
    e.fail_if_nonzero32()?;
    e.load64(Register::RAX, reading)?;
    e.fail_if_zero64()
}

fn gamepad_state(e: &mut Emitter) -> Res {
    let reading = e.cell("game_reading");
    let state = e.buffer("gamepad_state", 32);
    e.vcall(reading, 22, &[Arg::addr(state)])?;
    e.asm.movzx(eax, al)?;
    e.fail_if_zero32()?;
    e.fail_if_mem32_ne(state, BUTTON_A)
}
