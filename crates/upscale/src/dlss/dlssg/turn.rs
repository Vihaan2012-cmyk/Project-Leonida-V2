//! The one lock every NGX call of this feature holds.
//!
//! NGX is not thread-safe, not even across instances, so creation, evaluation, release, initialisation and shutdown
//! all run under it. The guard is not reentrant: a function that holds it calls nothing that takes it again.

use std::sync::{Mutex, MutexGuard};

static TURN: Mutex<()> = Mutex::new(());

/// Waits for the NGX turn. A thread that panicked inside NGX does not stop the others.
pub fn ngx_turn() -> MutexGuard<'static, ()> {
    TURN.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
