//! The device's resources as other layers reach them: the upscaler reads and writes whole textures, and a
//! DirectStorage request copies bytes into a buffer. Each call is synchronous and takes the submission lock, so it
//! runs between submissions.

use std::ffi::c_void;
use std::sync::PoisonError;

use super::hooks;
use super::objects::resource_of;
use super::state::current;
use content_filter::ResourceId as FilterId;

/// A texture's first mip and layer as tightly packed rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureData {
    pub width: u32,
    pub height: u32,
    /// The `DXGI_FORMAT` of the resource.
    pub format: u32,
    pub bytes: Vec<u8>,
}

fn texel_bytes(format: u32) -> Option<usize> {
    crate::descriptor::format::format(format)
        .filter(|f| f.block == [1, 1])
        .map(|f| f.bytes as usize)
}

/// Reads mip 0 of the texture behind a COM pointer, whatever the GPU last wrote.
pub fn read_texture(resource: usize) -> Result<TextureData, String> {
    let asm = current().ok_or("no device")?;
    let _exec = asm.exec.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: the title's `ID3D12Resource*`.
    let object = unsafe { resource_of(resource as *mut c_void) }.ok_or("not a resource")?;
    let resources = asm.resources();
    let r = resources.get(object.id).ok_or("unknown resource")?;
    let image = r.image.as_ref().ok_or("not a texture")?;
    let bytes_per = texel_bytes(r.desc.format).ok_or("a format with blocks or planes")?;
    let (width, height) = (image.extent[0], image.extent[1]);
    let handles = resources.space().handles().clone();
    let bytes = image
        .download(&handles, 0, 0, width as usize * height as usize * bytes_per)
        .map_err(|e| e.to_string())?;
    Ok(TextureData {
        width,
        height,
        format: r.desc.format,
        bytes,
    })
}

/// Writes mip 0 of a texture; the data must be the whole mip.
pub fn write_texture(resource: usize, width: u32, height: u32, bytes: &[u8]) -> Result<(), String> {
    let asm = current().ok_or("no device")?;
    let _exec = asm.exec.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: the title's `ID3D12Resource*`.
    let object = unsafe { resource_of(resource as *mut c_void) }.ok_or("not a resource")?;
    let resources = asm.resources();
    let r = resources.get(object.id).ok_or("unknown resource")?;
    let image = r.image.as_ref().ok_or("not a texture")?;
    let bytes_per = texel_bytes(r.desc.format).ok_or("a format with blocks or planes")?;
    if (width, height) != (image.extent[0], image.extent[1])
        || bytes.len() != width as usize * height as usize * bytes_per
    {
        return Err(format!(
            "{width}x{height} is not the whole {}x{} texture",
            image.extent[0], image.extent[1]
        ));
    }
    let handles = resources.space().handles().clone();
    image
        .upload(&handles, 0, 0, bytes)
        .map_err(|e| e.to_string())
}

/// Copies `data` into a buffer at `offset`: the buffer is host memory at its address, so the CPU writes it and a
/// mirrored window is told.
pub fn write_buffer(resource: usize, offset: u64, data: &[u8]) -> Result<(), String> {
    let asm = current().ok_or("no device")?;
    // SAFETY: the title's `ID3D12Resource*`.
    let object = unsafe { resource_of(resource as *mut c_void) }.ok_or("not a resource")?;
    let resources = asm.resources();
    let r = resources.get(object.id).ok_or("unknown resource")?;
    let address = r.cpu_address().ok_or("not a buffer with host memory")?;
    let end = offset
        .checked_add(data.len() as u64)
        .filter(|end| *end <= r.desc.width)
        .ok_or("the data runs past the end of the buffer")?;
    // SAFETY: `offset..end` is inside the buffer's committed host memory.
    unsafe {
        std::ptr::copy_nonoverlapping(
            data.as_ptr(),
            (address + offset) as usize as *mut u8,
            data.len(),
        )
    };
    resources.space().mark_dirty(address + offset, end - offset);
    if let Some(filter) = hooks::content_filter() {
        filter.on_upload(FilterId(u64::from(object.id)), offset, data);
    }
    Ok(())
}
