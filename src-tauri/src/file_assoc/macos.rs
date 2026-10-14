//! macOS: honest about what the OS allows.
//!
//! Neither half of the feature is reachable from a running app:
//!
//! * **Setting the default** needs `LSSetDefaultRoleHandlerForContentType` from
//!   the Launch Services framework. macOS ships no command-line equivalent, and
//!   calling it would mean binding a native framework -- a dependency this
//!   project deliberately avoids.
//! * **Per-format document icons** come from `CFBundleTypeIconFile` in the
//!   bundle's `Info.plist`, which is read at launch and covered by the code
//!   signature. Rewriting it at runtime breaks the signature.
//!
//! So the settings panel shows the format list as information and points at the
//! Finder's own "Open With -> Change All" flow, rather than offering buttons
//! that cannot do anything.

use super::plan::AssocTarget;
use crate::error::AppError;

pub const SUPPORTS_PER_FORMAT_ICONS: bool = false;
pub const CAN_ASSOCIATE: bool = false;
pub const CAN_SET_DEFAULT: bool = false;

pub const NOTE: &str = "macOS 不允许应用在运行时建立文件关联：设置默认程序需要 Launch Services API（系统未提供命令行），\
按格式替换文档图标需要构建期写入 Info.plist 且会破坏代码签名。\
请在 Finder 中右键图片 → 显示简介 → 「打开方式」选择 Lumina → 点「全部更改」。";

/// Always false: Launch Services cannot be queried for this without FFI, and
/// guessing "associated" would be worse than reporting nothing.
pub fn is_associated(_ext: &str) -> bool {
    false
}

pub fn associate(_target: &AssocTarget, _exts: &[String]) -> Result<(), AppError> {
    // Reached only if the frontend ignores `CAN_ASSOCIATE`. Succeeding quietly
    // keeps the command total rather than inventing an error the user cannot act on.
    Ok(())
}

pub fn dissociate(_target: &AssocTarget, _exts: &[String]) -> Result<(), AppError> {
    Ok(())
}
