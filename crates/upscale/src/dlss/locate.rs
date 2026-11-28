//! Finding NVIDIA's `nvngx_dlss.dll`: the path the player configured, the `LEONIDA_DLSS_DLL` variable, NVIDIA's NGX
//! folders (`%ProgramData%\NVIDIA\NGX\models` and the folder the driver registers for NGX), then the folder of the
//! running executable. When nothing is found the message says where to put the file.

use std::path::{Path, PathBuf};

use super::DLSS_LIBRARY_NAME;

/// Where a library was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Configured,
    Environment,
    NgxModels,
    DriverStore,
    ExecutableFolder,
    /// One of the folders [`super::DlssConfig::search_folders`] named.
    SearchFolder,
}

impl Source {
    pub fn describe(self) -> &'static str {
        match self {
            Source::Configured => "the dlss_path setting",
            Source::Environment => "the LEONIDA_DLSS_DLL variable",
            Source::NgxModels => "NVIDIA's NGX models folder",
            Source::DriverStore => "the NGX folder of the NVIDIA driver",
            Source::ExecutableFolder => "the folder next to leonida.exe",
            Source::SearchFolder => "a search folder",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub path: PathBuf,
    pub source: Source,
}

/// Why no usable library was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotFound {
    /// The folders that were looked in, in order.
    pub searched: Vec<PathBuf>,
    /// Set when an explicitly named file is unusable; nothing else is tried then.
    pub problem: Option<String>,
}

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(problem) = &self.problem {
            return write!(f, "{problem}");
        }
        write!(
            f,
            "{DLSS_LIBRARY_NAME} was not found. Get it from NVIDIA's DLSS SDK and either set dlss_path in \
             upscaling.toml to the file or put it next to leonida.exe"
        )?;
        if !self.searched.is_empty() {
            let list: Vec<String> = self
                .searched
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            write!(f, " (looked in: {})", list.join(", "))?;
        }
        Ok(())
    }
}

/// Checks that `path` is a Windows library with the name NGX searches for.
pub fn check_file(path: &Path) -> Result<(), String> {
    let mut head = [0u8; 2];
    let read = std::fs::File::open(path).and_then(|mut file| {
        std::io::Read::read_exact(&mut file, &mut head)?;
        Ok(())
    });
    match read {
        Err(error) => Err(format!("{}: {error}", path.display())),
        Ok(()) if &head != b"MZ" => Err(format!("{} is not a Windows library", path.display())),
        Ok(()) => {
            let named = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case(DLSS_LIBRARY_NAME));
            if named {
                Ok(())
            } else {
                Err(format!(
                    "{} must be named {DLSS_LIBRARY_NAME}: NGX finds the DLSS library by that name",
                    path.display()
                ))
            }
        }
    }
}

/// `nvngx_dlss.dll` in `dir` or, for at most `depth` levels, below it.
pub fn find_in(dir: &Path, depth: u32) -> Option<PathBuf> {
    let direct = dir.join(DLSS_LIBRARY_NAME);
    if check_file(&direct).is_ok() {
        return Some(direct);
    }
    if depth == 0 {
        return None;
    }
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| entry.path())
        .collect();
    children.sort();
    children
        .into_iter()
        .find_map(|child| find_in(&child, depth - 1))
}

fn models_folder() -> Option<PathBuf> {
    let base = std::env::var_os("ProgramData")?;
    Some(
        PathBuf::from(base)
            .join("NVIDIA")
            .join("NGX")
            .join("models"),
    )
}

fn executable_folder() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// Looks for the library. `configured` is the player's `dlss_path`; `folders` replaces the default search places when
/// given (the NGX folders and the executable's folder).
pub fn locate(configured: Option<&Path>, folders: Option<&[PathBuf]>) -> Result<Located, NotFound> {
    let env = std::env::var_os("LEONIDA_DLSS_DLL")
        .filter(|v| !v.is_empty() && folders.is_none())
        .map(PathBuf::from);
    locate_with(configured, env, folders)
}

/// As [`locate`], with the `LEONIDA_DLSS_DLL` value given.
pub fn locate_with(
    configured: Option<&Path>,
    env: Option<PathBuf>,
    folders: Option<&[PathBuf]>,
) -> Result<Located, NotFound> {
    if let Some(path) = configured {
        let file = if path.is_dir() {
            path.join(DLSS_LIBRARY_NAME)
        } else {
            path.to_path_buf()
        };
        return match check_file(&file) {
            Ok(()) => Ok(Located {
                path: file,
                source: Source::Configured,
            }),
            Err(problem) => Err(NotFound {
                searched: Vec::new(),
                problem: Some(format!("dlss_path: {problem}")),
            }),
        };
    }
    let mut searched = Vec::new();
    if let Some(file) = env {
        return match check_file(&file) {
            Ok(()) => Ok(Located {
                path: file,
                source: Source::Environment,
            }),
            Err(problem) => Err(NotFound {
                searched,
                problem: Some(format!("LEONIDA_DLSS_DLL: {problem}")),
            }),
        };
    }
    let places: Vec<(PathBuf, Source, u32)> = match folders {
        Some(folders) => folders
            .iter()
            .map(|f| (f.clone(), Source::SearchFolder, 0))
            .collect(),
        None => {
            let mut places = Vec::new();
            if let Some(dir) = models_folder() {
                places.push((dir, Source::NgxModels, 4));
            }
            if let Some(dir) = super::ngx::registered_ngx_folder() {
                places.push((dir, Source::DriverStore, 0));
            }
            if let Some(dir) = executable_folder() {
                places.push((dir, Source::ExecutableFolder, 0));
            }
            places
        }
    };
    for (dir, source, depth) in places {
        if let Some(path) = find_in(&dir, depth) {
            return Ok(Located { path, source });
        }
        searched.push(dir);
    }
    Err(NotFound {
        searched,
        problem: None,
    })
}
