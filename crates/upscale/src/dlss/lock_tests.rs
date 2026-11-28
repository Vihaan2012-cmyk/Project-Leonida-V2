//! The NGX lock, and the checks that run NGX on the real GPU from several threads and without its updater.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::lock::{held_by_this_thread, ngx_lock};
use super::tests::library_config;
use super::*;
use crate::{ImageRef, PixelFormat, UpscaleInputs, Upscaler};

/// A thread running `body`, and whether it has finished.
struct Spawned {
    handle: std::thread::JoinHandle<()>,
    done: Arc<AtomicBool>,
}

fn spawn(body: impl FnOnce() + Send + 'static) -> Spawned {
    let done = Arc::new(AtomicBool::new(false));
    let flag = done.clone();
    let handle = std::thread::spawn(move || {
        body();
        flag.store(true, Ordering::SeqCst);
    });
    Spawned { handle, done }
}

impl Spawned {
    /// Gives the thread time to get through if nothing stops it, then says whether it did.
    fn finished_after(&self, wait: Duration) -> bool {
        std::thread::sleep(wait);
        self.done.load(Ordering::SeqCst)
    }
}

#[test]
fn the_lock_keeps_other_threads_out_until_it_is_released() {
    let guard = ngx_lock();
    let other = spawn(|| drop(ngx_lock()));
    assert!(
        !other.finished_after(Duration::from_millis(200)),
        "another thread entered NGX while this one held the lock"
    );
    drop(guard);
    other.handle.join().unwrap();
    assert!(other.done.load(Ordering::SeqCst));
}

#[test]
fn the_lock_is_reentrant_and_knows_its_holder() {
    assert!(!held_by_this_thread());
    let outer = ngx_lock();
    let inner = ngx_lock();
    assert!(held_by_this_thread());
    drop(outer);
    assert!(held_by_this_thread());
    let other = spawn(|| drop(ngx_lock()));
    assert!(!other.finished_after(Duration::from_millis(100)));
    drop(inner);
    assert!(!held_by_this_thread());
    other.handle.join().unwrap();
}

#[test]
fn parameter_calls_wait_for_the_lock() {
    let guard = ngx_lock();
    let other = spawn(|| {
        let mut fake = super::tests::FakeParameter::new();
        let mut map = fake.map();
        map.set_u32(ffi::names::WIDTH, 1);
        let _ = map.get_u32(ffi::names::OUT_WIDTH);
    });
    assert!(
        !other.finished_after(Duration::from_millis(200)),
        "a parameter call ran while another thread held the NGX lock"
    );
    drop(guard);
    other.handle.join().unwrap();
}

#[cfg(windows)]
#[test]
fn opening_ngx_switches_its_updater_off_first() {
    let missing = std::path::Path::new("E:/leonida-no-such-folder/nvngx.dll");
    assert!(ngx::Api::open(Some(missing)).is_err());
    assert_eq!(
        std::env::var("__NGX_DISABLE_UPDATER").as_deref(),
        Ok("1"),
        "NGX's over-the-air updater would start"
    );
}

/// A smooth frame and flat depth and motion for a render size.
fn drive(dlss: &mut DlssUpscaler, output: [u32; 2], frames: u32) -> crate::Image {
    let render = dlss
        .optimal_settings(output, DlssMode::Quality)
        .expect("optimal settings")
        .render_size;
    let texels = (render[0] * render[1]) as usize;
    let depth: Vec<u8> = (0..texels).flat_map(|_| 0.5f32.to_le_bytes()).collect();
    let motion = vec![0u8; texels * 8];
    let mut last = None;
    for frame in 0..frames {
        let jitter = crate::jitter_offset(frame, 8);
        let mut data = Vec::with_capacity(texels * 4);
        for y in 0..render[1] {
            for x in 0..render[0] {
                let u = x as f32 / render[0] as f32;
                let v = y as f32 / render[1] as f32;
                data.extend([0.2 + 0.5 * u, 0.3 + 0.4 * v, 0.6, 1.0]);
            }
        }
        let color =
            crate::Image::from_rgba_f32(render[0], render[1], PixelFormat::Rgba16Float, &data)
                .unwrap();
        let mut inputs = UpscaleInputs::new(color.as_ref(), render, output);
        inputs.depth = Some(ImageRef::new(
            render[0],
            render[1],
            PixelFormat::R32Float,
            &depth,
        ));
        inputs.motion = Some(ImageRef::new(
            render[0],
            render[1],
            PixelFormat::Rg32Float,
            &motion,
        ));
        inputs.jitter = jitter;
        inputs.reset = frame == 0;
        inputs.frame_time_ms = 16.6;
        last = Some(dlss.upscale(&inputs).expect("DLSS evaluates"));
    }
    last.unwrap()
}

#[test]
#[ignore = "needs an NVIDIA GPU and nvngx_dlss.dll (set LEONIDA_DLSS_DLL)"]
fn several_threads_create_and_evaluate_dlss_at_once() {
    const THREADS: usize = 4;
    let config = library_config();
    let start = std::sync::Barrier::new(THREADS);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..THREADS)
            .map(|i| {
                let (config, start) = (&config, &start);
                scope.spawn(move || {
                    start.wait();
                    let mut dlss = DlssUpscaler::new(config).expect("DLSS starts");
                    let output = [640 + 64 * i as u32, 360 + 36 * i as u32];
                    let result = drive(&mut dlss, output, 8);
                    assert_eq!([result.width, result.height], output);
                    let rgba = result
                        .as_ref()
                        .rgba_f32(output[0], output[1], "result")
                        .unwrap();
                    assert!(rgba.iter().all(|v| v.is_finite()));
                    let mean = rgba.iter().sum::<f32>() / rgba.len() as f32;
                    assert!(
                        mean > 0.2,
                        "thread {i}: the output is nearly black ({mean})"
                    );
                })
            })
            .collect();
        for worker in workers {
            worker.join().expect("a DLSS thread panicked");
        }
    });
}

#[cfg(windows)]
mod updater {
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    /// How many `nvngx_update.exe` processes exist that this process started.
    fn updaters_started_by_us() -> usize {
        let me = std::process::id();
        let mut count = 0;
        // SAFETY: a process snapshot is read through a handle that is closed again; `entry` is a plain struct whose
        // size field is set as the API requires.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot.is_null() || snapshot as isize == -1 {
                return 0;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry) != 0;
            while more {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
                if name.eq_ignore_ascii_case("nvngx_update.exe") && entry.th32ParentProcessID == me
                {
                    count += 1;
                }
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        count
    }

    #[test]
    #[ignore = "needs an NVIDIA GPU and nvngx_dlss.dll (set LEONIDA_DLSS_DLL)"]
    fn ngx_does_not_start_its_updater() {
        let config = super::library_config();
        let stop = AtomicBool::new(false);
        let launched = std::thread::scope(|scope| {
            let watcher = scope.spawn(|| {
                let mut most = 0;
                while !stop.load(Ordering::SeqCst) {
                    most = most.max(updaters_started_by_us());
                    std::thread::sleep(std::time::Duration::from_millis(3));
                }
                most.max(updaters_started_by_us())
            });
            let mut dlss = super::DlssUpscaler::new(&config).expect("DLSS starts");
            super::drive(&mut dlss, [640, 360], 3);
            drop(dlss);
            std::thread::sleep(std::time::Duration::from_millis(500));
            stop.store(true, Ordering::SeqCst);
            watcher.join().unwrap()
        });
        assert_eq!(launched, 0, "NGX started its over-the-air updater");
    }
}
