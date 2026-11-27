//! How the `XMem*` shims tell the GPU memory part about the game's memory.
//!
//! The shims run in the game's call path, with no handle on the device. They call whatever `GpuMemoryHooks` is
//! installed: each time GPU-visible memory is committed, mapped or unmapped, and to learn where such memory may
//! be placed. `AddressSpaceHooks` implements the trait for an `AddressSpace`, so assembling the device only takes
//! `install_address_space(space)`. With nothing installed the shims behave as they do without a GPU.

use std::sync::{Arc, Mutex, PoisonError, RwLock};

use super::{AddressSpace, MemoryError, PAGE, SyncPolicy};

/// What the shims report. Every method may be called from any game thread; a failure makes the shim fail the
/// game's call and trace why, because memory the GPU cannot see is worse than an allocation that fails.
pub trait GpuMemoryHooks: Send + Sync {
    /// Host memory at `va..va + len`, 64 KiB aligned and committed, is now in use by the game: `AddressSpace::map_host`.
    fn mapped(&self, va: u64, len: u64, policy: SyncPolicy) -> Result<(), MemoryError>;

    /// `count` physical pages were mapped at `va` (`XMemMapPhysicalPages`).
    fn pages_mapped(&self, va: u64, count: u64, policy: SyncPolicy) -> Result<(), MemoryError>;

    /// One physical page was mapped at each of `addresses` (`XMemMapPhysicalPagesScatter`).
    fn pages_mapped_scatter(
        &self,
        addresses: &[u64],
        policy: SyncPolicy,
    ) -> Result<(), MemoryError>;

    /// `va..va + len` is about to be unmapped, freed or replaced. Called before the host memory changes.
    fn unmapped(&self, va: u64, len: u64) -> Result<(), MemoryError>;

    /// A page at each of `addresses` is about to be unmapped. Implementations may batch.
    fn unmapped_scatter(&self, addresses: &[u64]) -> Result<(), MemoryError> {
        let mut sorted = addresses.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let mut run = 0;
        while run < sorted.len() {
            let mut end = run + 1;
            while end < sorted.len() && sorted[end] == sorted[end - 1] + PAGE {
                end += 1;
            }
            self.unmapped(sorted[run], (end - run) as u64 * PAGE)?;
            run = end;
        }
        Ok(())
    }

    /// The host wrote `va..va + len` of memory already reported as mapped, outside the game's own code (a
    /// DirectStorage read landing in physical pages). Mirrored windows take the host's bytes over the GPU's
    /// results there.
    fn host_written(&self, _va: u64, _len: u64) {}

    /// The first address and the length of the range GPU-visible memory must be placed in.
    fn window(&self) -> (u64, u64);
}

static HOOKS: RwLock<Option<Arc<dyn GpuMemoryHooks>>> = RwLock::new(None);

/// Makes `hooks` what the shims call, replacing any earlier ones.
pub fn install(hooks: Arc<dyn GpuMemoryHooks>) {
    *HOOKS.write().unwrap_or_else(PoisonError::into_inner) = Some(hooks);
}

/// Goes back to the behaviour without a GPU. Memory the shims already reported stays reported.
pub fn uninstall() {
    *HOOKS.write().unwrap_or_else(PoisonError::into_inner) = None;
}

