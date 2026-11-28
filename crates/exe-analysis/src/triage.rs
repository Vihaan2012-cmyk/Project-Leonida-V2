//! Static malware triage of one PE image: section entropy, imports that malware tends to use, exports, TLS
//! callbacks, overlay, resources and telling strings. It reads the file as data and keeps only names, counts and
//! short matched strings, never code bytes.

use std::collections::BTreeMap;
use std::fmt;

use pe_image::authenticode::certificate_table;
use pe_image::{DIRECTORY_RESOURCE, DIRECTORY_TLS, Image};

/// Executable sections above this many bits per byte look packed or encrypted.
pub const HIGH_ENTROPY_EXECUTABLE: f64 = 7.2;
/// Any other section or overlay above this looks compressed or encrypted.
pub const HIGH_ENTROPY_DATA: f64 = 7.6;
const STRING_MIN: usize = 6;
const DISTINCT_PER_KIND: usize = 150;
const SHOWN_TEXT: usize = 140;

/// Shannon entropy in bits per byte, 0 for empty input.
pub fn entropy(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut counts = [0u64; 256];
    for byte in data {
        counts[usize::from(*byte)] += 1;
    }
    let total = data.len() as f64;
    counts
        .iter()
        .filter(|c| **c != 0)
        .map(|c| {
            let p = *c as f64 / total;
            -p * p.log2()
        })
        .sum()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ApiCategory {
    ProcessInjection,
    Hook,
    Persistence,
    Credential,
    Network,
    ProcessCreation,
    AntiAnalysis,
    DynamicResolution,
    MemoryProtection,
    InputCapture,
}

impl ApiCategory {
    pub fn title(self) -> &'static str {
        match self {
            Self::ProcessInjection => "process injection",
            Self::Hook => "hooks",
            Self::Persistence => "persistence (registry, services)",
            Self::Credential => "credentials",
            Self::Network => "network",
            Self::ProcessCreation => "process creation",
            Self::AntiAnalysis => "anti-analysis (IsDebuggerPresent is normal)",
            Self::DynamicResolution => "dynamic resolution",
            Self::MemoryProtection => "memory permissions",
            Self::InputCapture => "input capture",
        }
    }

    /// Whether finding any of these is worth a line in the indicator list; the others are only counted.
    fn notable(self) -> bool {
        !matches!(
            self,
            Self::AntiAnalysis | Self::DynamicResolution | Self::MemoryProtection
        )
    }
}

const SUFFIXES: [&str; 7] = ["", "A", "W", "Ex", "ExA", "ExW", "2"];

fn is_variant(name: &str, base: &str) -> bool {
    name.strip_prefix(base)
        .is_some_and(|rest| SUFFIXES.contains(&rest))
}

