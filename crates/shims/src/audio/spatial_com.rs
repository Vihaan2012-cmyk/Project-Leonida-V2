//! The Windows audio objects the title calls for its output, implemented here: a device enumerator, one default
//! render endpoint, its audio client (only `GetMixFormat` is used) and its spatial audio client with one
//! static-bed render stream. Which calls the title makes, read from its code (RVAs are image offsets):
//!
//! - 0x43aea20: `CoCreateInstance(MMDeviceEnumerator)`, `RegisterEndpointNotificationCallback`,
//!   `EnumAudioEndpoints(render, active)` with `GetCount` / `Item` / `OpenPropertyStore` / `GetValue` on each
//!   (an endpoint form factor of 3 or 5 marks headphones), then `GetDefaultAudioEndpoint` and `GetId` (0x5c59500).
//! - 0x1e7f580: `Activate(IAudioClient)` and `GetMixFormat` (only the channel count at `+2` is read), then
//!   `Activate(ISpatialAudioClient)`, `GetSupportedAudioObjectFormatEnumerator`, `GetCount`, `GetFormat(0)`,
//!   `IsAudioObjectFormatSupported`, and per device a manual-reset-free event plus `ActivateSpatialAudioStream` with a
//!   blob of the 0x28-byte activation parameters: format, the static object mask, no dynamic objects, category 6,
//!   the event.
//! - 0x43ade50 / 0x43ae310 each period: wait on the event (100 ms), `BeginUpdatingAudioObjects`,
//!   `ActivateSpatialAudioObject(type)` the first time a position is needed, `GetBuffer`, copy the speaker's ring
//!   into it, `EndUpdatingAudioObjects`. After a timeout it calls `Reset` and gives up after four.
//! - 0x5bb6410 / 0x5bb64a0 at shutdown: `Release` on everything, `UnregisterEndpointNotificationCallback`.
//!
//! The title never calls `Start` or `Stop` in the code read, so a stream paces from the moment it is activated.
//! Anything the title does not call answers `E_NOTIMPL`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Com::{CoCreateInstance, CoTaskMemAlloc};
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent};

use super::engine::OutputMode;
use super::mixer::VoiceId;
use super::output::SharedMixer;
use super::spatial::{self, PERIOD_FRAMES, Pacer, Speaker, kind};
use super::{engine, with_engine};

type P = *mut c_void;
type Hr = i32;

const S_OK: Hr = 0;
const E_NOTIMPL: Hr = 0x8000_4001u32 as i32;
const E_NOINTERFACE: Hr = 0x8000_4002u32 as i32;
const E_POINTER: Hr = 0x8000_4003u32 as i32;
const E_INVALIDARG: Hr = 0x8007_0057u32 as i32;
const E_OUTOFMEMORY: Hr = 0x8007_000Eu32 as i32;
const CLASS_E_NOAGGREGATION: Hr = 0x8004_0110u32 as i32;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Guid(u32, u16, u16, [u8; 8]);

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    Guid(d1, d2, d3, d4)
}

pub const CLSID_MM_DEVICE_ENUMERATOR: Guid = guid(
    0xBCDE0395,
    0xE52F,
    0x467C,
    [0x8E, 0x3D, 0xC4, 0x57, 0x92, 0x91, 0x69, 0x2E],
);
const IID_UNKNOWN: Guid = guid(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_MM_DEVICE_ENUMERATOR: Guid = guid(
    0xA95664D2,
    0x9614,
    0x4F35,
    [0xA7, 0x46, 0xDE, 0x8D, 0xB6, 0x36, 0x17, 0xE6],
);
const IID_MM_DEVICE: Guid = guid(
    0xD666063F,
    0x1587,
    0x4E43,
    [0x81, 0xF1, 0xB9, 0x48, 0xE8, 0x07, 0x36, 0x3F],
);
const IID_MM_DEVICE_COLLECTION: Guid = guid(
    0x0BD7A1BE,
    0x7A1A,
    0x44DB,
    [0x83, 0x97, 0xCC, 0x53, 0x92, 0x38, 0x7B, 0x5E],
);
const IID_PROPERTY_STORE: Guid = guid(
    0x886D8EEB,
    0x8CF2,
    0x4446,
    [0x8D, 0x02, 0xCD, 0xBA, 0x1D, 0xBD, 0xCF, 0x99],
);
const IID_AUDIO_CLIENT: Guid = guid(
    0x1CB9AD4C,
    0xDBFA,
    0x4C32,
    [0xB1, 0x78, 0xC2, 0xF5, 0x68, 0xA7, 0x03, 0xB2],
);
const IID_SPATIAL_AUDIO_CLIENT: Guid = guid(
    0xBBF8E066,
    0xAAAA,
    0x49BE,
    [0x9A, 0x4D, 0xFD, 0x2A, 0x85, 0x8E, 0xA2, 0x7F],
);
const IID_SPATIAL_RENDER_STREAM: Guid = guid(
    0xBAB5F473,
    0xB423,
    0x477B,
    [0x85, 0xF5, 0xB5, 0xA3, 0x32, 0xA0, 0x41, 0x53],
);
const IID_SPATIAL_OBJECT: Guid = guid(
    0xDDE28967,
    0x521B,
    0x46E5,
    [0x8F, 0x00, 0xBD, 0x6F, 0x2B, 0xC8, 0xAB, 0x1D],
);
const IID_FORMAT_ENUMERATOR: Guid = guid(
    0xDCDAA5EF,
    0x1AB6,
    0x4A85,
    [0xB3, 0x32, 0xC5, 0x35, 0x2F, 0xA4, 0x01, 0xDD],
);

#[repr(C)]
struct WaveFormat {
    tag: u16,
    channels: u16,
    rate: u32,
    bytes_per_second: u32,
    block_align: u16,
    bits: u16,
    extra: u16,
}

/// The one object format offered: mono 32-bit float at 48 kHz.
static OBJECT_FORMAT: WaveFormat = WaveFormat {
    tag: 3,
    channels: 1,
    rate: spatial::OBJECT_RATE,
    bytes_per_second: spatial::OBJECT_RATE * 4,
    block_align: 4,
    bits: 32,
    extra: 0,
};

#[repr(C)]
struct Unknown {
    query: extern "system" fn(P, *const Guid, *mut P) -> Hr,
    add_ref: extern "system" fn(P) -> u32,
    release: extern "system" fn(P) -> u32,
}

trait Interface {
    const IIDS: &'static [Guid];
}

/// A COM object: the vtable pointer first, as the title reads it.
#[repr(C)]
struct Com<V: 'static, T> {
    vtbl: &'static V,
    refs: AtomicU32,
    state: T,
}

