//! XboxBvhBuilder.dll: ray-tracing acceleration structures in the console node format.
//!
//! The executable imports seven members of the `XboxBvhBuilder` class. They are C++ methods, so the object is
//! the first argument; its size is not measured, so none of its bytes is touched and the per-object state sits
//! in a table keyed by the object's address:
//!
//! - `XboxBvhBuilder(ID3D12Device11*, bool)` and the destructor: register and forget the object.
//! - `GetRaytracingAccelerationStructurePrebuildInfo(inputs, stream_desc, out)`: the largest structure a build
//!   of those inputs can write, from the layout of the `bvh-build` crate; scratch sizes are a token 256 bytes
//!   because builds use host memory.
//! - `BuildRaytracingAccelerationStructureBatched2X(list, count, descs)`: each 0x50-byte batched desc (see
//!   xdxr.md) is built or, with `PERFORM_UPDATE`, refitted at once on the CPU, reading and writing GPU virtual
//!   addresses as host addresses, as the console's unified memory allows. Postbuild infos 0 and 3 get the size.
//! - `CopyRaytracingAccelerationStructure(list, dest, source, mode)`: clone and compact copy the structure's
//!   bytes, whose size is in its header.
//! - `Set64BitMortonCodes(bool)`: remembered; builds always use 30-bit keys, which changes tree quality only.
//! - `RehydrateRaytracingAccelerationStructureBatched2X`: unmeasured past the packing of bvh.md section 4a;
//!   noted and answered `E_NOTIMPL`.
//!
//! Commands run when they are recorded, not when the list executes: nothing here sees a command list.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use bvh_build::{
    BlasInfo, Cpu, Geometry, Header, Instance, TriangleSource, build_blas, build_tlas, flags,
    index_bytes, prebuild_size, read_triangles, refit_blas, refit_tlas, triangle_count,
    vertex_bytes,
};

use crate::Shim;
use crate::trace::note;

const E_NOTIMPL: i32 = 0x8000_4001_u32 as i32;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;

const BATCH_STRIDE: u64 = 0x50;
const GEOMETRY_STRIDE: u64 = 56;
const INSTANCE_STRIDE: u64 = 64;
const TOKEN_SCRATCH: u64 = 256;

const POSTBUILD_COMPACTED_SIZE: u32 = 0;
const POSTBUILD_SERIALIZATION: u32 = 2;
const POSTBUILD_CURRENT_SIZE: u32 = 3;
const POSTBUILD_OFFLINE_STATISTICS: u32 = 0x10;

/// Guest memory by address: the whole address space on the console, an arena in tests.
pub trait Memory {
    fn read(&self, address: u64, length: usize) -> Option<Vec<u8>>;
    fn write(&self, address: u64, data: &[u8]) -> bool;
}

/// Why a call could not be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Unreadable(&'static str),
    Unwritable(&'static str),
    Unsupported(&'static str),
    Build(bvh_build::BuildError),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Unreadable(what) => write!(f, "cannot read {what}"),
            Failure::Unwritable(what) => write!(f, "cannot write {what}"),
            Failure::Unsupported(what) => write!(f, "{what} is not supported"),
            Failure::Build(error) => write!(f, "{error}"),
        }
    }
}

impl From<bvh_build::BuildError> for Failure {
    fn from(error: bvh_build::BuildError) -> Failure {
        Failure::Build(error)
    }
}

type Outcome<T> = Result<T, Failure>;

fn bytes(memory: &dyn Memory, address: u64, length: usize, what: &'static str) -> Outcome<Vec<u8>> {
    if length == 0 {
        return Ok(Vec::new());
    }
    memory
        .read(address, length)
        .ok_or(Failure::Unreadable(what))
}

fn u32_at(memory: &dyn Memory, address: u64, what: &'static str) -> Outcome<u32> {
    let b = bytes(memory, address, 4, what)?;
    Ok(u32::from_le_bytes(b.try_into().expect("4 bytes")))
}

