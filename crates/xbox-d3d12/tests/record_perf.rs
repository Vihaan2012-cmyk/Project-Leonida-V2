//! What recording a game-shaped frame into a command list and converting it for replay costs on the CPU, and how
//! many allocations each step makes.
//!
//! The benchmark is `#[ignore]`d: `cargo test --release -p xbox-d3d12 --test record_perf -- --ignored --nocapture`.
//! Every test skips without a GPU that can host the layer.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use ash::vk;
use xbox_d3d12::assembly::bridge::{self, BufferRef, Resolver};
use xbox_d3d12::assembly::indirect::IndirectLayout;
use xbox_d3d12::assembly::{Assembly, AssemblyConfig, create_assembled_device_on};
use xbox_d3d12::com::{ComPtr, Guid, iid};
use xbox_d3d12::execute::{Command, ImageRef, IndirectKind, TargetView};
use xbox_d3d12::pipeline::Pipeline;
use xbox_d3d12::root::{HeapBases, RootSignature};
use xbox_d3d12::stream::CommandStream;

struct Counting;

static ALLOCS: AtomicU64 = AtomicU64::new(0);

// SAFETY: every call is forwarded to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: as the caller promises.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as the caller promises.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: as the caller promises.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

type Ptr = *mut c_void;

static ONE_GPU_TEST_AT_A_TIME: Mutex<()> = Mutex::new(());

struct Harness {
    device: Ptr,
    _guard: MutexGuard<'static, ()>,
}

fn harness() -> Option<Harness> {
    let guard = ONE_GPU_TEST_AT_A_TIME
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    static ASSEMBLY: OnceLock<Option<Arc<Assembly>>> = OnceLock::new();
    let asm = ASSEMBLY
        .get_or_init(
            || match Assembly::build(&AssemblyConfig::offscreen(64, 64)) {
                Ok(asm) => Some(asm),
                Err(e) => {
                    eprintln!("skipped: {e}");
                    None
                }
            },
        )
        .clone()?;
    let params = [0u32; 24];
    let mut device = std::ptr::null_mut();
    // SAFETY: valid pointers for each argument.
    let result = unsafe {
        create_assembled_device_on(&asm, params.as_ptr(), [7u8; 16].as_ptr(), &mut device)
    };
    assert_eq!(result, 0);
    Some(Harness {
        device,
        _guard: guard,
    })
}

unsafe fn slot<F: Copy>(object: Ptr, index: usize) -> F {
    // SAFETY: the object's first word is its vtable; the caller names the right signature.
    unsafe {
        let address = *(*(object as *const *const usize)).add(index);
        std::mem::transmute_copy::<usize, F>(&address)
    }
}

fn guid(g: &Guid) -> [u8; 16] {
    // SAFETY: `Guid` is 16 bytes of plain data.
    unsafe { std::mem::transmute::<Guid, [u8; 16]>(*g) }
}

fn buffer_desc(bytes: u64) -> [u8; 56] {
    let mut d = [0u8; 56];
    let mut put = |at: usize, v: &[u8]| d[at..at + v.len()].copy_from_slice(v);
    put(0, &1u32.to_le_bytes());
    put(16, &bytes.to_le_bytes());
    put(24, &1u32.to_le_bytes());
    put(28, &(1u32 | 1 << 16).to_le_bytes());
    put(36, &1u32.to_le_bytes());
    put(44, &1u32.to_le_bytes());
    put(48, &4u32.to_le_bytes());
    d
}

struct Frame {
    list: Ptr,
    allocator: Ptr,
    buffers: Vec<Ptr>,
}

