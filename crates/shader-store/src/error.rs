/// What can go wrong reading or writing the shader store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("shader database: {0}")]
    Db(String),
    #[error("a stored shader uses record layout {found}; this build reads layout {expected}")]
    Schema { found: u16, expected: u16 },
    #[error("a stored shader is damaged: {0}")]
    Corrupt(String),
    #[error("shader store file: {0}")]
    Io(#[from] std::io::Error),
}

macro_rules! from_redb {
    ($($kind:ty),*) => {$(
        impl From<$kind> for StoreError {
            fn from(error: $kind) -> Self {
                StoreError::Db(error.to_string())
            }
        }
    )*};
}

from_redb!(
    redb::Error,
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError
);
