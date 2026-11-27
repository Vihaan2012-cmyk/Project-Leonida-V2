use std::sync::{Arc, Mutex};

use ash::vk;
use gpu_allocator::MemoryLocation as AllocLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};

use crate::device::{Device, lock};
use crate::error::{RenderError, Result};

/// Where a resource's memory should live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryLocation {
    /// Device-local, not mapped.
    GpuOnly,
    /// Mapped memory the CPU writes and the GPU reads (resizable-BAR memory when there is some).
    Upload,
    /// Mapped, cached memory the CPU reads back.
    Readback,
}

impl MemoryLocation {
    pub(crate) fn alloc(self) -> AllocLocation {
        match self {
            MemoryLocation::GpuOnly => AllocLocation::GpuOnly,
            MemoryLocation::Upload => AllocLocation::CpuToGpu,
            MemoryLocation::Readback => AllocLocation::GpuToCpu,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BufferDesc<'a> {
    pub name: &'a str,
    pub size: u64,
    /// `TRANSFER_SRC`, `TRANSFER_DST` and `SHADER_DEVICE_ADDRESS` are always added.
    pub usage: vk::BufferUsageFlags,
    pub location: MemoryLocation,
}

enum Backing {
    Allocated(Mutex<Option<Allocation>>),
    Imported,
}

struct BufferInner {
    device: Device,
    handle: vk::Buffer,
    size: u64,
    usage: vk::BufferUsageFlags,
    address: u64,
    backing: Backing,
    memory: vk::DeviceMemory,
    memory_offset: u64,
    memory_size: u64,
    coherent: bool,
    mapped: usize,
}

impl Drop for BufferInner {
    fn drop(&mut self) {
        // SAFETY: the buffer and its memory belong to the device `self.device` keeps alive; nothing refers to
        // them once the last `Buffer` clone is gone (command buffers hold clones until they complete).
        unsafe {
            self.device.raw().destroy_buffer(self.handle, None);
            match &self.backing {
                Backing::Allocated(allocation) => {
                    if let Some(allocation) = lock(allocation).take()
                        && let Some(allocator) = self.device.allocator().as_mut()
                    {
                        let _ = allocator.free(allocation);
                    }
                }
                Backing::Imported => self.device.raw().free_memory(self.memory, None),
            }
        }
    }
}

/// A Vulkan buffer with its memory. Clones share the buffer; the memory is freed with the last one.
#[derive(Clone)]
pub struct Buffer {
    inner: Arc<BufferInner>,
}

fn usage_with_defaults(usage: vk::BufferUsageFlags) -> vk::BufferUsageFlags {
    usage
        | vk::BufferUsageFlags::TRANSFER_SRC
        | vk::BufferUsageFlags::TRANSFER_DST
        | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
}

impl Buffer {
    pub fn new(device: &Device, desc: &BufferDesc<'_>) -> Result<Buffer> {
        let usage = usage_with_defaults(desc.usage);
        let size = desc.size.max(4);
        let raw = device.raw();
        // SAFETY: the buffer is bound once to memory sized from its own requirements; on failure it is destroyed.
        unsafe {
            let handle = raw.create_buffer(
                &vk::BufferCreateInfo::default().size(size).usage(usage),
                None,
            )?;
            let requirements = raw.get_buffer_memory_requirements(handle);
            let allocation = device
                .allocator()
                .as_mut()
                .ok_or(RenderError::NoMemoryType)
                .and_then(|a| {
                    a.allocate(&AllocationCreateDesc {
                        name: desc.name,
                        requirements,
                        location: desc.location.alloc(),
                        linear: true,
                        allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                    })
                    .map_err(Into::into)
                });
            let allocation = match allocation {
                Ok(a) => a,
                Err(e) => {
                    raw.destroy_buffer(handle, None);
                    return Err(e);
                }
            };
            if let Err(e) = raw.bind_buffer_memory(handle, allocation.memory(), allocation.offset())
            {
                raw.destroy_buffer(handle, None);
                if let Some(a) = device.allocator().as_mut() {
                    let _ = a.free(allocation);
                }
                return Err(e.into());
            }
            let address = raw
                .get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(handle));
            let mapped = allocation.mapped_ptr().map_or(0, |p| p.as_ptr() as usize);
            let coherent = allocation
                .memory_properties()
                .contains(vk::MemoryPropertyFlags::HOST_COHERENT);
            Ok(Buffer {
                inner: Arc::new(BufferInner {
                    device: device.clone(),
                    handle,
                    size,
                    usage,
                    address,
                    memory: allocation.memory(),
                    memory_offset: allocation.offset(),
                    memory_size: allocation.size(),
                    coherent,
                    mapped,
                    backing: Backing::Allocated(Mutex::new(Some(allocation))),
                }),
            })
        }
    }

    /// A device-local buffer holding `data`, uploaded on queue 0 and waited for.
    pub fn with_data(
        device: &Device,
        name: &str,
        usage: vk::BufferUsageFlags,
        data: &[u8],
    ) -> Result<Buffer> {
        let buffer = Buffer::new(
            device,
            &BufferDesc {
                name,
                size: data.len() as u64,
                usage,
                location: MemoryLocation::GpuOnly,
            },
        )?;
        let staging = Buffer::new(
            device,
            &BufferDesc {
                name: "staging",
                size: data.len() as u64,
                usage: vk::BufferUsageFlags::empty(),
                location: MemoryLocation::Upload,
            },
        )?;
        staging.write(0, data)?;
        device.queue(0).immediate(|cb| {
            cb.copy_buffer(
                &staging,
                &buffer,
                &[vk::BufferCopy::default().size(data.len() as u64)],
            );
            Ok(())
        })?;
        Ok(buffer)
    }

    /// Makes a buffer over host memory with `VK_EXT_external_memory_host`, so the GPU reads and writes `ptr`
    /// itself.
    ///
    /// # Safety
    /// `ptr..ptr + size` must stay allocated, and not be unmapped or moved, until the buffer and every GPU
    /// command that uses it are gone. Both `ptr` and `size` must be multiples of
    /// `Features::host_import_alignment`.
    pub unsafe fn from_host_memory(
        device: &Device,
        ptr: *mut u8,
        size: u64,
        usage: vk::BufferUsageFlags,
    ) -> Result<Buffer> {
        let Some(ext) = device.inner.host_ext.as_ref() else {
            return Err(RenderError::Unsupported("VK_EXT_external_memory_host"));
        };
        let align = device.features().host_import_alignment;
        if !(ptr as u64).is_multiple_of(align) || !size.is_multiple_of(align) || size == 0 {
            return Err(RenderError::Misaligned(align));
        }
        let usage = usage_with_defaults(usage);
        let handle_type = vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT;
        let raw = device.raw();
        // SAFETY: the buffer and memory are created and bound once here and destroyed on every error path; the
        // caller guarantees the host range outlives them.
        unsafe {
            let mut properties = vk::MemoryHostPointerPropertiesEXT::default();
            (ext.fp().get_memory_host_pointer_properties_ext)(
                raw.handle(),
                handle_type,
                ptr.cast(),
                &mut properties,
            )
            .result()?;
            let mut external =
                vk::ExternalMemoryBufferCreateInfo::default().handle_types(handle_type);
            let handle = raw.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(usage)
                    .push_next(&mut external),
                None,
            )?;
            let requirements = raw.get_buffer_memory_requirements(handle);
            let kind = device.memory_type(
                properties.memory_type_bits & requirements.memory_type_bits,
                vk::MemoryPropertyFlags::empty(),
                vk::MemoryPropertyFlags::empty(),
            );
            let Some(kind) = kind.filter(|_| requirements.size <= size) else {
                raw.destroy_buffer(handle, None);
                return Err(RenderError::NoMemoryType);
            };
            let mut import = vk::ImportMemoryHostPointerInfoEXT::default()
                .handle_type(handle_type)
                .host_pointer(ptr.cast());
            let mut flags = vk::MemoryAllocateFlagsInfo::default()
                .flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
            let memory = raw.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(size)
                    .memory_type_index(kind)
                    .push_next(&mut import)
                    .push_next(&mut flags),
                None,
            );
            let memory = match memory {
                Ok(m) => m,
                Err(e) => {
                    raw.destroy_buffer(handle, None);
                    return Err(e.into());
                }
            };
            if let Err(e) = raw.bind_buffer_memory(handle, memory, 0) {
                raw.destroy_buffer(handle, None);
                raw.free_memory(memory, None);
                return Err(e.into());
            }
            let address = raw
                .get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(handle));
            let coherent = device.inner.memory.memory_types[kind as usize]
                .property_flags
                .contains(vk::MemoryPropertyFlags::HOST_COHERENT);
            Ok(Buffer {
                inner: Arc::new(BufferInner {
                    device: device.clone(),
                    handle,
                    size,
                    usage,
                    address,
                    backing: Backing::Imported,
                    memory,
                    memory_offset: 0,
                    memory_size: size,
                    coherent,
                    mapped: ptr as usize,
                }),
            })
        }
    }

    pub fn handle(&self) -> vk::Buffer {
        self.inner.handle
    }

    pub fn size(&self) -> u64 {
        self.inner.size
    }

    pub fn usage(&self) -> vk::BufferUsageFlags {
        self.inner.usage
    }

    pub fn device(&self) -> &Device {
        &self.inner.device
    }

    /// The buffer's GPU virtual address, for shaders that take pointers.
    pub fn device_address(&self) -> u64 {
        self.inner.address
    }

    pub fn is_mapped(&self) -> bool {
        self.inner.mapped != 0
    }

    fn check(&self, offset: u64, len: usize) -> Result<()> {
        if self.inner.mapped == 0 {
            return Err(RenderError::Invalid(
                "the buffer is not host-visible".into(),
            ));
        }
        if offset
            .checked_add(len as u64)
            .is_none_or(|end| end > self.inner.size)
        {
            return Err(RenderError::Invalid(format!(
                "range {offset}+{len} is outside a {}-byte buffer",
                self.inner.size
            )));
        }
        Ok(())
    }

    fn sync(&self, offset: u64, len: usize, invalidate: bool) -> Result<()> {
        let inner = &self.inner;
        if inner.coherent || len == 0 {
            return Ok(());
        }
        let atom = inner.device.features().non_coherent_atom_size;
        let start = (inner.memory_offset + offset) / atom * atom;
        let end = (inner.memory_offset + offset + len as u64).div_ceil(atom) * atom;
        let limit = inner.memory_offset + inner.memory_size;
        let size = if end > limit {
            vk::WHOLE_SIZE
        } else {
            end - start
        };
        let range = vk::MappedMemoryRange::default()
            .memory(inner.memory)
            .offset(start)
            .size(size);
        // SAFETY: the range lies inside this buffer's memory and is atom-aligned.
        unsafe {
            if invalidate {
                inner
                    .device
                    .raw()
                    .invalidate_mapped_memory_ranges(&[range])?;
            } else {
                inner.device.raw().flush_mapped_memory_ranges(&[range])?;
            }
        }
        Ok(())
    }

    /// Copies `data` into the mapped buffer at `offset`. The caller makes sure the GPU is not using the range.
    pub fn write(&self, offset: u64, data: &[u8]) -> Result<()> {
        self.check(offset, data.len())?;
        // SAFETY: `check` proved the range is inside the mapping, which lives as long as the buffer.
        unsafe {
            let dst = (self.inner.mapped as *mut u8).add(offset as usize);
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }
        self.sync(offset, data.len(), false)
    }

    /// Copies from the mapped buffer at `offset` into `out`. The GPU's writes must have completed.
    pub fn read(&self, offset: u64, out: &mut [u8]) -> Result<()> {
        self.check(offset, out.len())?;
        self.sync(offset, out.len(), true)?;
        // SAFETY: `check` proved the range is inside the mapping, which lives as long as the buffer.
        unsafe {
            let src = (self.inner.mapped as *const u8).add(offset as usize);
            std::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), out.len());
        }
        Ok(())
    }

    pub fn read_vec(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut out = vec![0; len];
        self.read(offset, &mut out)?;
        Ok(out)
    }
}