impl Harness {
    fn frame(&self) -> Frame {
        let (mut allocator, mut list) = (std::ptr::null_mut(), std::ptr::null_mut());
        // SAFETY: device slots 9, 27 and 73.
        unsafe {
            let create_allocator: unsafe extern "win64" fn(Ptr, u32, *const u8, *mut Ptr) -> u32 =
                slot(self.device, 9);
            assert_eq!(
                create_allocator(
                    self.device,
                    0,
                    guid(&iid::COMMAND_ALLOCATOR).as_ptr(),
                    &mut allocator
                ),
                0
            );
            let create_list: unsafe extern "win64" fn(
                Ptr,
                *const u32,
                Ptr,
                Ptr,
                *const u8,
                *mut Ptr,
            ) -> u32 = slot(self.device, 73);
            let desc = [0u32; 4];
            assert_eq!(
                create_list(
                    self.device,
                    desc.as_ptr(),
                    allocator,
                    std::ptr::null_mut(),
                    guid(&iid::GRAPHICS_COMMAND_LIST).as_ptr(),
                    &mut list
                ),
                0
            );
        }
        let buffers = (0..13)
            .map(|_| {
                let heap = [1u32, 0, 0, 1, 1];
                let desc = buffer_desc(0x1000);
                let mut out = std::ptr::null_mut();
                // SAFETY: device slot 27 with the PC argument order.
                unsafe {
                    let create: unsafe extern "win64" fn(
                        Ptr,
                        *const u32,
                        u32,
                        *const u8,
                        u32,
                        *const c_void,
                        *const u8,
                        *mut Ptr,
                    ) -> u32 = slot(self.device, 27);
                    assert_eq!(
                        create(
                            self.device,
                            heap.as_ptr(),
                            0,
                            desc.as_ptr(),
                            8,
                            std::ptr::null(),
                            guid(&iid::RESOURCE).as_ptr(),
                            &mut out,
                        ),
                        0
                    );
                }
                out
            })
            .collect();
        Frame {
            list,
            allocator,
            buffers,
        }
    }
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as u32) % below
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Viewport([f32; 6]);

