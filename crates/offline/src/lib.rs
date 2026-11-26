//! Everything the game could use to reach the network. All of it is blocked: there are no exceptions.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    Sockets,
    WebRequests,
    NameLookups,
    NetworkSettings,
    NetworkShares,
    RemoteCalls,
    XboxLive,
    RemoteFiles,
}

impl Reason {
    pub fn describe(self) -> &'static str {
        match self {
            Reason::Sockets => "direct connections",
            Reason::WebRequests => "web requests",
            Reason::NameLookups => "address lookups",
            Reason::NetworkSettings => "network status and settings",
            Reason::NetworkShares => "shared folders on other computers",
            Reason::RemoteCalls => "calls to other computers",
            Reason::XboxLive => "Xbox Live",
            Reason::RemoteFiles => "loading files from another computer",
        }
    }
}

const DLLS: &[(&str, Reason)] = &[
    ("ws2_32.dll", Reason::Sockets),
    ("wsock32.dll", Reason::Sockets),
    ("mswsock.dll", Reason::Sockets),
    ("winhttp.dll", Reason::WebRequests),
    ("wininet.dll", Reason::WebRequests),
    ("urlmon.dll", Reason::WebRequests),
    ("webio.dll", Reason::WebRequests),
    ("websocket.dll", Reason::WebRequests),
    ("httpapi.dll", Reason::WebRequests),
    ("libhttpclient.gdk.dll", Reason::WebRequests),
    ("xcurl.dll", Reason::WebRequests),
    ("dnsapi.dll", Reason::NameLookups),
    ("iphlpapi.dll", Reason::NetworkSettings),
    ("nsi.dll", Reason::NetworkSettings),
    ("wlanapi.dll", Reason::NetworkSettings),
    ("rasapi32.dll", Reason::NetworkSettings),
    ("fwpuclnt.dll", Reason::NetworkSettings),
    ("netapi32.dll", Reason::NetworkShares),
    ("mpr.dll", Reason::NetworkShares),
];

const DLL_PREFIXES: &[(&str, Reason)] = &[("ext-ms-win-networking-", Reason::NetworkSettings)];

const FUNCTIONS: &[(Option<&str>, &str, Reason)] = &[
    (None, "XNetworking", Reason::NetworkSettings),
    (None, "Xbl", Reason::XboxLive),
    (None, "XUserGetTokenAndSignature", Reason::XboxLive),
    (Some("rpcrt4.dll"), "Rpc", Reason::RemoteCalls),
    (Some("rpcrt4.dll"), "I_Rpc", Reason::RemoteCalls),
    (Some("rpcrt4.dll"), "Ndr", Reason::RemoteCalls),
];

const SCHEMES: &[&str] = &[
    "http", "https", "ws", "wss", "ftp", "ftps", "sftp", "smb", "tcp", "udp",
];

/// Why a DLL, or a function in it, is blocked. `dll` may carry a folder or omit `.dll`, as `LoadLibrary` allows.
pub fn import(dll: &str, function: Option<&str>) -> Option<Reason> {
    let name = dll_name(dll);
    let by_dll = DLLS
        .iter()
        .find(|(n, _)| *n == name)
        .or_else(|| DLL_PREFIXES.iter().find(|(p, _)| name.starts_with(p)))
        .map(|(_, reason)| *reason);
    by_dll.or_else(|| rule(Some(&name), function?))
}

/// Why a function is blocked whichever library provides it.
pub fn function(function: &str) -> Option<Reason> {
    rule(None, function)
}

fn rule(dll: Option<&str>, function: &str) -> Option<Reason> {
    FUNCTIONS
        .iter()
        .find(|(owner, prefix, _)| {
            owner.is_none_or(|o| Some(o) == dll) && function.starts_with(prefix)
        })
        .map(|(_, _, reason)| *reason)
}

