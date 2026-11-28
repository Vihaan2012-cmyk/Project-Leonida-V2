//! Times real translated game shaders on the GPU and checks that two translations of one shader agree.
//!
//! `shader-bench --compute 20 --pixel 8 --save base` translates a spread of the game's compute and pixel shaders
//! and keeps their SPIR-V in `base`. Run again with `--baseline base`, it times each shader as `base` has it
//! and as the translator builds it now, on the same synthetic inputs, and compares what both wrote.

mod compare;
mod inputs;
mod pick;
mod run;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

struct Args {
    game: Option<PathBuf>,
    compute: usize,
    pixel: usize,
    names: Vec<String>,
    save: Option<PathBuf>,
    baseline: Option<PathBuf>,
    repeats: u32,
    plain: u32,
    list: bool,
    max_code: usize,
}

fn parse() -> Result<Args> {
    let mut args = Args {
        game: std::env::var_os("LEONIDA_GAME_DIR").map(PathBuf::from),
        compute: 0,
        pixel: 0,
        names: Vec::new(),
        save: None,
        baseline: None,
        repeats: 16,
        plain: 1024,
        list: false,
        max_code: 120_000,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().with_context(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--game" => args.game = Some(value()?.into()),
            "--compute" => args.compute = value()?.parse()?,
            "--pixel" => args.pixel = value()?.parse()?,
            "--names" => args.names = value()?.split(',').map(str::to_string).collect(),
            "--save" => args.save = Some(value()?.into()),
            "--baseline" => args.baseline = Some(value()?.into()),
            "--repeats" => args.repeats = value()?.parse()?,
            "--plain" => args.plain = value()?.parse()?,
            "--list" => args.list = true,
            "--max-code" => args.max_code = value()?.parse()?,
            other => bail!("unknown argument {other}"),
        }
    }
    Ok(args)
}

fn main() -> Result<()> {
    let args = parse()?;
    let Some(game) = args.game.clone() else {
        println!("LEONIDA_GAME_DIR is not set: nothing to do");
        return Ok(());
    };
    let programs = pick::pick(&game, args.compute, args.pixel, &args.names, args.max_code)?;
    if args.list {
        for program in &programs {
            println!(
                "{} {} code {} bytes, wave64 {}",
                program.kind,
                program.name,
                program.code.len(),
                program.wave64
            );
        }
        return Ok(());
    }
    for folder in args.save.iter().chain(&args.baseline) {
        std::fs::create_dir_all(folder)?;
    }
    run::all(&args.into(), programs)
}

pub struct Settings {
    pub save: Option<PathBuf>,
    pub baseline: Option<PathBuf>,
    pub repeats: u32,
    pub plain: u32,
}

impl From<&Args> for Settings {
    fn from(args: &Args) -> Settings {
        Settings {
            save: args.save.clone(),
            baseline: args.baseline.clone(),
            repeats: args.repeats,
            plain: args.plain,
        }
    }
}

impl From<Args> for Settings {
    fn from(args: Args) -> Settings {
        Settings::from(&args)
    }
}