fn create<V: 'static, T>(vtbl: &'static V, state: T) -> P {
    Box::into_raw(Box::new(Com {
        vtbl,
        refs: AtomicU32::new(1),
        state,
    }))
    .cast()
}

/// # Safety
/// `this` must come from [`create`] with the same `V` and `T` and still be alive.
unsafe fn state<'a, V: 'static, T>(this: P) -> &'a T {
    // SAFETY: guaranteed by the caller.
    unsafe { &(*this.cast::<Com<V, T>>()).state }
}

extern "system" fn query<V: Interface + 'static, T>(this: P, iid: *const Guid, out: *mut P) -> Hr {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: COM passes a readable IID and a writable out pointer.
    let (wanted, known) = unsafe {
        (
            iid.read_unaligned(),
            wanted_is_known::<V>(iid.read_unaligned()),
        )
    };
    if known || wanted == IID_UNKNOWN {
        add_ref::<V, T>(this);
        // SAFETY: as above.
        unsafe { *out = this };
        S_OK
    } else {
        // SAFETY: as above.
        unsafe { *out = std::ptr::null_mut() };
        E_NOINTERFACE
    }
}

fn wanted_is_known<V: Interface>(iid: Guid) -> bool {
    V::IIDS.contains(&iid)
}

extern "system" fn add_ref<V: 'static, T>(this: P) -> u32 {
    // SAFETY: COM calls through an object made by `create`.
    unsafe { &(*this.cast::<Com<V, T>>()).refs }.fetch_add(1, Ordering::AcqRel) + 1
}

extern "system" fn release<V: 'static, T>(this: P) -> u32 {
    // SAFETY: as above; the last release frees the box.
    unsafe {
        let left = (*this.cast::<Com<V, T>>())
            .refs
            .fetch_sub(1, Ordering::AcqRel)
            - 1;
        if left == 0 {
            drop(Box::from_raw(this.cast::<Com<V, T>>()));
        }
        left
    }
}

macro_rules! unknown {
    ($v:ty, $t:ty) => {
        Unknown {
            query: query::<$v, $t>,
            add_ref: add_ref::<$v, $t>,
            release: release::<$v, $t>,
        }
    };
}

fn write<T>(out: *mut T, value: T) -> Hr {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: COM passes a writable out pointer.
    unsafe { out.write_unaligned(value) };
    S_OK
}

fn task_alloc(bytes: usize) -> *mut u8 {
    // SAFETY: plain allocation.
    unsafe { CoTaskMemAlloc(bytes) }.cast()
}

// IMMDeviceEnumerator

#[repr(C)]
struct EnumeratorVtbl {
    base: Unknown,
    enum_endpoints: extern "system" fn(P, u32, u32, *mut P) -> Hr,
    get_default: extern "system" fn(P, u32, u32, *mut P) -> Hr,
    get_device: extern "system" fn(P, *const u16, *mut P) -> Hr,
    register: extern "system" fn(P, P) -> Hr,
    unregister: extern "system" fn(P, P) -> Hr,
}

impl Interface for EnumeratorVtbl {
    const IIDS: &'static [Guid] = &[IID_MM_DEVICE_ENUMERATOR];
}

static ENUMERATOR: EnumeratorVtbl = EnumeratorVtbl {
    base: unknown!(EnumeratorVtbl, ()),
    enum_endpoints,
    get_default,
    get_device,
    register,
    unregister,
};

extern "system" fn enum_endpoints(_: P, _: u32, _: u32, out: *mut P) -> Hr {
    write(out, create(&COLLECTION, ()))
}

extern "system" fn get_default(_: P, _: u32, _: u32, out: *mut P) -> Hr {
    write(out, create(&DEVICE, ()))
}

extern "system" fn get_device(_: P, _: *const u16, out: *mut P) -> Hr {
    write(out, create(&DEVICE, ()))
}

extern "system" fn register(_: P, _: P) -> Hr {
    S_OK
}

extern "system" fn unregister(_: P, _: P) -> Hr {
    S_OK
}

// IMMDeviceCollection

#[repr(C)]
struct CollectionVtbl {
    base: Unknown,
    count: extern "system" fn(P, *mut u32) -> Hr,
    item: extern "system" fn(P, u32, *mut P) -> Hr,
}

impl Interface for CollectionVtbl {
    const IIDS: &'static [Guid] = &[IID_MM_DEVICE_COLLECTION];
}

static COLLECTION: CollectionVtbl = CollectionVtbl {
    base: unknown!(CollectionVtbl, ()),
    count: collection_count,
    item: collection_item,
};

extern "system" fn collection_count(_: P, out: *mut u32) -> Hr {
    write(out, 1)
}

extern "system" fn collection_item(_: P, index: u32, out: *mut P) -> Hr {
    if index == 0 {
        write(out, create(&DEVICE, ()))
    } else {
        E_INVALIDARG
    }
}

// IMMDevice

#[repr(C)]
struct DeviceVtbl {
    base: Unknown,
    activate: extern "system" fn(P, *const Guid, u32, P, *mut P) -> Hr,
    open_property_store: extern "system" fn(P, u32, *mut P) -> Hr,
    get_id: extern "system" fn(P, *mut *mut u16) -> Hr,
    get_state: extern "system" fn(P, *mut u32) -> Hr,
}

impl Interface for DeviceVtbl {
    const IIDS: &'static [Guid] = &[IID_MM_DEVICE];
}

static DEVICE: DeviceVtbl = DeviceVtbl {
    base: unknown!(DeviceVtbl, ()),
    activate: device_activate,
    open_property_store,
    get_id,
    get_state,
};

