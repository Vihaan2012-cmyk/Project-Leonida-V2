use std::collections::BTreeMap;
use std::fs::File;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use game_package::{Manifest, map_file};
use pe_image::Image;
use pe_loader::ImportRef;
use shims::{Binder, Binding};

use crate::cli::RunArgs;
use crate::cmd_exe::executable;
use crate::game::resolve_game;

pub fn run_command(args: &RunArgs) -> Result<ExitCode> {
    if !args.dry_run {
        bail!(
            "running the game is not ready yet: thread-local storage and the console's graphics interface are \
             still missing. Use --dry-run to see how its imports would be provided."
        );
    }
    let game = resolve_game(args.game.as_ref(), &Manifest::builtin())?;
    let path = match &args.file {
        Some(name) if name.contains(['/', '\\']) || name.contains("..") => {
            bail!("--file names a file directly inside the game folder")
        }
        Some(name) => game.join(name),
        None => executable(&game)?,
    };
    let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let bytes = map_file(&file).context("mapping the file")?;
    let image = Image::parse(&bytes)?;
    let binder = Binder::new(&[]);
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut gaps: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for dll in image.imports() {
        for entry in &dll.entries {
            let import = ImportRef {
                dll: &dll.name,
                name: entry.name.as_deref(),
                ordinal: entry.ordinal,
            };
            let function = match (&entry.name, entry.ordinal) {
                (Some(name), _) => name.clone(),
                (None, Some(ordinal)) => format!("#{ordinal}"),
                (None, None) => "?".into(),
            };
            let binding = binder.classify(&import);
            let kind = match binding {
                Binding::Shim => "provided by Leonida",
                Binding::Host => "provided by Windows",
                Binding::Blocked(_) => "blocked, as offline play requires",
                Binding::Missing => "missing",
            };
            *counts.entry(kind).or_default() += 1;
            if args.all || matches!(binding, Binding::Missing | Binding::Blocked(_)) {
                let label = match binding {
                    Binding::Blocked(reason) => format!("blocked ({})", reason.describe()),
                    _ => kind.to_string(),
                };
                gaps.entry((label, dll.name.clone()))
                    .or_default()
                    .push(function);
            }
        }
    }
    println!(
        "{}",
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    );
    for (kind, count) in &counts {
        println!("  {count:>5}  {kind}");
    }
    for ((label, dll), functions) in &gaps {
        println!("{label}: {dll} ({})", functions.len());
        for function in functions {
            println!("    {function}");
        }
    }
    Ok(ExitCode::SUCCESS)
}