/// Why opening `path` would reach another computer.
pub fn path(path: &str) -> Option<Reason> {
    let lower = path.trim_start().to_ascii_lowercase();
    if lower.starts_with("tcpip:") {
        return Some(Reason::RemoteFiles);
    }
    if let Some((scheme, rest)) = lower.split_once("://") {
        if SCHEMES.contains(&scheme) {
            return Some(Reason::WebRequests);
        }
        if scheme == "file" && !rest.starts_with('/') {
            return Some(Reason::NetworkShares);
        }
    }
    let unified = lower.replace('/', "\\");
    let unc = ["\\\\?\\unc\\", "\\\\.\\unc\\", "\\??\\unc\\"]
        .iter()
        .any(|p| unified.starts_with(p));
    let share = unified.starts_with("\\\\")
        && !unified.starts_with("\\\\?\\")
        && !unified.starts_with("\\\\.\\");
    (unc || share).then_some(Reason::NetworkShares)
}

fn dll_name(dll: &str) -> String {
    let base = dll.rsplit(['\\', '/']).next().unwrap_or(dll).trim();
    let mut name = base.to_ascii_lowercase();
    if !name.contains('.') {
        name.push_str(".dll");
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_network_dlls_however_named() {
        assert_eq!(import("WS2_32.dll", Some("connect")), Some(Reason::Sockets));
        assert_eq!(import("ws2_32", None), Some(Reason::Sockets));
        assert_eq!(
            import("C:\\Windows\\System32\\WinHttp.DLL", None),
            Some(Reason::WebRequests)
        );
        assert_eq!(
            import("libHttpClient.GDK.dll", Some("HCInitialize")),
            Some(Reason::WebRequests)
        );
        assert_eq!(import("XCurl.dll", None), Some(Reason::WebRequests));
        assert_eq!(
            import("ext-ms-win-networking-wlanapi-l1-1-0.dll", None),
            Some(Reason::NetworkSettings)
        );
    }

    #[test]
    fn blocks_network_functions_in_allowed_dlls() {
        assert_eq!(
            import(
                "xgameruntime.dll",
                Some("XNetworkingQueryPreferredLocalUdpMultiplayerPortAsync")
            ),
            Some(Reason::NetworkSettings)
        );
        assert_eq!(
            import("xgameruntime.dll", Some("XUserGetTokenAndSignatureAsync")),
            Some(Reason::XboxLive)
        );
        assert_eq!(
            import("rpcrt4.dll", Some("RpcStringBindingComposeW")),
            Some(Reason::RemoteCalls)
        );
        assert_eq!(import("rpcrt4.dll", Some("UuidCreate")), None);
        assert_eq!(import("xgameruntime.dll", Some("XUserAddAsync")), None);
        assert_eq!(function("XblInitialize"), Some(Reason::XboxLive));
        assert_eq!(function("RpcBindingFree"), None);
    }

    #[test]
    fn leaves_local_libraries_alone() {
        for dll in [
            "kernel32.dll",
            "xmem.dll",
            "d3d12_xs.dll",
            "bcrypt.dll",
            "api-ms-win-core-file-l1-1-0.dll",
        ] {
            assert_eq!(import(dll, Some("Anything")), None, "{dll}");
        }
    }

    #[test]
    fn blocks_paths_on_other_computers() {
        assert_eq!(path("tcpip:12345:LISTEN"), Some(Reason::RemoteFiles));
        assert_eq!(path("TCPIP:4000:10.0.0.2"), Some(Reason::RemoteFiles));
        assert_eq!(path("https://example.com/a"), Some(Reason::WebRequests));
        assert_eq!(path("\\\\server\\share\\file"), Some(Reason::NetworkShares));
        assert_eq!(path("//server/share/file"), Some(Reason::NetworkShares));
        assert_eq!(
            path("\\\\?\\UNC\\server\\share"),
            Some(Reason::NetworkShares)
        );
        assert_eq!(path("file://server/share"), Some(Reason::NetworkShares));
    }

    #[test]
    fn allows_local_paths() {
        for local in [
            "C:\\games\\x.rpf",
            "\\\\?\\C:\\games\\x.rpf",
            "\\\\.\\pipe\\local",
            "file:///C:/x",
            "americas://Story000/startContent?MODE=Current",
            "platform:/data/x.meta",
        ] {
            assert_eq!(path(local), None, "{local}");
        }
    }
}
