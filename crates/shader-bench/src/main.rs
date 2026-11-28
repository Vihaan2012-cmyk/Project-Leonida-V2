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

pub struct Args {
    game: Option<PathBuf>,
    compute: usize,
    pixel: usize,
    names: Vec<String>,
    save: Option<PathBuf>,
    baseline: Option<PathBuf>,
    candidate: Option<PathBuf>,
    repeats: u32,
    plain: u32,
    list: bool,
    max_code: usize,
    max_words: usize,
    skip: Vec<String>,
    translate_only: bool,
    timeout: u64,
    child: bool,
}

fn parse() -> Result<Args> {
    let mut args = Args {
        game: std::env::var_os("LEONIDA_GAME_DIR").map(PathBuf::from),
        compute: 0,
        pixel: 0,
        names: Vec::new(),
        save: None,
        baseline: None,
        candidate: None,
        repeats: 24,
        plain: 1024,
        list: false,
        max_code: 120_000,
        max_words: 250_000,
        skip: Vec::new(),
        translate_only: false,
        timeout: 0,
        child: false,
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
            "--candidate" => args.candidate = Some(value()?.into()),
            "--repeats" => args.repeats = value()?.parse()?,
            "--plain" => args.plain = value()?.parse()?,
            "--list" => args.list = true,
            "--max-words" => args.max_words = value()?.parse()?,
            "--skip" => args.skip = value()?.split(',').map(str::to_string).collect(),
            "--translate-only" => args.translate_only = true,
            "--like" => {
                for entry in std::fs::read_dir(value()?)? {
                    let path = entry?.path();
                    if path.extension().is_some_and(|e| e == "spv")
                        && let Some(stem) = path.file_stem()
                    {
                        args.names.push(stem.to_string_lossy().into_owned());
                    }
                }
                args.names.sort();
            }
            "--timeout" => args.timeout = value()?.parse()?,
            "--child" => args.child = true,
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
    let programs = pick::pick(
        &game,
        args.compute,
        args.pixel,
        &args.names,
        args.max_code,
        &args.skip,
    )?;
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
    if args.timeout > 0 && !args.child {
        return run::supervise(&args, programs);
    }
    run::all(&args.into(), programs)
}

impl Args {
    /// The flags a child process needs to run one program the way this process would.
    fn child_flags(&self, name: &str) -> Vec<String> {
        let mut flags = vec![
            "--child".to_string(),
            "--names".into(),
            name.into(),
            "--max-code".into(),
            usize::MAX.to_string(),
            "--max-words".into(),
            self.max_words.to_string(),
            "--repeats".into(),
            self.repeats.to_string(),
            "--plain".into(),
            self.plain.to_string(),
        ];
        for (flag, folder) in [
            ("--save", &self.save),
            ("--baseline", &self.baseline),
            ("--candidate", &self.candidate),
        ] {
            if let Some(folder) = folder {
                flags.push(flag.into());
                flags.push(folder.display().to_string());
            }
        }
        if self.translate_only {
            flags.push("--translate-only".into());
        }
        flags
    }
}

pub struct Settings {
    pub save: Option<PathBuf>,
    pub baseline: Option<PathBuf>,
    pub candidate: Option<PathBuf>,
    pub repeats: u32,
    pub plain: u32,
    pub max_words: usize,
    pub translate_only: bool,
}

impl From<&Args> for Settings {
    fn from(args: &Args) -> Settings {
        Settings {
            save: args.save.clone(),
            baseline: args.baseline.clone(),
            candidate: args.candidate.clone(),
            repeats: args.repeats,
            plain: args.plain,
            max_words: args.max_words,
            translate_only: args.translate_only,
        }
    }
}

impl From<Args> for Settings {
    fn from(args: Args) -> Settings {
        Settings::from(&args)
    }
}
