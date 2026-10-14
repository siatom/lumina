//! Fallback for platforms with no association mechanism (mobile targets).
//!
//! The crate also compiles for `#[cfg_attr(mobile, ...)]`, where none of the
//! desktop modules apply. Reporting "unsupported" keeps the settings panel
//! honest instead of failing to build.

use super::plan::AssocTarget;
use crate::error::AppError;

pub const SUPPORTS_PER_FORMAT_ICONS: bool = false;
pub const CAN_ASSOCIATE: bool = false;
pub const CAN_SET_DEFAULT: bool = false;

pub const NOTE: &str = "当前平台不支持文件关联。";

pub fn is_associated(_ext: &str) -> bool {
    false
}

pub fn associate(_target: &AssocTarget, _exts: &[String]) -> Result<(), AppError> {
    Err(AppError::Unsupported(NOTE.to_string()))
}

pub fn dissociate(_target: &AssocTarget, _exts: &[String]) -> Result<(), AppError> {
    Err(AppError::Unsupported(NOTE.to_string()))
}
