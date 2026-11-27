//! Read-only constants a patched instruction can reach with a 32-bit displacement.

use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RESERVE, PAGE_READONLY, PAGE_READWRITE, VirtualAlloc, VirtualProtect,
};

const PAGE: usize = 4096;
const GRANULARITY: usize = 64 * 1024;
const REACH: usize = i32::MAX as usize - 2 * GRANULARITY;
const TRIES: usize = 8192;

/// Puts `value` in a new read-only page whose address is within a signed 32-bit displacement of every byte of
/// `base..base + size`, and returns the address of the value.
pub fn read_only_f32_near(base: usize, size: usize, value: f32) -> Result<usize, String> {
    let end = base.saturating_add(size);
    let low = end.saturating_sub(REACH).max(GRANULARITY);
    let high = base.saturating_add(REACH);
    let mut below = base.saturating_sub(GRANULARITY) & !(GRANULARITY - 1);
    let mut above = end.next_multiple_of(GRANULARITY);
    for _ in 0..TRIES {
        for candidate in [below, above] {
            if candidate < low || candidate > high || candidate == 0 {
                continue;
            }
            // SAFETY: a fresh allocation at an address nothing owns; it fails when the address is taken.
            let page = unsafe {
                VirtualAlloc(
                    candidate as *const _,
                    PAGE,
                    MEM_RESERVE | MEM_COMMIT,
                    PAGE_READWRITE,
                )
            };
            if page.is_null() {
                continue;
            }
            // SAFETY: the page is ours, committed and writable, and 4 bytes fit in it.
            unsafe { page.cast::<f32>().write(value) };
            let mut previous = 0;
            // SAFETY: the whole page is ours; it only loses write access.
            let sealed = unsafe { VirtualProtect(page, PAGE, PAGE_READONLY, &mut previous) };
            if sealed == 0 {
                return Err("the constant page could not be made read-only".into());
            }
            return Ok(page as usize);
        }
        below = below.saturating_sub(GRANULARITY);
        above += GRANULARITY;
    }
    Err("no free page within 2 GiB of the image".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constant_is_near_the_image_and_holds_its_value() {
        let image = vec![0u8; 1 << 20];
        let base = image.as_ptr() as usize;
        let at = read_only_f32_near(base, image.len(), 0.5).expect("a page");
        let reach = |from: usize| (at as i64 - from as i64).abs();
        assert!(reach(base) < i64::from(i32::MAX));
        assert!(reach(base + image.len()) < i64::from(i32::MAX));
        // SAFETY: `at` is the page this function just made and left readable.
        assert_eq!(unsafe { (at as *const f32).read() }, 0.5);
        let other = read_only_f32_near(base, image.len(), 2.0).expect("another page");
        assert_ne!(other, at);
    }
}
