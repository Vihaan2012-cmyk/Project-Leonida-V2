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

type DspConnect = unsafe extern "win64" fn(usize, u32, u32, u32, u32, *mut u64) -> u32;
type DspActivate = unsafe extern "win64" fn(u64, *const u8, *mut usize, *mut u64) -> u32;
type DspSubmit = unsafe extern "win64" fn(u64, *const u8, *mut u32) -> u32;
type DspClose = unsafe extern "win64" fn(u64) -> u32;

struct Dsp {
    connect: DspConnect,
    activate: DspActivate,
    submit: DspSubmit,
    deactivate: DspClose,
    disconnect: DspClose,
}

fn dsp_shims() -> Dsp {
    Dsp {
        connect: shim("chad.dll", "XDspConnectWithMaximumStreamLimit"),
        activate: shim("chad.dll", "XDspActivate"),
        submit: shim("chad.dll", "XDspSubmitCommand"),
        deactivate: shim("chad.dll", "XDspDeactivate"),
        disconnect: shim("chad.dll", "XDspDisconnect"),
    }
}

/// The reverb node's side of the protocol: the 0x1c-byte info it builds for a stream.
fn info_bytes(flags: u32, mode: u32, response: &[f32]) -> [u8; 0x1c] {
    let mut info = [0u8; 0x1c];
    info[0..4].copy_from_slice(&1u32.to_le_bytes());
    info[4..8].copy_from_slice(&flags.to_le_bytes());
    info[8..12].copy_from_slice(&0x400u32.to_le_bytes());
    info[12..16].copy_from_slice(&mode.to_le_bytes());
    info[16..20].copy_from_slice(&(response.len() as u32).to_le_bytes());
    info[0x14..0x1c].copy_from_slice(&(response.as_ptr() as u64).to_le_bytes());
    info
}

/// The 0x20-byte command: a pair of gains of 1.0, then the pair's two buffers.
fn command_bytes(input: &[f32], output: &mut [f32]) -> [u8; 0x20] {
    let mut command = [0u8; 0x20];
    command[0..4].copy_from_slice(&1.0f32.to_le_bytes());
    command[4..8].copy_from_slice(&1.0f32.to_le_bytes());
    command[8..0x10].copy_from_slice(&(input.as_ptr() as u64).to_le_bytes());
    command[0x10..0x18].copy_from_slice(&(output.as_mut_ptr() as u64).to_le_bytes());
    command
}

/// What the title checks before it reads a batch back (0x4e5acc0): the status block's fence and error words.
fn status_words(status: usize) -> [u32; 4] {
    // SAFETY: the DSP gave this address; the block is four words and lives until the stream is deactivated.
    unsafe { (status as *const [u32; 4]).read_volatile() }
}