extern "system" fn device_activate(_: P, iid: *const Guid, _: u32, _: P, out: *mut P) -> Hr {
    if iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: COM passes a readable IID.
    let iid = unsafe { iid.read_unaligned() };
    if iid == IID_AUDIO_CLIENT {
        write(out, create(&AUDIO_CLIENT, ()))
    } else if iid == IID_SPATIAL_AUDIO_CLIENT {
        write(out, create(&SPATIAL_CLIENT, ()))
    } else {
        E_NOINTERFACE
    }
}

extern "system" fn open_property_store(_: P, _: u32, out: *mut P) -> Hr {
    write(out, create(&PROPERTY_STORE, ()))
}

extern "system" fn get_id(_: P, out: *mut *mut u16) -> Hr {
    let name: Vec<u16> = "leonida-default-render\0".encode_utf16().collect();
    let memory = task_alloc(name.len() * 2).cast::<u16>();
    if memory.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: the allocation holds `name.len()` UTF-16 units.
    unsafe { std::ptr::copy_nonoverlapping(name.as_ptr(), memory, name.len()) };
    write(out, memory)
}

extern "system" fn get_state(_: P, out: *mut u32) -> Hr {
    write(out, 1)
}

// IPropertyStore

#[repr(C)]
struct PropertyStoreVtbl {
    base: Unknown,
    count: extern "system" fn(P, *mut u32) -> Hr,
    get_at: extern "system" fn(P, u32, P) -> Hr,
    get_value: extern "system" fn(P, P, *mut PropVariant) -> Hr,
}

impl Interface for PropertyStoreVtbl {
    const IIDS: &'static [Guid] = &[IID_PROPERTY_STORE];
}

#[repr(C)]
struct PropVariant {
    vt: u16,
    reserved: [u16; 3],
    value: u64,
    extra: u64,
}

const VT_UI4: u16 = 19;
const ENDPOINT_FORM_FACTOR_SPEAKERS: u64 = 1;

static PROPERTY_STORE: PropertyStoreVtbl = PropertyStoreVtbl {
    base: unknown!(PropertyStoreVtbl, ()),
    count: store_count,
    get_at: store_get_at,
    get_value: store_get_value,
};

extern "system" fn store_count(_: P, out: *mut u32) -> Hr {
    write(out, 0)
}

extern "system" fn store_get_at(_: P, _: u32, _: P) -> Hr {
    E_INVALIDARG
}

extern "system" fn store_get_value(_: P, _: P, out: *mut PropVariant) -> Hr {
    write(
        out,
        PropVariant {
            vt: VT_UI4,
            reserved: [0; 3],
            value: ENDPOINT_FORM_FACTOR_SPEAKERS,
            extra: 0,
        },
    )
}

// IAudioClient: the title only asks for the mix format

#[repr(C)]
struct AudioClientVtbl {
    base: Unknown,
    initialize: extern "system" fn(P) -> Hr,
    buffer_size: extern "system" fn(P) -> Hr,
    stream_latency: extern "system" fn(P) -> Hr,
    current_padding: extern "system" fn(P) -> Hr,
    is_format_supported: extern "system" fn(P) -> Hr,
    mix_format: extern "system" fn(P, *mut *mut WaveFormat) -> Hr,
}

impl Interface for AudioClientVtbl {
    const IIDS: &'static [Guid] = &[IID_AUDIO_CLIENT];
}

static AUDIO_CLIENT: AudioClientVtbl = AudioClientVtbl {
    base: unknown!(AudioClientVtbl, ()),
    initialize: not_implemented,
    buffer_size: not_implemented,
    stream_latency: not_implemented,
    current_padding: not_implemented,
    is_format_supported: not_implemented,
    mix_format,
};

extern "system" fn not_implemented(_: P) -> Hr {
    E_NOTIMPL
}

extern "system" fn mix_format(_: P, out: *mut *mut WaveFormat) -> Hr {
    let memory = task_alloc(std::mem::size_of::<WaveFormat>()).cast::<WaveFormat>();
    if memory.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: the allocation is a whole `WaveFormat`.
    unsafe {
        memory.write_unaligned(WaveFormat {
            tag: 3,
            channels: 2,
            rate: spatial::OBJECT_RATE,
            bytes_per_second: spatial::OBJECT_RATE * 8,
            block_align: 8,
            bits: 32,
            extra: 0,
        });
    }
    write(out, memory)
}

// IAudioFormatEnumerator

#[repr(C)]
struct FormatEnumeratorVtbl {
    base: Unknown,
    count: extern "system" fn(P, *mut u32) -> Hr,
    format: extern "system" fn(P, u32, *mut *const WaveFormat) -> Hr,
}

impl Interface for FormatEnumeratorVtbl {
    const IIDS: &'static [Guid] = &[IID_FORMAT_ENUMERATOR];
}

static FORMAT_ENUMERATOR: FormatEnumeratorVtbl = FormatEnumeratorVtbl {
    base: unknown!(FormatEnumeratorVtbl, ()),
    count: formats_count,
    format: formats_format,
};

extern "system" fn formats_count(_: P, out: *mut u32) -> Hr {
    write(out, 1)
}

extern "system" fn formats_format(_: P, index: u32, out: *mut *const WaveFormat) -> Hr {
    if index == 0 {
        write(out, &OBJECT_FORMAT as *const WaveFormat)
    } else {
        E_INVALIDARG
    }
}

// ISpatialAudioClient

#[repr(C)]
struct SpatialClientVtbl {
    base: Unknown,
    static_position: extern "system" fn(P, u32, *mut f32, *mut f32, *mut f32) -> Hr,
    native_mask: extern "system" fn(P, *mut u32) -> Hr,
    max_dynamic: extern "system" fn(P, *const WaveFormat, *mut u32) -> Hr,
    format_enumerator: extern "system" fn(P, *mut P) -> Hr,
    max_frames: extern "system" fn(P, *const WaveFormat, *mut u32) -> Hr,
    format_supported: extern "system" fn(P, *const WaveFormat) -> Hr,
    stream_available: extern "system" fn(P, *const Guid, P) -> Hr,
    activate_stream: extern "system" fn(P, *const PropVariantBlob, *const Guid, *mut P) -> Hr,
}

impl Interface for SpatialClientVtbl {
    const IIDS: &'static [Guid] = &[IID_SPATIAL_AUDIO_CLIENT];
}

