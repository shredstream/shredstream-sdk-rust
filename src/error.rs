use thiserror::Error;

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("bincode: {0}")]
    Bincode(#[from] bincode::Error),
    #[error("corruption: {0}")]
    Corruption(String),
}
