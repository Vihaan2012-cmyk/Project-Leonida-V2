//! The package folder through the file shims, and a DirectStorage request for the same file.

use iced_x86::Register;
use iced_x86::code_asm::eax;

use super::{FILE, HANDLE, PACKAGE_FILE, SLEEP, package_file_bytes};
use crate::emit::{Arg, Emitter, Res, Sym, Target};
use crate::script::{Script, Stage};

const GENERIC_READ: u64 = 0x8000_0000;
const GENERIC_WRITE: u64 = 0x4000_0000;
const OPEN_EXISTING: u64 = 3;
const CREATE_ALWAYS: u64 = 2;
const NORMAL: u64 = 0x80;
pub const SAVE_FILE: &str = r"G:\data\slot.bin";
pub const SAVE_BYTES: u64 = 64;

const IID_FACTORY: &str = "6924ea0c-c3cd-4826-b10a-f64f4ed927c1";
const IID_QUEUE: &str = "cfdbd83f-9e06-4fda-8ea5-69042137f49b";
const IID_FILE: &str = "5de95e7b-955a-4868-a73c-243b29f4b8da";
const IID_STATUS: &str = "82397587-7cd5-453b-a02e-31379bd64656";

fn length() -> u64 {
    package_file_bytes().len() as u64
}

pub fn add(script: Script) -> Script {
    script
        .step(
            Stage::Files,
            "CreateFileW opens a file of the package",
            open,
        )
        .step(
            Stage::Files,
            "GetFileSizeEx and ReadFile read the package file",
            read,
        )
        .step(
            Stage::Files,
            "a file written below the package goes to Output",
            write,
        )
        .step(
            Stage::Files,
            "DStorageGetFactory gives the factory and its queue",
            storage_factory,
        )
        .step(
            Stage::Files,
            "a DirectStorage request reads the file into memory",
            storage_request,
        )
}

fn open(e: &mut Emitter) -> Res {
    let create = e.import(FILE, "CreateFileW");
    let path = e.wide("package_path", PACKAGE_FILE);
    let handle = e.buffer("package_handle", 8);
    e.call(
        create,
        &[
            Arg::addr(path),
            Arg::Imm(GENERIC_READ),
            Arg::Imm(1),
            Arg::Imm(0),
            Arg::Imm(OPEN_EXISTING),
            Arg::Imm(NORMAL),
            Arg::Imm(0),
        ],
    )?;
    e.fail_if_invalid_handle()?;
    e.store64(handle, Register::RAX)
}

fn read(e: &mut Emitter) -> Res {
    let size_of = e.import(FILE, "GetFileSizeEx");
    let read = e.import(FILE, "ReadFile");
    let close = e.import(HANDLE, "CloseHandle");
    let handle = e.cell("package_handle");
    let size = e.buffer("package_size", 8);
    let buffer = e.buffer("package_buffer", length() as usize);
    let done = e.buffer("package_read", 8);
    e.call(size_of, &[Arg::load64(handle), Arg::addr(size)])?;
    e.fail_if_zero32()?;
    e.load64(Register::RAX, size)?;
    e.fail_if_ne64(length())?;
    e.call(
        read,
        &[
            Arg::load64(handle),
            Arg::addr(buffer),
            Arg::Imm(length()),
            Arg::addr(done),
            Arg::Imm(0),
        ],
    )?;
    e.fail_if_zero32()?;
    e.load64(Register::RAX, done)?;
    e.fail_if_ne64(length())?;
    let bytes = package_file_bytes();
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
    e.fail_if_mem32_ne(buffer, word(0))?;
    e.fail_if_mem32_ne(buffer.at(length() as u32 - 4), word(bytes.len() - 4))?;
    e.call(close, &[Arg::load64(handle)])?;
    e.fail_if_zero32()
}

fn write(e: &mut Emitter) -> Res {
    let create = e.import(FILE, "CreateFileW");
    let write = e.import(FILE, "WriteFile");
    let close = e.import(HANDLE, "CloseHandle");
    let path = e.wide("save_path", SAVE_FILE);
    let handle = e.buffer("save_handle", 8);
    let buffer = e.cell("package_buffer");
    let done = e.buffer("save_written", 8);
    e.call(
        create,
        &[
            Arg::addr(path),
            Arg::Imm(GENERIC_WRITE),
            Arg::Imm(0),
            Arg::Imm(0),
            Arg::Imm(CREATE_ALWAYS),
            Arg::Imm(NORMAL),
            Arg::Imm(0),
        ],
    )?;
    e.fail_if_invalid_handle()?;
    e.store64(handle, Register::RAX)?;
    e.call(
        write,
        &[
            Arg::load64(handle),
            Arg::addr(buffer),
            Arg::Imm(SAVE_BYTES),
            Arg::addr(done),
            Arg::Imm(0),
        ],
    )?;
    e.fail_if_zero32()?;
    e.load64(Register::RAX, done)?;
    e.fail_if_ne64(SAVE_BYTES)?;
    e.call(close, &[Arg::load64(handle)])?;
    e.fail_if_zero32()
}