fn u64_at(memory: &dyn Memory, address: u64, what: &'static str) -> Outcome<u64> {
    let b = bytes(memory, address, 8, what)?;
    Ok(u64::from_le_bytes(b.try_into().expect("8 bytes")))
}

fn words(data: &[u8]) -> Vec<u32> {
    data.chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().expect("4 bytes")))
        .collect()
}

/// The inputs of a build, as `D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS` lays them out.
struct Inputs {
    top_level: bool,
    flags: u32,
    count: u32,
    pointers: bool,
    address: u64,
}

fn read_inputs(memory: &dyn Memory, at: u64) -> Outcome<Inputs> {
    let w = words(&bytes(memory, at, 16, "the build inputs")?);
    let address = u64_at(memory, at + 16, "the build inputs")?;
    let top_level = match w[0] {
        0 => true,
        1 => false,
        _ => {
            return Err(Failure::Unsupported(
                "a build type other than top or bottom level",
            ));
        }
    };
    Ok(Inputs {
        top_level,
        flags: w[1],
        count: w[2],
        pointers: w[3] == 1,
        address,
    })
}

fn geometry_address(memory: &dyn Memory, inputs: &Inputs, index: u32) -> Outcome<u64> {
    if inputs.pointers {
        u64_at(
            memory,
            inputs.address + 8 * u64::from(index),
            "the geometry pointers",
        )
    } else {
        Ok(inputs.address + GEOMETRY_STRIDE * u64::from(index))
    }
}

fn instance_address(memory: &dyn Memory, inputs: &Inputs, index: u32) -> Outcome<u64> {
    if inputs.pointers {
        u64_at(
            memory,
            inputs.address + 8 * u64::from(index),
            "the instance pointers",
        )
    } else {
        Ok(inputs.address + INSTANCE_STRIDE * u64::from(index))
    }
}

struct Triangles {
    transform: u64,
    index_format: u32,
    vertex_format: u32,
    index_count: u32,
    vertex_count: u32,
    index_buffer: u64,
    vertex_buffer: u64,
    vertex_stride: u64,
    flags: u32,
}

fn read_triangles_desc(memory: &dyn Memory, at: u64) -> Outcome<Triangles> {
    let head = words(&bytes(memory, at, 8, "a geometry desc")?);
    if head[0] != 0 {
        return Err(Failure::Unsupported(
            "procedural AABB geometry (its node layout is not measured)",
        ));
    }
    let b = bytes(memory, at + 8, 48, "a triangle geometry desc")?;
    let w = words(&b);
    let q = |i: usize| u64::from_le_bytes(b[8 * i..8 * i + 8].try_into().expect("8 bytes"));
    Ok(Triangles {
        transform: q(0),
        index_format: w[2],
        vertex_format: w[3],
        index_count: w[4],
        vertex_count: w[5],
        index_buffer: q(3),
        vertex_buffer: q(4),
        vertex_stride: q(5),
        flags: head[1],
    })
}

fn read_geometry(memory: &dyn Memory, at: u64) -> Outcome<Geometry> {
    let d = read_triangles_desc(memory, at)?;
    let transform = if d.transform != 0 {
        let m = words(&bytes(memory, d.transform, 48, "a geometry transform")?);
        let mut matrix = [0f32; 12];
        for (k, v) in m.iter().enumerate() {
            matrix[k] = f32::from_bits(*v);
        }
        Some(matrix)
    } else {
        None
    };
    let vertex_size = vertex_bytes(d.vertex_format).ok_or(Failure::Unsupported(
        "a vertex format other than the DXR ones",
    ))?;
    let index_size = index_bytes(d.index_format).ok_or(Failure::Unsupported(
        "an index format other than 16 or 32-bit",
    ))?;
    let vertex_length = if d.vertex_count == 0 {
        0
    } else {
        d.vertex_stride as usize * (d.vertex_count as usize - 1) + vertex_size
    };
    let vertices = bytes(memory, d.vertex_buffer, vertex_length, "a vertex buffer")?;
    let indices = bytes(
        memory,
        d.index_buffer,
        index_size * d.index_count as usize,
        "an index buffer",
    )?;
    let triangles = read_triangles(&TriangleSource {
        vertex_format: d.vertex_format,
        vertex_stride: d.vertex_stride as usize,
        vertex_count: d.vertex_count as usize,
        vertices: &vertices,
        index_format: d.index_format,
        index_count: d.index_count as usize,
        indices: &indices,
        transform,
    })?;
    Ok(Geometry {
        triangles,
        flags: d.flags,
    })
}

