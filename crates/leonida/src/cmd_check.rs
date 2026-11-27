use std::process::ExitCode;

use anyhow::{Result, bail};
use game_package::{Manifest, check_files, match_version};

use crate::cli::CheckArgs;
use crate::game::resolve_game;

pub fn check_command(args: &CheckArgs) -> Result<ExitCode> {
    let manifests = Manifest::builtin();
    let game = resolve_game(args.game.as_ref(), &manifests)?;
    let Some(manifest) = match_version(&game, &manifests) else {
        bail!("{} is not a version Leonida knows", game.display());
    };
    let report = check_files(&game, manifest, args.full);
    println!("{}: {} files checked", game.display(), report.checked);
    let list = |title: &str, files: &[String]| {
        if !files.is_empty() {
            println!("{title} ({}):", files.len());
            for file in files {
                println!("  {file}");
            }
        }
    };
    list("missing", &report.missing);
    list("the wrong size", &report.wrong_size);
    list("different contents", &report.hash_mismatch);
    list("could not be read", &report.unreadable);
    if !report.exe_present {
        println!("the game executable is not in the folder");
    }
    if report.is_ok() && report.exe_present {
        let depth = if args.full {
            "present, the right size and the right contents"
        } else {
            "present and the right size (run with --full to compare contents too)"
        };
        println!("every file is {depth}");
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}