fn storage_factory(e: &mut Emitter) -> Res {
    let get = e.import("XCRDAPI.dll", "DStorageGetFactory");
    let factory_id = e.guid("ds_factory_iid", IID_FACTORY);
    let queue_id = e.guid("ds_queue_iid", IID_QUEUE);
    let factory = e.buffer("ds_factory", 8);
    let queue = e.buffer("ds_queue", 8);
    // DSTORAGE_QUEUE_DESC: source type (file), capacity, priority, name, device.
    let mut desc = [0u8; 24];
    desc[4..6].copy_from_slice(&0x2000u16.to_le_bytes());
    let desc = e.bytes("ds_queue_desc", &desc);
    e.call(get, &[Arg::addr(factory_id), Arg::addr(factory)])?;
    e.fail_if_hresult_failed()?;
    e.load64(Register::RAX, factory)?;
    e.fail_if_zero64()?;
    e.vcall(
        factory,
        3,
        &[Arg::addr(desc), Arg::addr(queue_id), Arg::addr(queue)],
    )?;
    e.fail_if_hresult_failed()?;
    e.load64(Register::RAX, queue)?;
    e.fail_if_zero64()
}

fn storage_request(e: &mut Emitter) -> Res {
    let sleep = e.import(SLEEP, "Sleep");
    let factory = e.cell("ds_factory");
    let queue = e.cell("ds_queue");
    let path = e.wide("package_path", PACKAGE_FILE);
    let file_id = e.guid("ds_file_iid", IID_FILE);
    let status_id = e.guid("ds_status_iid", IID_STATUS);
    let file = e.buffer("ds_file", 8);
    let status = e.buffer("ds_status", 8);
    let counter = e.buffer("ds_polls", 4);
    let destination = e.buffer("ds_destination", length() as usize);
    // DSTORAGE_REQUEST: options 0 is a file source into memory, uncompressed. The destination pointer is an
    // absolute address, so the field is relocated with the image.
    let mut request = [0u8; 96];
    request[24..32].copy_from_slice(&length().to_le_bytes());
    request[40..48].copy_from_slice(&length().to_le_bytes());
    request[72..76].copy_from_slice(&(length() as u32).to_le_bytes());
    let request = e.bytes_with_pointers(
        "ds_request",
        &request,
        vec![(32, Target::Place(destination.into()))],
    );
    e.vcall(
        factory,
        4,
        &[Arg::addr(path), Arg::addr(file_id), Arg::addr(file)],
    )?;
    e.fail_if_hresult_failed()?;
    e.vcall(
        factory,
        5,
        &[
            Arg::Imm(1),
            Arg::Imm(0),
            Arg::addr(status_id),
            Arg::addr(status),
        ],
    )?;
    e.fail_if_hresult_failed()?;
    e.load64(Register::RAX, file)?;
    e.store64(request.at(8), Register::RAX)?;
    e.vcall(queue, 3, &[Arg::addr(request)])?;
    e.vcall(queue, 4, &[Arg::load64(status), Arg::Imm(0)])?;
    e.vcall(queue, 6, &[])?;
    wait_complete(e, status, counter, sleep)?;
    e.vcall(status, 4, &[Arg::Imm(0)])?;
    e.fail_if_nonzero32()?;
    let bytes = package_file_bytes();
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
    e.fail_if_mem32_ne(destination, word(0))?;
    e.fail_if_mem32_ne(destination.at(2048), word(2048))?;
    e.fail_if_mem32_ne(destination.at(length() as u32 - 4), word(bytes.len() - 4))
}

/// Polls `IsComplete(0)` of a status array, sleeping a millisecond between tries, up to five seconds.
fn wait_complete(e: &mut Emitter, status: Sym, counter: Sym, sleep: Sym) -> Res {
    let mut again = e.label();
    let mut complete = e.label();
    e.store32_imm(counter, 0)?;
    e.place(&mut again)?;
    e.vcall(status, 3, &[Arg::Imm(0)])?;
    e.asm.test(eax, eax)?;
    e.asm.jnz(complete)?;
    e.add32_imm(counter, 1)?;
    e.call(sleep, &[Arg::Imm(1)])?;
    e.load32(Register::EAX, counter)?;
    e.asm.cmp(eax, 5000)?;
    e.asm.jb(again)?;
    e.fail()?;
    e.place(&mut complete)?;
    Ok(())
}