/// A `PROPVARIANT` of type `VT_BLOB`: the size at `+8` and the address at `+0x10`.
#[repr(C)]
struct PropVariantBlob {
    vt: u16,
    reserved: [u16; 3],
    size: u32,
    pad: u32,
    data: *const u8,
}

const VT_BLOB: u16 = 0x41;
const ACTIVATION_BYTES: u32 = 0x28;

static SPATIAL_CLIENT: SpatialClientVtbl = SpatialClientVtbl {
    base: unknown!(SpatialClientVtbl, ()),
    static_position,
    native_mask,
    max_dynamic,
    format_enumerator,
    max_frames,
    format_supported,
    stream_available,
    activate_stream,
};

extern "system" fn static_position(_: P, _: u32, _: *mut f32, _: *mut f32, _: *mut f32) -> Hr {
    E_NOTIMPL
}

extern "system" fn native_mask(_: P, out: *mut u32) -> Hr {
    write(out, kind::FRONT_LEFT | kind::FRONT_RIGHT)
}

extern "system" fn max_dynamic(_: P, _: *const WaveFormat, out: *mut u32) -> Hr {
    write(out, 0)
}

extern "system" fn format_enumerator(_: P, out: *mut P) -> Hr {
    write(out, create(&FORMAT_ENUMERATOR, ()))
}

extern "system" fn max_frames(_: P, _: *const WaveFormat, out: *mut u32) -> Hr {
    write(out, PERIOD_FRAMES as u32)
}

extern "system" fn format_supported(_: P, _: *const WaveFormat) -> Hr {
    S_OK
}

extern "system" fn stream_available(_: P, _: *const Guid, _: P) -> Hr {
    S_OK
}

extern "system" fn activate_stream(
    _this: P,
    activation: *const PropVariantBlob,
    iid: *const Guid,
    out: *mut P,
) -> Hr {
    if activation.is_null() || iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: COM passes readable activation parameters and IID.
    let (blob, iid) = unsafe { (&*activation, iid.read_unaligned()) };
    if iid != IID_SPATIAL_RENDER_STREAM {
        return E_NOINTERFACE;
    }
    if blob.vt != VT_BLOB || blob.size < ACTIVATION_BYTES || blob.data.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: a blob of at least 0x28 bytes: the format, the static mask, the dynamic object counts, the
    // category, the event handle and the notify object.
    let (mask, event) = unsafe {
        (
            blob.data.add(8).cast::<u32>().read_unaligned(),
            blob.data.add(0x18).cast::<usize>().read_unaligned(),
        )
    };
    match Stream::new(mask, event) {
        Some(stream) => write(out, create(&RENDER_STREAM, stream)),
        None => E_OUTOFMEMORY,
    }
}

// The stream and its objects

struct ObjectShared {
    kind: u32,
    volume: Mutex<f32>,
    buffer: Mutex<Vec<f32>>,
    raw: *mut f32,
    active: AtomicBool,
}

// SAFETY: `raw` points into `buffer`'s allocation, which is never resized; the title and the stream take turns
// on it inside one update, as the interface requires.
unsafe impl Send for ObjectShared {}
// SAFETY: as above.
unsafe impl Sync for ObjectShared {}

impl ObjectShared {
    fn new(kind: u32) -> ObjectShared {
        let mut buffer = vec![0.0f32; PERIOD_FRAMES];
        let raw = buffer.as_mut_ptr();
        ObjectShared {
            kind,
            volume: Mutex::new(1.0),
            buffer: Mutex::new(buffer),
            raw,
            active: AtomicBool::new(true),
        }
    }
}

struct Stream {
    mask: u32,
    mixer: SharedMixer,
    voice: VoiceId,
    objects: Mutex<Vec<Arc<ObjectShared>>>,
    pacer: Mutex<Option<Pacer>>,
}

impl Stream {
    fn new(mask: u32, event: usize) -> Option<Stream> {
        let mixer = with_engine(|e| e.mixer());
        Stream::with(mixer, mask, event, || engine().pump())
    }

    fn with(
        mixer: SharedMixer,
        mask: u32,
        event: usize,
        pump: impl Fn() + Send + 'static,
    ) -> Option<Stream> {
        let voice = spatial::add_voice(&mixer)?;
        let notify = move || {
            if event != 0 {
                // SAFETY: the title's event handle stays open while the stream does.
                unsafe { SetEvent(event as *mut c_void) };
            }
        };
        let pacer = Pacer::start(mixer.clone(), voice, notify, pump)?;
        Some(Stream {
            mask,
            mixer,
            voice,
            objects: Mutex::new(Vec::new()),
            pacer: Mutex::new(Some(pacer)),
        })
    }

    fn pace(&self, running: bool) {
        if let Some(pacer) = self.pacer.lock().ok().as_deref().and_then(Option::as_ref) {
            pacer.set_paused(!running);
        }
    }

    /// Mixes the period every active speaker was given into stereo and queues it; the buffers are silent again for
    /// the next update, so a period the title leaves unwritten plays as silence.
    fn end_update(&self) {
        let objects = self.objects.lock().map(|o| o.clone()).unwrap_or_default();
        let mut held = Vec::new();
        for object in objects.iter().filter(|o| o.active.load(Ordering::Acquire)) {
            if let Ok(samples) = object.buffer.lock() {
                let volume = object.volume.lock().map_or(1.0, |v| *v);
                held.push((object.kind, volume, samples));
            }
        }
        let mut stereo = vec![0.0f32; PERIOD_FRAMES * 2];
        let speakers: Vec<Speaker<'_>> = held
            .iter()
            .map(|(object, volume, samples)| Speaker {
                object: *object,
                volume: *volume,
                samples,
            })
            .collect();
        spatial::downmix(&speakers, &mut stereo);
        drop(speakers);
        for (_, _, samples) in &mut held {
            samples.iter_mut().for_each(|s| *s = 0.0);
        }
        drop(held);
        spatial::queue_period(&self.mixer, self.voice, stereo);
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        if let Ok(mut pacer) = self.pacer.lock() {
            pacer.take();
        }
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.remove_voice(self.voice);
        }
    }
}

