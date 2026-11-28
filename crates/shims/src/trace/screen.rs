//! `LEONIDA_CAPTURE=<seconds>`: every that many seconds, the program's window as the desktop composes it is written
//! to `screen-<n>.bmp` in the trace folder (the last eight kept), so a boot shows what the player saw at each point.

use std::path::{Path, PathBuf};
use std::time::Duration;

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject,
};
use windows_sys::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClientRect, GetWindowThreadProcessId, IsWindowVisible,
};

/// `PW_RENDERFULLCONTENT`: what the compositor shows, swap chain contents included.
const FULL_CONTENT: PRINT_WINDOW_FLAGS = 2;
const PW_CLIENTONLY: PRINT_WINDOW_FLAGS = 1;
const KEPT: u64 = 8;

unsafe extern "system" fn visit(window: HWND, found: LPARAM) -> i32 {
    let mut process = 0u32;
    // SAFETY: a window handle Windows enumerates and an out word.
    unsafe { GetWindowThreadProcessId(window, &mut process) };
    // SAFETY: as above; reads this process's id.
    if process == unsafe { GetCurrentProcessId() } && unsafe { IsWindowVisible(window) } != 0 {
        // SAFETY: `found` is the address of the caller's handle.
        unsafe { *(found as *mut HWND) = window };
        return 0;
    }
    1
}

fn window() -> Option<HWND> {
    let mut found: HWND = std::ptr::null_mut();
    // SAFETY: the callback writes only the handle passed by address.
    unsafe { EnumWindows(Some(visit), &mut found as *mut HWND as LPARAM) };
    (!found.is_null()).then_some(found)
}

/// The window's client area as 32-bit BGRA rows, bottom row first, with its size.
fn grab(window: HWND) -> Option<(u32, u32, Vec<u8>)> {
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    // SAFETY: a window of this process and an out rectangle.
    if unsafe { GetClientRect(window, &mut rect) } == 0 {
        return None;
    }
    let (width, height) = (
        (rect.right - rect.left).max(0),
        (rect.bottom - rect.top).max(0),
    );
    if width == 0 || height == 0 {
        return None;
    }
    // SAFETY: GDI objects made here and released before returning; the bitmap info and pixel buffer are sized for
    // the bitmap.
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let memory = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let old = SelectObject(memory, bitmap);
        let printed = PrintWindow(window, memory, FULL_CONTENT | PW_CLIENTONLY) != 0;
        SelectObject(memory, old);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let rows = GetDIBits(
            memory,
            bitmap,
            0,
            height as u32,
            pixels.as_mut_ptr().cast(),
            &mut info,
            DIB_RGB_COLORS,
        );
        DeleteObject(bitmap);
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);
        (printed && rows == height).then_some((width as u32, height as u32, pixels))
    }
}

/// A 32-bit bottom-up BMP file of `pixels`.
pub fn bmp(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let image = pixels.len() as u32;
    let mut file = Vec::with_capacity(54 + pixels.len());
    file.extend_from_slice(b"BM");
    file.extend_from_slice(&(54 + image).to_le_bytes());
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(&54u32.to_le_bytes());
    file.extend_from_slice(&40u32.to_le_bytes());
    file.extend_from_slice(&(width as i32).to_le_bytes());
    file.extend_from_slice(&(height as i32).to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes());
    file.extend_from_slice(&32u16.to_le_bytes());
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(&image.to_le_bytes());
    file.extend_from_slice(&[0u8; 16]);
    file.extend_from_slice(pixels);
    file
}

fn save(folder: &Path, index: u64) {
    let Some((width, height, pixels)) = window().and_then(grab) else {
        return;
    };
    let _ = std::fs::write(
        folder.join(format!("screen-{index}.bmp")),
        bmp(width, height, &pixels),
    );
    if index >= KEPT {
        let _ = std::fs::remove_file(folder.join(format!("screen-{}.bmp", index - KEPT)));
    }
}

/// Starts the captures when `LEONIDA_CAPTURE` is set.
pub fn schedule(folder: PathBuf) {
    let Some(seconds) = std::env::var("LEONIDA_CAPTURE")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    else {
        return;
    };
    std::thread::spawn(move || {
        let mut index = 0;
        loop {
            std::thread::sleep(Duration::from_secs(seconds.max(1)));
            save(&folder, index);
            index += 1;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bmp_has_its_header_then_the_rows() {
        let file = bmp(2, 1, &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&file[..2], b"BM");
        assert_eq!(file.len(), 54 + 8);
        assert_eq!(u32::from_le_bytes(file[2..6].try_into().unwrap()), 62);
        assert_eq!(i32::from_le_bytes(file[18..22].try_into().unwrap()), 2);
        assert_eq!(&file[54..], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }
}
