use std::fs::File;
use std::ops::ControlFlow;
use std::process::ExitCode;

use anyhow::{Context, Result};
use game_package::{Manifest, map_file, package_path};
use shader_container::{ScanSummary, scan::scan};

use crate::cli::ScanArgs;
use crate::game::resolve_game;

pub fn scan_command(args: &ScanArgs) -> Result<ExitCode> {
    let game = resolve_game(args.game.as_ref(), &Manifest::builtin())?;
    let path = package_path(&game, "shaders.rpf");
    let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let archive = map_file(&file).with_context(|| format!("mapping {}", path.display()))?;
    let result = scan(&archive, |_| ControlFlow::Continue(()));
    print!("{}", ScanSummary::of(&result).report());
    Ok(if result.stats.carved == 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