#[repr(C)]
struct RenderStreamVtbl {
    base: Unknown,
    available: extern "system" fn(P, *mut u32) -> Hr,
    service: extern "system" fn(P, *const Guid, *mut P) -> Hr,
    start: extern "system" fn(P) -> Hr,
    stop: extern "system" fn(P) -> Hr,
    reset: extern "system" fn(P) -> Hr,
    begin: extern "system" fn(P, *mut u32, *mut u32) -> Hr,
    end: extern "system" fn(P) -> Hr,
    activate_object: extern "system" fn(P, u32, *mut P) -> Hr,
}

impl Interface for RenderStreamVtbl {
    const IIDS: &'static [Guid] = &[IID_SPATIAL_RENDER_STREAM];
}

/// # Safety
/// `this` must be a live render stream.
unsafe fn stream<'a>(this: P) -> &'a Stream {
    // SAFETY: guaranteed by the caller.
    unsafe { state::<RenderStreamVtbl, Stream>(this) }
}

static RENDER_STREAM: RenderStreamVtbl = RenderStreamVtbl {
    base: unknown!(RenderStreamVtbl, Stream),
    available: stream_available_objects,
    service: stream_service,
    start: stream_start,
    stop: stream_stop,
    reset: stream_reset,
    begin: stream_begin,
    end: stream_end,
    activate_object: stream_activate_object,
};

extern "system" fn stream_available_objects(_: P, out: *mut u32) -> Hr {
    write(out, 0)
}

extern "system" fn stream_service(_: P, _: *const Guid, _: *mut P) -> Hr {
    E_NOINTERFACE
}

extern "system" fn stream_start(this: P) -> Hr {
    // SAFETY: COM calls with the object it holds.
    unsafe { stream(this) }.pace(true);
    S_OK
}

extern "system" fn stream_stop(this: P) -> Hr {
    // SAFETY: as above.
    unsafe { stream(this) }.pace(false);
    S_OK
}

extern "system" fn stream_reset(this: P) -> Hr {
    // SAFETY: as above.
    let stream = unsafe { stream(this) };
    if let Ok(mut mixer) = stream.mixer.lock() {
        mixer.flush(stream.voice);
    }
    S_OK
}

extern "system" fn stream_begin(_: P, dynamic: *mut u32, frames: *mut u32) -> Hr {
    let first = write(dynamic, 0);
    if first != S_OK {
        return first;
    }
    write(frames, PERIOD_FRAMES as u32)
}

extern "system" fn stream_end(this: P) -> Hr {
    // SAFETY: as above.
    unsafe { stream(this) }.end_update();
    S_OK
}

extern "system" fn stream_activate_object(this: P, object: u32, out: *mut P) -> Hr {
    // SAFETY: as above.
    let stream = unsafe { stream(this) };
    if object == kind::DYNAMIC || object & stream.mask == 0 || object.count_ones() != 1 {
        return E_INVALIDARG;
    }
    let Ok(mut objects) = stream.objects.lock() else {
        return E_OUTOFMEMORY;
    };
    let shared = match objects.iter().find(|o| o.kind == object) {
        Some(existing) => existing.clone(),
        None => {
            let created = Arc::new(ObjectShared::new(object));
            objects.push(created.clone());
            created
        }
    };
    write(out, create(&SPATIAL_OBJECT, shared))
}

#[repr(C)]
struct ObjectVtbl {
    base: Unknown,
    buffer: extern "system" fn(P, *mut *mut f32, *mut u32) -> Hr,
    end_of_stream: extern "system" fn(P, u32) -> Hr,
    is_active: extern "system" fn(P, *mut i32) -> Hr,
    object_type: extern "system" fn(P, *mut u32) -> Hr,
    position: extern "system" fn(P, f32, f32, f32) -> Hr,
    volume: extern "system" fn(P, f32) -> Hr,
}

impl Interface for ObjectVtbl {
    const IIDS: &'static [Guid] = &[IID_SPATIAL_OBJECT];
}

/// # Safety
/// `this` must be a live spatial audio object.
unsafe fn object<'a>(this: P) -> &'a Arc<ObjectShared> {
    // SAFETY: guaranteed by the caller.
    unsafe { state::<ObjectVtbl, Arc<ObjectShared>>(this) }
}

static SPATIAL_OBJECT: ObjectVtbl = ObjectVtbl {
    base: unknown!(ObjectVtbl, Arc<ObjectShared>),
    buffer: object_buffer,
    end_of_stream: object_end_of_stream,
    is_active: object_is_active,
    object_type: object_kind,
    position: object_position,
    volume: object_volume,
};

extern "system" fn object_buffer(this: P, ptr: *mut *mut f32, bytes: *mut u32) -> Hr {
    // SAFETY: COM calls with the object it holds.
    let object = unsafe { object(this) };
    let first = write(ptr, object.raw);
    if first != S_OK {
        return first;
    }
    write(bytes, (PERIOD_FRAMES * 4) as u32)
}

extern "system" fn object_end_of_stream(this: P, _: u32) -> Hr {
    // SAFETY: as above.
    unsafe { object(this) }
        .active
        .store(false, Ordering::Release);
    S_OK
}

extern "system" fn object_is_active(this: P, out: *mut i32) -> Hr {
    // SAFETY: as above.
    let active = unsafe { object(this) }.active.load(Ordering::Acquire);
    write(out, i32::from(active))
}

extern "system" fn object_kind(this: P, out: *mut u32) -> Hr {
    // SAFETY: as above.
    write(out, unsafe { object(this) }.kind)
}

extern "system" fn object_position(_: P, _: f32, _: f32, _: f32) -> Hr {
    S_OK
}

extern "system" fn object_volume(this: P, volume: f32) -> Hr {
    // SAFETY: as above.
    if let Ok(mut slot) = unsafe { object(this) }.volume.lock() {
        *slot = volume;
    }
    S_OK
}

/// Makes the enumerator object `CoCreateInstance` would return.
fn create_enumerator(iid: *const Guid, out: *mut P) -> Hr {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: COM passes a readable IID when it is not null.
    if !iid.is_null()
        && unsafe { iid.read_unaligned() } != IID_MM_DEVICE_ENUMERATOR
        && unsafe { iid.read_unaligned() } != IID_UNKNOWN
    {
        return E_NOINTERFACE;
    }
    write(out, create(&ENUMERATOR, ()))
}

