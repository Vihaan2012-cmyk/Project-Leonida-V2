//! Threads and CPU topology as the console exposes them (affinity masks, ideal processors, core counts),
//! mapped onto the PC's processors.
//!
//! The executable imports `SetThreadGroupAffinity`, `GetThreadGroupAffinity`, `GetLogicalProcessorInformation`,
//! `SetThreadPriority` and `GetThreadPriority` for this; `CreateThread`, `SetProcessPriorityBoost` and
//! `IsProcessorFeaturePresent` also appear and go to Windows unchanged. `SetThreadAffinityMask`,
//! `SetThreadIdealProcessor(Ex)`, `GetSystemInfo`, `GetNativeSystemInfo`, `GetCurrentProcessorNumber` and
//! `GetActiveProcessorCount` are not imported by the executable but are shimmed too, since the satellite DLLs
//! may use them.
//!
//! What the game sees is a fixed console layout of [`CONSOLE_CORES`] cores with two threads each, numbered
//! so that logical processors `2k` and `2k + 1` share core `k`. Nothing in the available documents measures
//! the real console's counts or which masks the game uses, so the layout is the public Zen 2 one (8 cores,
//! SMT, two four-core complexes) and lives in constants.

use crate::Shim;

pub const CONSOLE_CORES: usize = 8;
pub const CONSOLE_LOGICAL: usize = CONSOLE_CORES * 2;
/// The affinity mask with every console logical processor.
/// Fewest fastest-class cores for the console layout to leave the slower ones out of it.
const MIN_LAYOUT_CORES: usize = 4;

pub const CONSOLE_MASK: u64 = (1 << CONSOLE_LOGICAL) - 1;

/// One physical core of the PC: the logical processors it runs (bits of `mask`, within one processor
/// group) and Windows' efficiency class for it (higher is faster; every core is 0 on a non-hybrid CPU).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcCore {
    pub group: u16,
    pub efficiency: u8,
    pub mask: u64,
}

/// Maps console logical processors onto PC ones.
///
/// Only one processor group is used for pinned threads (the one with the most logical processors), so a
/// thread's mask never spans groups. Console core `k` (logical processors `2k` and `2k + 1`) lands on:
///
/// - PC core `k` when the PC has at least [`CONSOLE_CORES`] cores of the fastest efficiency class (the
///   extra cores stay free for threads the game leaves unpinned), with its two logical processors in order;
/// - PC core `k * cores / 8` when it has fewer, so the game's pinned groups (for example console cores 4 to 6
///   for rendering, core 0 for its own thread) stay apart from each other instead of wrapping around onto
///   the same few cores;
/// - on a PC with 16 or more fastest-class logical processors but no SMT, each console processor takes its
///   own core.
///
/// Efficiency cores take part in the layout only when the fastest class has fewer than four cores; otherwise
/// only masks that cover every console processor reach them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapper {
    group: u16,
    table: [u8; CONSOLE_LOGICAL],
    all: u64,
}

impl Mapper {
    pub fn new(cores: &[PcCore]) -> Option<Mapper> {
        let mut size: std::collections::BTreeMap<u16, u32> = Default::default();
        for core in cores {
            *size.entry(core.group).or_default() += core.mask.count_ones();
        }
        let group = size
            .iter()
            .max_by_key(|&(group, count)| (*count, std::cmp::Reverse(*group)))
            .map(|(group, _)| *group)?;
        let mut cores: Vec<PcCore> = cores
            .iter()
            .copied()
            .filter(|c| c.group == group && c.mask != 0)
            .collect();
        if cores.is_empty() {
            return None;
        }
        cores.sort_by_key(|c| std::cmp::Reverse(c.efficiency));
        let fastest = cores[0].efficiency;
        let fast = cores.iter().take_while(|c| c.efficiency == fastest).count();
        let layout = if fast >= MIN_LAYOUT_CORES {
            &cores[..fast]
        } else {
            &cores[..]
        };
        let bits = |core: &PcCore| -> Vec<u8> {
            (0..64u8).filter(|bit| core.mask >> bit & 1 == 1).collect()
        };
        let order: Vec<u8> = layout.iter().flat_map(bits).collect();
        let mut table = [0; CONSOLE_LOGICAL];
        for (i, slot) in table.iter_mut().enumerate() {
            *slot = if order.len() >= CONSOLE_LOGICAL {
                order[i]
            } else {
                let k = i / 2;
                let core = &layout[if layout.len() >= CONSOLE_CORES {
                    k
                } else {
                    k * layout.len() / CONSOLE_CORES
                }];
                let threads = bits(core);
                threads[(i % 2).min(threads.len() - 1)]
            };
        }
        Some(Mapper {
            group,
            table,
            all: cores.iter().fold(0, |all, c| all | c.mask),
        })
    }

    /// The PC processor group pinned threads use.
    pub fn group(&self) -> u16 {
        self.group
    }

