/// Everything the codec and container code can refuse.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("{what}: need {need} bytes at offset {offset}, only {have} available")]
    Truncated {
        what: &'static str,
        offset: usize,
        need: usize,
        have: usize,
    },
    #[error("{0}")]
    Invalid(String),
    #[error("{0} is not supported (not measured or not implemented)")]
    Unsupported(String),
    #[error("corrupt XMA2 data: {0}")]
    Corrupt(&'static str),
    #[error("stream has no {0} tag")]
    MissingTag(&'static str),
    #[error("{0}")]
    Io(String),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