// Choosing between the host's spatial audio and this one

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Use the host's when it can open the stream the title asks for, ours otherwise.
    Auto,
    Host,
    Emulate,
}

impl Policy {
    pub fn from_setting(value: Option<&str>) -> Policy {
        match value.map(str::to_ascii_lowercase).as_deref() {
            Some("host") => Policy::Host,
            Some("emulate") => Policy::Emulate,
            _ => Policy::Auto,
        }
    }
}

fn wants_emulation() -> bool {
    if with_engine(|e| e.mode()) == OutputMode::Off {
        return true;
    }
    match Policy::from_setting(std::env::var("LEONIDA_SPATIAL_AUDIO").ok().as_deref()) {
        Policy::Emulate => true,
        Policy::Host => false,
        Policy::Auto => {
            static HOST_WORKS: OnceLock<bool> = OnceLock::new();
            let works = *HOST_WORKS.get_or_init(host_opens_a_stereo_bed);
            engine().note_once("spatial-policy", || {
                if works {
                    "the host's spatial audio opened a stereo static bed: the title plays through it".into()
                } else {
                    "the host's spatial audio could not open a stereo static bed: the title plays through Leonida's own output".into()
                }
            });
            !works
        }
    }
}

/// Opens, with the host's own objects, the stream the title asks for: a stereo static bed and no dynamic objects.
fn host_opens_a_stereo_bed() -> bool {
    let mut enumerator: P = std::ptr::null_mut();
    // SAFETY: plain COM creation with valid pointers.
    let hr = unsafe {
        CoCreateInstance(
            (&CLSID_MM_DEVICE_ENUMERATOR as *const Guid).cast(),
            std::ptr::null_mut(),
            0x17,
            (&IID_MM_DEVICE_ENUMERATOR as *const Guid).cast(),
            &mut enumerator,
        )
    };
    if hr < 0 || enumerator.is_null() {
        return false;
    }
    let mut device: P = std::ptr::null_mut();
    let mut client: P = std::ptr::null_mut();
    let mut formats: P = std::ptr::null_mut();
    let mut format: *const WaveFormat = std::ptr::null();
    let mut stream: P = std::ptr::null_mut();
    // SAFETY: each call goes through the vtable of the interface that was just obtained, with the signatures the
    // interface documents; every object obtained is released below.
    unsafe {
        let ok = (vtbl::<EnumeratorVtbl>(enumerator).get_default)(enumerator, 0, 0, &mut device)
            >= 0
            && (vtbl::<DeviceVtbl>(device).activate)(
                device,
                &IID_SPATIAL_AUDIO_CLIENT,
                1,
                std::ptr::null_mut(),
                &mut client,
            ) >= 0
            && (vtbl::<SpatialClientVtbl>(client).format_enumerator)(client, &mut formats) >= 0
            && (vtbl::<FormatEnumeratorVtbl>(formats).format)(formats, 0, &mut format) >= 0
            && {
                let event = CreateEventW(std::ptr::null(), 0, 0, std::ptr::null());
                let mut params = [0u8; ACTIVATION_BYTES as usize];
                params[..8].copy_from_slice(&(format as usize).to_le_bytes());
                params[8..12]
                    .copy_from_slice(&(kind::FRONT_LEFT | kind::FRONT_RIGHT).to_le_bytes());
                params[0x14..0x18].copy_from_slice(&6u32.to_le_bytes());
                params[0x18..0x20].copy_from_slice(&(event as usize).to_le_bytes());
                let blob = PropVariantBlob {
                    vt: VT_BLOB,
                    reserved: [0; 3],
                    size: ACTIVATION_BYTES,
                    pad: 0,
                    data: params.as_ptr(),
                };
                let hr = (vtbl::<SpatialClientVtbl>(client).activate_stream)(
                    client,
                    &blob,
                    &IID_SPATIAL_RENDER_STREAM,
                    &mut stream,
                );
                if !event.is_null() {
                    CloseHandle(event);
                }
                hr >= 0
            };
        for object in [stream, formats, client, device, enumerator] {
            if !object.is_null() {
                (vtbl::<Unknown>(object).release)(object);
            }
        }
        ok
    }
}

/// # Safety
/// `object` must be a live COM object whose vtable starts with `V`'s layout.
unsafe fn vtbl<'a, V>(object: P) -> &'a V {
    // SAFETY: guaranteed by the caller.
    unsafe { &**object.cast::<*const V>() }
}