#[repr(C)]
#[derive(Clone, Copy)]
struct VertexView {
    location: u64,
    size: u32,
    stride: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct IndexView {
    location: u64,
    size: u32,
    format: u32,
}

#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct RawBarrier([u8; 32]);

fn transition(resource: Ptr, before: u32, after: u32) -> RawBarrier {
    let mut b = [0u8; 32];
    b[8..16].copy_from_slice(&(resource as u64).to_le_bytes());
    b[16..20].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    b[20..24].copy_from_slice(&before.to_le_bytes());
    b[24..28].copy_from_slice(&after.to_le_bytes());
    RawBarrier(b)
}

#[derive(Clone, Copy)]
struct Shape {
    passes: usize,
    draws_per_pass: usize,
}

/// The calls of a frame shaped like the game's, as `replay_perf`'s `game_frame` makes them.
fn record_frame(frame: &Frame, shape: Shape) {
    let list = frame.list;
    let mut rng = Lcg(7);
    // SAFETY: list slots with the signatures of the PC interface.
    unsafe {
        let barrier: unsafe extern "win64" fn(Ptr, u32, *const RawBarrier) = slot(list, 26);
        let set_targets: unsafe extern "win64" fn(Ptr, u32, *const u64, u32, *const u64) =
            slot(list, 50);
        let clear: unsafe extern "win64" fn(Ptr, u64, *const [f32; 4], u32, *const c_void) =
            slot(list, 52);
        let viewports: unsafe extern "win64" fn(Ptr, u32, *const Viewport) = slot(list, 21);
        let scissors: unsafe extern "win64" fn(Ptr, u32, *const [i32; 4]) = slot(list, 22);
        let topology: unsafe extern "win64" fn(Ptr, u32) = slot(list, 20);
        let constants: unsafe extern "win64" fn(Ptr, u32, u32, *const u32, u32) = slot(list, 40);
        let table: unsafe extern "win64" fn(Ptr, u32, u64) = slot(list, 36);
        let cbv: unsafe extern "win64" fn(Ptr, u32, u64) = slot(list, 42);
        let vertex: unsafe extern "win64" fn(Ptr, u32, u32, *const VertexView) = slot(list, 48);
        let index: unsafe extern "win64" fn(Ptr, *const IndexView) = slot(list, 47);
        let draw_indexed: unsafe extern "win64" fn(Ptr, u32, u32, u32, i32, u32) = slot(list, 13);
        let draw: unsafe extern "win64" fn(Ptr, u32, u32, u32, u32) = slot(list, 12);
        let dispatch: unsafe extern "win64" fn(Ptr, u32, u32, u32) = slot(list, 14);
        let heaps: unsafe extern "win64" fn(Ptr, u32, *const Ptr) = slot(list, 32);
        heaps(list, 0, std::ptr::null());
        for pass in 0..shape.passes {
            let transitions: Vec<RawBarrier> = (0..13)
                .map(|k| {
                    let (before, after) = match k % 3 {
                        0 => (4, 0x80),
                        1 => (0x80, 4),
                        _ => (4, 0x800),
                    };
                    transition(frame.buffers[(pass + k) % frame.buffers.len()], before, after)
                })
                .collect();
            barrier(list, transitions.len() as u32, transitions.as_ptr());
            let target = 0x1000u64;
            set_targets(list, 1, &target, 0, std::ptr::null());
            clear(list, target, &[0.0; 4], 0, std::ptr::null());
            viewports(list, 1, &Viewport([0.0, 0.0, 64.0, 64.0, 0.0, 1.0]));
            scissors(list, 1, &[0, 0, 64, 64]);
            topology(list, 4);
            for d in 0..shape.draws_per_pass {
                if d % 3 == 0 {
                    let values = [rng.next(1000), rng.next(1000), 3, 4];
                    constants(list, 0, 4, values.as_ptr(), 0);
                }
                if d % 4 == 0 {
                    table(list, 1, 0x1000 + u64::from(rng.next(64)) * 32);
                }
                if d % 7 == 0 {
                    cbv(list, 2, 0x2000 + u64::from(rng.next(256)) * 256);
                }
                if d % 5 == 0 {
                    topology(list, 4);
                }
                if d % 2 == 0 {
                    let mesh = u64::from(rng.next(64)) * 256;
                    let views = [
                        VertexView {
                            location: 0x10000 + mesh,
                            size: 1024,
                            stride: 32,
                        },
                        VertexView {
                            location: 0x14000 + mesh,
                            size: 512,
                            stride: 16,
                        },
                    ];
                    vertex(list, 0, 2, views.as_ptr());
                    index(
                        list,
                        &IndexView {
                            location: 0x40000 + mesh,
                            size: 512,
                            format: 57,
                        },
                    );
                }
                if d % 8 != 7 {
                    draw_indexed(list, 36, 1, 0, 0, 0);
                } else {
                    draw(list, 3, 1, 0, 0);
                }
            }
            if pass % 5 == 4 {
                dispatch(list, 4, 1, 1);
            }
        }
    }
}

struct Stub {
    image: ImageRef,
    target: TargetView,
}

impl Resolver for Stub {
    fn buffer(&self, _resource: &ComPtr) -> Option<BufferRef> {
        Some(BufferRef {
            address: 0x1000,
            size: 0x1000,
        })
    }

    fn image(&self, _resource: &ComPtr) -> Option<ImageRef> {
        Some(self.image)
    }

    fn buffer_containing(&self, _address: u64) -> Option<BufferRef> {
        None
    }

    fn root_signature(&self, _object: &ComPtr) -> Option<Arc<RootSignature>> {
        None
    }

    fn pipeline(&self, _object: &ComPtr) -> Option<Arc<Pipeline>> {
        None
    }

    fn command_signature(&self, _object: &ComPtr) -> Option<IndirectLayout> {
        Some(IndirectLayout {
            kind: Some(IndirectKind::Draw),
            stride: 16,
        })
    }

    fn heap_bases(&self, _heaps: &[ComPtr]) -> HeapBases {
        HeapBases::default()
    }

    fn color_target(&self, _handle: u64) -> Option<TargetView> {
        Some(self.target)
    }

    fn depth_target(&self, _handle: u64) -> Option<TargetView> {
        None
    }

    fn bundle(&self, _object: &ComPtr) -> Option<Arc<CommandStream>> {
        None
    }

