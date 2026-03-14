//! Error types and Result wrapper for the NAND Flash Viewer

use std::fmt;
use std::io;

/// Custom error type for NAND Flash Viewer operations
#[derive(Debug)]
pub enum Error {
    /// I/O error when reading dump file
    IoError(io::Error),
    /// Invalid file metadata (page length, block size)
    InvalidMetadata(String),
    /// Tile generation failed
    TileGenerationFailed(String),
    /// Cache operation failed
    CacheError(String),
    /// PNG encoding/decoding error
    PngError(String),
    /// Image encoding/decoding error (QOI, PNG, etc.)
    ImageError(String),
    /// Invalid tile coordinates
    InvalidCoordinates(String),
    /// Worker thread error
    WorkerError(String),
    /// Resource not found
    NotFound(String),
    /// Generic error with message
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::IoError(e) => write!(f, "I/O error: {}", e),
            Error::InvalidMetadata(msg) => write!(f, "Invalid metadata: {}", msg),
            Error::TileGenerationFailed(msg) => write!(f, "Tile generation failed: {}", msg),
            Error::CacheError(msg) => write!(f, "Cache error: {}", msg),
            Error::PngError(msg) => write!(f, "PNG error: {}", msg),
            Error::ImageError(msg) => write!(f, "Image error: {}", msg),
            Error::InvalidCoordinates(msg) => write!(f, "Invalid coordinates: {}", msg),
            Error::WorkerError(msg) => write!(f, "Worker error: {}", msg),
            Error::NotFound(msg) => write!(f, "Not found: {}", msg),
            Error::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::IoError(err)
    }
}

/// Result type alias for NAND Flash Viewer operations
pub type Result<T> = std::result::Result<T, Error>;
