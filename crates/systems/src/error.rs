use thiserror::Error;

/// Everything that can go wrong reading or writing behaviour data.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("not an RBF0 stream (no RBF0 magic)")]
    BadMagic,
    #[error("the stream ends inside a record at byte {0}")]
    Truncated(usize),
    #[error("{0} bytes follow the root element's close")]
    TrailingBytes(usize),
    #[error("descriptor index {index} at byte {offset} is outside the table")]
    BadDescriptor { index: u8, offset: usize },
    #[error("descriptor name {0:?} is introduced twice")]
    DuplicateDescriptor(String),
    #[error("unknown record type {kind:#04x} at byte {offset}")]
    UnknownRecord { kind: u8, offset: usize },
    #[error("element header words at byte {0} are not zero")]
    HeaderWords(usize),
    #[error("the element at byte {0} does not have the value records its attribute count promises")]
    AttributeCount(usize),
    #[error("a record at byte {0} has no open element")]
    NoOpenElement(usize),
    #[error("a name or string at byte {0} is not valid UTF-8")]
    InvalidUtf8(usize),
    #[error("nesting is deeper than {0} elements")]
    TooDeep(usize),
    #[error("more than 253 distinct descriptor names")]
    TooManyDescriptors,
    #[error("a {what} is too long for its record ({len} bytes)")]
    TooLong { what: &'static str, len: usize },
    #[error("a value does not fit the stream's {0}-bit layout")]
    ValueRange(u32),
    #[error("XML: {0}")]
    Xml(String),
    #[error("not a string table: {0}")]
    NotATable(&'static str),
    #[error("text {key} is {new} bytes but its slot holds {old}")]
    TextDoesNotFit { key: String, new: usize, old: usize },
    #[error("text {0} shares storage with another key that was edited differently")]
    SharedText(String),
    #[error("JSON: {0}")]
    Json(String),
    #[error("{0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;