#[test]
fn a_reverb_stream_completes_its_fence_before_the_submit_returns_the_way_the_title_polls_it() {
    let _guard = serial();
    let dsp = dsp_shims();
    let mut response = vec![0.0f32; 1500];
    response[3] = 0.5;
    response[1100] = -0.25;
    let (mut connection, mut status, mut stream, mut fence) = (0u64, 0usize, 0u64, 0u32);
    let info = info_bytes(0, 1, &response);
    let mut input = vec![0.0f32; 1024];
    input[0] = 1.0;
    let mut output = vec![7.0f32; 1024];
    // SAFETY: the shapes the executable uses; every pointer is valid.
    unsafe {
        assert_eq!((dsp.connect)(0, 0x60000, 0x18, 0x80, 1, &mut connection), 0);
        assert_ne!(connection, 0);
        assert_eq!(
            (dsp.activate)(connection, info.as_ptr(), &mut status, &mut stream),
            0
        );
        assert!(status != 0 && stream != 0);
        let words = status_words(status);
        assert_eq!(
            (words[0], words[1], words[3]),
            (1, 0, 0),
            "running, nothing done, no error"
        );

        let command = command_bytes(&input, &mut output);
        assert_eq!((dsp.submit)(stream, command.as_ptr(), &mut fence), 0);
        assert_eq!(fence, 1);
        let words = status_words(status);
        assert_eq!(
            (words[1], words[3]),
            (fence, 0),
            "done matches the fence, no error"
        );
        assert!((output[3] - 0.5).abs() < 1e-4);
        assert!(
            output[4..].iter().all(|s| s.abs() < 1e-4),
            "the rest of the batch is quiet"
        );

        let silence = vec![0.0f32; 1024];
        let mut tail = vec![7.0f32; 1024];
        let command = command_bytes(&silence, &mut tail);
        assert_eq!((dsp.submit)(stream, command.as_ptr(), &mut fence), 0);
        assert_eq!(fence, 2);
        assert_eq!(status_words(status)[1], 2);
        assert!(
            (tail[1100 - 1024] + 0.25).abs() < 1e-4,
            "the tail rings into the next batch"
        );

        assert_eq!((dsp.deactivate)(stream), 0);
        assert_eq!(
            status_words(status)[0],
            0,
            "a stopped stream no longer runs"
        );
        assert_ne!(
            (dsp.deactivate)(stream),
            0,
            "a second close names no stream"
        );
        let command = command_bytes(&silence, &mut tail);
        assert_ne!((dsp.submit)(stream, command.as_ptr(), &mut fence), 0);
        assert_eq!((dsp.disconnect)(connection), 0);
        assert_ne!((dsp.disconnect)(connection), 0);
    }
}

#[test]
fn a_stereo_reverb_stream_takes_left_then_right_and_gives_them_back_the_same_way() {
    let _guard = serial();
    let dsp = dsp_shims();
    let mut response = vec![0.0f32; 2 * 64];
    response[2] = 1.0;
    response[64 + 4] = 0.5;
    let (mut connection, mut status, mut stream, mut fence) = (0u64, 0usize, 0u64, 0u32);
    let info = info_bytes(2 | 8, 2, &response);
    let mut input = vec![0.0f32; 512];
    input[10] = 1.0;
    input[256 + 20] = 1.0;
    let mut output = vec![0.0f32; 512];
    // SAFETY: the shapes the executable uses; every pointer is valid.
    unsafe {
        assert_eq!((dsp.connect)(0, 0x60000, 0x1e, 0x80, 1, &mut connection), 0);
        assert_eq!(
            (dsp.activate)(connection, info.as_ptr(), &mut status, &mut stream),
            0
        );
        let command = command_bytes(&input, &mut output);
        assert_eq!((dsp.submit)(stream, command.as_ptr(), &mut fence), 0);
        assert_eq!(status_words(status)[1], fence);
        assert!((output[12] - 1.0).abs() < 1e-4);
        assert!((output[256 + 24] - 0.5).abs() < 1e-4);
        assert!(output[256 + 12].abs() < 1e-4);
        assert_eq!((dsp.disconnect)(connection), 0);
        assert_ne!(
            (dsp.deactivate)(stream),
            0,
            "disconnecting closed the stream"
        );
    }
}