fn read_geometries(memory: &dyn Memory, inputs: &Inputs) -> Outcome<Vec<Geometry>> {
    (0..inputs.count)
        .map(|i| read_geometry(memory, geometry_address(memory, inputs, i)?))
        .collect()
}

fn read_instances(memory: &dyn Memory, inputs: &Inputs) -> Outcome<Vec<Instance>> {
    (0..inputs.count)
        .map(|i| {
            let at = instance_address(memory, inputs, i)?;
            let b = bytes(memory, at, INSTANCE_STRIDE as usize, "an instance desc")?;
            let w = words(&b);
            let mut transform = [0f32; 12];
            for (k, value) in w[..12].iter().enumerate() {
                transform[k] = f32::from_bits(*value);
            }
            Ok(Instance {
                transform,
                id: w[12] & 0xFF_FFFF,
                mask: w[12] >> 24,
                contribution: w[13] & 0xFF_FFFF,
                flags: w[13] >> 24,
                blas: u64::from_le_bytes(b[56..64].try_into().expect("8 bytes")),
            })
        })
        .collect()
}

/// A structure's whole bytes, its size read from its header first.
fn read_structure(memory: &dyn Memory, address: u64, what: &'static str) -> Outcome<Vec<u8>> {
    let head = bytes(memory, address, 64, what)?;
    let size = 64 * (u64::from(words(&head)[1]) + 1);
    bytes(memory, address, size as usize, what)
}

/// The BLAS an instance names, read from guest memory; each address once per build.
fn blas_lookup<'a>(
    memory: &'a dyn Memory,
    failures: &'a Mutex<Option<Failure>>,
) -> impl Fn(u64) -> Option<BlasInfo> + 'a {
    let cache = Mutex::new(HashMap::<u64, Option<BlasInfo>>::new());
    move |address| {
        if address == 0 {
            return None;
        }
        let mut cache = cache.lock().ok()?;
        *cache.entry(address).or_insert_with(|| {
            let structure = match read_structure(memory, address, "an instance's BLAS") {
                Ok(s) => s,
                Err(failure) => {
                    *failures.lock().expect("no panic while held") = Some(failure);
                    return None;
                }
            };
            bvh_build::blas_info(&structure).ok()
        })
    }
}

/// What a prebuild-info query answers: the most a build of these inputs can write.
pub fn prebuild(memory: &dyn Memory, inputs: u64, out: u64) -> Outcome<()> {
    let inputs = read_inputs(memory, inputs)?;
    let count = if inputs.top_level {
        inputs.count as usize
    } else {
        let mut total = 0;
        for i in 0..inputs.count {
            let d = read_triangles_desc(memory, geometry_address(memory, &inputs, i)?)?;
            total += triangle_count(
                d.index_format,
                d.index_count as usize,
                d.vertex_count as usize,
            );
        }
        total
    };
    let size = prebuild_size(count, !inputs.top_level, inputs.flags)? as u64;
    let mut result = Vec::new();
    for value in [size, TOKEN_SCRATCH, TOKEN_SCRATCH] {
        result.extend_from_slice(&value.to_le_bytes());
    }
    if memory.write(out, &result) {
        Ok(())
    } else {
        Err(Failure::Unwritable("the prebuild info"))
    }
}

