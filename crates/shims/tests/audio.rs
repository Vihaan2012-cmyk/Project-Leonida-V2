//! The audio libraries, called the way the executable calls them.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use shims::audio::{
    Buffer, DecodeError, Decoded, Looping, OutputMode, VoiceParams, XmaRequest, set_decoder,
    set_output_mode, with_engine,
};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    set_output_mode(OutputMode::Off);
    guard
}

fn shim<T: Copy>(dll: &str, name: &str) -> T {
    let address = shims::builtin()
        .into_iter()
        .find(|s| s.dll == dll && s.name == name)
        .unwrap_or_else(|| panic!("{dll}!{name} registered"))
        .address;
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
    // SAFETY: `T` is the function pointer type matching the shim's measured shape.
    unsafe { std::mem::transmute_copy(&address) }
}

const ALL: [(&str, &str); 19] = [
    ("xapu.dll", "XApuConnect"),
    ("xapu.dll", "XApuDequeueResult"),
    ("xapu.dll", "XApuDisconnect"),
    ("xapu.dll", "XApuEnqueueCommand"),
    ("xapu.dll", "XApuGetResultCount"),
    ("AcpHal.dll", "AcpHalAllocateShapeContexts"),
    ("AcpHal.dll", "AcpHalCreate"),
    ("AcpHal.dll", "AcpHalReleaseShapeContexts"),
    ("AcpHal.dll", "ApuAlloc"),
    ("AcpHal.dll", "ApuCreateHeap"),
    ("AcpHal.dll", "ApuFree"),
    ("AcpHal.dll", "ApuIsVirtualAddressValid"),
    ("AcpHal.dll", "ApuMapApuAddress"),
    ("AcpHal.dll", "ApuMapVirtualAddress"),
    ("chad.dll", "XDspActivate"),
    ("chad.dll", "XDspConnectWithMaximumStreamLimit"),
    ("chad.dll", "XDspDeactivate"),
    ("chad.dll", "XDspDisconnect"),
    ("chad.dll", "XDspSubmitCommand"),
];

#[test]
fn all_nineteen_imports_are_provided() {
    let shims = shims::builtin();
    for (dll, name) in ALL {
        assert!(
            shims.iter().any(|s| s.dll == dll && s.name == name),
            "{dll}!{name}"
        );
    }
}

#[test]
fn the_boot_call_sequence_succeeds_the_way_the_title_checks() {
    let _guard = serial();
    type Create = unsafe extern "win64" fn(*mut usize, u64) -> i32;
    type Contexts = unsafe extern "win64" fn(*mut u64) -> i32;
    type Release = unsafe extern "win64" fn(usize) -> i32;
    type Heap = unsafe extern "win64" fn(u64, u64) -> i32;
    type Alloc = unsafe extern "win64" fn(*mut usize, *mut u32, u32, u32, u64) -> i32;
    type Free = unsafe extern "win64" fn(usize) -> i32;
    type Valid = unsafe extern "win64" fn(usize, usize) -> u32;
    type MapVirtual = unsafe extern "win64" fn(usize, usize) -> u32;
    type MapApu = unsafe extern "win64" fn(u32) -> usize;
    let create: Create = shim("AcpHal.dll", "AcpHalCreate");
    let contexts: Contexts = shim("AcpHal.dll", "AcpHalAllocateShapeContexts");
    let release: Release = shim("AcpHal.dll", "AcpHalReleaseShapeContexts");
    let heap: Heap = shim("AcpHal.dll", "ApuCreateHeap");
    let alloc: Alloc = shim("AcpHal.dll", "ApuAlloc");
    let free: Free = shim("AcpHal.dll", "ApuFree");
    let valid: Valid = shim("AcpHal.dll", "ApuIsVirtualAddressValid");
    let map_virtual: MapVirtual = shim("AcpHal.dll", "ApuMapVirtualAddress");
    let map_apu: MapApu = shim("AcpHal.dll", "ApuMapApuAddress");

    let mut acp = 0usize;
    let mut info = [0u64; 8];
    info[2] = 350;
    let (mut block, mut apu) = (0usize, 0u32);
    // SAFETY: the shapes the executable uses; every pointer is valid.
    unsafe {
        assert!(create(&mut acp, 0) >= 0);
        assert_ne!(acp, 0);
        assert!(contexts(info.as_mut_ptr()) >= 0);
        let shape = info[7] as *const u8;
        assert!(!shape.is_null(), "the contexts' address goes to +0x38");
        assert!(
            std::slice::from_raw_parts(shape, 350 * 64)
                .iter()
                .all(|b| *b == 0)
        );
        info[2] = 0;
        assert!(contexts(info.as_mut_ptr()) < 0);
        assert!(heap(0, 0x226e00) >= 0);
        assert!(alloc(&mut block, std::ptr::null_mut(), 0x2000, 0x800, 0) >= 0);
        assert_ne!(block, 0);
        assert_eq!(block % 0x800, 0);
        (block as *mut u32).write(0xA5A5);
        for size in [0x40usize, 0x80, 0x800, 0x800] {
            assert_eq!(valid(block, size) & 0xFF, 1);
            let mapped = map_virtual(block, size);
            assert_ne!(mapped, 0);
            assert_eq!(map_apu(mapped), block);
        }
        assert!(alloc(&mut block, &mut apu, 0x100, 0x800, 0) >= 0);
        assert_eq!(map_apu(apu), block);
        assert_eq!(map_apu(0xDEAD_0000), 0);
        assert_eq!(valid(0, 0x40) & 0xFF, 0);
        assert_eq!(free(block), 0);
        assert_eq!(release(acp), 0);
    }
}