#[test]
fn a_stream_without_a_response_or_over_the_limit_is_refused_and_never_blocks() {
    let _guard = serial();
    let dsp = dsp_shims();
    let response = vec![1.0f32; 16];
    let (mut connection, mut status, mut stream) = (0u64, 0usize, 0u64);
    // SAFETY: the shapes the executable uses; every pointer is valid.
    unsafe {
        assert_eq!((dsp.connect)(0, 0, 0x18, 2, 1, &mut connection), 0);
        let none = info_bytes(0, 1, &[]);
        assert_ne!(
            (dsp.activate)(connection, none.as_ptr(), &mut status, &mut stream),
            0
        );
        let info = info_bytes(0, 1, &response);
        for _ in 0..2 {
            assert_eq!(
                (dsp.activate)(connection, info.as_ptr(), &mut status, &mut stream),
                0
            );
        }
        assert_ne!(
            (dsp.activate)(connection, info.as_ptr(), &mut status, &mut stream),
            0
        );
        assert_ne!(
            (dsp.activate)(0xdead, info.as_ptr(), &mut status, &mut stream),
            0
        );
        assert_ne!(
            (dsp.activate)(connection, std::ptr::null(), &mut status, &mut stream),
            0
        );
        let mut fence = 0u32;
        let (input, mut output) = (vec![0.0f32; 1024], vec![0.0f32; 1024]);
        let mut command = command_bytes(&input, &mut output);
        command[8..0x10].fill(0);
        assert_ne!(
            (dsp.submit)(stream, command.as_ptr(), &mut fence),
            0,
            "a null buffer"
        );
        assert_eq!(status_words(status)[1], 0, "the fence is untouched");
        assert_eq!((dsp.disconnect)(connection), 0);
    }
}

