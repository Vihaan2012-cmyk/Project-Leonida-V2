use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("not a Bink 2 stream")]
    NotBink,
    #[error("the stream ends inside the header or frame index")]
    Truncated,
    #[error("bad header: {0}")]
    Header(&'static str),
    #[error("frame {0} is outside the movie")]
    NoSuchFrame(u32),
    #[error("frame {frame}: {reason}")]
    Frame { frame: u32, reason: &'static str },
    #[error("frame {0} needs a reference frame that was not decoded")]
    MissingReference(u32),
    #[error("audio: {0}")]
    Audio(&'static str),
    #[error("revision `{0}` is not supported")]
    Unsupported(char),
}

pub type Result<T> = std::result::Result<T, Error>;