    /// A console affinity mask as a PC mask in [`group`](Self::group). `None` for an empty mask, which the
    /// console refuses too. Bits beyond the console's processors are ignored; a mask with none of the
    /// console's processors, and one with all of them, become every processor of the group.
    pub fn map_mask(&self, console: u64) -> Option<u64> {
        if console == 0 {
            return None;
        }
        let valid = console & CONSOLE_MASK;
        if valid == 0 || valid == CONSOLE_MASK {
            return Some(self.all);
        }
        Some(
            (0..CONSOLE_LOGICAL)
                .filter(|i| valid >> i & 1 == 1)
                .fold(0, |mask, i| mask | 1 << self.table[i]),
        )
    }

    /// The PC logical processor (number within [`group`](Self::group)) for a console one.
    pub fn map_processor(&self, console: u32) -> u8 {
        self.table[console as usize % CONSOLE_LOGICAL]
    }

    /// The console processor to report for a PC processor.
    pub fn console_processor(&self, group: u16, number: u8) -> u32 {
        if group == self.group
            && let Some(i) = self.table.iter().position(|&n| n == number)
        {
            return i as u32;
        }
        u32::from(number) % CONSOLE_LOGICAL as u32
    }

    /// The PC mask for "every processor" in [`group`](Self::group).
    pub fn all(&self) -> u64 {
        self.all
    }
}

/// Reads `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX` records for `RelationProcessorCore`.
pub fn parse_core_records(bytes: &[u8]) -> Vec<PcCore> {
    let mut cores = Vec::new();
    let mut at = 0;
    while at + 8 <= bytes.len() {
        let relationship = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().expect("4 bytes")) as usize;
        if size < 8 || at + size > bytes.len() {
            break;
        }
        let record = &bytes[at..at + size];
        if relationship == 0 && record.len() >= 32 {
            let efficiency = record[9];
            let groups = u16::from_le_bytes([record[30], record[31]]) as usize;
            for g in 0..groups {
                let base = 32 + g * 16;
                if base + 16 > record.len() {
                    break;
                }
                let mask = u64::from_le_bytes(record[base..base + 8].try_into().expect("8 bytes"));
                let group = u16::from_le_bytes([record[base + 8], record[base + 9]]);
                cores.push(PcCore {
                    group,
                    efficiency,
                    mask,
                });
            }
        }
        at += size;
    }
    cores
}

const RELATION_CORE: u32 = 0;
const RELATION_NUMA: u32 = 1;
const RELATION_CACHE: u32 = 2;
const RELATION_PACKAGE: u32 = 3;
const CACHE_UNIFIED: u32 = 0;
const CACHE_INSTRUCTION: u32 = 1;
const CACHE_DATA: u32 = 2;
const RECORD: usize = 32;

fn record(mask: u64, relationship: u32, body: [u8; 16]) -> [u8; RECORD] {
    let mut out = [0; RECORD];
    out[..8].copy_from_slice(&mask.to_le_bytes());
    out[8..12].copy_from_slice(&relationship.to_le_bytes());
    out[16..].copy_from_slice(&body);
    out
}

fn cache(level: u8, associativity: u8, size: u32, kind: u32) -> [u8; 16] {
    let mut body = [0; 16];
    body[0] = level;
    body[1] = associativity;
    body[2..4].copy_from_slice(&64u16.to_le_bytes());
    body[4..8].copy_from_slice(&size.to_le_bytes());
    body[8..12].copy_from_slice(&kind.to_le_bytes());
    body
}

/// The console's processors as `GetLogicalProcessorInformation` returns them (32-byte
/// `SYSTEM_LOGICAL_PROCESSOR_INFORMATION` records): per core a core record with the SMT flag and its
/// 32 KiB L1 data, 32 KiB L1 instruction and 512 KiB L2 caches; one 4 MiB L3 per four cores; then one NUMA
/// node and one package over everything.
pub fn console_processor_information() -> Vec<u8> {
    let mut out = Vec::new();
    for core in 0..CONSOLE_CORES {
        let mask = 3u64 << (core * 2);
        let mut flags = [0u8; 16];
        flags[0] = 1;
        out.extend(record(mask, RELATION_CORE, flags));
        out.extend(record(
            mask,
            RELATION_CACHE,
            cache(1, 8, 32 << 10, CACHE_DATA),
        ));
        out.extend(record(
            mask,
            RELATION_CACHE,
            cache(1, 8, 32 << 10, CACHE_INSTRUCTION),
        ));
        out.extend(record(
            mask,
            RELATION_CACHE,
            cache(2, 8, 512 << 10, CACHE_UNIFIED),
        ));
    }
    for complex in 0..CONSOLE_CORES / 4 {
        let mask = 0xFFu64 << (complex * 8);
        out.extend(record(
            mask,
            RELATION_CACHE,
            cache(3, 16, 4 << 20, CACHE_UNIFIED),
        ));
    }
    out.extend(record(CONSOLE_MASK, RELATION_NUMA, [0; 16]));
    out.extend(record(CONSOLE_MASK, RELATION_PACKAGE, [0; 16]));
    out
}