#[test]
fn closing_a_reverb_stream_leaves_the_mixer_and_its_voices_alone() {
    let _guard = serial();
    let dsp = dsp_shims();
    with_engine(|e| e.deactivate());
    with_engine(|e| e.take_completions());
    let voice = with_engine(|e| {
        e.mixer();
        e.add_voice(VoiceParams::new(48_000))
    });
    let buffer = with_engine(|e| {
        let id = e.new_buffer_id();
        let pcm = vec![100i16; 4800];
        e.queue_buffer(voice, Buffer::from_i16(id, &pcm, 1, Looping::Once));
        id
    });
    assert!(with_engine(|e| e.is_active() && !e.has_device()));
    let response = vec![1.0f32; 8];
    let (mut connection, mut status, mut stream, mut fence) = (0u64, 0usize, 0u64, 0u32);
    let info = info_bytes(0, 1, &response);
    let (input, mut output) = (vec![0.0f32; 1024], vec![0.0f32; 1024]);
    // SAFETY: the shapes the executable uses; every pointer is valid.
    unsafe {
        assert_eq!((dsp.connect)(0, 0x60000, 0x18, 0x80, 1, &mut connection), 0);
        assert_eq!(
            (dsp.activate)(connection, info.as_ptr(), &mut status, &mut stream),
            0
        );
        let command = command_bytes(&input, &mut output);
        assert_eq!((dsp.submit)(stream, command.as_ptr(), &mut fence), 0);
        assert!(with_engine(|e| e.take_completions()).is_empty());
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!((dsp.submit)(stream, command.as_ptr(), &mut fence), 0);
        let done = with_engine(|e| e.take_completions());
        assert_eq!(
            done.len(),
            1,
            "each submit also keeps the silent clock running"
        );
        assert_eq!(done[0].buffer, buffer);
        assert_eq!((dsp.deactivate)(stream), 0);
        assert_eq!((dsp.disconnect)(connection), 0);
    }
    assert!(
        with_engine(|e| e.is_active()),
        "closing a DSP stream does not stop the mixer"
    );
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

mod spatial {
    use std::ffi::c_void;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    use super::*;

    type Ptr = *mut c_void;
    type Hr = i32;

    const CLSID_ENUMERATOR: [u8; 16] = [
        0x95, 0x03, 0xde, 0xbc, 0x2f, 0xe5, 0x7c, 0x46, 0x8e, 0x3d, 0xc4, 0x57, 0x92, 0x91, 0x69,
        0x2e,
    ];
    const IID_ENUMERATOR: [u8; 16] = [
        0xd2, 0x64, 0x56, 0xa9, 0x14, 0x96, 0x35, 0x4f, 0xa7, 0x46, 0xde, 0x8d, 0xb6, 0x36, 0x17,
        0xe6,
    ];
    const IID_SPATIAL_CLIENT: [u8; 16] = [
        0x66, 0xe0, 0xf8, 0xbb, 0xaa, 0xaa, 0xbe, 0x49, 0x9a, 0x4d, 0xfd, 0x2a, 0x85, 0x8e, 0xa2,
        0x7f,
    ];
    const IID_RENDER_STREAM: [u8; 16] = [
        0x73, 0xf4, 0xb5, 0xba, 0x23, 0xb4, 0x7b, 0x47, 0x85, 0xf5, 0xb5, 0xa3, 0x32, 0xa0, 0x41,
        0x53,
    ];
    const FRONT_LEFT: u32 = 2;
    const FRONT_RIGHT: u32 = 4;

    type CoCreate = unsafe extern "system" fn(*const u8, Ptr, u32, *const u8, *mut Ptr) -> Hr;
    type Release = unsafe extern "system" fn(Ptr) -> u32;
    type GetDefault = unsafe extern "system" fn(Ptr, u32, u32, *mut Ptr) -> Hr;
    type Activate = unsafe extern "system" fn(Ptr, *const u8, u32, Ptr, *mut Ptr) -> Hr;
    type Formats = unsafe extern "system" fn(Ptr, *mut Ptr) -> Hr;
    type GetFormat = unsafe extern "system" fn(Ptr, u32, *mut *const u8) -> Hr;
    type Supported = unsafe extern "system" fn(Ptr, *const u8) -> Hr;
    type ActivateStream = unsafe extern "system" fn(Ptr, *const u8, *const u8, *mut Ptr) -> Hr;
    type Begin = unsafe extern "system" fn(Ptr, *mut u32, *mut u32) -> Hr;
    type End = unsafe extern "system" fn(Ptr) -> Hr;
    type ActivateObject = unsafe extern "system" fn(Ptr, u32, *mut Ptr) -> Hr;
    type GetBuffer = unsafe extern "system" fn(Ptr, *mut *mut f32, *mut u32) -> Hr;

    /// The function at `index` of `object` vtable.
    unsafe fn slot<F: Copy>(object: Ptr, index: usize) -> F {
        assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<usize>());
        // SAFETY: the caller passes a COM object, whose first word is its vtable.
        unsafe {
            let vtable = *(object as *const *const usize);
            std::mem::transmute_copy(&*vtable.add(index))
        }
    }

    #[test]
    fn the_output_loop_of_the_title_runs_on_the_emulated_spatial_client_without_waiting_in_vain() {
        let _guard = serial();
        let create: CoCreate = shim("api-ms-win-core-com-l1-1-0.dll", "CoCreateInstance");
        let voices_before = with_engine(|e| e.mixer().lock().unwrap().voice_count());
        let frames_before = with_engine(|e| e.mixer().lock().unwrap().rendered_frames());
        let mut enumerator: Ptr = std::ptr::null_mut();
        let mut device: Ptr = std::ptr::null_mut();
        let mut client: Ptr = std::ptr::null_mut();
        let mut formats: Ptr = std::ptr::null_mut();
        let mut stream: Ptr = std::ptr::null_mut();
        let mut format: *const u8 = std::ptr::null();
        // SAFETY: the calls the executable makes, on objects the shim just returned.
        unsafe {
            assert_eq!(
                create(
                    CLSID_ENUMERATOR.as_ptr(),
                    std::ptr::null_mut(),
                    0x17,
                    IID_ENUMERATOR.as_ptr(),
                    &mut enumerator
                ),
                0
            );
            assert_eq!(
                slot::<GetDefault>(enumerator, 4)(enumerator, 0, 0, &mut device),
                0
            );
            assert_eq!(
                slot::<Activate>(device, 3)(
                    device,
                    IID_SPATIAL_CLIENT.as_ptr(),
                    1,
                    std::ptr::null_mut(),
                    &mut client
                ),
                0
            );
            assert_eq!(slot::<Formats>(client, 6)(client, &mut formats), 0);
            assert_eq!(slot::<GetFormat>(formats, 4)(formats, 0, &mut format), 0);
            assert_eq!(slot::<Supported>(client, 8)(client, format), 0);

            let event = CreateEventW(std::ptr::null(), 0, 0, std::ptr::null());
            let mut params = [0u8; 0x28];
            params[..8].copy_from_slice(&(format as usize).to_le_bytes());
            params[8..12].copy_from_slice(&(FRONT_LEFT | FRONT_RIGHT).to_le_bytes());
            params[0x14..0x18].copy_from_slice(&6u32.to_le_bytes());
            params[0x18..0x20].copy_from_slice(&(event as usize).to_le_bytes());
            let mut variant = [0u8; 24];
            variant[0..2].copy_from_slice(&0x41u16.to_le_bytes());
            variant[8..12].copy_from_slice(&0x28u32.to_le_bytes());
            variant[16..24].copy_from_slice(&(params.as_ptr() as usize).to_le_bytes());
            assert_eq!(
                slot::<ActivateStream>(client, 10)(
                    client,
                    variant.as_ptr(),
                    IID_RENDER_STREAM.as_ptr(),
                    &mut stream
                ),
                0
            );

            let mut objects: [Ptr; 2] = [std::ptr::null_mut(); 2];
            let started = Instant::now();
            for period in 0..30 {
                assert_eq!(
                    WaitForSingleObject(event, 1000),
                    0,
                    "period {period}: the event was never raised"
                );
                let (mut dynamic, mut frames) = (7u32, 0u32);
                assert_eq!(
                    slot::<Begin>(stream, 8)(stream, &mut dynamic, &mut frames),
                    0
                );
                assert_eq!((dynamic, frames), (0, 480));
                for (index, kind) in [FRONT_LEFT, FRONT_RIGHT].into_iter().enumerate() {
                    if objects[index].is_null() {
                        assert_eq!(
                            slot::<ActivateObject>(stream, 10)(stream, kind, &mut objects[index]),
                            0
                        );
                    }
                    let (mut buffer, mut bytes) = (std::ptr::null_mut::<f32>(), 0u32);
                    assert_eq!(
                        slot::<GetBuffer>(objects[index], 3)(
                            objects[index],
                            &mut buffer,
                            &mut bytes
                        ),
                        0
                    );
                    assert_eq!(bytes, 480 * 4);
                    std::slice::from_raw_parts_mut(buffer, frames as usize).fill(if index == 0 {
                        0.5
                    } else {
                        -0.5
                    });
                }
                assert_eq!(slot::<End>(stream, 9)(stream), 0);
            }
            let took = started.elapsed();
            assert!(
                took < Duration::from_secs(3),
                "30 periods of 10 ms took {took:?}"
            );
            for object in objects {
                slot::<Release>(object, 2)(object);
            }
            for object in [stream, formats, client, device, enumerator] {
                slot::<Release>(object, 2)(object);
            }
            CloseHandle(event);
        }
        let frames_after = with_engine(|e| e.mixer().lock().unwrap().rendered_frames());
        assert!(
            frames_after - frames_before >= 15 * 480,
            "the silent clock followed the periods of the title"
        );
        assert_eq!(
            with_engine(|e| e.mixer().lock().unwrap().voice_count()),
            voices_before,
            "releasing the stream removed its voice"
        );
    }

    #[test]
    fn other_classes_pass_through_to_the_host() {
        let _guard = serial();
        let create: CoCreate = shim("api-ms-win-core-com-l1-1-0.dll", "CoCreateInstance");
        let unknown_class = [0x5au8; 16];
        let mut out: Ptr = std::ptr::null_mut();
        // SAFETY: valid pointers; the host answers for a class nobody registered.
        let hr = unsafe {
            create(
                unknown_class.as_ptr(),
                std::ptr::null_mut(),
                0x17,
                IID_ENUMERATOR.as_ptr(),
                &mut out,
            )
        };
        assert!(
            hr < 0 && out.is_null(),
            "the host refuses, not the emulation: {hr:#x}"
        );
    }
}
