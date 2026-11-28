//! The one lock every NGX call goes through. NVIDIA documents NGX as not thread safe, and its instances share
//! process-wide state, so two contexts (each with its own device) must never be inside NGX at the same moment.
//!
//! The lock is reentrant per thread: [`ngx_lock`] may be called again by code that already holds it (an evaluation
//! that sets parameters, an initialisation that reads the capability map). Other NGX users in the crate, such as
//! frame generation, take the same lock around their calls.

use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::sync::{Mutex, MutexGuard};

static NGX: Mutex<()> = Mutex::new(());

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    static HELD: RefCell<Option<MutexGuard<'static, ()>>> = const { RefCell::new(None) };
}

/// Held while the calling thread may be inside NGX; the lock is released when the thread's last guard is dropped.
pub(crate) struct NgxGuard {
    _not_send: PhantomData<*const ()>,
}

/// Waits for every other thread to leave NGX and returns the guard that keeps them out.
pub(crate) fn ngx_lock() -> NgxGuard {
    if DEPTH.with(Cell::get) == 0 {
        let guard = NGX.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        HELD.with(|held| *held.borrow_mut() = Some(guard));
    }
    DEPTH.with(|depth| depth.set(depth.get() + 1));
    NgxGuard {
        _not_send: PhantomData,
    }
}

/// Whether the calling thread holds the NGX lock.
pub(crate) fn held_by_this_thread() -> bool {
    DEPTH.with(Cell::get) > 0
}

impl Drop for NgxGuard {
    fn drop(&mut self) {
        let left = DEPTH.with(|depth| {
            depth.set(depth.get() - 1);
            depth.get()
        });
        if left == 0 {
            HELD.with(|held| held.borrow_mut().take());
        }
    }
}