/// What a game thread asked for, as the console would report it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadView {
    pub affinity: u64,
    pub ideal: u32,
    pub priority: Option<i32>,
}

impl Default for ThreadView {
    fn default() -> Self {
        ThreadView {
            affinity: CONSOLE_MASK,
            ideal: 0,
            priority: None,
        }
    }
}

const TIME_CRITICAL: i32 = 15;
const HIGHEST: i32 = 2;

/// The priority to give Windows for one the game asked for: time-critical becomes highest, since a
/// console's cores are the game's alone but a PC's are shared with the system and the audio stack.
pub fn host_priority(requested: i32) -> i32 {
    if requested == TIME_CRITICAL {
        HIGHEST
    } else {
        requested
    }
}

#[cfg(windows)]
mod imp {
    use std::collections::BTreeMap;
    use std::ffi::c_void;
    use std::sync::{Mutex, OnceLock};

    use windows_sys::Win32::Foundation::SetLastError;
    use windows_sys::Win32::System::Threading::{GetThreadPriority, SetThreadPriority};

    use super::*;
    use crate::trace::note;

    type Handle = *mut c_void;
    const ERROR_INVALID_PARAMETER: u32 = 87;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    const ALL_GROUPS: u16 = 0xFFFF;
    const PRIORITY_ERROR: i32 = 0x7FFF_FFFF;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct GroupAffinity {
        pub mask: usize,
        pub group: u16,
        pub reserved: [u16; 3],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct ProcessorNumber {
        group: u16,
        number: u8,
        reserved: u8,
    }

    #[repr(C)]
    pub struct SystemInfo {
        architecture: u16,
        reserved: u16,
        page_size: u32,
        minimum_address: usize,
        maximum_address: usize,
        active_mask: usize,
        processors: u32,
        processor_type: u32,
        granularity: u32,
        level: u16,
        revision: u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLogicalProcessorInformationEx(
            relationship: u32,
            buffer: *mut u8,
            length: *mut u32,
        ) -> i32;
        fn SetThreadGroupAffinity(
            thread: Handle,
            affinity: *const GroupAffinity,
            previous: *mut GroupAffinity,
        ) -> i32;
        fn SetThreadIdealProcessorEx(
            thread: Handle,
            ideal: *const ProcessorNumber,
            previous: *mut ProcessorNumber,
        ) -> i32;
        fn GetCurrentProcessorNumberEx(number: *mut ProcessorNumber);
        fn GetThreadId(thread: Handle) -> u32;
        fn GetSystemInfo(info: *mut SystemInfo);
        fn GetNativeSystemInfo(info: *mut SystemInfo);
    }

    static MAPPER: OnceLock<Mapper> = OnceLock::new();
    static VIEWS: Mutex<BTreeMap<u32, ThreadView>> = Mutex::new(BTreeMap::new());

    pub fn host_cores() -> Vec<PcCore> {
        let mut length = 0u32;
        // SAFETY: a size query with no buffer.
        unsafe {
            GetLogicalProcessorInformationEx(RELATION_CORE, std::ptr::null_mut(), &mut length)
        };
        if length == 0 {
            return Vec::new();
        }
        let mut buffer = vec![0u8; length as usize];
        // SAFETY: `buffer` has `length` bytes.
        let ok = unsafe {
            GetLogicalProcessorInformationEx(RELATION_CORE, buffer.as_mut_ptr(), &mut length)
        };
        if ok == 0 {
            return Vec::new();
        }
        parse_core_records(&buffer[..length as usize])
    }

    fn mapper() -> &'static Mapper {
        MAPPER.get_or_init(|| {
            let mapper = Mapper::new(&host_cores()).or_else(|| {
                note(
                    "could not read the processor topology; console processors map to PCs 1:1"
                        .into(),
                );
                let cores: Vec<PcCore> = (0..CONSOLE_LOGICAL as u64)
                    .map(|i| PcCore {
                        group: 0,
                        efficiency: 0,
                        mask: 1 << i,
                    })
                    .collect();
                Mapper::new(&cores)
            });
            mapper.expect("a non-empty fallback topology")
        })
    }

    fn thread_id(thread: Handle) -> Option<u32> {
        // SAFETY: asks Windows for the id of a handle the program passed.
        let id = unsafe { GetThreadId(thread) };
        (id != 0).then_some(id)
    }

    fn view(id: u32) -> ThreadView {
        VIEWS
            .lock()
            .ok()
            .and_then(|views| views.get(&id).copied())
            .unwrap_or_default()
    }

    fn update(id: u32, change: impl FnOnce(&mut ThreadView)) {
        if let Ok(mut views) = VIEWS.lock() {
            change(views.entry(id).or_default());
        }
    }