fn postbuild(memory: &dyn Memory, count: u32, array: u64, size: u64) -> Outcome<()> {
    for i in 0..u64::from(count) {
        let at = array + 16 * i;
        let destination = u64_at(memory, at, "a postbuild desc")?;
        match u32_at(memory, at + 8, "a postbuild desc")? {
            POSTBUILD_COMPACTED_SIZE | POSTBUILD_CURRENT_SIZE => {
                if !memory.write(destination, &size.to_le_bytes()) {
                    return Err(Failure::Unwritable("a postbuild size"));
                }
            }
            POSTBUILD_SERIALIZATION => {
                return Err(Failure::Unsupported("serialization postbuild info"));
            }
            _ => {}
        }
    }
    Ok(())
}

fn rejects_offline_statistics(memory: &dyn Memory, count: u32, array: u64) -> Outcome<bool> {
    for i in 0..u64::from(count) {
        if u32_at(memory, array + 16 * i + 8, "a postbuild desc")? == POSTBUILD_OFFLINE_STATISTICS {
            return Ok(true);
        }
    }
    Ok(false)
}

/// One batched desc: a build into the destination, or a refit of the source into it.
fn build_one(memory: &dyn Memory, desc: u64) -> Outcome<()> {
    let destination = u64_at(memory, desc, "a batched desc")?;
    let inputs = read_inputs(memory, desc + 8)?;
    let source = u64_at(memory, desc + 0x20, "a batched desc")?;
    let post_count = u32_at(memory, desc + 0x30, "a batched desc")?;
    let post_array = u64_at(memory, desc + 0x38, "a batched desc")?;
    if rejects_offline_statistics(memory, post_count, post_array)? {
        return Err(Failure::Unsupported(
            "offline statistics postbuild info (the console library throws E_INVALIDARG for it)",
        ));
    }
    let failures = Mutex::new(None);
    let lookup = blas_lookup(memory, &failures);
    let result = if inputs.flags & flags::PERFORM_UPDATE != 0 {
        let mut structure = read_structure(memory, source, "the source structure")?;
        if inputs.top_level {
            refit_tlas(&mut structure, &read_instances(memory, &inputs)?, &lookup)?;
        } else {
            refit_blas(&mut structure, &read_geometries(memory, &inputs)?)?;
        }
        structure
    } else if inputs.top_level {
        build_tlas(&read_instances(memory, &inputs)?, &lookup, &mut Cpu)?
    } else {
        build_blas(&read_geometries(memory, &inputs)?, inputs.flags, &mut Cpu)?
    };
    if let Some(failure) = failures.lock().expect("no panic while held").take() {
        return Err(failure);
    }
    if !memory.write(destination, &result) {
        return Err(Failure::Unwritable("the destination structure"));
    }
    postbuild(memory, post_count, post_array, result.len() as u64)
}

pub fn build_batched(memory: &dyn Memory, count: u32, descs: u64) -> Vec<Failure> {
    (0..u64::from(count))
        .filter_map(|i| build_one(memory, descs + BATCH_STRIDE * i).err())
        .collect()
}

/// Copy modes of `D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE`: clone and compact copy the bytes.
pub fn copy(memory: &dyn Memory, destination: u64, source: u64, mode: u32) -> Outcome<()> {
    match mode {
        0 | 1 => {
            let structure = read_structure(memory, source, "the source structure")?;
            let header = Header::read(&structure)?;
            let size = header.size();
            if memory.write(destination, &structure[..size]) {
                Ok(())
            } else {
                Err(Failure::Unwritable("the copy destination"))
            }
        }
        2 => Err(Failure::Unsupported("the tools visualization decode copy")),
        3 => Err(Failure::Unsupported("serialization")),
        _ => Err(Failure::Unsupported(
            "deserialization or an unknown copy mode",
        )),
    }
}

/// The objects the game constructed, each with whether it asked for 64-bit Morton codes.
static OBJECTS: Mutex<Option<HashMap<usize, bool>>> = Mutex::new(None);
static NOTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn note_once(text: String) {
    let first = NOTED
        .lock()
        .map(|mut seen| seen.get_or_insert_with(HashSet::new).insert(text.clone()))
        .unwrap_or(true);
    if first {
        note(text);
    }
}