#[test]
fn the_hal_object_answers_through_its_vtable_the_way_the_title_calls_it() {
    let _guard = serial();
    type Create = unsafe extern "win64" fn(*mut usize, u64) -> i32;
    type Init = unsafe extern "win64" fn(usize, u64, u64) -> i32;
    type Submit = unsafe extern "win64" fn(usize, u32, u32, i32, *const u32, u64) -> i32;
    type Stop = unsafe extern "win64" fn(usize);
    let create: Create = shim("AcpHal.dll", "AcpHalCreate");
    let mut object = 0usize;
    // SAFETY: the shapes the executable uses; the object's first word is its vtable.
    unsafe {
        assert!(create(&mut object, 0) >= 0);
        let vtable = (object as *const *const usize).read();
        assert!(!vtable.is_null(), "the title calls through the first word");
        let init: Init = std::mem::transmute(vtable.read());
        let stop: Stop = std::mem::transmute(vtable.add(1).read());
        let submit: Submit = std::mem::transmute(vtable.add(2).read());
        let release: Stop = std::mem::transmute(vtable.add(5).read());
        assert!(init(object, 0x1000, 0x1000) >= 0);
        assert!(with_engine(|e| e.hal_mut().started()));
        assert!(submit(object, 2, 0xf0f0_f0f0, -1, [0xc0u32].as_ptr(), 0) >= 0);
        assert!(submit(object, 5, 0xf6f6_f6f6, -1, [3u32].as_ptr(), 0) >= 0);
        assert!(with_engine(|e| e.hal_mut().is_enabled(3)));
        assert!(submit(object, 0x12, 0xf1f1_f1f1, -1, [3u32, 4].as_ptr(), 0) >= 0);
        assert_eq!(with_engine(|e| e.hal_mut().take_reads()), vec![(3, 4)]);
        assert!(submit(object, 7, 4, -1, [3u32].as_ptr(), 0) >= 0);
        assert!(!with_engine(|e| e.hal_mut().is_enabled(3)));
        stop(object);
        assert!(!with_engine(|e| e.hal_mut().started()));
        release(object);
    }
}