    fn set_affinity(thread: Handle, console: u64) -> Option<u64> {
        let id = thread_id(thread);
        let Some(host) = mapper().map_mask(console) else {
            // SAFETY: sets this thread's last-error value.
            unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
            return None;
        };
        let try_mask = |mask: u64| {
            let wanted = GroupAffinity {
                mask: mask as usize,
                group: mapper().group(),
                reserved: [0; 3],
            };
            let mut before = GroupAffinity::default();
            // SAFETY: both structures live through the call.
            unsafe { SetThreadGroupAffinity(thread, &wanted, &mut before) != 0 }
        };
        if !try_mask(host) && !try_mask(mapper().all()) {
            return None;
        }
        let valid = console & CONSOLE_MASK;
        let kept = if valid == 0 { CONSOLE_MASK } else { valid };
        match id {
            Some(id) => {
                let mut before = CONSOLE_MASK;
                update(id, |v| before = std::mem::replace(&mut v.affinity, kept));
                Some(before)
            }
            None => Some(CONSOLE_MASK),
        }
    }

    pub unsafe extern "win64" fn set_thread_group_affinity(
        thread: Handle,
        affinity: *const GroupAffinity,
        previous: *mut GroupAffinity,
    ) -> i32 {
        if affinity.is_null() {
            // SAFETY: sets this thread's last-error value.
            unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
            return 0;
        }
        // SAFETY: the caller passes a GROUP_AFFINITY.
        let wanted = unsafe { *affinity };
        match set_affinity(thread, wanted.mask as u64) {
            Some(before) => {
                if !previous.is_null() {
                    // SAFETY: the caller passes room for the previous affinity.
                    unsafe {
                        *previous = GroupAffinity {
                            mask: before as usize,
                            group: 0,
                            reserved: [0; 3],
                        }
                    };
                }
                1
            }
            None => 0,
        }
    }

    pub unsafe extern "win64" fn get_thread_group_affinity(
        thread: Handle,
        out: *mut GroupAffinity,
    ) -> i32 {
        let Some(id) = thread_id(thread) else {
            // SAFETY: sets this thread's last-error value.
            unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
            return 0;
        };
        if out.is_null() {
            // SAFETY: sets this thread's last-error value.
            unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
            return 0;
        }
        // SAFETY: the caller passes room for a GROUP_AFFINITY.
        unsafe {
            *out = GroupAffinity {
                mask: view(id).affinity as usize,
                group: 0,
                reserved: [0; 3],
            }
        };
        1
    }

    pub extern "win64" fn set_thread_affinity_mask(thread: Handle, mask: usize) -> usize {
        set_affinity(thread, mask as u64).unwrap_or(0) as usize
    }

    pub unsafe extern "win64" fn set_thread_ideal_processor_ex(
        thread: Handle,
        ideal: *const ProcessorNumber,
        previous: *mut ProcessorNumber,
    ) -> i32 {
        if ideal.is_null() {
            // SAFETY: sets this thread's last-error value.
            unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
            return 0;
        }
        // SAFETY: the caller passes a PROCESSOR_NUMBER.
        let wanted = unsafe { *ideal };
        let mapper = mapper();
        let host = ProcessorNumber {
            group: mapper.group(),
            number: mapper.map_processor(u32::from(wanted.number)),
            reserved: 0,
        };
        // SAFETY: both structures live through the call.
        if unsafe { SetThreadIdealProcessorEx(thread, &host, std::ptr::null_mut()) } == 0 {
            return 0;
        }
        let mut before = 0;
        if let Some(id) = thread_id(thread) {
            update(id, |v| {
                before = std::mem::replace(&mut v.ideal, u32::from(wanted.number))
            });
        }
        if !previous.is_null() {
            // SAFETY: the caller passes room for the previous ideal processor.
            unsafe {
                *previous = ProcessorNumber {
                    group: 0,
                    number: before as u8,
                    reserved: 0,
                }
            };
        }
        1
    }

    pub extern "win64" fn set_thread_ideal_processor(thread: Handle, ideal: u32) -> u32 {
        let id = thread_id(thread);
        let before = id.map_or(0, |id| view(id).ideal);
        if ideal >= 64 {
            return before;
        }
        let wanted = ProcessorNumber {
            group: 0,
            number: ideal as u8,
            reserved: 0,
        };
        // SAFETY: a valid PROCESSOR_NUMBER on this stack.
        match unsafe { set_thread_ideal_processor_ex(thread, &wanted, std::ptr::null_mut()) } {
            0 => u32::MAX,
            _ => before,
        }
    }

