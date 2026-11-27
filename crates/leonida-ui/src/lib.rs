#![cfg_attr(not(windows), allow(dead_code))]

mod cache;
mod checks;
mod child;
mod form;
mod game;
mod jobs;
mod model;
mod modlist;
mod play;
mod settings;
#[cfg(windows)]
mod window;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

#[cfg(windows)]
pub fn run() -> Result<(), Error> {
    window::run()
}

#[cfg(not(windows))]
pub fn run() -> Result<(), Error> {
    Err("the Leonida window is only available on Windows; see `leonida --help` for the command line".into())
}
