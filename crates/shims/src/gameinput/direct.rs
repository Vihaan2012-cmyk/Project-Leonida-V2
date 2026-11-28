//! Raw mouse counts handed to the code patch of the camera instead of the right stick.
//!
//! The patch (`enhancements::camera_mouse`) reads a 16 byte cell in the game's image: `dx: i32`, `dy: i32`,
//! `scale_x: f32`, `scale_y: f32`. It takes the counts with an atomic exchange, so a count is used once. While a
//! cell is attached [`push`] fills it and the hub stops turning the mouse into stick movement.

use std::sync::atomic::{AtomicI32, AtomicPtr, Ordering};

/// Most counts the cell may hold; movement collected while the camera does not run does not turn into a long spin.
pub const LIMIT: i32 = 1000;

static CELL: AtomicPtr<AtomicI32> = AtomicPtr::new(std::ptr::null_mut());

/// Starts filling the cell at `cell`.
///
/// # Safety
/// `cell` must point at 16 writable, 4 byte aligned bytes that stay valid until [`detach`].
pub unsafe fn attach(cell: *mut i32) {
    CELL.store(cell.cast(), Ordering::Release);
}

/// Stops filling the cell; the mouse moves the stick again.
pub fn detach() {
    CELL.store(std::ptr::null_mut(), Ordering::Release);
}

pub fn attached() -> bool {
    !CELL.load(Ordering::Acquire).is_null()
}

/// Adds motion to the cell, clamped to [`LIMIT`] counts per axis. Returns false when no cell is attached.
pub fn push(dx: i32, dy: i32) -> bool {
    let cell = CELL.load(Ordering::Acquire);
    if cell.is_null() {
        return false;
    }
    for (axis, delta) in [(0usize, dx), (1, dy)] {
        if delta == 0 {
            continue;
        }
        // SAFETY: `attach` promised 16 valid bytes; the axes are the first two words.
        let word = unsafe { &*cell.add(axis) };
        let mut old = word.load(Ordering::Relaxed);
        loop {
            let new = old.saturating_add(delta).clamp(-LIMIT, LIMIT);
            match word.compare_exchange_weak(old, new, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => break,
                Err(seen) => old = seen,
            }
        }
    }
    true
}

/// Sets the radians per count of each axis, sign included.
pub fn set_scales(x: f32, y: f32) {
    let cell = CELL.load(Ordering::Acquire);
    if cell.is_null() {
        return;
    }
    for (axis, value) in [(2usize, x), (3, y)] {
        // SAFETY: as in [`push`]; the scales are the last two words.
        unsafe { &*cell.add(axis) }.store(value.to_bits() as i32, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn counts_collect_in_the_cell_and_clamp() {
        let _guard = SERIAL.lock().unwrap();
        let mut cell = [0i32; 4];
        assert!(!push(1, 1));
        // SAFETY: `cell` outlives the attachment, which ends before the end of the test.
        unsafe { attach(cell.as_mut_ptr()) };
        assert!(attached());
        assert!(push(3, -2));
        assert!(push(4, -5));
        assert_eq!(cell[..2], [7, -7]);
        push(5000, -5000);
        assert_eq!(cell[..2], [LIMIT, -LIMIT]);
        set_scales(0.5, -0.25);
        assert_eq!(f32::from_bits(cell[2] as u32), 0.5);
        assert_eq!(f32::from_bits(cell[3] as u32), -0.25);
        detach();
        assert!(!push(1, 1));
        assert_eq!(cell[..2], [LIMIT, -LIMIT]);
    }
}