#[test]
fn opus_commands_open_decode_and_close_with_results_the_title_state_machine_expects() {
    let _guard = serial();
    type Connect = unsafe extern "win64" fn(*mut u64, *mut usize, *mut u64) -> i32;
    type Disconnect = unsafe extern "win64" fn(u64, u64) -> i32;
    type Enqueue = unsafe extern "win64" fn(u64, *const u8, u64) -> i32;
    type Dequeue = unsafe extern "win64" fn(u64, *mut u8) -> i32;
    type Count = unsafe extern "win64" fn() -> u32;
    let connect: Connect = shim("xapu.dll", "XApuConnect");
    let disconnect: Disconnect = shim("xapu.dll", "XApuDisconnect");
    let enqueue: Enqueue = shim("xapu.dll", "XApuEnqueueCommand");
    let dequeue: Dequeue = shim("xapu.dll", "XApuDequeueResult");
    let count: Count = shim("xapu.dll", "XApuGetResultCount");
    let (mut info, mut region, mut handle) = ([0u64; 2], 0usize, 0u64);
    // SAFETY: the shapes the executable uses; every pointer is valid.
    unsafe {
        assert!(connect(info.as_mut_ptr(), &mut region, &mut handle) >= 0);
        assert!(handle != 0 && (handle as i64) > 0);
        assert_ne!(region, 0);
        assert_eq!(count(), 0);
        let mut open = [0u8; 0x40];
        open[0] = 1;
        open[4] = 5;
        open[0x18] = 2;
        assert_eq!(enqueue(handle, open.as_ptr(), 0), 0);
        let mut decode = [0u8; 0x40];
        decode[0] = 3;
        decode[4] = 5;
        decode[0x24..0x28].copy_from_slice(&960u32.to_le_bytes());
        decode[0x28..0x2c].copy_from_slice(&40u32.to_le_bytes());
        decode[0x2c..0x34].copy_from_slice(&(region as u64).to_le_bytes());
        decode[0x34..0x38].copy_from_slice(&0x1e00u32.to_le_bytes());
        decode[0x38..0x40].copy_from_slice(&((region + 0x14000) as u64).to_le_bytes());
        assert_eq!(enqueue(handle, decode.as_ptr(), 0), 0);
        let mut close = [0u8; 0x40];
        close[0] = 2;
        close[4] = 5;
        assert_eq!(enqueue(handle, close.as_ptr(), 0), 0);
        assert!(enqueue(handle, std::ptr::null(), 0) < 0);
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while count() < 3 {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut seen = Vec::new();
        for _ in 0..3 {
            let mut result = [0xFFu8; 0x40];
            assert_eq!(dequeue(handle, result.as_mut_ptr()), 0);
            let word = |at: usize| u32::from_le_bytes(result[at..at + 4].try_into().unwrap());
            seen.push((word(0), word(4), word(0x34)));
        }
        assert_eq!(seen, [(1, 5, 0), (3, 5, 0x1e00), (2, 5, 0)]);
        let mut result = [0u8; 0x40];
        assert!(
            dequeue(handle, result.as_mut_ptr()) < 0,
            "empty is a failure code"
        );
        assert_eq!(disconnect(handle, 0x20), 0);
    }
}

#[test]
fn the_dsp_calls_return_zero_and_voices_finish_on_the_clock() {
    let _guard = serial();
    type Connect = unsafe extern "win64" fn(u64, u64, u64, u64) -> u32;
    type Call0 = unsafe extern "win64" fn() -> u32;
    type Submit = unsafe extern "win64" fn(u64, u64) -> u32;
    let connect: Connect = shim("chad.dll", "XDspConnectWithMaximumStreamLimit");
    let activate: Call0 = shim("chad.dll", "XDspActivate");
    let deactivate: Call0 = shim("chad.dll", "XDspDeactivate");
    let disconnect: Call0 = shim("chad.dll", "XDspDisconnect");
    let submit: Submit = shim("chad.dll", "XDspSubmitCommand");
    // SAFETY: the shapes the executable uses.
    unsafe {
        assert_eq!(connect(0, 0, 0, 0x80), 0);
        assert_eq!(activate(), 0);
        with_engine(|e| e.take_completions());
        assert!(with_engine(|e| e.is_active() && !e.has_device()));
        let voice = with_engine(|e| e.add_voice(VoiceParams::new(48_000)));
        let buffer = with_engine(|e| {
            let id = e.new_buffer_id();
            let pcm = vec![100i16; 4800];
            e.queue_buffer(voice, Buffer::from_i16(id, &pcm, 1, Looping::Once));
            id
        });
        assert_eq!(submit(1, 2), 0);
        assert!(with_engine(|e| e.take_completions()).is_empty());
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(submit(1, 2), 0);
        let done = with_engine(|e| e.take_completions());
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].buffer, buffer);
        assert_eq!(deactivate(), 0);
        assert_eq!(disconnect(), 0);
    }
}

fn fake_decoder(request: &XmaRequest<'_>) -> Result<Decoded, DecodeError> {
    if request.data.is_empty() {
        return Err(DecodeError::Failed("empty".into()));
    }
    Ok(Decoded {
        pcm: vec![i16::from(request.data[0]); 960],
        sample_rate: 48_000,
        channels: 1,
    })
}

#[test]
fn the_xma_integration_point_feeds_the_mixer() {
    let _guard = serial();
    set_decoder(Some(fake_decoder));
    let request = XmaRequest {
        data: &[9, 9, 9],
        sample_rate: 32_000,
        channels: 1,
        frames: 5,
    };
    let (voice, buffer) = with_engine(|e| {
        e.activate();
        e.play_xma(&request, Looping::Once, 1.0).unwrap()
    });
    assert_ne!(voice, 0);
    let mixer = with_engine(|e| e.mixer());
    let mut out = vec![0.0f32; 2 * 10];
    mixer.lock().unwrap().render(&mut out);
    assert!(
        (out[0] - 9.0 / 32768.0).abs() < 1e-9,
        "the decoded samples play"
    );
    let empty = XmaRequest {
        data: &[],
        ..request.clone()
    };
    assert!(with_engine(|e| e.play_xma(&empty, Looping::Once, 1.0)).is_err());
    mixer.lock().unwrap().render(&mut vec![0.0f32; 2 * 960]);
    let done = with_engine(|e| e.take_completions());
    assert!(done.iter().any(|c| c.buffer == buffer));
    set_decoder(None);
    with_engine(|e| e.deactivate());
}
