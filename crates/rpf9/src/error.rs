use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Rpf9Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("not an RPF9 archive: {0}")]
    NotAnArchive(&'static str),
    #[error("resource lies outside the archive: offset {offset}, {len} bytes")]
    OutOfRange { offset: u64, len: u64 },
    #[error("corrupt {codec} data at archive offset {offset}: {detail}")]
    Corrupt {
        codec: &'static str,
        offset: u64,
        detail: String,
    },
    #[error("segment at archive offset {offset} declares {declared} bytes but holds {actual}")]
    SizeMismatch {
        offset: u64,
        declared: u64,
        actual: u64,
    },
    #[error("asset index: {0}")]
    Index(String),
}

impl Rpf9Error {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Rpf9Error::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

macro_rules! index_error {
    ($($ty:ty),*) => {$(
        impl From<$ty> for Rpf9Error {
            fn from(error: $ty) -> Self {
                Rpf9Error::Index(error.to_string())
            }
        }
    )*};
}

index_error!(
    redb::Error,
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError,
    postcard::Error
);
