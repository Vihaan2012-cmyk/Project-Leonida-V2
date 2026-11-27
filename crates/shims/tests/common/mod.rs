//! Shared by the tests that run the console memory functions against installed GPU memory hooks. The hooks are
//! global, so every such test holds `Installed`.
#![allow(dead_code, unsafe_code)]

use std::sync::{Arc, Mutex, MutexGuard};

use xbox_d3d12::memory::hooks::{self, GpuMemoryHooks};
use xbox_d3d12::memory::{MemoryError, SyncPolicy};

pub const PAGE: u64 = 64 * 1024;
pub const GIB: u64 = 1 << 30;

pub fn shim<T: Copy>(name: &str) -> T {
    shim_of("xmem.dll", name)
}

pub fn shim_of<T: Copy>(dll: &str, name: &str) -> T {
    let address = shims::builtin()
        .into_iter()
        .find(|s| s.name == name && s.dll == dll)
        .unwrap_or_else(|| panic!("{dll}!{name} registered"))
        .address;
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
    // SAFETY: `T` is the function pointer type matching the shim's measured shape.
    unsafe { std::mem::transmute_copy(&address) }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Mapped(u64, u64, SyncPolicy),
    Pages(u64, u64, SyncPolicy),
    Scatter(Vec<u64>, SyncPolicy),
    Unmapped(u64, u64),
}

pub struct Recorder {
    events: Mutex<Vec<Event>>,
    window: Mutex<(u64, u64)>,
}

impl Recorder {
    fn record(&self, event: Event) -> Result<(), MemoryError> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }
}

impl GpuMemoryHooks for Recorder {
    fn mapped(&self, va: u64, len: u64, policy: SyncPolicy) -> Result<(), MemoryError> {
        self.record(Event::Mapped(va, len, policy))
    }
    fn pages_mapped(&self, va: u64, count: u64, policy: SyncPolicy) -> Result<(), MemoryError> {
        self.record(Event::Pages(va, count, policy))
    }
    fn pages_mapped_scatter(&self, a: &[u64], policy: SyncPolicy) -> Result<(), MemoryError> {
        self.record(Event::Scatter(a.to_vec(), policy))
    }
    fn unmapped(&self, va: u64, len: u64) -> Result<(), MemoryError> {
        self.record(Event::Unmapped(va, len))
    }
    fn window(&self) -> (u64, u64) {
        *self.window.lock().unwrap()
    }
}

static LOCK: Mutex<()> = Mutex::new(());

/// Holds the lock, with a recorder installed over `window` until dropped.
pub struct Installed {
    recorder: Arc<Recorder>,
    _lock: MutexGuard<'static, ()>,
}

impl Installed {
    pub fn new(window: (u64, u64)) -> Installed {
        let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let recorder = Arc::new(Recorder {
            events: Mutex::new(Vec::new()),
            window: Mutex::new(window),
        });
        hooks::install(recorder.clone());
        Installed {
            recorder,
            _lock: lock,
        }
    }

    pub fn take(&self) -> Vec<Event> {
        std::mem::take(&mut *self.recorder.events.lock().unwrap())
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        hooks::uninstall();
    }
}

/// Holds the lock with no hooks installed.
pub fn without_hooks() -> MutexGuard<'static, ()> {
    let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    hooks::uninstall();
    lock
}
