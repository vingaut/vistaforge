use thiserror::Error;

/// Error type shared by the core data model.
#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("codec error: {0}")]
    Codec(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("heightfield error: {0}")]
    HeightField(#[from] crate::heightfield::HeightFieldError),
}

pub type Result<T> = std::result::Result<T, Error>;
