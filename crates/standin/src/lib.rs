//! A stand-in title: a small PE64 program built in memory that boots the way the console game does (loader
//! queries, memory, files, threads, input, audio, graphics) and reports how far it got, so the whole runtime can
//! be exercised without the game.
//!
//! [`Script`] holds boot steps; each step emits its call sequence with the Windows x64 calling convention through
//! an [`Emitter`], and [`Script::build`] turns the steps into an image with its imports, TLS callback,
//! relocations and exception table.

mod emit;
mod harness;
mod image;
mod script;
pub mod steps;

pub use emit::{Arg, BuildError, Emitter, Place, StepInfo, Sym};
pub use harness::{Environment, Failure, Report, RunError, RunOptions, run};
pub use image::{BASE, Built};
pub use script::{FAILED, PASSED, Script, Stage};
