//! `LEONIDA_MEMORY_MAP=<seconds>`: every that many seconds, `memory.txt` in the trace folder says where the process's
//! memory is: committed and resident bytes by kind (private, mapped sections, images), and the allocations holding
//! the most resident memory, with the image offset of the call that made each when it is known.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_IMAGE, MEM_MAPPED, MEM_PRIVATE, MEMORY_BASIC_INFORMATION, VirtualQuery,
};
use windows_sys::Win32::System::ProcessStatus::K32QueryWorkingSet;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

const SMALL_PAGE: usize = 4096;
const TOP: usize = 30;

struct Region {
    base: usize,
    size: usize,
    allocation: usize,
    kind: u32,
}

fn committed_regions() -> Vec<Region> {
    let mut regions = Vec::new();
    let mut at = 0usize;
    while at < 0x7FFF_FFFF_0000 {
        // SAFETY: an all-zero structure is a valid value for VirtualQuery to overwrite.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: VirtualQuery only inspects the address space.
        let got = unsafe { VirtualQuery(at as *const _, &mut info, std::mem::size_of_val(&info)) };
        if got == 0 {
            break;
        }
        let base = info.BaseAddress as usize;
        let size = info.RegionSize;
        if info.State == MEM_COMMIT {
            regions.push(Region {
                base,
                size,
                allocation: info.AllocationBase as usize,
                kind: info.Type,
            });
        }
        let next = base + size;
        if next <= at {
            break;
        }
        at = next;
    }
    regions
}

/// The virtual addresses of the pages in the working set.
fn resident_pages() -> Vec<usize> {
    let mut words = vec![0usize; 1 << 20];
    loop {
        // SAFETY: the buffer is `words.len()` words long; the call writes the count and then that many entries.
        let ok = unsafe {
            K32QueryWorkingSet(
                GetCurrentProcess(),
                words.as_mut_ptr().cast(),
                (words.len() * std::mem::size_of::<usize>()) as u32,
            )
        };
        let count = words[0];
        if ok != 0 && count < words.len() {
            return words[1..=count]
                .iter()
                .map(|entry| entry & !(SMALL_PAGE - 1))
                .collect();
        }
        if ok == 0 && count < words.len() {
            return Vec::new();
        }
        words = vec![0usize; count + (1 << 16) + 1];
    }
}

fn kind_name(kind: u32) -> &'static str {
    match kind {
        MEM_PRIVATE => "private",
        MEM_MAPPED => "mapped",
        MEM_IMAGE => "image",
        _ => "other",
    }
}

/// The file name of the module an address lies in, if it lies in one.
fn module_of(address: usize) -> Option<String> {
    // SAFETY: an all-zero structure is a valid value for VirtualQuery to overwrite.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: VirtualQuery only inspects the address space.
    let got = unsafe { VirtualQuery(address as *const _, &mut info, std::mem::size_of_val(&info)) };
    if got == 0 || info.Type != MEM_IMAGE {
        return None;
    }
    let mut name = [0u16; 260];
    // SAFETY: the allocation base of an image region is its module handle; the buffer holds 260 characters.
    let length = unsafe {
        windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(
            info.AllocationBase as _,
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    (length > 0).then(|| {
        let path = String::from_utf16_lossy(&name[..length as usize]);
        path.rsplit('\\').next().unwrap_or(&path).to_string()
    })
}

fn gib(bytes: usize) -> f64 {
    bytes as f64 / (1u64 << 30) as f64
}

/// The memory report as text.
pub fn report() -> String {
    let regions = committed_regions();
    let mut pages = resident_pages();
    pages.sort_unstable();
    let mut committed = [0usize; 4];
    let mut resident = [0usize; 4];
    let slot = |kind: u32| match kind {
        MEM_PRIVATE => 0,
        MEM_MAPPED => 1,
        MEM_IMAGE => 2,
        _ => 3,
    };
    let mut by_allocation: std::collections::HashMap<usize, (usize, usize, u32)> =
        std::collections::HashMap::new();
    let mut page = 0usize;
    for region in &regions {
        committed[slot(region.kind)] += region.size;
        let end = region.base + region.size;
        while page < pages.len() && pages[page] < region.base {
            page += 1;
        }
        let first = page;
        while page < pages.len() && pages[page] < end {
            page += 1;
        }
        let here = (page - first) * SMALL_PAGE;
        resident[slot(region.kind)] += here;
        let entry = by_allocation
            .entry(region.allocation)
            .or_insert((0, 0, region.kind));
        entry.0 += region.size;
        entry.1 += here;
    }
    let mut text = String::new();
    let _ = writeln!(text, "kind: committed GiB, resident GiB");
    for (i, name) in ["private", "mapped", "image", "other"].iter().enumerate() {
        let _ = writeln!(
            text,
            "  {name}: {:.2}, {:.2}",
            gib(committed[i]),
            gib(resident[i])
        );
    }
    let _ = writeln!(
        text,
        "  all: {:.2}, {:.2} ({} resident pages)",
        gib(committed.iter().sum()),
        gib(resident.iter().sum()),
        pages.len()
    );
    let mut largest: Vec<_> = by_allocation.into_iter().collect();
    largest.sort_by_key(|entry| std::cmp::Reverse(entry.1.1));
    let _ = writeln!(
        text,
        "largest allocations by resident memory (base: kind, committed, resident):"
    );
    for (base, (size, here, kind)) in largest.into_iter().take(TOP) {
        let bytes = super::crash::read_memory(base, 32);
        let owner = bytes
            .as_ref()
            .map(|b| usize::from_le_bytes(b[24..32].try_into().unwrap_or([0; 8])))
            .and_then(module_of)
            .map(|m| format!(" (points into {m})"))
            .unwrap_or_default();
        let head = bytes
            .map(|bytes| {
                bytes
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_else(|| "unreadable".into());
        let head = format!("{head}{owner}");
        let _ = writeln!(
            text,
            "  {base:#x}: {}, {:.3} GiB, {:.3} GiB; starts {head}",
            kind_name(kind),
            gib(size),
            gib(here)
        );
    }
    let _ = writeln!(text, "Leonida's own blocks of 256 MiB or more:");
    text.push_str(&crate::heap::large_blocks());
    text
}

/// Starts the reports when `LEONIDA_MEMORY_MAP` is set.
pub fn schedule(folder: PathBuf) {
    let Some(seconds) = std::env::var("LEONIDA_MEMORY_MAP")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    else {
        return;
    };
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(seconds.max(1)));
            let _ = std::fs::write(folder.join("memory.txt"), report());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_counts_this_processes_memory() {
        let block = vec![1u8; 8 << 20];
        let text = report();
        assert!(text.contains("private:"));
        assert!(text.contains("largest allocations"));
        assert_eq!(block[block.len() - 1], 1);
    }
}
