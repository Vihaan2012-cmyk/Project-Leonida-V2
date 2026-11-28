use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use game_package::{Manifest, map_file};
use pe_image::Image;
use pe_loader::{ImportRef, LoadOptions, Placement};
use shims::{Binder, Binding};

use crate::cli::RunArgs;
use crate::cmd_exe::executable;
use crate::game::resolve_game;
use crate::runtime::{self, RuntimePlan};

pub fn run_command(args: &RunArgs) -> Result<ExitCode> {
    if !args.dry_run && !args.start_the_game {
        bail!(
            "this would start the game. Add --start-the-game if you mean to; --dry-run only reads its imports."
        );
    }
    let settings = runtime::load_settings(&args.config, !args.dry_run)?;
    let given = args
        .game
        .clone()
        .or_else(|| settings.general.game_folder.clone());
    let game = resolve_game(given.as_ref(), &Manifest::builtin())?;
    let plan = RuntimePlan::from(args, &settings, &game)?;
    let path = match &args.file {
        Some(name) if name.contains(['/', '\\']) || name.contains("..") => {
            bail!("--file names a file directly inside the game folder")
        }
        Some(name) => game.join(name),
        None => executable(&game)?,
    };
    if !args.dry_run {
        return run(&path, args, &plan);
    }
    runtime::print_plan(&plan);
    let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let bytes = map_file(&file).context("mapping the file")?;
    println!("{}", runtime::describe_mouse_patch(&plan, &bytes));
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

/// Loads the executable with its imports bound and calls its entry point, which normally ends the process.
#[allow(unsafe_code)]
fn run(path: &Path, args: &RunArgs, plan: &RuntimePlan) -> Result<ExitCode> {
    runtime::print_plan(plan);
    runtime::configure(plan)?;
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut binder = Binder::new(&[]);
    if args.trace {
        binder = binder.traced();
    }
    let options = LoadOptions {
        placement: Placement::Preferred,
        helpers: Some(plan.output_dir.join("runtime")),
    };
    let module = pe_loader::load(&bytes, &mut binder, &options)
        .with_context(|| format!("loading {}", path.display()))?;
    binder.seal().map_err(|e| anyhow!(e))?;
    shims::trace::set_image(module.base(), module.size());
    runtime::patch(plan, &bytes, &module);
    let binder: &'static Binder = Box::leak(Box::new(binder));
    let boot = plan.output_dir.join("boot");
    if let Some(trace) = binder.trace() {
        shims::trace::watch_crashes(boot.clone(), module.base(), module.size());
        trace.attach();
        // SAFETY: the mapping of the module just loaded, which stays loaded.
        let mut memory = unsafe { enhancements::patch::Mapped::new(module.base(), module.size()) };
        match enhancements::diagnostics::enable(
            &mut memory,
            &enhancements::diagnostics::FASTFAIL_SITES,
        ) {
            Ok(applied) => println!("trace: {} fast-fail sites made visible", applied.len()),
            Err(error) => eprintln!("warning: fast-fail sites stay hidden from the trace: {error}"),
        }
        let folder = boot.clone();
        std::thread::spawn(move || {
            loop {
                let _ = trace.write(&folder);
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }
    println!("starting {} at {:#x}", path.display(), module.entry());
    // SAFETY: the player asked to run this executable; its imports are bound and its pages protected.
    let code = unsafe { module.call_entry() };
    runtime::detach_mouse();
    shims::d3d12::shutdown();
    if let Some(trace) = binder.trace() {
        trace.write(&boot)?;
    }
    println!("the entry point returned {code:#x}");
    Ok(ExitCode::SUCCESS)
}