fn with_state<T>(this: usize, f: impl FnOnce(&mut bool) -> T) -> Option<T> {
    let mut objects = OBJECTS.lock().ok()?;
    objects
        .get_or_insert_with(HashMap::new)
        .get_mut(&this)
        .map(f)
}

fn report(call: &str, failure: &Failure) {
    note_once(format!("XboxBvhBuilder {call}: {failure}"));
}

fn stream_note(stream: u64, memory: &dyn Memory) {
    if stream != 0
        && let Ok(size) = u64_at(memory, stream, "the stream desc")
        && size != 0
    {
        note_once(
            "XboxBvhBuilder: the XDXR stream desc's settings are not applied (record meanings are not measured)"
                .into(),
        );
    }
}

#[cfg(windows)]
mod host {
    use std::ffi::c_void;

    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
        PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE,
        PAGE_WRITECOPY, VirtualQuery,
    };

    use super::{
        BATCH_STRIDE, E_INVALIDARG, E_NOTIMPL, Memory, build_batched, copy, note_once, prebuild,
        report, stream_note, with_state,
    };
    use crate::Shim;
    use std::collections::HashMap;

    /// The process's own address space, every range checked against the page protections first.
    pub struct Process;

    fn accessible(address: u64, length: usize, write: bool) -> bool {
        let Some(end) = address.checked_add(length as u64) else {
            return false;
        };
        if address == 0 {
            return false;
        }
        let allowed = if write {
            PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
        } else {
            PAGE_READONLY
                | PAGE_READWRITE
                | PAGE_WRITECOPY
                | PAGE_EXECUTE_READ
                | PAGE_EXECUTE_READWRITE
                | PAGE_EXECUTE_WRITECOPY
        };
        let mut at = address;
        while at < end {
            // SAFETY: VirtualQuery only reads the address space's bookkeeping for `at`.
            let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of::<MEMORY_BASIC_INFORMATION>();
            // SAFETY: `info` is a writable MEMORY_BASIC_INFORMATION of the size passed.
            let got = unsafe { VirtualQuery(at as *const c_void, &mut info, size) };
            if got == 0
                || info.State != MEM_COMMIT
                || info.Protect & (PAGE_NOACCESS | PAGE_GUARD) != 0
                || info.Protect & allowed == 0
            {
                return false;
            }
            at = info.BaseAddress as u64 + info.RegionSize as u64;
        }
        true
    }

    impl Memory for Process {
        fn read(&self, address: u64, length: usize) -> Option<Vec<u8>> {
            if !accessible(address, length, false) {
                return None;
            }
            let mut out = vec![0u8; length];
            // SAFETY: the range was just checked committed and readable, and `out` has `length` bytes.
            unsafe {
                std::ptr::copy_nonoverlapping(address as *const u8, out.as_mut_ptr(), length);
            }
            Some(out)
        }

        fn write(&self, address: u64, data: &[u8]) -> bool {
            if !accessible(address, data.len(), true) {
                return false;
            }
            // SAFETY: the range was just checked committed and writable, and `data` has its own length.
            unsafe {
                std::ptr::copy(data.as_ptr(), address as *mut u8, data.len());
            }
            true
        }
    }

    pub extern "win64" fn construct(this: usize, device: usize, _wide: u8) -> usize {
        let mut objects = super::OBJECTS.lock().expect("no panic while held");
        let _ = device;
        objects.get_or_insert_with(HashMap::new).insert(this, false);
        this
    }

    pub extern "win64" fn destruct(this: usize) {
        if let Ok(mut objects) = super::OBJECTS.lock()
            && let Some(map) = objects.as_mut()
        {
            map.remove(&this);
        }
    }

    pub extern "win64" fn set_64_bit_morton(this: usize, wide: u8) {
        if with_state(this, |flag| *flag = wide != 0).is_none() {
            note_once("XboxBvhBuilder::Set64BitMortonCodes on an unregistered object".into());
        } else if wide != 0 {
            note_once(
                "XboxBvhBuilder: 64-bit Morton codes requested; builds keep 30-bit keys (tree quality only)"
                    .into(),
            );
        }
    }

    pub extern "win64" fn prebuild_info(this: usize, inputs: u64, stream: u64, out: u64) {
        let _ = this;
        stream_note(stream, &Process);
        if let Err(failure) = prebuild(&Process, inputs, out) {
            report("GetRaytracingAccelerationStructurePrebuildInfo", &failure);
            let _ = Process.write(out, &[0u8; 24]);
        }
    }

    pub extern "win64" fn build_batched_2x(this: usize, _list: usize, count: u32, descs: u64) {
        let _ = this;
        let _ = BATCH_STRIDE;
        for failure in build_batched(&Process, count, descs) {
            report("BuildRaytracingAccelerationStructureBatched2X", &failure);
        }
    }

    pub extern "win64" fn copy_structure(
        this: usize,
        _list: usize,
        destination: u64,
        source: u64,
        mode: u32,
    ) {
        let _ = this;
        if let Err(failure) = copy(&Process, destination, source, mode) {
            report("CopyRaytracingAccelerationStructure", &failure);
        }
    }

    pub extern "win64" fn rehydrate_batched_2x(
        this: usize,
        _list: usize,
        count: u32,
        descs: u64,
        _extra: u64,
    ) -> i32 {
        let _ = this;
        if count == 0 {
            return 0;
        }
        note_once(
            "XboxBvhBuilder::RehydrateRaytracingAccelerationStructureBatched2X: rehydrating dehydrated packs needs the leaf geometry and the packing of bvh.md section 4a, which is not implemented"
                .into(),
        );
        if descs == 0 { E_INVALIDARG } else { E_NOTIMPL }
    }

    pub fn shims() -> Vec<Shim> {
        let list: [(&'static str, usize); 6] = [
            (
                "??0XboxBvhBuilder@@QEAA@PEAUID3D12Device11@@_N@Z",
                construct as *const () as usize,
            ),
            ("??1XboxBvhBuilder@@QEAA@XZ", destruct as *const () as usize),
            (
                "?BuildRaytracingAccelerationStructureBatched2X@XboxBvhBuilder@@QEAAXPEAUID3D12GraphicsCommandList6@@IPEBUD3D12XBOX_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_BATCHED_DESC2@@@Z",
                build_batched_2x as *const () as usize,
            ),
            (
                "?CopyRaytracingAccelerationStructure@XboxBvhBuilder@@QEAAXPEAUID3D12GraphicsCommandList4@@_K1W4D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE@@@Z",
                copy_structure as *const () as usize,
            ),
            (
                "?GetRaytracingAccelerationStructurePrebuildInfo@XboxBvhBuilder@@QEAAXPEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS@@PEBUD3D12XBOX_XDXR_STREAM_DESC@@PEAUD3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO@@@Z",
                prebuild_info as *const () as usize,
            ),
            (
                "?RehydrateRaytracingAccelerationStructureBatched2X@XboxBvhBuilder@@QEAAJPEAUID3D12GraphicsCommandList8@@IPEBUD3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC@@PEB_K@Z",
                rehydrate_batched_2x as *const () as usize,
            ),
        ];
        let mut shims: Vec<Shim> = list
            .into_iter()
            .map(|(name, address)| Shim {
                dll: "XboxBvhBuilder.dll",
                name,
                address,
            })
            .collect();
        shims.push(Shim {
            dll: "XboxBvhBuilder.dll",
            name: "?Set64BitMortonCodes@XboxBvhBuilder@@QEAAX_N@Z",
            address: set_64_bit_morton as *const () as usize,
        });
        shims
    }
}

pub fn shims() -> Vec<Shim> {
    #[cfg(windows)]
    {
        host::shims()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests;