    fn note(&self, _text: &str) {}
}

fn stub() -> Stub {
    Stub {
        image: ImageRef {
            image: vk::Image::null(),
            format: vk::Format::R8G8B8A8_UNORM,
            aspect: vk::ImageAspectFlags::COLOR,
            extent: [64, 64, 1],
            mips: 1,
            layers: 1,
        },
        target: TargetView {
            view: vk::ImageView::null(),
            format: vk::Format::R8G8B8A8_UNORM,
            extent: [64, 64],
            samples: vk::SampleCountFlags::TYPE_1,
        },
    }
}

fn median(times: &mut [Duration]) -> Duration {
    times.sort();
    times[times.len() / 2]
}

fn allocations() -> u64 {
    ALLOCS.load(Ordering::Relaxed)
}

#[test]
#[ignore = "benchmark: run with --release --ignored --nocapture"]
fn recording_and_converting_a_game_frame_costs_microseconds_per_draw() {
    let Some(h) = harness() else { return };
    let frame = h.frame();
    let shape = Shape {
        passes: 40,
        draws_per_pass: 250,
    };
    let draws = (shape.passes * shape.draws_per_pass) as f64;
    let resolver = stub();
    let (mut record, mut convert, mut free_stream, mut free_converted) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut counts = [0u64; 4];
    let mut commands = 0;
    let mut converted_commands = 0;
    // SAFETY: list slots 9 and 10 and `closed_stream`'s list is a list of this layer.
    unsafe {
        let close: unsafe extern "win64" fn(Ptr) -> u32 = slot(frame.list, 9);
        let reset: unsafe extern "win64" fn(Ptr, Ptr, Ptr) -> u32 = slot(frame.list, 10);
        for round in 0..60 {
            let a0 = allocations();
            let t0 = Instant::now();
            record_frame(&frame, shape);
            assert_eq!(close(frame.list), 0);
            let t1 = t0.elapsed();
            let a1 = allocations();
            let stream = xbox_d3d12::list::closed_stream(frame.list).expect("a closed list");
            commands = stream.commands.len();
            let t = Instant::now();
            let converted = bridge::convert(&stream, &resolver);
            let t2 = t.elapsed();
            let a2 = allocations();
            converted_commands = converted.commands.len();
            let t = Instant::now();
            drop(converted);
            let t3 = t.elapsed();
            let a3 = allocations();
            let t = Instant::now();
            assert_eq!(reset(frame.list, frame.allocator, std::ptr::null_mut()), 0);
            drop(stream);
            let t4 = t.elapsed();
            let a4 = allocations();
            if round >= 10 {
                record.push(t1);
                convert.push(t2);
                free_converted.push(t3);
                free_stream.push(t4);
                counts = [a1 - a0, a2 - a1, a3 - a2, a4 - a3];
            }
        }
    }
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let (r, c, fc, fs) = (
        median(&mut record),
        median(&mut convert),
        median(&mut free_converted),
        median(&mut free_stream),
    );
    println!("stream commands {commands}  replay commands {converted_commands}  draws {draws}");
    println!(
        "record (vtable calls + Close): median {:.3} ms ({:.4} us/draw), {} allocations",
        ms(r),
        ms(r) * 1e3 / draws,
        counts[0]
    );
    println!(
        "convert (bridge): median {:.3} ms ({:.4} us/draw), {} allocations",
        ms(c),
        ms(c) * 1e3 / draws,
        counts[1]
    );
    println!(
        "free replay commands: median {:.3} ms, {} allocations",
        ms(fc),
        counts[2]
    );
    println!(
        "Reset + free stream: median {:.3} ms, {} allocations",
        ms(fs),
        counts[3]
    );
    println!(
        "total per frame: {:.3} ms ({:.4} us/draw)",
        ms(r + c + fc + fs),
        ms(r + c + fc + fs) * 1e3 / draws
    );
    println!(
        "sizes: stream::Command {} bytes, execute::Command {} bytes",
        std::mem::size_of::<xbox_d3d12::stream::Command>(),
        std::mem::size_of::<Command>()
    );
}
