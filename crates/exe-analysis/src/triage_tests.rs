use pe_image::{DIRECTORY_RESOURCE, DIRECTORY_TLS, Image};

use crate::testimage::Builder;
use crate::triage::*;

fn noise(len: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

fn directory_entry(index: usize) -> u32 {
    0x84 + 20 + 112 + 8 * index as u32
}

fn put32(bytes: &mut [u8], at: u32, value: u32) {
    bytes[at as usize..at as usize + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn entropy_of_known_inputs() {
    assert_eq!(entropy(&[]), 0.0);
    assert_eq!(entropy(&[7; 100]), 0.0);
    let two: Vec<u8> = (0..100).map(|i| (i % 2) as u8).collect();
    assert!((entropy(&two) - 1.0).abs() < 1e-9);
    let all: Vec<u8> = (0..=255u8).cycle().take(1024).collect();
    assert!((entropy(&all) - 8.0).abs() < 1e-9);
    assert!(entropy(&noise(8192)) > 7.9);
}

#[test]
fn classifies_risky_imports() {
    use ApiCategory::*;
    for (name, category) in [
        ("OpenProcess", ProcessInjection),
        ("WriteProcessMemory", ProcessInjection),
        ("CreateRemoteThreadEx", ProcessInjection),
        ("NtUnmapViewOfSection", ProcessInjection),
        ("QueueUserAPC", ProcessInjection),
        ("SetThreadContext", ProcessInjection),
        ("SetWindowsHookExW", Hook),
        ("RegSetValueExW", Persistence),
        ("RegCreateKeyExA", Persistence),
        ("CreateServiceW", Persistence),
        ("CryptUnprotectData", Credential),
        ("CredReadW", Credential),
        ("WinHttpOpen", Network),
        ("InternetOpenUrlA", Network),
        ("socket", Network),
        ("URLDownloadToFileW", Network),
        ("CreateProcessW", ProcessCreation),
        ("ShellExecuteExW", ProcessCreation),
        ("WinExec", ProcessCreation),
        ("IsDebuggerPresent", AntiAnalysis),
        ("GetProcAddress", DynamicResolution),
        ("LoadLibraryExW", DynamicResolution),
        ("VirtualProtect", MemoryProtection),
    ] {
        assert_eq!(classify_api(name), Some(category), "{name}");
    }
    for name in [
        "ExitProcess",
        "CreateFileW",
        "OpenProcessToken",
        "sendmail",
        "CloseHandle",
    ] {
        assert_eq!(classify_api(name), None, "{name}");
    }
}

#[test]
fn reports_imports_by_dll_with_risky_ones_flagged() {
    let mut builder = Builder::new();
    builder.import(
        "kernel32.dll",
        &[
            "ExitProcess",
            "OpenProcess",
            "WriteProcessMemory",
            "IsDebuggerPresent",
            "GetProcAddress",
            "LoadLibraryA",
            "CreateProcessW",
        ],
    );
    let bytes = builder.finish();
    let image = Image::parse(&bytes).unwrap();
    let triage = analyse(&image);
    assert_eq!(triage.imports.len(), 1);
    assert_eq!(triage.imports[0].names.len(), 7);
    let names = |c| -> Vec<&str> { triage.hits_of(c).iter().map(|h| h.name.as_str()).collect() };
    assert_eq!(
        names(ApiCategory::ProcessInjection),
        ["OpenProcess", "WriteProcessMemory"]
    );
    assert_eq!(names(ApiCategory::AntiAnalysis), ["IsDebuggerPresent"]);
    assert_eq!(names(ApiCategory::DynamicResolution).len(), 2);
    assert_eq!(names(ApiCategory::ProcessCreation), ["CreateProcessW"]);
    assert!(
        triage
            .indicators
            .iter()
            .any(|i| i.contains("process injection") && i.contains("WriteProcessMemory"))
    );
    assert!(
        !triage
            .indicators
            .iter()
            .any(|i| i.contains("IsDebuggerPresent"))
    );
}

#[test]
fn flags_writable_executable_and_high_entropy_sections() {
    let mut builder = Builder::new();
    builder.put(Builder::TEXT, &noise(0x2000));
    let mut bytes = builder.finish();
    let third_section_flags = 0x84 + 20 + 240 + 40 * 2 + 36;
    put32(&mut bytes, third_section_flags, 0xE000_0040);
    let image = Image::parse(&bytes).unwrap();
    let triage = analyse(&image);
    assert!(triage.sections[0].entropy > HIGH_ENTROPY_EXECUTABLE);
    assert!(triage.sections[1].entropy < 1.0);
    assert!(
        triage
            .indicators
            .iter()
            .any(|i| i.contains("section .data is writable and executable"))
    );
    assert!(
        triage
            .indicators
            .iter()
            .any(|i| i.contains("executable section .text has entropy"))
    );
    assert_eq!(triage.entry_section.as_deref(), Some(".text"));
}

#[test]
fn quiet_image_has_no_indicators() {
    let mut builder = Builder::new();
    builder.import("kernel32.dll", &["ExitProcess", "CreateFileW"]);
    let bytes = builder.finish();
    let triage = analyse(&Image::parse(&bytes).unwrap());
    assert_eq!(triage.indicators, Vec::<String>::new());
    assert!(triage.overlay.is_none() && triage.tls_callbacks.is_empty());
}

#[test]
fn overlay_after_the_last_section_is_measured() {
    let mut bytes = Builder::new().finish();
    bytes.extend(noise(3000));
    let triage = analyse(&Image::parse(&bytes).unwrap());
    let overlay = triage.overlay.unwrap();
    assert_eq!((overlay.offset, overlay.len), (0x8000, 3000));
    assert!(overlay.entropy > 7.5);
}

#[test]
fn tls_callbacks_are_listed() {
    let mut builder = Builder::new();
    let directory = Builder::DATA + 0x100;
    let array = Builder::DATA + 0x200;
    builder.put64(directory + 24, Builder::va(array));
    builder.put64(array, Builder::va(Builder::TEXT + 0x10));
    builder.put64(array + 8, Builder::va(Builder::TEXT + 0x80));
    let mut bytes = builder.finish();
    put32(&mut bytes, directory_entry(DIRECTORY_TLS), directory);
    put32(&mut bytes, directory_entry(DIRECTORY_TLS) + 4, 40);
    let triage = analyse(&Image::parse(&bytes).unwrap());
    assert_eq!(
        triage.tls_callbacks,
        vec![Builder::TEXT + 0x10, Builder::TEXT + 0x80]
    );
    assert!(
        triage
            .indicators
            .iter()
            .any(|i| i.contains("2 TLS callback"))
    );
}

#[test]
fn resource_types_and_embedded_executables() {
    let mut builder = Builder::new();
    let root = Builder::DATA + 0x400;
    let data = Builder::DATA + 0x500;
    builder.put(root + 14, &1u16.to_le_bytes());
    builder.put32(root + 16, 10);
    builder.put32(root + 20, 0x8000_0030);
    builder.put(root + 0x30 + 14, &1u16.to_le_bytes());
    builder.put32(root + 0x30 + 16, 1);
    builder.put32(root + 0x30 + 20, 0x8000_0060);
    builder.put(root + 0x60 + 14, &1u16.to_le_bytes());
    builder.put32(root + 0x60 + 16, 0x409);
    builder.put32(root + 0x60 + 20, 0x90);
    builder.put32(root + 0x90, data);
    builder.put32(root + 0x94, 16);
    builder.put(data, b"MZ\x90\0padpadpadpad");
    let mut bytes = builder.finish();
    put32(&mut bytes, directory_entry(DIRECTORY_RESOURCE), root);
    put32(&mut bytes, directory_entry(DIRECTORY_RESOURCE) + 4, 0x200);
    let triage = analyse(&Image::parse(&bytes).unwrap());
    assert_eq!(
        triage.resources,
        vec![ResourceType {
            kind: "RCDATA".into(),
            entries: 1,
            bytes: 16,
            starting_with_mz: 1
        }]
    );
    assert!(
        triage
            .indicators
            .iter()
            .any(|i| i.contains("RCDATA") && i.contains("MZ"))
    );
}

fn version_block(key: &str, value: &[u8], value_len: usize, kind: u16, children: &[u8]) -> Vec<u8> {
    fn pad(v: &mut Vec<u8>) {
        while !v.len().is_multiple_of(4) {
            v.push(0);
        }
    }
    let mut v = vec![0u8; 6];
    v.extend(key.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
    pad(&mut v);
    v.extend(value);
    pad(&mut v);
    v.extend(children);
    let len = v.len() as u16;
    v[0..2].copy_from_slice(&len.to_le_bytes());
    v[2..4].copy_from_slice(&(value_len as u16).to_le_bytes());
    v[4..6].copy_from_slice(&kind.to_le_bytes());
    pad(&mut v);
    v
}

fn string_entry(key: &str, value: &str) -> Vec<u8> {
    let units: Vec<u8> = value
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    version_block(key, &units, units.len() / 2, 1, &[])
}

#[test]
fn reads_version_strings() {
    let table = version_block(
        "040904b0",
        &[],
        0,
        1,
        &[
            string_entry("CompanyName", "Microsoft Corporation"),
            string_entry("Comments", ""),
            string_entry("FileVersion", "14.0.24215.1"),
        ]
        .concat(),
    );
    let file_info = version_block("StringFileInfo", &[], 0, 1, &table);
    let root = version_block("VS_VERSION_INFO", &[0u8; 52], 52, 0, &file_info);
    assert_eq!(
        version_strings(&root),
        vec![
            (
                "CompanyName".to_string(),
                "Microsoft Corporation".to_string()
            ),
            ("Comments".to_string(), String::new()),
            ("FileVersion".to_string(), "14.0.24215.1".to_string()),
        ]
    );
}

fn wide(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn hits_for(strings: &[&[u8]]) -> StringHits {
    let mut data = Vec::new();
    for s in strings {
        data.extend([0u8, 0xFF]);
        if data.len() % 2 == 1 {
            data.push(0);
        }
        data.extend(*s);
    }
    data.extend([0u8, 0xFF]);
    scan_strings(&data)
}

fn found(hits: &StringHits, kind: StringKind) -> Vec<&str> {
    hits.found
        .get(&kind)
        .map(|m| m.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

#[test]
fn matches_urls_hosts_and_addresses() {
    let hits = hits_for(&[
        b"GET https://evil.example.com/a.bin HTTP/1.1",
        &wide("http://wide.example.net/x"),
        b"connect to 192.168.1.5 now",
        b"version 10.0.19041.1 build",
        b"kernel32.dll and user32.dll",
        b"login.xboxlive.com",
    ]);
    assert_eq!(
        found(&hits, StringKind::Url),
        [
            "http://wide.example.net/x",
            "https://evil.example.com/a.bin"
        ]
    );
    assert_eq!(found(&hits, StringKind::Ip), ["192.168.1.5"]);
    assert_eq!(
        found(&hits, StringKind::Domain),
        ["evil.example.com", "login.xboxlive.com", "wide.example.net"]
    );
}

#[test]
fn matches_registry_paths_files_and_commands() {
    let hits = hits_for(&[
        b"HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
        b"C:\\Users\\bob\\AppData\\Roaming\\x.dll",
        b"d:\\build\\engine\\game.pdb",
        b"D:\\src\\engine\\main.cpp",
        b"%APPDATA%\\thing",
        b"powershell -enc AAAA",
        b"cmd.exe /c del x",
        b"schtasks /create",
        b"nothing to see here at all",
    ]);
    assert_eq!(found(&hits, StringKind::Registry).len(), 1);
    assert_eq!(
        found(&hits, StringKind::Path),
        [
            "%APPDATA%\\thing",
            "C:\\Users\\bob\\AppData\\Roaming\\x.dll"
        ]
    );
    assert_eq!(
        found(&hits, StringKind::PdbPath),
        ["d:\\build\\engine\\game.pdb"]
    );
    assert_eq!(
        found(&hits, StringKind::SourcePath),
        ["D:\\src\\engine\\main.cpp"]
    );
    assert_eq!(found(&hits, StringKind::CommandLine).len(), 3);
}

#[test]
fn keywords_need_word_boundaries() {
    let hits = hits_for(&[
        b"open wallet.dat file",
        b"xmrig --donate-level",
        b"stratum+tcp://pool.example.org:3333",
        b"the miner process",
        b"determiners and walleted",
        b"Login Data",
    ]);
    let keywords = found(&hits, StringKind::Keyword);
    assert_eq!(keywords.len(), 5);
    assert!(!keywords.iter().any(|k| k.contains("determiners")));
    assert_eq!(
        found(&hits, StringKind::Url),
        ["stratum+tcp://pool.example.org:3333"]
    );
}

#[test]
fn string_scan_counts_repeats_and_ignores_short_runs() {
    let hits = hits_for(&[
        b"https://a.example.com/x",
        b"https://a.example.com/x",
        b"http",
    ]);
    assert_eq!(hits.found[&StringKind::Url]["https://a.example.com/x"], 2);
    assert_eq!(hits.count(StringKind::Url), 2);
    assert_eq!(hits.scanned, 2);
}