    /// # Safety
    /// `buffer` has `*length` bytes when it is not null; `length` is not null.
    pub unsafe extern "win64" fn get_logical_processor_information(
        buffer: *mut u8,
        length: *mut u32,
    ) -> i32 {
        if length.is_null() {
            // SAFETY: sets this thread's last-error value.
            unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
            return 0;
        }
        let info = console_processor_information();
        // SAFETY: as the caller promises.
        let room = unsafe { *length } as usize;
        // SAFETY: as above.
        unsafe { *length = info.len() as u32 };
        if buffer.is_null() || room < info.len() {
            // SAFETY: sets this thread's last-error value.
            unsafe { SetLastError(ERROR_INSUFFICIENT_BUFFER) };
            return 0;
        }
        // SAFETY: `buffer` has room for `info.len()` bytes, checked above.
        unsafe { std::ptr::copy_nonoverlapping(info.as_ptr(), buffer, info.len()) };
        1
    }

    fn console_system_info(info: &mut SystemInfo) {
        info.processors = CONSOLE_LOGICAL as u32;
        info.active_mask = CONSOLE_MASK as usize;
    }

    pub unsafe extern "win64" fn get_system_info(info: *mut SystemInfo) {
        if info.is_null() {
            return;
        }
        // SAFETY: the caller passes a SYSTEM_INFO.
        unsafe {
            GetSystemInfo(info);
            console_system_info(&mut *info);
        }
    }

    pub unsafe extern "win64" fn get_native_system_info(info: *mut SystemInfo) {
        if info.is_null() {
            return;
        }
        // SAFETY: the caller passes a SYSTEM_INFO.
        unsafe {
            GetNativeSystemInfo(info);
            console_system_info(&mut *info);
        }
    }

    pub extern "win64" fn get_current_processor_number() -> u32 {
        let mut host = ProcessorNumber::default();
        // SAFETY: a PROCESSOR_NUMBER on this stack.
        unsafe { GetCurrentProcessorNumberEx(&mut host) };
        mapper().console_processor(host.group, host.number)
    }

    pub extern "win64" fn get_active_processor_count(group: u16) -> u32 {
        if group == 0 || group == ALL_GROUPS {
            CONSOLE_LOGICAL as u32
        } else {
            0
        }
    }

    pub extern "win64" fn set_thread_priority(thread: Handle, priority: i32) -> i32 {
        // SAFETY: Windows validates the handle and value.
        let ok = unsafe { SetThreadPriority(thread, host_priority(priority)) };
        if ok != 0
            && let Some(id) = thread_id(thread)
        {
            update(id, |v| v.priority = Some(priority));
        }
        ok
    }

    pub extern "win64" fn get_thread_priority(thread: Handle) -> i32 {
        // SAFETY: Windows validates the handle.
        let real = unsafe { GetThreadPriority(thread) };
        if real == PRIORITY_ERROR {
            return real;
        }
        thread_id(thread)
            .and_then(|id| view(id).priority)
            .unwrap_or(real)
    }
}

/// The API-set names the same kernel functions are imported under, besides `kernel32` and `kernelbase`.
#[cfg(windows)]
const DLLS: &[&str] = &[
    "kernel32.dll",
    "kernelbase.dll",
    "api-ms-win-core-processthreads-l1-1-0.dll",
    "api-ms-win-core-processthreads-l1-1-1.dll",
    "api-ms-win-core-processthreads-l1-1-2.dll",
    "api-ms-win-core-processthreads-l1-1-3.dll",
    "api-ms-win-core-processtopology-l1-1-0.dll",
    "api-ms-win-core-sysinfo-l1-1-0.dll",
    "api-ms-win-core-sysinfo-l1-2-0.dll",
    "api-ms-win-core-sysinfo-l1-2-1.dll",
];