/// What kind of risk an imported function name carries, if any. Matching is on the name only, not the DLL.
pub fn classify_api(name: &str) -> Option<ApiCategory> {
    use ApiCategory::*;
    const BASES: &[(ApiCategory, &[&str])] = &[
        (
            ProcessInjection,
            &[
                "OpenProcess",
                "WriteProcessMemory",
                "ReadProcessMemory",
                "CreateRemoteThread",
                "NtUnmapViewOfSection",
                "ZwUnmapViewOfSection",
                "QueueUserAPC",
                "NtQueueApcThread",
                "SetThreadContext",
                "NtSetContextThread",
                "NtWriteVirtualMemory",
                "NtCreateThreadEx",
                "RtlCreateUserThread",
                "VirtualAllocEx",
                "VirtualProtectEx",
                "NtMapViewOfSection",
            ],
        ),
        (Hook, &["SetWindowsHook", "UnhookWindowsHook"]),
        (
            Persistence,
            &[
                "RegSetValue",
                "RegCreateKey",
                "RegSetKeyValue",
                "CreateService",
                "OpenSCManager",
                "ChangeServiceConfig",
                "StartService",
                "SchRpcRegisterTask",
            ],
        ),
        (
            Credential,
            &[
                "CryptUnprotectData",
                "CryptProtectData",
                "CredEnumerate",
                "CredRead",
                "CredWrite",
                "CredUIPromptForCredentials",
                "LsaRetrievePrivateData",
                "LogonUser",
                "NetUserAdd",
                "SamConnect",
            ],
        ),
        (
            Network,
            &[
                "URLDownloadToFile",
                "URLDownloadToCacheFile",
                "HttpSendRequest",
                "HttpOpenRequest",
                "FtpPutFile",
                "FtpGetFile",
                "DnsQuery",
                "getaddrinfo",
                "GetAddrInfo",
                "gethostbyname",
                "socket",
                "connect",
                "bind",
                "listen",
                "accept",
                "send",
                "recv",
                "sendto",
                "recvfrom",
                "closesocket",
                "WSAStartup",
                "WSASocket",
                "WSAConnect",
                "WSASend",
                "WSARecv",
                "InternetOpen",
                "InternetConnect",
                "InternetReadFile",
                "InternetOpenUrl",
            ],
        ),
        (
            ProcessCreation,
            &[
                "CreateProcess",
                "CreateProcessAsUser",
                "CreateProcessWithLogon",
                "CreateProcessWithToken",
                "ShellExecute",
                "ShellExecuteEx",
                "WinExec",
                "system",
                "_wsystem",
                "_spawnl",
                "_spawnv",
                "_execl",
                "_execv",
                "_wspawnl",
                "_wexecl",
            ],
        ),
        (
            AntiAnalysis,
            &[
                "IsDebuggerPresent",
                "CheckRemoteDebuggerPresent",
                "NtQueryInformationProcess",
                "ZwQueryInformationProcess",
                "OutputDebugString",
                "NtSetInformationThread",
                "DebugActiveProcess",
                "DebugBreak",
            ],
        ),
        (
            DynamicResolution,
            &[
                "GetProcAddress",
                "LoadLibrary",
                "LoadLibraryEx",
                "GetModuleHandle",
                "LdrLoadDll",
                "LdrGetProcedureAddress",
            ],
        ),
        (
            MemoryProtection,
            &[
                "VirtualAlloc",
                "VirtualProtect",
                "NtProtectVirtualMemory",
                "NtAllocateVirtualMemory",
            ],
        ),
        (
            InputCapture,
            &[
                "GetAsyncKeyState",
                "GetKeyboardState",
                "RegisterRawInputDevices",
                "GetClipboardData",
                "SetClipboardViewer",
            ],
        ),
    ];
    const FAMILIES: &[&str] = &["WinHttp", "WSA", "Internet", "curl_"];
    for (category, bases) in BASES {
        if bases.iter().any(|base| is_variant(name, base)) {
            return Some(*category);
        }
    }
    FAMILIES
        .iter()
        .any(|prefix| name.starts_with(prefix))
        .then_some(Network)
}