/// `CoCreateInstance` for the title: our enumerator when the output policy says so, the host's call otherwise.
pub(super) extern "system" fn co_create_instance(
    clsid: *const Guid,
    outer: P,
    context: u32,
    iid: *const Guid,
    out: *mut P,
) -> Hr {
    // SAFETY: COM passes a readable CLSID when it is not null.
    let ours = !clsid.is_null() && unsafe { clsid.read_unaligned() } == CLSID_MM_DEVICE_ENUMERATOR;
    if ours && wants_emulation() {
        if !outer.is_null() {
            return CLASS_E_NOAGGREGATION;
        }
        return create_enumerator(iid, out);
    }
    // SAFETY: the arguments are the caller's, passed through.
    unsafe { CoCreateInstance(clsid.cast(), outer, context, iid.cast(), out) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asking_the_host_whether_it_can_open_the_stream_answers_either_way_without_crashing() {
        // SAFETY: plain COM initialisation for this thread.
        unsafe { windows_sys::Win32::System::Com::CoInitializeEx(std::ptr::null(), 0) };
        let answer = host_opens_a_stereo_bed();
        eprintln!("host spatial audio opens a stereo static bed: {answer}");
    }

    #[test]
    fn the_policy_setting_is_read_without_regard_to_case_and_defaults_to_auto() {
        assert_eq!(Policy::from_setting(None), Policy::Auto);
        assert_eq!(Policy::from_setting(Some("HOST")), Policy::Host);
        assert_eq!(Policy::from_setting(Some("Emulate")), Policy::Emulate);
        assert_eq!(Policy::from_setting(Some("nonsense")), Policy::Auto);
    }

    use super::super::mixer::Mixer;

    /// A render stream on a mixer of its own that nothing else drains, with no pump and no event.
    fn quiet_stream(mask: u32) -> (P, SharedMixer) {
        let mixer: SharedMixer = Arc::new(Mutex::new(Mixer::new(spatial::OBJECT_RATE)));
        let stream = Stream::with(mixer.clone(), mask, 0, || {}).expect("a stream");
        (create(&RENDER_STREAM, stream), mixer)
    }

    fn activate(stream: P, object: u32) -> Result<P, Hr> {
        let mut out: P = std::ptr::null_mut();
        // SAFETY: `stream` is a live render stream made by `create`.
        let hr =
            unsafe { (vtbl::<RenderStreamVtbl>(stream).activate_object)(stream, object, &mut out) };
        if hr == S_OK { Ok(out) } else { Err(hr) }
    }

    fn fill(object: P, value: f32) {
        let (mut ptr, mut bytes) = (std::ptr::null_mut::<f32>(), 0u32);
        // SAFETY: `object` is a live spatial audio object and its buffer holds a period of floats.
        unsafe {
            assert_eq!(
                (vtbl::<ObjectVtbl>(object).buffer)(object, &mut ptr, &mut bytes),
                S_OK
            );
            assert_eq!(bytes as usize, PERIOD_FRAMES * 4);
            std::slice::from_raw_parts_mut(ptr, PERIOD_FRAMES).fill(value);
        }
    }

    fn end(stream: P) {
        // SAFETY: as above.
        assert_eq!(
            unsafe { (vtbl::<RenderStreamVtbl>(stream).end)(stream) },
            S_OK
        );
    }

    fn render(mixer: &SharedMixer, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; frames * 2];
        mixer.lock().unwrap().render(&mut out);
        out
    }

    #[test]
    fn a_period_written_to_the_front_speakers_reaches_the_mixer_as_stereo_and_buffers_start_silent_again()
     {
        let (stream, mixer) = quiet_stream(kind::FRONT_LEFT | kind::FRONT_RIGHT);
        let (mut dynamic, mut frames) = (9u32, 0u32);
        // SAFETY: `stream` is a live render stream.
        unsafe {
            assert_eq!(
                (vtbl::<RenderStreamVtbl>(stream).begin)(stream, &mut dynamic, &mut frames),
                S_OK
            );
        }
        assert_eq!((dynamic, frames as usize), (0, PERIOD_FRAMES));
        let left = activate(stream, kind::FRONT_LEFT).unwrap();
        let right = activate(stream, kind::FRONT_RIGHT).unwrap();
        fill(left, 0.5);
        fill(right, 0.25);
        end(stream);
        let out = render(&mixer, PERIOD_FRAMES);
        assert!((out[0] - 0.5).abs() < 1e-6 && (out[1] - 0.25).abs() < 1e-6);
        assert!((out[2 * (PERIOD_FRAMES - 1)] - 0.5).abs() < 1e-6);
        end(stream);
        let silent = render(&mixer, PERIOD_FRAMES);
        assert!(
            silent.iter().all(|s| *s == 0.0),
            "an unwritten period plays as silence"
        );
        assert!(mixer.lock().unwrap().take_completions().is_empty());
        release::<RenderStreamVtbl, Stream>(stream);
        release::<ObjectVtbl, Arc<ObjectShared>>(left);
        release::<ObjectVtbl, Arc<ObjectShared>>(right);
        assert_eq!(
            mixer.lock().unwrap().voice_count(),
            0,
            "the last release removes the voice"
        );
    }

    #[test]
    fn positions_outside_the_mask_and_dynamic_objects_are_refused_and_an_active_position_is_shared()
    {
        let (stream, _mixer) = quiet_stream(kind::FRONT_LEFT | kind::FRONT_CENTER);
        assert_eq!(activate(stream, kind::DYNAMIC).err(), Some(E_INVALIDARG));
        assert_eq!(
            activate(stream, kind::FRONT_RIGHT).err(),
            Some(E_INVALIDARG)
        );
        assert_eq!(
            activate(stream, kind::FRONT_LEFT | kind::FRONT_CENTER).err(),
            Some(E_INVALIDARG)
        );
        let first = activate(stream, kind::FRONT_CENTER).unwrap();
        let again = activate(stream, kind::FRONT_CENTER).unwrap();
        fill(first, 0.5);
        let (mut ptr, mut bytes) = (std::ptr::null_mut::<f32>(), 0u32);
        // SAFETY: `again` is a live object.
        unsafe { (vtbl::<ObjectVtbl>(again).buffer)(again, &mut ptr, &mut bytes) };
        // SAFETY: the buffer holds a period of floats.
        assert_eq!(unsafe { *ptr }, 0.5, "both handles name one buffer");
        let mut kind_out = 0u32;
        // SAFETY: as above.
        unsafe { (vtbl::<ObjectVtbl>(again).object_type)(again, &mut kind_out) };
        assert_eq!(kind_out, kind::FRONT_CENTER);
        release::<ObjectVtbl, Arc<ObjectShared>>(first);
        release::<ObjectVtbl, Arc<ObjectShared>>(again);
        release::<RenderStreamVtbl, Stream>(stream);
    }

    #[test]
    fn reset_drops_what_waits_and_volume_scales_one_position() {
        let (stream, mixer) = quiet_stream(kind::FRONT_LEFT);
        let left = activate(stream, kind::FRONT_LEFT).unwrap();
        fill(left, 1.0);
        end(stream);
        assert_eq!(mixer.lock().unwrap().voice_count(), 1);
        // SAFETY: live render stream and object.
        unsafe {
            assert_eq!((vtbl::<RenderStreamVtbl>(stream).reset)(stream), S_OK);
            assert_eq!((vtbl::<ObjectVtbl>(left).volume)(left, 0.5), S_OK);
        }
        assert!(
            render(&mixer, 8).iter().all(|s| *s == 0.0),
            "the queued period is gone"
        );
        fill(left, 1.0);
        end(stream);
        assert!((render(&mixer, 1)[0] - 0.5).abs() < 1e-6);
        release::<ObjectVtbl, Arc<ObjectShared>>(left);
        release::<RenderStreamVtbl, Stream>(stream);
    }

    #[test]
    fn the_enumerator_walks_to_a_device_a_mix_format_and_a_spatial_client_the_way_the_title_does() {
        let mut enumerator: P = std::ptr::null_mut();
        assert_eq!(
            create_enumerator(&IID_MM_DEVICE_ENUMERATOR, &mut enumerator),
            S_OK
        );
        let mut other: P = std::ptr::null_mut();
        assert_eq!(
            create_enumerator(&IID_AUDIO_CLIENT, &mut other),
            E_NOINTERFACE
        );
        // SAFETY: every object below was just made by this module and is released once.
        unsafe {
            let en = vtbl::<EnumeratorVtbl>(enumerator);
            let mut collection: P = std::ptr::null_mut();
            assert_eq!((en.enum_endpoints)(enumerator, 0, 1, &mut collection), S_OK);
            let mut count = 0u32;
            assert_eq!(
                (vtbl::<CollectionVtbl>(collection).count)(collection, &mut count),
                S_OK
            );
            assert_eq!(count, 1);
            let mut device: P = std::ptr::null_mut();
            assert_eq!(
                (vtbl::<CollectionVtbl>(collection).item)(collection, 0, &mut device),
                S_OK
            );
            assert_ne!(
                (vtbl::<CollectionVtbl>(collection).item)(collection, 1, &mut other),
                S_OK
            );
            let mut props: P = std::ptr::null_mut();
            assert_eq!(
                (vtbl::<DeviceVtbl>(device).open_property_store)(device, 0, &mut props),
                S_OK
            );
            let mut value = PropVariant {
                vt: 0,
                reserved: [0; 3],
                value: 0,
                extra: 0,
            };
            assert_eq!(
                (vtbl::<PropertyStoreVtbl>(props).get_value)(
                    props,
                    std::ptr::null_mut(),
                    &mut value
                ),
                S_OK
            );
            assert_eq!(
                (value.vt, value.value as u32),
                (VT_UI4, 1),
                "speakers, not headphones"
            );

            let mut default: P = std::ptr::null_mut();
            assert_eq!((en.get_default)(enumerator, 0, 0, &mut default), S_OK);
            let mut id: *mut u16 = std::ptr::null_mut();
            assert_eq!((vtbl::<DeviceVtbl>(default).get_id)(default, &mut id), S_OK);
            assert_eq!(*id, u16::from(b'l'));
            windows_sys::Win32::System::Com::CoTaskMemFree(id.cast());

            let mut audio: P = std::ptr::null_mut();
            let device_vtbl = vtbl::<DeviceVtbl>(default);
            assert_eq!(
                (device_vtbl.activate)(
                    default,
                    &IID_AUDIO_CLIENT,
                    1,
                    std::ptr::null_mut(),
                    &mut audio
                ),
                S_OK
            );
            let mut format: *mut WaveFormat = std::ptr::null_mut();
            assert_eq!(
                (vtbl::<AudioClientVtbl>(audio).mix_format)(audio, &mut format),
                S_OK
            );
            assert_eq!(((*format).channels, (*format).rate), (2, 48_000));
            windows_sys::Win32::System::Com::CoTaskMemFree(format.cast());

            let mut spatial: P = std::ptr::null_mut();
            assert_eq!(
                (device_vtbl.activate)(
                    default,
                    &IID_SPATIAL_AUDIO_CLIENT,
                    1,
                    std::ptr::null_mut(),
                    &mut spatial
                ),
                S_OK
            );
            assert_eq!(
                (device_vtbl.activate)(
                    default,
                    &IID_SPATIAL_RENDER_STREAM,
                    1,
                    std::ptr::null_mut(),
                    &mut other
                ),
                E_NOINTERFACE
            );
            let client = vtbl::<SpatialClientVtbl>(spatial);
            let mut formats: P = std::ptr::null_mut();
            assert_eq!((client.format_enumerator)(spatial, &mut formats), S_OK);
            let mut offered: *const WaveFormat = std::ptr::null();
            assert_eq!(
                (vtbl::<FormatEnumeratorVtbl>(formats).format)(formats, 0, &mut offered),
                S_OK
            );
            assert_eq!(
                ((*offered).channels, (*offered).bits, (*offered).tag),
                (1, 32, 3)
            );
            assert_eq!((client.format_supported)(spatial, offered), S_OK);
            let mut dynamic = 9u32;
            assert_eq!((client.max_dynamic)(spatial, offered, &mut dynamic), S_OK);
            assert_eq!(dynamic, 0);

            let mut blob_bytes = [0u8; ACTIVATION_BYTES as usize];
            blob_bytes[8..12]
                .copy_from_slice(&(kind::FRONT_LEFT | kind::FRONT_RIGHT).to_le_bytes());
            let blob = PropVariantBlob {
                vt: VT_BLOB,
                reserved: [0; 3],
                size: ACTIVATION_BYTES,
                pad: 0,
                data: blob_bytes.as_ptr(),
            };
            let mut stream: P = std::ptr::null_mut();
            let short = PropVariantBlob { size: 8, ..blob };
            assert_eq!(
                (client.activate_stream)(spatial, &short, &IID_SPATIAL_RENDER_STREAM, &mut stream),
                E_INVALIDARG
            );
            assert_eq!(
                (client.activate_stream)(spatial, &blob, &IID_AUDIO_CLIENT, &mut stream),
                E_NOINTERFACE
            );
            crate::audio::set_output_mode(OutputMode::Off);
            assert_eq!(
                (client.activate_stream)(spatial, &blob, &IID_SPATIAL_RENDER_STREAM, &mut stream),
                S_OK
            );
            assert!(!stream.is_null());
            for object in [
                stream, formats, spatial, audio, default, props, device, collection, enumerator,
            ] {
                (vtbl::<Unknown>(object).release)(object);
            }
        }
    }

    #[test]
    fn the_object_format_is_the_one_wave_format_the_enumerator_offers() {
        assert_eq!(std::mem::size_of::<PropVariant>(), 24);
        assert_eq!(std::mem::size_of::<PropVariantBlob>(), 24);
        assert_eq!(OBJECT_FORMAT.channels, 1);
        assert_eq!(OBJECT_FORMAT.rate, 48_000);
        assert_eq!(OBJECT_FORMAT.bytes_per_second, 192_000);
    }
}
