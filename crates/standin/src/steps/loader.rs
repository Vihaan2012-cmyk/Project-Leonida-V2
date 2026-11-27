//! What the game does before anything else: its TLS callback has run, its thread-local variables read, its
//! relocations and exception table are in place, and it asks the GDK runtime for its classes.

use iced_x86::Register;
use iced_x86::code_asm::{dword_ptr, eax, rcx};

use super::{LIBRARY_LOADER, LIBRARY_LOADER_W, RTL_SUPPORT, TITLE_ID, XGAME_CLASS, XSYSTEM_CLASS};
use crate::emit::{Arg, Emitter, Res, Sym, Target};
use crate::image::{TLS_FIRST, TLS_SECOND};
use crate::script::{Script, Stage};

pub fn add(script: Script) -> Script {
    script
        .step(
            Stage::Loader,
            "the TLS callback ran for the process",
            tls_callback_ran,
        )
        .step(
            Stage::Loader,
            "thread-local variables read through gs:[0x58]",
            tls_template,
        )
        .step(
            Stage::Loader,
            "relocated pointers hold the image's address",
            relocated_pointer,
        )
        .step(
            Stage::Loader,
            "GetModuleHandle(NULL) names the image",
            main_module,
        )
        .step(
            Stage::Loader,
            "the exception table finds the entry point",
            exception_table,
        )
        .step(
            Stage::Loader,
            "xgameruntime.dll loads and exports its functions",
            runtime_exports,
        )
        .step(
            Stage::Loader,
            "the GDK runtime initializes",
            runtime_initialize,
        )
        .step(
            Stage::Loader,
            "QueryApiImpl serves XSystemImpl and its console id",
            query_system,
        )
        .step(
            Stage::Loader,
            "QueryApiImpl serves XGameImpl and the title id",
            query_game,
        )
}

/// `DLL_PROCESS_ATTACH` is reason 1.
fn tls_callback_ran(e: &mut Emitter) -> Res {
    let mask = e.cell("tls_mask");
    e.load32(Register::EAX, mask)?;
    e.fail_unless_bit(1)?;
    let calls = e.cell("tls_calls");
    e.load32(Register::EAX, calls)?;
    e.fail_if_zero32()
}

fn tls_template(e: &mut Emitter) -> Res {
    e.tls_block()?;
    e.asm.mov(eax, dword_ptr(rcx))?;
    e.fail_if_ne32(TLS_FIRST)?;
    e.asm.mov(eax, dword_ptr(rcx + 4))?;
    e.fail_if_ne32(TLS_SECOND)
}

fn relocated_pointer(e: &mut Emitter) -> Res {
    let pointer = e.bytes_with_pointers(
        "image_pointer",
        &[0; 8],
        vec![(0, Target::Place(Sym::Image.into()))],
    );
    e.load64(Register::RAX, pointer)?;
    e.fail_if_rax_ne_address(Sym::Image)
}

fn main_module(e: &mut Emitter) -> Res {
    let get = e.import(LIBRARY_LOADER, "GetModuleHandleW");
    e.call(get, &[Arg::Imm(0)])?;
    e.fail_if_rax_ne_address(Sym::Image)
}

fn exception_table(e: &mut Emitter) -> Res {
    let lookup = e.import(RTL_SUPPORT, "RtlLookupFunctionEntry");
    let here = e.buffer("control_pc", 8);
    let base = e.buffer("function_base", 8);
    let found = e.buffer("function_entry", 8);
    e.current_address()?;
    e.store64(here, Register::RCX)?;
    e.call(lookup, &[Arg::load64(here), Arg::addr(base), Arg::Imm(0)])?;
    e.fail_if_zero64()?;
    e.store64(found, Register::RAX)?;
    e.load64(Register::RAX, base)?;
    e.fail_if_rax_ne_address(Sym::Image)
}

fn runtime_exports(e: &mut Emitter) -> Res {
    let load = e.import(LIBRARY_LOADER_W, "LoadLibraryW");
    let find = e.import(LIBRARY_LOADER, "GetProcAddress");
    let library = e.wide("xgameruntime_name", "xgameruntime.dll");
    let handle = e.buffer("xgameruntime", 8);
    e.call(load, &[Arg::addr(library)])?;
    e.fail_if_zero64()?;
    e.store64(handle, Register::RAX)?;
    for (name, cell) in [
        ("InitializeApiImplEx2", "gdk_initialize"),
        ("QueryApiImpl", "gdk_query"),
        ("XErrorReport", "gdk_error_report"),
    ] {
        let text = e.ascii(&format!("{name}_name"), name);
        let slot = e.buffer(cell, 8);
        e.call(find, &[Arg::load64(handle), Arg::addr(text)])?;
        e.fail_if_zero64()?;
        e.store64(slot, Register::RAX)?;
    }
    let missing = e.ascii("missing_export", "NotAnExportOfTheRuntime");
    e.call(find, &[Arg::load64(handle), Arg::addr(missing)])?;
    e.fail_if_nonzero64()
}

fn runtime_initialize(e: &mut Emitter) -> Res {
    let initialize = e.cell("gdk_initialize");
    e.call(
        initialize,
        &[Arg::Imm(0), Arg::Imm(0), Arg::Imm(0), Arg::Imm(0)],
    )?;
    e.fail_if_nonzero32()
}

fn query_class(e: &mut Emitter, class: &str, name: &str) -> Res<Sym> {
    let id = e.guid(&format!("{name}_class"), class);
    let object = e.buffer(&format!("{name}_object"), 8);
    let query = e.cell("gdk_query");
    e.call(query, &[Arg::addr(id), Arg::addr(id), Arg::addr(object)])?;
    e.fail_if_hresult_failed()?;
    e.load64(Register::RAX, object)?;
    e.fail_if_zero64()?;
    Ok(object)
}

fn query_system(e: &mut Emitter) -> Res {
    let object = query_class(e, XSYSTEM_CLASS, "system")?;
    let text = e.buffer("console_id", 64);
    let used = e.buffer("console_id_length", 8);
    e.vcall(object, 3, &[Arg::Imm(64), Arg::addr(text), Arg::addr(used)])?;
    e.fail_if_nonzero32()?;
    e.fail_if_mem32_ne(used, 39)?;
    e.fail_if_mem32_ne(text, 0x3030_3030)
}

fn query_game(e: &mut Emitter) -> Res {
    let object = query_class(e, XGAME_CLASS, "game")?;
    let id = e.buffer("title_id", 4);
    e.vcall(object, 3, &[Arg::addr(id)])?;
    e.fail_if_nonzero32()?;
    e.fail_if_mem32_ne(id, TITLE_ID)
}