#[derive(Debug, Clone, PartialEq)]
pub struct SectionReport {
    pub name: String,
    pub virtual_address: u32,
    pub virtual_size: u32,
    pub raw_offset: u32,
    pub raw_size: u32,
    pub executable: bool,
    pub writable: bool,
    pub entropy: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DllImports {
    pub name: String,
    pub delayed: bool,
    pub names: Vec<String>,
    pub ordinals: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiHit {
    pub category: ApiCategory,
    pub dll: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Overlay {
    pub offset: usize,
    pub len: usize,
    pub entropy: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceType {
    pub kind: String,
    pub entries: usize,
    pub bytes: u64,
    pub starting_with_mz: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StringKind {
    Url,
    Ip,
    Domain,
    Registry,
    Path,
    PdbPath,
    SourcePath,
    CommandLine,
    Keyword,
}

impl StringKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Url => "URLs",
            Self::Ip => "IP addresses",
            Self::Domain => "domain names",
            Self::Registry => "registry paths",
            Self::Path => "file paths",
            Self::PdbPath => "debug-symbol (.pdb) paths",
            Self::SourcePath => "source-file paths",
            Self::CommandLine => "command lines and system tools",
            Self::Keyword => "wallet, miner and stealer keywords",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StringHits {
    /// Distinct matched strings with how often each occurs.
    pub found: BTreeMap<StringKind, BTreeMap<String, usize>>,
    /// Matches beyond the distinct-string cap, per kind.
    pub dropped: BTreeMap<StringKind, usize>,
    /// How many strings of at least the minimum length the file holds.
    pub scanned: usize,
}

impl StringHits {
    fn add(&mut self, kind: StringKind, text: &str) {
        let shown: String = text.chars().take(SHOWN_TEXT).collect();
        let map = self.found.entry(kind).or_default();
        if let Some(count) = map.get_mut(&shown) {
            *count += 1;
        } else if map.len() < DISTINCT_PER_KIND {
            map.insert(shown, 1);
        } else {
            *self.dropped.entry(kind).or_default() += 1;
        }
    }

    pub fn count(&self, kind: StringKind) -> usize {
        self.found
            .get(&kind)
            .map_or(0, |m| m.values().sum::<usize>())
            + self.dropped.get(&kind).copied().unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Triage {
    pub file_len: usize,
    pub machine: u16,
    pub timestamp: u32,
    pub characteristics: u16,
    pub subsystem: u16,
    pub dll_characteristics: u16,
    pub image_base: u64,
    pub sections: Vec<SectionReport>,
    pub entry: u32,
    pub entry_section: Option<String>,
    pub imports: Vec<DllImports>,
    pub hits: Vec<ApiHit>,
    pub exports: Vec<String>,
    pub export_ordinal_only: usize,
    pub export_forwarders: usize,
    pub tls_callbacks: Vec<u32>,
    pub overlay: Option<Overlay>,
    pub certificate_bytes: usize,
    pub resources: Vec<ResourceType>,
    pub version_info: Vec<(String, String)>,
    pub embedded_pe_markers: usize,
    pub strings: StringHits,
    pub indicators: Vec<String>,
}

impl Triage {
    pub fn hits_of(&self, category: ApiCategory) -> Vec<&ApiHit> {
        self.hits
            .iter()
            .filter(|h| h.category == category)
            .collect()
    }

    pub fn api_total(&self) -> usize {
        self.imports
            .iter()
            .map(|d| d.names.len() + d.ordinals)
            .sum()
    }
}

impl fmt::Display for ApiCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.title())
    }
}

fn u16_le(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_le(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_le(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

fn section_bytes(bytes: &[u8], offset: u32, size: u32) -> &[u8] {
    let start = (offset as usize).min(bytes.len());
    let end = start.saturating_add(size as usize).min(bytes.len());
    &bytes[start..end]
}

fn resource_type_name(id: u32) -> String {
    match id {
        1 => "CURSOR",
        2 => "BITMAP",
        3 => "ICON",
        4 => "MENU",
        5 => "DIALOG",
        6 => "STRING",
        9 => "ACCELERATOR",
        10 => "RCDATA",
        11 => "MESSAGETABLE",
        12 => "GROUP_CURSOR",
        14 => "GROUP_ICON",
        16 => "VERSION",
        17 => "DLGINCLUDE",
        19 => "PLUGPLAY",
        20 => "VXD",
        21 => "ANICURSOR",
        22 => "ANIICON",
        23 => "HTML",
        24 => "MANIFEST",
        other => return format!("#{other}"),
    }
    .to_string()
}

struct Resources {
    types: Vec<ResourceType>,
    version: Vec<(String, String)>,
}

fn read_resources(image: &Image<'_>) -> Resources {
    let mut out = Resources {
        types: Vec::new(),
        version: Vec::new(),
    };
    let Some((root, _)) = image.directory(DIRECTORY_RESOURCE) else {
        return out;
    };
    let entries = |dir: u32| -> Vec<(u32, u32)> {
        let Some(head) = image.read(root + dir, 16) else {
            return Vec::new();
        };
        let count =
            usize::from(u16_le(head, 12).unwrap_or(0)) + usize::from(u16_le(head, 14).unwrap_or(0));
        (0..count.min(4096))
            .filter_map(|index| {
                let at = image.read(root + dir + 16 + 8 * index as u32, 8)?;
                Some((u32_le(at, 0)?, u32_le(at, 4)?))
            })
            .collect()
    };
    let leaves = |dir: u32| -> Vec<(u32, u32)> {
        let mut found = Vec::new();
        let mut stack = vec![(dir, 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            for (_, target) in entries(dir) {
                if target & 0x8000_0000 != 0 {
                    if depth < 4 {
                        stack.push((target & 0x7FFF_FFFF, depth + 1));
                    }
                } else if let Some(data) = image.read(root + target, 8)
                    && let (Some(rva), Some(size)) = (u32_le(data, 0), u32_le(data, 4))
                {
                    found.push((rva, size));
                }
            }
        }
        found
    };
    for (name, target) in entries(0) {
        if target & 0x8000_0000 == 0 {
            continue;
        }
        let kind = if name & 0x8000_0000 != 0 {
            let at = root + (name & 0x7FFF_FFFF);
            let length = image.read(at, 2).and_then(|b| u16_le(b, 0)).unwrap_or(0);
            let text = image
                .read(at + 2, usize::from(length) * 2)
                .map(|raw| {
                    String::from_utf16_lossy(
                        &raw.chunks_exact(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]))
                            .collect::<Vec<_>>(),
                    )
                })
                .unwrap_or_default();
            format!("\"{text}\"")
        } else {
            resource_type_name(name)
        };
        let items = leaves(target & 0x7FFF_FFFF);
        let mut bytes = 0u64;
        let mut mz = 0;
        for (rva, size) in &items {
            bytes += u64::from(*size);
            if image.read(*rva, 2) == Some(b"MZ") {
                mz += 1;
            }
            if name == 16
                && out.version.is_empty()
                && let Some(data) = image.read(*rva, (*size as usize).min(1 << 16))
            {
                out.version = version_strings(data);
            }
        }
        out.types.push(ResourceType {
            kind,
            entries: items.len(),
            bytes,
            starting_with_mz: mz,
        });
    }
    out
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}

/// The (name, value) strings of a VS_VERSION_INFO resource.
pub(crate) fn version_strings(data: &[u8]) -> Vec<(String, String)> {
    struct Block {
        len: usize,
        value_len: usize,
        key: String,
        value_at: usize,
    }
    fn block(data: &[u8], at: usize) -> Option<Block> {
        let len = usize::from(u16_le(data, at)?);
        let value_len = usize::from(u16_le(data, at + 2)?);
        let mut units = Vec::new();
        let mut pos = at + 6;
        loop {
            let unit = u16_le(data, pos)?;
            pos += 2;
            if unit == 0 {
                break;
            }
            units.push(unit);
            if units.len() > 128 {
                return None;
            }
        }
        (len >= 6).then(|| Block {
            len,
            value_len,
            key: String::from_utf16_lossy(&units),
            value_at: align4(pos),
        })
    }
    let mut out = Vec::new();
    let Some(root) = block(data, 0) else {
        return out;
    };
    let root_end = root.len.min(data.len());
    let mut at = align4(root.value_at + root.value_len);
    while at + 6 < root_end {
        let Some(info) = block(data, at) else { break };
        if info.key == "StringFileInfo" {
            let info_end = (at + info.len).min(root_end);
            let mut table_at = info.value_at;
            while table_at + 6 < info_end {
                let Some(table) = block(data, table_at) else {
                    break;
                };
                let table_end = (table_at + table.len).min(info_end);
                let mut item_at = table.value_at;
                while item_at + 6 < table_end {
                    let Some(item) = block(data, item_at) else {
                        break;
                    };
                    let units: Vec<u16> = (0..item.value_len)
                        .map_while(|i| u16_le(data, item.value_at + 2 * i))
                        .take_while(|u| *u != 0)
                        .collect();
                    out.push((item.key, String::from_utf16_lossy(&units)));
                    item_at = align4(item_at + item.len.max(6));
                }
                table_at = align4(table_at + table.len.max(6));
            }
        }
        at = align4(at + info.len.max(6));
    }
    out
}

fn tls_callbacks(image: &Image<'_>) -> Vec<u32> {
    let Some((rva, size)) = image.directory(DIRECTORY_TLS) else {
        return Vec::new();
    };
    let Some(directory) = image.read(rva, (size as usize).clamp(40, 64)) else {
        return Vec::new();
    };
    let Some(array) = u64_le(directory, 24).filter(|a| *a >= image.image_base) else {
        return Vec::new();
    };
    let array = (array - image.image_base) as u32;
    (0..64u32)
        .map_while(|index| {
            let va = image
                .read(array + 8 * index, 8)
                .and_then(|b| u64_le(b, 0))?;
            (va != 0).then(|| va.wrapping_sub(image.image_base) as u32)
        })
        .collect()
}

fn count_embedded_pe(bytes: &[u8]) -> usize {
    const MARKER: &[u8] = b"This program cannot be run in DOS mode";
    memchr::memmem::find_iter(bytes, MARKER)
        .filter(|at| *at > 0x400)
        .count()
}

/// Calls `visit` for each run of at least `min` printable ASCII characters, in the file as ASCII and as UTF-16LE.
pub fn for_each_string(bytes: &[u8], min: usize, mut visit: impl FnMut(&str)) {
    let mut start = None;
    for (index, byte) in bytes.iter().enumerate() {
        if (0x20..0x7F).contains(byte) {
            start.get_or_insert(index);
        } else if let Some(begin) = start.take()
            && index - begin >= min
            && let Ok(text) = std::str::from_utf8(&bytes[begin..index])
        {
            visit(text);
        }
    }
    if let Some(begin) = start
        && bytes.len() - begin >= min
        && let Ok(text) = std::str::from_utf8(&bytes[begin..])
    {
        visit(text);
    }
    let mut text = String::new();
    for pair in bytes.chunks_exact(2) {
        if (0x20..0x7F).contains(&pair[0]) && pair[1] == 0 {
            text.push(char::from(pair[0]));
        } else {
            if text.len() >= min {
                visit(&text);
            }
            text.clear();
        }
    }
    if text.len() >= min {
        visit(&text);
    }
}

const TLDS: &[&str] = &[
    "com", "net", "org", "edu", "gov", "mil", "info", "biz", "xyz", "ru", "cn", "tk", "onion",
    "io", "su", "ml", "ga", "cf", "gq", "top", "pw", "ws",
];

const COMMANDS: &[&str] = &[
    "cmd.exe",
    "cmd /c",
    "cmd /k",
    "powershell",
    "pwsh",
    "rundll32",
    "regsvr32",
    "mshta",
    "wscript",
    "cscript",
    "bitsadmin",
    "certutil",
    "schtasks",
    "wmic",
    "msiexec",
    "net user",
    "reg add",
    "reg.exe",
    "taskkill",
    "vssadmin",
    "bcdedit",
    "curl.exe",
    "wget ",
];

const REGISTRY: &[&str] = &[
    "hkey_",
    "hklm\\",
    "hkcu\\",
    "\\registry\\machine",
    "\\registry\\user",
    "currentversion\\run",
    "software\\microsoft\\windows",
    "system\\currentcontrolset",
    "software\\classes",
];

const ENV_PATHS: &[&str] = &[
    "%appdata%",
    "%temp%",
    "%tmp%",
    "%localappdata%",
    "%programdata%",
    "%userprofile%",
    "%systemroot%",
    "%windir%",
    "\\users\\",
    "\\windows\\system32",
    "\\appdata\\",
];

/// Keywords matched as whole words (letters on both sides disqualify the match).
const WORD_KEYWORDS: &[&str] = &[
    "wallet",
    "miner",
    "mining",
    "stealer",
    "keylogger",
    "ransom",
    "payload",
    "shellcode",
    "backdoor",
    "botnet",
    "mimikatz",
    "bitcoin",
    "monero",
    "xmrig",
    "ethereum",
    "metamask",
];

/// Keywords matched anywhere in the string.
const SUBSTRING_KEYWORDS: &[&str] = &[
    "stratum+",
    "cryptonight",
    "wallet.dat",
    "login data",
    "cookies.sqlite",
    "\\user data\\",
    "discord token",
    "lsass",
    "your files have been",
    "keylog",
];

fn contains_word(haystack: &str, word: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut from = 0;
    while let Some(found) = haystack[from..].find(word) {
        let start = from + found;
        let end = start + word.len();
        let before = start == 0 || !bytes[start - 1].is_ascii_alphabetic();
        let after = end >= bytes.len() || !bytes[end].is_ascii_alphabetic();
        if before && after {
            return true;
        }
        from = start + 1;
    }
    false
}

fn is_ipv4(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

fn is_domain(token: &str) -> bool {
    let labels: Vec<&str> = token.split('.').collect();
    labels.len() >= 2
        && TLDS.contains(&labels[labels.len() - 1])
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        && labels[labels.len() - 2].len() >= 2
}

fn scan_string(text: &str, hits: &mut StringHits) {
    let lower = text.to_ascii_lowercase();
    for scheme in [
        "https://",
        "http://",
        "ftp://",
        "wss://",
        "ws://",
        "stratum+tcp://",
    ] {
        if let Some(at) = lower.find(scheme) {
            let rest = &text[at..];
            let end = rest
                .find(|c: char| c.is_whitespace() || "\"'<>)".contains(c))
                .unwrap_or(rest.len());
            if end > scheme.len() + 1 {
                hits.add(StringKind::Url, &rest[..end]);
            }
            break;
        }
    }
    for token in lower.split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-')) {
        let token = token.trim_matches('.');
        if token.len() < 4 {
            continue;
        }
        if is_ipv4(token) {
            hits.add(StringKind::Ip, token);
        } else if is_domain(token) {
            hits.add(StringKind::Domain, token);
        }
    }
    if REGISTRY.iter().any(|p| lower.contains(p)) {
        hits.add(StringKind::Registry, text);
    }
    let bytes = text.as_bytes();
    let has_drive = bytes.windows(3).enumerate().any(|(i, w)| {
        w[0].is_ascii_alphabetic()
            && w[1] == b':'
            && (w[2] == b'\\' || w[2] == b'/')
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
    });
    let unc =
        lower.starts_with("\\\\") && lower.len() > 4 && lower.as_bytes()[2].is_ascii_alphanumeric();
    if has_drive || unc || ENV_PATHS.iter().any(|p| lower.contains(p)) {
        let kind = if lower.ends_with(".pdb") {
            StringKind::PdbPath
        } else if [".cpp", ".c", ".h", ".hpp", ".inl", ".cc", ".cxx", ".asm"]
            .iter()
            .any(|ext| lower.ends_with(ext))
        {
            StringKind::SourcePath
        } else {
            StringKind::Path
        };
        hits.add(kind, text);
    }
    if COMMANDS.iter().any(|c| lower.contains(c)) {
        hits.add(StringKind::CommandLine, text);
    }
    if WORD_KEYWORDS.iter().any(|k| contains_word(&lower, k))
        || SUBSTRING_KEYWORDS.iter().any(|k| lower.contains(k))
    {
        hits.add(StringKind::Keyword, text);
    }
}

pub fn scan_strings(bytes: &[u8]) -> StringHits {
    let mut hits = StringHits::default();
    for_each_string(bytes, STRING_MIN, |text| {
        hits.scanned += 1;
        scan_string(text, &mut hits);
    });
    hits
}

/// Everything the triage reports about `image`, which must have been parsed from `bytes`.
pub fn analyse(image: &Image<'_>) -> Triage {
    let bytes = image.bytes();
    let sections: Vec<SectionReport> = image
        .sections
        .iter()
        .map(|s| SectionReport {
            name: s.name.clone(),
            virtual_address: s.virtual_address,
            virtual_size: s.virtual_size,
            raw_offset: s.raw_offset,
            raw_size: s.raw_size,
            executable: s.executable(),
            writable: s.writable(),
            entropy: entropy(section_bytes(bytes, s.raw_offset, s.raw_size)),
        })
        .collect();
    let imports: Vec<DllImports> = image
        .imports()
        .into_iter()
        .map(|dll| DllImports {
            ordinals: dll.entries.iter().filter(|e| e.name.is_none()).count(),
            names: dll.entries.into_iter().filter_map(|e| e.name).collect(),
            name: dll.name,
            delayed: dll.delayed,
        })
        .collect();
    let hits: Vec<ApiHit> = imports
        .iter()
        .flat_map(|dll| {
            dll.names.iter().filter_map(|name| {
                classify_api(name).map(|category| ApiHit {
                    category,
                    dll: dll.name.clone(),
                    name: name.clone(),
                })
            })
        })
        .collect();
    let exports_all = image.exports();
    let exports: Vec<String> = exports_all.iter().filter_map(|e| e.name.clone()).collect();
    let export_forwarders = exports_all.iter().filter(|e| e.forwarder.is_some()).count();
    let last_end = image
        .sections
        .iter()
        .map(|s| s.raw_offset as usize + s.raw_size as usize)
        .max()
        .unwrap_or(0)
        .min(bytes.len());
    let certificate = certificate_table(image);
    let overlay_end = certificate.map_or(bytes.len(), |(offset, _)| offset.max(last_end));
    let overlay = (overlay_end > last_end).then(|| Overlay {
        offset: last_end,
        len: overlay_end - last_end,
        entropy: entropy(&bytes[last_end..overlay_end]),
    });
    let resources = read_resources(image);
    let mut triage = Triage {
        file_len: bytes.len(),
        machine: image.machine,
        timestamp: image.timestamp,
        characteristics: image.characteristics,
        subsystem: image.subsystem,
        dll_characteristics: image.dll_characteristics,
        image_base: image.image_base,
        entry: image.entry,
        entry_section: image.section_of(image.entry).map(|s| s.name.clone()),
        sections,
        imports,
        hits,
        export_ordinal_only: exports_all.len() - exports.len(),
        exports,
        export_forwarders,
        tls_callbacks: tls_callbacks(image),
        overlay,
        certificate_bytes: certificate.map_or(0, |(_, size)| size),
        resources: resources.types,
        version_info: resources.version,
        embedded_pe_markers: count_embedded_pe(bytes),
        strings: scan_strings(bytes),
        indicators: Vec::new(),
    };
    triage.indicators = indicators(&triage);
    triage
}

fn indicators(t: &Triage) -> Vec<String> {
    let mut out = Vec::new();
    for s in &t.sections {
        if s.executable && s.writable {
            out.push(format!("section {} is writable and executable", s.name));
        }
        if s.executable && s.entropy >= HIGH_ENTROPY_EXECUTABLE {
            out.push(format!(
                "executable section {} has entropy {:.2} (packed or encrypted code looks like this)",
                s.name, s.entropy
            ));
        } else if !s.executable && s.entropy >= HIGH_ENTROPY_DATA && s.raw_size >= 4096 {
            out.push(format!(
                "data section {} has entropy {:.2} (compressed or encrypted data)",
                s.name, s.entropy
            ));
        }
    }
    if t.entry != 0 {
        match t.sections.iter().find(|s| {
            t.entry >= s.virtual_address
                && t.entry - s.virtual_address < s.virtual_size.max(s.raw_size)
        }) {
            None => out.push("entry point lies outside every section".to_string()),
            Some(s) if !s.executable => {
                out.push(format!(
                    "entry point is in non-executable section {}",
                    s.name
                ));
            }
            Some(s) if s.writable => {
                out.push(format!("entry point is in writable section {}", s.name));
            }
            Some(_) => {}
        }
    }
    if !t.tls_callbacks.is_empty() {
        out.push(format!(
            "{} TLS callback(s) run before the entry point",
            t.tls_callbacks.len()
        ));
    }
    if let Some(o) = &t.overlay {
        out.push(format!(
            "{} bytes of overlay after the last section (entropy {:.2})",
            o.len, o.entropy
        ));
    }
    if t.embedded_pe_markers > 0 {
        out.push(format!(
            "{} embedded 'This program cannot be run in DOS mode' marker(s) beyond the header",
            t.embedded_pe_markers
        ));
    }
    for r in t.resources.iter().filter(|r| r.starting_with_mz > 0) {
        out.push(format!(
            "{} resource(s) of type {} start with MZ",
            r.starting_with_mz, r.kind
        ));
    }
    let categories: std::collections::BTreeSet<_> = t.hits.iter().map(|h| h.category).collect();
    for category in categories.into_iter().filter(|c| c.notable()) {
        let names: Vec<&str> = t
            .hits_of(category)
            .iter()
            .map(|h| h.name.as_str())
            .collect();
        out.push(format!("imports for {category}: {}", names.join(", ")));
    }
    for kind in [
        StringKind::Url,
        StringKind::Ip,
        StringKind::Domain,
        StringKind::Registry,
        StringKind::Path,
        StringKind::CommandLine,
        StringKind::Keyword,
    ] {
        let count = t.strings.count(kind);
        if count > 0 {
            out.push(format!("{count} string match(es) for {}", kind.title()));
        }
    }
    out
}
