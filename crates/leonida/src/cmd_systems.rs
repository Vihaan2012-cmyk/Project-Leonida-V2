use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use systems::{catalog, convert};

use crate::cli::{SystemsArgs, SystemsCommand};

pub fn systems_command(args: &SystemsArgs) -> Result<ExitCode> {
    match &args.command {
        SystemsCommand::Export { input, output } => export(input, output)?,
        SystemsCommand::Import {
            input,
            output,
            base,
        } => import(input, output, base.as_deref())?,
        SystemsCommand::Map => map(),
        SystemsCommand::Notes { folder } => notes(folder)?,
    }
    Ok(ExitCode::SUCCESS)
}

fn write(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, data).with_context(|| format!("writing {}", path.display()))
}

fn export(input: &Path, output: &Path) -> Result<()> {
    let bytes = fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let hint = input.file_stem().and_then(|s| s.to_str());
    let done =
        convert::export(&bytes, hint).with_context(|| format!("converting {}", input.display()))?;
    let target = if output.is_dir() {
        output.join(&done.path)
    } else {
        output.to_path_buf()
    };
    write(&target, &done.data)?;
    println!("{} -> {}", input.display(), target.display());
    Ok(())
}

fn import(input: &Path, output: &Path, base: Option<&Path>) -> Result<()> {
    let bytes = fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let base = base
        .map(|b| fs::read(b).with_context(|| format!("reading {}", b.display())))
        .transpose()?;
    let data = convert::import(&bytes, base.as_deref())
        .with_context(|| format!("converting {}", input.display()))?;
    if output.is_dir() {
        bail!("{} is a folder; name the file to write", output.display());
    }
    write(output, &data)?;
    println!("{} -> {}", input.display(), output.display());
    Ok(())
}

fn map() {
    println!("Systems/ folder by root element:");
    for (root, dir) in catalog::EXACT_ROOTS {
        println!("  {root:<42} Systems/{dir}");
    }
    for (prefix, dir) in catalog::PREFIX_ROOTS {
        println!("  {:<42} Systems/{dir}", format!("{prefix}*"));
    }
    println!("  {:<42} Systems/Misc", "anything else");
    println!("  {:<42} Systems/Text/<language>.json", "string tables");
    println!("\nNot decoded yet:");
    for u in catalog::UNDECODED {
        println!("  Systems/{}: {}", u.dir, u.missing);
    }
}

fn notes(folder: &Path) -> Result<()> {
    for (path, text) in catalog::note_files() {
        let target: PathBuf = folder.join(&path);
        write(&target, text.as_bytes())?;
        println!("{}", target.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_rbf0() -> Vec<u8> {
        let mut b = b"RBF0\xFB\xFF\x01\x00".to_vec();
        b.extend([0, 0, 1, 0, b'R', 0, 0, 0, 0, 0, 0, 0xFF, 0xFF]);
        b
    }

    #[test]
    fn export_then_import_gives_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("thing.rbf");
        fs::write(&original, tiny_rbf0()).unwrap();
        let xml = dir.path().join("thing.xml");
        export(&original, &xml).unwrap();
        let text = fs::read_to_string(&xml).unwrap();
        assert!(text.contains("<rbf0 width=\"wide\">"));
        let back = dir.path().join("back.rbf");
        import(&xml, &back, None).unwrap();
        assert_eq!(fs::read(back).unwrap(), tiny_rbf0());
    }

    #[test]
    fn export_into_a_folder_uses_the_systems_path() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("x.xml");
        fs::write(&original, "<CVehiclePoints/>").unwrap();
        let out = dir.path().join("out");
        fs::create_dir(&out).unwrap();
        export(&original, &out).unwrap();
        assert!(out.join("Systems/Vehicles/x.xml").is_file());
    }

    #[test]
    fn unknown_files_are_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("x.bin");
        fs::write(&original, [1, 2, 3]).unwrap();
        assert!(export(&original, &dir.path().join("y")).is_err());
    }
}
