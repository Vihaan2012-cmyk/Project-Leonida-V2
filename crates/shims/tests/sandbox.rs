//! What the sandbox does to library loading, module names and the system functions a program may ask for by name,
//! called the way the program calls them.
#![cfg(windows)]
#![allow(unsafe_code, unused_unsafe)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pe_loader::ImportRef;
use shims::{Binder, Binding};

type LoadLibraryW = unsafe extern "win64" fn(*const u16) -> usize;
type LoadLibraryExW = unsafe extern "win64" fn(*const u16, usize, u32) -> usize;
type GetModuleHandleW = unsafe extern "win64" fn(*const u16) -> usize;
type GetProcAddress = unsafe extern "win64" fn(usize, *const u8) -> usize;
type GetModuleFileNameW = unsafe extern "win64" fn(usize, *mut u16, u32) -> u32;
type GetModuleFileNameA = unsafe extern "win64" fn(usize, *mut u8, u32) -> u32;

fn shim_address(dll: &str, name: &str) -> usize {
    shims::builtin()
        .into_iter()
        .find(|s| s.dll == dll && s.name == name)
        .unwrap_or_else(|| panic!("{dll}!{name} registered"))
        .address
}

fn fn_at<T: Copy>(name: &str) -> T {
    let address = shim_address("kernel32.dll", name);
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
    // SAFETY: `T` is the function pointer type of the shim's documented shape.
    unsafe { std::mem::transmute_copy(&address) }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn cstr(text: &str) -> Vec<u8> {
    text.bytes().chain([0]).collect()
}

fn last_error() -> u32 {
    // SAFETY: reads the calling thread's last error.
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

struct World {
    package: PathBuf,
    _dir: tempfile::TempDir,
}

fn world() -> &'static World {
    static WORLD: OnceLock<World> = OnceLock::new();
    WORLD.get_or_init(|| {
        let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
        std::fs::create_dir_all(&scratch).expect("scratch");
        let dir = tempfile::Builder::new()
            .prefix("sandbox-")
            .tempdir_in(&scratch)
            .expect("temp dir");
        let root = PathBuf::from(
            dir.path()
                .canonicalize()
                .expect("canonical")
                .to_string_lossy()
                .trim_start_matches("\\\\?\\"),
        );
        let package = root.join("package");
        let output = root.join("Output");
        std::fs::create_dir_all(&package).expect("package");
        std::fs::create_dir_all(&output).expect("output");
        let system = shims::hostlib::system_directory().expect("system folder");
        for decoy in [
            "msvcp140.dll",
            "vcruntime140.dll",
            "kernel32.dll",
            "xmem.dll",
        ] {
            std::fs::copy(system.join("version.dll"), package.join(decoy)).expect("decoy");
        }
        shims::loader::configure(Some(package.clone()), output.clone());
        assert!(shims::files::configure_sandboxed(
            package.clone(),
            output,
            Vec::new()
        ));
        shims::loader::set_main_path(package.join("game.exe"));
        World { package, _dir: dir }
    })
}

fn module_path(module: usize) -> String {
    let mut buffer = [0u16; 1024];
    // SAFETY: fills a buffer of the length given.
    let length = unsafe {
        windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(
            module as *mut _,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    String::from_utf16_lossy(&buffer[..length]).to_lowercase()
}

#[test]
fn library_names_with_folders_are_refused_unless_they_name_the_system_folder() {
    let w = world();
    let load: LoadLibraryW = fn_at("LoadLibraryW");
    let system = shims::hostlib::system_directory().expect("system folder");
    // SAFETY: valid terminated strings.
    unsafe {
        assert_ne!(load(wide("kernel32.dll").as_ptr()), 0);
        assert_ne!(load(wide("KERNEL32").as_ptr()), 0);
        let full = system.join("kernel32.dll");
        assert_ne!(load(wide(&full.to_string_lossy()).as_ptr()), 0);
        for refused in [
            "C:\\Users\\someone\\Downloads\\kernel32.dll".to_string(),
            "..\\kernel32.dll".to_string(),
            "\\\\server\\share\\kernel32.dll".to_string(),
            format!("{}\\..\\kernel32.dll", system.display()),
            w.package
                .join("msvcp140.dll")
                .to_string_lossy()
                .into_owned(),
            w.package
                .join("kernel32.dll")
                .to_string_lossy()
                .into_owned(),
            "kernel32.dll:stream".to_string(),
        ] {
            assert_eq!(load(wide(&refused).as_ptr()), 0, "{refused}");
            assert_eq!(last_error(), 126, "{refused}");
        }
        assert_ne!(
            load(wide(&w.package.join("xmem.dll").to_string_lossy()).as_ptr()),
            0,
            "a console library named in the game folder is answered with a stand-in, never loaded"
        );
        assert!(
            shims::trace::notes()
                .iter()
                .any(|n| n.contains("outside the Windows system folder")),
        );
    }
}

#[test]
fn a_library_of_the_game_folder_is_never_what_a_system_name_loads() {
    let w = world();
    let load: LoadLibraryW = fn_at("LoadLibraryW");
    let load_ex: LoadLibraryExW = fn_at("LoadLibraryExW");
    let system = shims::hostlib::system_directory()
        .expect("system folder")
        .to_string_lossy()
        .to_lowercase();
    assert!(w.package.join("msvcp140.dll").is_file());
    // SAFETY: valid terminated strings.
    unsafe {
        for flags in [0, 0x8, 0x100, 0x200, 0x400, 0x1000, 0x1A00] {
            for name in ["msvcp140.dll", "vcruntime140.dll", "kernel32.dll"] {
                let module = load_ex(wide(name).as_ptr(), 0, flags);
                if module != 0 {
                    let path = module_path(module);
                    assert!(
                        path.starts_with(&system),
                        "{name} with {flags:#x} came from {path}"
                    );
                    assert!(!path.contains("sandbox-"), "{path}");
                }
            }
        }
        let module = load(wide("msvcp140.dll").as_ptr());
        if module != 0 {
            assert!(module_path(module).starts_with(&system));
        } else {
            assert_eq!(last_error(), 126);
        }
    }
}

#[test]
fn the_games_module_names_are_in_the_game_folder() {
    let w = world();
    let name_w: GetModuleFileNameW = fn_at("GetModuleFileNameW");
    let name_a: GetModuleFileNameA = fn_at("GetModuleFileNameA");
    let handle: GetModuleHandleW = fn_at("GetModuleHandleW");
    let load: LoadLibraryW = fn_at("LoadLibraryW");
    let expected = w.package.join("game.exe").to_string_lossy().into_owned();
    // SAFETY: buffers of the sizes given.
    unsafe {
        let mut buffer = [0u16; 512];
        let length = name_w(0, buffer.as_mut_ptr(), 512) as usize;
        assert_eq!(String::from_utf16_lossy(&buffer[..length]), expected);
        let mut narrow = [0u8; 512];
        let length = name_a(0, narrow.as_mut_ptr(), 512) as usize;
        assert_eq!(&narrow[..length], expected.as_bytes());
        assert_eq!(narrow[length], 0);
        let mut small = [0xFFFFu16; 8];
        assert_eq!(name_w(0, small.as_mut_ptr(), 8), 8);
        assert_eq!(last_error(), 122);
        assert_eq!(small[7], 0);
        assert_eq!(String::from_utf16_lossy(&small[..7]), expected[..7]);
        assert_eq!(name_w(0, buffer.as_mut_ptr(), 0), 0);
        let console = load(wide("xmem.dll").as_ptr());
        let length = name_w(console, buffer.as_mut_ptr(), 512) as usize;
        assert_eq!(
            String::from_utf16_lossy(&buffer[..length]),
            w.package.join("xmem.dll").to_string_lossy()
        );
        let kernel = handle(wide("kernel32.dll").as_ptr());
        let length = name_w(kernel, buffer.as_mut_ptr(), 512) as usize;
        assert!(
            String::from_utf16_lossy(&buffer[..length])
                .to_lowercase()
                .ends_with("kernel32.dll")
        );
    }
}

#[test]
fn system_functions_the_shims_do_not_model_cannot_be_found_by_name() {
    world();
    let handle: GetModuleHandleW = fn_at("GetModuleHandleW");
    let proc: GetProcAddress = fn_at("GetProcAddress");
    // SAFETY: valid terminated strings.
    unsafe {
        let kernel = handle(wide("kernel32.dll").as_ptr());
        assert_ne!(kernel, 0);
        for name in [
            "CreateHardLinkW",
            "CreateProcessW",
            "CreateNamedPipeW",
            "DeviceIoControl",
        ] {
            assert_eq!(proc(kernel, cstr(name).as_ptr()), 0, "{name}");
            assert_eq!(last_error(), 127, "{name}");
        }
        assert_eq!(
            proc(kernel, cstr("CreateFileA").as_ptr()),
            shim_address("kernel32.dll", "CreateFileA")
        );
        assert_eq!(
            proc(kernel, cstr("CopyFileW").as_ptr()),
            shim_address("kernel32.dll", "CopyFileW")
        );
        let advapi = fn_at::<LoadLibraryW>("LoadLibraryW")(wide("advapi32.dll").as_ptr());
        assert_eq!(
            proc(advapi, cstr("RegOpenKeyW").as_ptr()),
            shim_address("advapi32.dll", "RegOpenKeyW")
        );
        let ntdll = handle(wide("ntdll.dll").as_ptr());
        if ntdll != 0 {
            assert_eq!(proc(ntdll, cstr("NtCreateFile").as_ptr()), 0);
        }
        let ucrt = fn_at::<LoadLibraryW>("LoadLibraryW")(wide("ucrtbase.dll").as_ptr());
        assert_eq!(
            proc(ucrt, cstr("fopen").as_ptr()),
            shim_address("ucrtbase.dll", "fopen")
        );
        assert_eq!(
            proc(ucrt, cstr("_wopen").as_ptr()),
            shim_address("ucrtbase.dll", "_wopen")
        );
        assert_eq!(proc(ucrt, cstr("_wfindfirst64").as_ptr()), 0);
        assert_eq!(proc(ucrt, cstr("tmpfile").as_ptr()), 0);
    }
}

#[test]
fn imports_of_blocked_system_functions_are_left_unbound() {
    world();
    let binder = Binder::new(&[]);
    let import = |dll, name| ImportRef {
        dll,
        name: Some(name),
        ordinal: None,
    };
    for (dll, name) in [
        ("KERNEL32.dll", "CreateHardLinkW"),
        (
            "api-ms-win-core-processthreads-l1-1-0.dll",
            "CreateProcessW",
        ),
        ("ntdll.dll", "NtCreateFile"),
        ("api-ms-win-crt-stdio-l1-1-0.dll", "tmpfile"),
    ] {
        assert_eq!(
            binder.classify(&import(dll, name)),
            Binding::Missing,
            "{dll}!{name}"
        );
    }
    for (dll, name) in [
        ("KERNEL32.dll", "CreateFileA"),
        ("api-ms-win-core-file-l2-1-0.dll", "CopyFileExW"),
        ("ADVAPI32.dll", "RegOpenKeyW"),
        ("api-ms-win-crt-stdio-l1-1-0.dll", "_wsopen_s"),
        ("MSVCP140.dll", "?_Fiopen@std@@YAPEAU_iobuf@@PEBDHH@Z"),
        ("api-ms-win-core-namedpipe-l1-1-0.dll", "WaitNamedPipeW"),
        (
            "api-ms-win-core-windowserrorreporting-l1-1-0.dll",
            "WerRegisterFile",
        ),
    ] {
        assert_eq!(
            binder.classify(&import(dll, name)),
            Binding::Shim,
            "{dll}!{name}"
        );
    }
}

#[test]
fn host_imports_resolve_from_the_system_folder_only() {
    let w = world();
    let binder = Binder::new(&[]);
    let system = shims::hostlib::system_directory()
        .expect("system folder")
        .to_string_lossy()
        .to_lowercase();
    let import = ImportRef {
        dll: "KERNEL32.dll",
        name: Some("GetCurrentProcessId"),
        ordinal: None,
    };
    assert_eq!(binder.classify(&import), Binding::Host);
    assert!(w.package.join("kernel32.dll").is_file());
    let module = shims::hostlib::load("KERNEL32.dll", 0).expect("kernel32");
    assert!(module_path(module).starts_with(&system));
}
