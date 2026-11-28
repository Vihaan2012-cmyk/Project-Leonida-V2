//! System calls that reach files or other programs in ways the file shims do not model. With the sandbox on, an
//! import of one is left unbound (a call to it is trapped and traced) and `GetProcAddress` does not find it.

const NAMES: &[&str] = &[
    "CreateHardLinkW",
    "CreateHardLinkA",
    "CreateHardLinkTransactedW",
    "CreateSymbolicLinkW",
    "CreateSymbolicLinkA",
    "CreateSymbolicLinkTransactedW",
    "ReplaceFileW",
    "ReplaceFileA",
    "CopyFile2",
    "OpenFile",
    "CreateFileTransactedW",
    "CreateFileTransactedA",
    "DeleteFileTransactedW",
    "DeleteFileTransactedA",
    "CreateDirectoryTransactedW",
    "CreateDirectoryTransactedA",
    "RemoveDirectoryTransactedW",
    "RemoveDirectoryTransactedA",
    "MoveFileTransactedW",
    "MoveFileTransactedA",
    "SetFileAttributesTransactedW",
    "SetFileAttributesTransactedA",
    "GetFileAttributesTransactedW",
    "GetFileAttributesTransactedA",
    "FindFirstFileTransactedW",
    "FindFirstFileTransactedA",
    "FindFirstFileNameW",
    "FindFirstStreamW",
    "GetTempFileNameW",
    "GetTempFileNameA",
    "SetCurrentDirectoryW",
    "SetCurrentDirectoryA",
    "SetVolumeMountPointW",
    "DeleteVolumeMountPointW",
    "DefineDosDeviceW",
    "CreateNamedPipeW",
    "CreateNamedPipeA",
    "CallNamedPipeW",
    "CallNamedPipeA",
    "CreateMailslotW",
    "CreateMailslotA",
    "DeviceIoControl",
    "CreateProcessW",
    "CreateProcessA",
    "CreateProcessAsUserW",
    "CreateProcessWithLogonW",
    "WinExec",
    "ShellExecuteW",
    "ShellExecuteA",
    "ShellExecuteExW",
    "ShellExecuteExA",
    "NtCreateFile",
    "NtOpenFile",
    "NtDeleteFile",
    "NtQueryAttributesFile",
    "NtQueryFullAttributesFile",
    "NtCreateNamedPipeFile",
    "NtSetInformationFile",
    "NtFsControlFile",
    "ZwCreateFile",
    "ZwOpenFile",
    "ZwDeleteFile",
    "ZwSetInformationFile",
    "ZwFsControlFile",
    "LdrLoadDll",
    "_chdir",
    "_wchdir",
    "tmpfile",
    "tmpfile_s",
    "tmpnam",
    "tmpnam_s",
    "_tempnam",
    "_wtempnam",
    "_wtmpnam",
    "_wtmpnam_s",
    "_mktemp",
    "_wmktemp",
    "_mktemp_s",
    "_wmktemp_s",
    "system",
    "_wsystem",
    "_popen",
    "_wpopen",
];

const PREFIXES: &[&str] = &[
    "_findfirst",
    "_wfindfirst",
    "_findnext",
    "_wfindnext",
    "_findclose",
    "_spawn",
    "_wspawn",
    "_exec",
    "_wexec",
];

/// Whether a function of this name is one the file sandbox keeps from the game.
pub fn blocked_api(name: &str) -> bool {
    NAMES.contains(&name) || PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_that_slip_past_the_file_shims_are_blocked() {
        for name in [
            "CreateHardLinkW",
            "NtCreateFile",
            "_wfindfirst64i32",
            "_findnext32",
            "_wspawnl",
            "CreateProcessW",
            "DeviceIoControl",
            "tmpfile",
        ] {
            assert!(blocked_api(name), "{name}");
        }
    }

    #[test]
    fn the_calls_the_shims_cover_are_not() {
        for name in [
            "CreateFileW",
            "CreateFileA",
            "CopyFileW",
            "_wfopen",
            "fopen_s",
            "_wstat64",
            "_wopen",
            "RegOpenKeyW",
            "LoadLibraryW",
            "ReadFile",
        ] {
            assert!(!blocked_api(name), "{name}");
        }
    }
}
