//! Unified error type for the image pipeline.
//!
//! Commands convert this into a `String` at the IPC boundary (Tauri serializes
//! `Err(String)` into a rejected JS promise), so the frontend receives a
//! human-readable message. Internally we keep a typed error so call sites can
//! use `?` ergonomically.

use std::fmt;

#[derive(Debug)]
pub enum AppError {
    /// Filesystem / IO failure.
    Io(std::io::Error),
    /// Decoding / encoding failure from the `image` crate.
    Image(image::ImageError),
    /// The requested format is not part of Lumina's supported set.
    Unsupported(String),
    /// Anything else, with a descriptive message.
    Other(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Io(e) => write!(f, "I/O error: {e}"),
            AppError::Image(e) => write!(f, "image error: {e}"),
            AppError::Unsupported(m) => write!(f, "unsupported format: {m}"),
            AppError::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AppError::Io(e) => Some(e),
            AppError::Image(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e)
    }
}

impl From<image::ImageError> for AppError {
    fn from(e: image::ImageError) -> Self {
        AppError::Image(e)
    }
}