pub fn current() -> Option<Arc<dyn GpuMemoryHooks>> {
    HOOKS.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Installs hooks that forward to `space`.
pub fn install_address_space(space: Arc<Mutex<AddressSpace>>) {
    install(Arc::new(AddressSpaceHooks::new(space)));
}

/// The hooks of an `AddressSpace` shared behind a lock. The window is read on every call, because it grows when
/// something is mapped beyond it.
pub struct AddressSpaceHooks {
    space: Arc<Mutex<AddressSpace>>,
}

impl AddressSpaceHooks {
    pub fn new(space: Arc<Mutex<AddressSpace>>) -> AddressSpaceHooks {
        AddressSpaceHooks { space }
    }

    fn space(&self) -> std::sync::MutexGuard<'_, AddressSpace> {
        self.space.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl GpuMemoryHooks for AddressSpaceHooks {
    fn mapped(&self, va: u64, len: u64, policy: SyncPolicy) -> Result<(), MemoryError> {
        self.space().map_host(va, len, policy)
    }

    fn pages_mapped(&self, va: u64, count: u64, policy: SyncPolicy) -> Result<(), MemoryError> {
        self.space().map_pages(va, count, policy)
    }

    fn pages_mapped_scatter(
        &self,
        addresses: &[u64],
        policy: SyncPolicy,
    ) -> Result<(), MemoryError> {
        self.space().map_pages_scatter(addresses, policy)
    }

    fn unmapped(&self, va: u64, len: u64) -> Result<(), MemoryError> {
        self.space().unmap(va, len)
    }

    fn unmapped_scatter(&self, addresses: &[u64]) -> Result<(), MemoryError> {
        let mut sorted = addresses.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let mut space = self.space();
        space.begin_batch();
        let mut result = Ok(());
        let mut run = 0;
        while run < sorted.len() && result.is_ok() {
            let mut end = run + 1;
            while end < sorted.len() && sorted[end] == sorted[end - 1] + PAGE {
                end += 1;
            }
            result = space.unmap(sorted[run], (end - run) as u64 * PAGE);
            run = end;
        }
        let ended = space.end_batch();
        result.and(ended)
    }

    fn host_written(&self, va: u64, len: u64) {
        self.space().mark_dirty(va, len);
    }

    fn window(&self) -> (u64, u64) {
        self.space().window()
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;

    #[derive(Default)]
    struct Count {
        calls: Mutex<Vec<String>>,
    }

    impl GpuMemoryHooks for Count {
        fn mapped(&self, va: u64, len: u64, _: SyncPolicy) -> Result<(), MemoryError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("mapped {va:x} {len:x}"));
            Ok(())
        }
        fn pages_mapped(&self, va: u64, count: u64, _: SyncPolicy) -> Result<(), MemoryError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("pages {va:x} {count}"));
            Ok(())
        }
        fn pages_mapped_scatter(&self, a: &[u64], _: SyncPolicy) -> Result<(), MemoryError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("scatter {}", a.len()));
            Ok(())
        }
        fn unmapped(&self, va: u64, len: u64) -> Result<(), MemoryError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("unmapped {va:x} {len:x}"));
            Ok(())
        }
        fn window(&self) -> (u64, u64) {
            (0, 1 << 32)
        }
    }

    #[test]
    fn install_replaces_and_uninstall_clears() {
        assert!(current().is_none());
        let first = Arc::new(Count::default());
        install(first.clone());
        assert_eq!(current().unwrap().window(), (0, 1 << 32));
        current()
            .unwrap()
            .mapped(0x10000, 0x20000, SyncPolicy::Always)
            .unwrap();
        install(Arc::new(Count::default()));
        current().unwrap().unmapped(0x10000, 0x20000).unwrap();
        assert_eq!(
            first.calls.lock().unwrap().len(),
            1,
            "the old hooks hear no more"
        );
        uninstall();
        assert!(current().is_none());
    }

    #[test]
    fn the_default_scatter_unmap_joins_neighbours() {
        let count = Count::default();
        count
            .unmapped_scatter(&[0x40000, 0x10000, 0x20000, 0x20000, 0x80000])
            .unwrap();
        assert_eq!(
            *count.calls.lock().unwrap(),
            vec![
                "unmapped 10000 20000",
                "unmapped 40000 10000",
                "unmapped 80000 10000"
            ]
        );
    }

    #[test]
    fn the_adapter_binds_and_unbinds_through_an_address_space() {
        let gpus = TestGpu::open_all();
        if gpus.is_empty() {
            eprintln!("skipped: no GPU with sparse binding and host import");
        }
        for gpu in &gpus {
            let space = AddressSpace::new(
                gpu.handles.clone(),
                super::super::Config {
                    base: 0,
                    size: Some(1 << 38),
                    chunk: 1 << 30,
                },
            )
            .unwrap();
            let space = Arc::new(Mutex::new(space));
            let hooks = AddressSpaceHooks::new(space.clone());
            let window = space.lock().unwrap().window();
            assert_eq!(hooks.window(), window);
            let region = Region::new(8 * PAGE, false);
            let at = region.address;
            hooks.mapped(at, 2 * PAGE, SyncPolicy::Always).unwrap();
            hooks
                .pages_mapped(at + 2 * PAGE, 2, SyncPolicy::Always)
                .unwrap();
            hooks
                .pages_mapped_scatter(&[at + 6 * PAGE, at + 4 * PAGE], SyncPolicy::Always)
                .unwrap();
            let bound = |page: u64| space.lock().unwrap().is_bound(at + page * PAGE);
            assert!((0..5).all(bound) && bound(6) && !bound(7));
            hooks.unmapped(at, 2 * PAGE).unwrap();
            assert!(!bound(0) && !bound(1) && bound(2));
            hooks
                .unmapped_scatter(&[at + 2 * PAGE, at + 4 * PAGE, at + 6 * PAGE])
                .unwrap();
            assert!(!bound(2) && bound(3) && !bound(4) && !bound(6));
        }
    }
}