pub fn shims() -> Vec<Shim> {
    #[cfg(windows)]
    {
        let list: [(&str, usize); 13] = [
            (
                "SetThreadGroupAffinity",
                imp::set_thread_group_affinity as *const () as usize,
            ),
            (
                "GetThreadGroupAffinity",
                imp::get_thread_group_affinity as *const () as usize,
            ),
            (
                "SetThreadAffinityMask",
                imp::set_thread_affinity_mask as *const () as usize,
            ),
            (
                "SetThreadIdealProcessor",
                imp::set_thread_ideal_processor as *const () as usize,
            ),
            (
                "SetThreadIdealProcessorEx",
                imp::set_thread_ideal_processor_ex as *const () as usize,
            ),
            (
                "GetLogicalProcessorInformation",
                imp::get_logical_processor_information as *const () as usize,
            ),
            ("GetSystemInfo", imp::get_system_info as *const () as usize),
            (
                "GetNativeSystemInfo",
                imp::get_native_system_info as *const () as usize,
            ),
            (
                "GetCurrentProcessorNumber",
                imp::get_current_processor_number as *const () as usize,
            ),
            (
                "GetActiveProcessorCount",
                imp::get_active_processor_count as *const () as usize,
            ),
            (
                "GetMaximumProcessorCount",
                imp::get_active_processor_count as *const () as usize,
            ),
            (
                "SetThreadPriority",
                imp::set_thread_priority as *const () as usize,
            ),
            (
                "GetThreadPriority",
                imp::get_thread_priority as *const () as usize,
            ),
        ];
        list.into_iter()
            .flat_map(|(name, address)| DLLS.iter().map(move |dll| Shim { dll, name, address }))
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `groups` is a list of groups, each a list of (efficiency class, SMT threads) cores.
    fn pc(groups: &[&[(u8, u32)]]) -> Vec<PcCore> {
        let mut cores = Vec::new();
        for (group, list) in groups.iter().enumerate() {
            let mut next = 0;
            for &(efficiency, threads) in *list {
                cores.push(PcCore {
                    group: group as u16,
                    efficiency,
                    mask: ((1u64 << threads) - 1) << next,
                });
                next += threads;
            }
        }
        cores
    }

    fn table(cores: &[PcCore]) -> Vec<u8> {
        let m = Mapper::new(cores).unwrap();
        (0..16).map(|i| m.map_processor(i)).collect()
    }

    #[test]
    fn eight_cores_with_smt_is_the_console_layout() {
        let m = Mapper::new(&pc(&[&[(0, 2); 8]])).unwrap();
        assert_eq!(table(&pc(&[&[(0, 2); 8]])), (0..16).collect::<Vec<u8>>());
        assert_eq!(m.map_mask(0b11), Some(0b11));
        assert_eq!(m.map_mask(0x00F0), Some(0x00F0));
        assert_eq!(m.map_mask(CONSOLE_MASK), Some(0xFFFF));
    }

    #[test]
    fn six_cores_with_smt_share_cores_between_neighbouring_console_cores() {
        let cores = pc(&[&[(0, 2); 6]]);
        assert_eq!(
            table(&cores),
            vec![0, 1, 0, 1, 2, 3, 4, 5, 6, 7, 6, 7, 8, 9, 10, 11]
        );
        let m = Mapper::new(&cores).unwrap();
        assert_eq!(m.map_mask(CONSOLE_MASK), Some(0xFFF));
        assert_eq!(m.map_mask(0xF000), Some(0xF00));
    }

    #[test]
    fn the_games_render_and_main_masks_stay_apart_on_small_pcs() {
        let render = 0x3F00;
        let first_core = 0b1;
        for cores in [
            pc(&[&[(0, 2); 6]]),
            pc(&[&[(0, 2); 4]]),
            pc(&[&[(0, 1); 6]]),
            pc(&[&[(0, 2); 5]]),
        ] {
            let m = Mapper::new(&cores).unwrap();
            let on_render = m.map_mask(render).unwrap();
            assert_eq!(on_render & m.map_mask(first_core).unwrap(), 0);
            assert!(on_render.count_ones() >= 2);
            assert_ne!(on_render, m.all());
        }
    }

    #[test]
    fn eight_cores_without_smt_put_both_console_threads_of_a_core_on_it() {
        let cores = pc(&[&[(0, 1); 8]]);
        assert_eq!(
            table(&cores),
            vec![0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7]
        );
        let m = Mapper::new(&cores).unwrap();
        assert_eq!(m.map_mask(0b0101), Some(0b11));
        assert_eq!(m.map_mask(0b1_0000_0001), Some(0b1_0001));
    }

    #[test]
    fn sixteen_cores_without_smt_give_every_console_processor_its_own() {
        let cores = pc(&[&[(0, 1); 16]]);
        assert_eq!(table(&cores), (0..16).collect::<Vec<u8>>());
    }

    #[test]
    fn many_cores_leave_the_extras_to_unpinned_threads() {
        let cores = pc(&[&[(0, 2); 16]]);
        let m = Mapper::new(&cores).unwrap();
        assert_eq!(table(&cores), (0..16).collect::<Vec<u8>>());
        assert_eq!(m.map_mask(CONSOLE_MASK), Some(u64::from(u32::MAX)));
        assert_eq!(m.all(), u64::from(u32::MAX));
        assert_eq!(m.map_mask(0x0003), Some(0b11));
    }

    #[test]
    fn efficiency_cores_are_left_to_unpinned_threads() {
        let mut list = vec![(1, 2); 6];
        list.extend([(0, 1); 8]);
        let cores = pc(&[&list]);
        let m = Mapper::new(&cores).unwrap();
        for console in 0..16 {
            assert!(m.map_processor(console) < 12);
        }
        assert_eq!(m.map_mask(0x0FFF), Some(0xFF));
        assert_eq!(m.map_mask(CONSOLE_MASK), Some((1 << 20) - 1));
    }

    #[test]
    fn efficiency_cores_listed_first_are_still_left_out() {
        let mut list = vec![(0, 1); 4];
        list.extend([(1, 2); 4]);
        let m = Mapper::new(&pc(&[&list])).unwrap();
        assert_eq!(m.map_processor(0), 4);
        assert_eq!(m.map_processor(1), 5);
        assert_eq!(m.map_processor(8), 8);
        for console in 0..16 {
            assert!(m.map_processor(console) >= 4);
        }
    }

    #[test]
    fn too_few_fast_cores_bring_the_slow_ones_into_the_layout() {
        let mut list = vec![(1, 2); 2];
        list.extend([(0, 1); 6]);
        let m = Mapper::new(&pc(&[&list])).unwrap();
        assert_eq!(m.map_processor(0), 0);
        assert_eq!(m.map_processor(15), 9);
    }

    #[test]
    fn a_two_core_pc_splits_the_console_in_halves() {
        let cores = pc(&[&[(0, 2); 2]]);
        let m = Mapper::new(&cores).unwrap();
        assert_eq!(m.map_mask(0x00FF), Some(0b0011));
        assert_eq!(m.map_mask(0xFF00), Some(0b1100));
        assert_eq!(m.map_mask(0b10), Some(0b10));
    }

    #[test]
    fn the_biggest_group_is_used_when_there_are_several() {
        let cores = pc(&[&[(0, 2); 16], &[(0, 2); 8]]);
        let m = Mapper::new(&cores).unwrap();
        assert_eq!(m.group(), 0);
        let cores = pc(&[&[(0, 2); 8], &[(0, 2); 16]]);
        let m = Mapper::new(&cores).unwrap();
        assert_eq!(m.group(), 1);
        assert_eq!(m.all(), u64::from(u32::MAX));
        let tied = Mapper::new(&pc(&[&[(0, 1); 4], &[(0, 1); 4]])).unwrap();
        assert_eq!(tied.group(), 0);
    }

    #[test]
    fn masks_the_console_accepts_never_map_to_nothing() {
        let tables = [
            pc(&[&[(0, 2); 8]]),
            pc(&[&[(0, 1); 4]]),
            pc(&[&[(1, 2); 8], &[(0, 1); 16]]),
            pc(&[&[(0, 1)]]),
            pc(&[&[(0, 2); 32]]),
        ];
        for cores in &tables {
            let m = Mapper::new(cores).unwrap();
            for mask in (1u64..=CONSOLE_MASK)
                .step_by(7)
                .chain([1 << 15, 0xAAAA, 0x5555])
            {
                let mapped = m.map_mask(mask).unwrap();
                assert_ne!(mapped, 0);
                assert_eq!(mapped & !m.all(), 0);
            }
        }
    }

    #[test]
    fn empty_and_foreign_masks() {
        let m = Mapper::new(&pc(&[&[(0, 2); 4]])).unwrap();
        assert_eq!(m.map_mask(0), None);
        assert_eq!(m.map_mask(1 << 40), Some(m.all()));
        assert_eq!(m.map_mask(1 << 40 | 0b11), m.map_mask(0b11));
    }

    #[test]
    fn host_processors_report_as_console_ones() {
        let m = Mapper::new(&pc(&[&[(0, 2); 4]])).unwrap();
        assert_eq!(m.console_processor(0, 3), 5);
        assert_eq!(m.console_processor(0, 6), 12);
        assert_eq!(m.console_processor(1, 6), 6);
        let big = Mapper::new(&pc(&[&[(0, 2); 16]])).unwrap();
        assert_eq!(big.console_processor(0, 29), 13);
    }

    #[test]
    fn nothing_to_map_onto() {
        assert!(Mapper::new(&[]).is_none());
        assert!(
            Mapper::new(&[PcCore {
                group: 0,
                efficiency: 0,
                mask: 0
            }])
            .is_none()
        );
    }

    fn core_record(efficiency: u8, groups: &[(u64, u16)]) -> Vec<u8> {
        let size = 32 + 16 * groups.len();
        let mut out = vec![0u8; size];
        out[4..8].copy_from_slice(&(size as u32).to_le_bytes());
        out[8] = 1;
        out[9] = efficiency;
        out[30..32].copy_from_slice(&(groups.len() as u16).to_le_bytes());
        for (g, &(mask, group)) in groups.iter().enumerate() {
            out[32 + g * 16..40 + g * 16].copy_from_slice(&mask.to_le_bytes());
            out[40 + g * 16..42 + g * 16].copy_from_slice(&group.to_le_bytes());
        }
        out
    }

    #[test]
    fn core_records_parse() {
        let mut bytes = core_record(1, &[(0b11, 0)]);
        let mut other = vec![0u8; 48];
        other[0] = 3;
        other[4] = 48;
        bytes.extend(other);
        bytes.extend(core_record(0, &[(0b100, 0)]));
        bytes.extend(core_record(0, &[(1, 1)]));
        let cores = parse_core_records(&bytes);
        assert_eq!(
            cores,
            vec![
                PcCore {
                    group: 0,
                    efficiency: 1,
                    mask: 3
                },
                PcCore {
                    group: 0,
                    efficiency: 0,
                    mask: 4
                },
                PcCore {
                    group: 1,
                    efficiency: 0,
                    mask: 1
                },
            ]
        );
        assert!(parse_core_records(&bytes[..bytes.len() - 5]).len() == 2);
    }

    #[test]
    fn console_information_has_eight_smt_cores_and_two_l3_caches() {
        let info = console_processor_information();
        assert_eq!(info.len() % RECORD, 0);
        let mut cores = 0;
        let mut l3 = Vec::new();
        let mut numa = 0;
        let mut packages = 0;
        for rec in info.chunks(RECORD) {
            let mask = u64::from_le_bytes(rec[..8].try_into().unwrap());
            match u32::from_le_bytes(rec[8..12].try_into().unwrap()) {
                RELATION_CORE => {
                    assert_eq!(rec[16], 1);
                    assert_eq!(mask, 3 << (cores * 2));
                    cores += 1;
                }
                RELATION_CACHE if rec[16] == 3 => l3.push(mask),
                RELATION_CACHE => {
                    assert_eq!(
                        u16::from_le_bytes(rec[18..20].try_into().unwrap()),
                        64,
                        "cache line"
                    );
                    assert_eq!(mask.count_ones(), 2);
                }
                RELATION_NUMA => {
                    assert_eq!(mask, CONSOLE_MASK);
                    numa += 1;
                }
                RELATION_PACKAGE => {
                    assert_eq!(mask, CONSOLE_MASK);
                    packages += 1;
                }
                other => panic!("relationship {other}"),
            }
        }
        assert_eq!(cores, CONSOLE_CORES);
        assert_eq!(l3, vec![0xFF, 0xFF00]);
        assert_eq!((numa, packages), (1, 1));
    }

    #[test]
    fn time_critical_is_lowered_and_the_rest_passes() {
        assert_eq!(host_priority(15), 2);
        for p in [-15, -2, -1, 0, 1, 2] {
            assert_eq!(host_priority(p), p);
        }
        assert_eq!(host_priority(0x10000), 0x10000);
    }

    #[cfg(windows)]
    #[test]
    fn the_host_topology_gives_a_usable_mapping() {
        let cores = imp::host_cores();
        assert!(!cores.is_empty());
        let m = Mapper::new(&cores).unwrap();
        assert_ne!(m.map_mask(0b1).unwrap(), 0);
        assert_ne!(m.all(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn shims_cover_each_api_set_the_game_imports() {
        let shims = shims();
        for (dll, name) in [
            (
                "api-ms-win-core-processtopology-l1-1-0.dll",
                "SetThreadGroupAffinity",
            ),
            (
                "api-ms-win-core-processtopology-l1-1-0.dll",
                "GetThreadGroupAffinity",
            ),
            (
                "api-ms-win-core-sysinfo-l1-1-0.dll",
                "GetLogicalProcessorInformation",
            ),
            (
                "api-ms-win-core-processthreads-l1-1-0.dll",
                "SetThreadPriority",
            ),
            (
                "api-ms-win-core-processthreads-l1-1-0.dll",
                "GetThreadPriority",
            ),
            ("kernel32.dll", "SetThreadAffinityMask"),
        ] {
            assert!(
                shims.iter().any(|s| s.dll == dll && s.name == name),
                "{dll}!{name}"
            );
        }
        assert!(!shims.iter().any(|s| s.name == "SetProcessPriorityBoost"));
    }

    #[cfg(windows)]
    #[test]
    fn a_thread_reads_back_the_console_affinity_it_set() {
        use std::ffi::c_void;

        std::thread::spawn(move || {
            let current: *mut c_void = -2isize as *mut c_void;
            let before = imp::set_thread_affinity_mask(current, 0b11);
            assert_eq!(before, CONSOLE_MASK as usize);
            let mut got = imp::GroupAffinity::default();
            // SAFETY: a GROUP_AFFINITY on this stack and the current-thread pseudo handle.
            assert_eq!(
                unsafe { imp::get_thread_group_affinity(current, &mut got) },
                1
            );
            assert_eq!(got.mask, 0b11);
            assert_eq!(imp::set_thread_affinity_mask(current, 0b1100), 0b11);
            assert_eq!(imp::set_thread_affinity_mask(current, 0), 0);
            assert_eq!(
                imp::set_thread_affinity_mask(current, CONSOLE_MASK as usize),
                0b1100
            );
            assert!(imp::get_current_processor_number() < CONSOLE_LOGICAL as u32);
        })
        .join()
        .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn the_legacy_processor_information_call_follows_the_two_call_pattern() {
        let mut length = 0u32;
        // SAFETY: a size query with a null buffer.
        assert_eq!(
            unsafe { imp::get_logical_processor_information(std::ptr::null_mut(), &mut length) },
            0
        );
        assert_eq!(length as usize, console_processor_information().len());
        let mut buffer = vec![0u8; length as usize];
        // SAFETY: `buffer` has `length` bytes.
        assert_eq!(
            unsafe { imp::get_logical_processor_information(buffer.as_mut_ptr(), &mut length) },
            1
        );
        assert_eq!(buffer, console_processor_information());
        assert_eq!(imp::get_active_processor_count(0xFFFF), 16);
    }
}
