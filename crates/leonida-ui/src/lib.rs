mod build_steps;
mod game;
mod jobs;
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
