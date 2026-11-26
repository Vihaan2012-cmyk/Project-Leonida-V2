/// Why a shader container could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContainerError {
    #[error("truncated {0}")]
    Truncated(&'static str),
    #[error("{0}")]
    Invalid(String),
}
