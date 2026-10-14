//! Operating-system file association.
//!
//! Lumina can register itself as a handler for the image formats it opens, and
//! -- on Windows -- hang a distinct icon off each one so Explorer shows the
//! format's own wordmark instead of a generic Lumina glyph.
//!
//! # Structure
//!
//! * [`plan`] turns "associate these extensions" into a platform-neutral list of
//!   registry writes. Pure data, unit-tested everywhere.
//! * `windows` / `macos` / `linux` execute that intent with the platform's own
//!   mechanism, and answer "is this extension associated right now?".
//! * This module is the facade the IPC layer talks to: it owns the format
//!   table and assembles [`AssocStatus`].
//!
//! # What each platform can honestly promise
//!
//! | | Windows | macOS | Linux |
//! | --- | --- | --- | --- |
//! | Appear in "Open with" | yes | yes | yes |
//! | Set as default programmatically | **no** (UserChoice hash) | **no** | yes (`xdg-mime`) |
//! | Per-format file icons | **yes** | no | no |
//!
//! The UI reads those flags off [`AssocStatus`] and words itself accordingly --
//! it must not promise a silent "make default" where the OS forbids one.

/// The Windows registry layout as pure data.
///
/// Only `windows` consumes it at runtime, so on every other platform the module
/// is reachable solely from its own unit tests -- which is exactly the point:
/// the layout stays verified everywhere, even where it cannot be executed.
#[cfg_attr(not(windows), allow(dead_code))]
pub mod plan;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod unsupported;
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
use unsupported as platform;

use serde::Serialize;
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager};

use crate::error::AppError;

/// One image format family: the extensions it covers, the wordmark that stands
/// for it, and the label the settings panel shows.
pub struct Format {
    /// Icon file stem -- `icons/file-icons/<key>.ico`. Must match
    /// `scripts/make-file-icons.py`.
    pub key: &'static str,
    /// Label shown in the settings panel.
    pub label: &'static str,
    /// Text Windows puts in Explorer's "Type" column.
    pub description: &'static str,
    pub ext: &'static [&'static str],
}

/// The formats Lumina can open, grouped so that aliases (`jpg`/`jpeg`,
/// `tif`/`tiff`) share one wordmark but keep their own ProgIDs.
pub const FORMATS: &[Format] = &[
    Format {
        key: "png",
        label: "PNG 图像",
        description: "PNG Image",
        ext: &["png"],
    },
    Format {
        key: "jpg",
        label: "JPEG 图像",
        description: "JPEG Image",
        ext: &["jpg", "jpeg"],
    },
    Format {
        key: "gif",
        label: "GIF 图像",
        description: "GIF Image",
        ext: &["gif"],
    },
    Format {
        key: "webp",
        label: "WebP 图像",
        description: "WebP Image",
        ext: &["webp"],
    },
    Format {
        key: "bmp",
        label: "BMP 图像",
        description: "BMP Image",
        ext: &["bmp"],
    },
    Format {
        key: "tiff",
        label: "TIFF 图像",
        description: "TIFF Image",
        ext: &["tif", "tiff"],
    },
    Format {
        key: "ico",
        label: "图标文件",
        description: "Icon",
        ext: &["ico"],
    },
    Format {
        key: "svg",
        label: "矢量图",
        description: "SVG Image",
        ext: &["svg"],
    },
];

/// The format family owning `ext` (lower-cased, no dot).
pub fn format_of(ext: &str) -> Option<&'static Format> {
    FORMATS.iter().find(|f| f.ext.contains(&ext))
}

/// Every extension Lumina can associate, in [`FORMATS`] order.
///
/// Consumed by the Linux `.desktop` generator and by the tests; the Windows
/// path is handed its extension list by the caller instead.
#[cfg_attr(not(any(test, target_os = "linux")), allow(dead_code))]
pub fn all_extensions() -> Vec<String> {
    FORMATS
        .iter()
        .flat_map(|f| f.ext.iter().map(|e| e.to_string()))
        .collect()
}

/// Per-format association state.
#[derive(Serialize, Clone)]
pub struct FormatInfo {
    pub key: String,
    pub label: String,
    pub ext: Vec<String>,
    /// True when every extension in the family is registered to Lumina.
    pub associated: bool,
}

/// Everything the settings panel needs to render itself.
#[derive(Serialize, Clone)]
pub struct AssocStatus {
    /// `std::env::consts::OS`.
    pub platform: String,
    /// Whether the OS lets a per-extension icon be attached (Windows only).
    pub supports_per_format_icons: bool,
    /// Whether Lumina can register itself at all. False on macOS, where
    /// association is fixed at bundle-build time.
    pub can_associate: bool,
    /// Whether Lumina can make itself the default without the user confirming
    /// somewhere else. False on Windows and macOS by OS design.
    pub can_set_default: bool,
    pub formats: Vec<FormatInfo>,
    /// Platform-specific explanation, shown verbatim in the panel.
    pub note: String,
}

/// Current association state for every supported format.
pub fn status() -> AssocStatus {
    AssocStatus {
        platform: std::env::consts::OS.to_string(),
        supports_per_format_icons: platform::SUPPORTS_PER_FORMAT_ICONS,
        can_associate: platform::CAN_ASSOCIATE,
        can_set_default: platform::CAN_SET_DEFAULT,
        formats: FORMATS
            .iter()
            .map(|f| FormatInfo {
                key: f.key.to_string(),
                label: f.label.to_string(),
                ext: f.ext.iter().map(|e| e.to_string()).collect(),
                associated: f.ext.iter().all(|e| platform::is_associated(e)),
            })
            .collect(),
        note: platform::NOTE.to_string(),
    }
}

/// Register or withdraw Lumina for exactly `exts`, then report the new state.
///
/// Extensions outside [`FORMATS`] are dropped rather than rejected: the UI can
/// only ever offer supported ones, and silently ignoring a stray value is
/// friendlier than failing the whole batch.
pub fn set_enabled(app: &AppHandle, exts: &[String], enabled: bool) -> Result<AssocStatus, AppError> {
    let wanted: Vec<String> = exts
        .iter()
        .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
        .filter(|e| format_of(e).is_some())
        .collect();

    if !wanted.is_empty() {
        let target = assoc_target(app)?;
        if enabled {
            platform::associate(&target, &wanted)?;
        } else {
            platform::dissociate(&target, &wanted)?;
        }
    }
    Ok(status())
}

/// Absolute paths the association needs: the running executable and the
/// directory of per-format `.ico` files.
///
/// The icon directory is resolved through Tauri's path resolver rather than
/// assumed, because `resource_dir()` differs between a dev build
/// (`target/debug`) and a bundle (exe directory on Windows, `Contents/Resources`
/// on macOS).
fn assoc_target(app: &AppHandle) -> Result<plan::AssocTarget, AppError> {
    let exe = std::env::current_exe()
        .map_err(|e| AppError::Other(format!("无法定位 Lumina 可执行文件：{e}")))?;
    let ico_dir = app
        .path()
        .resolve("icons/file-icons", BaseDirectory::Resource)
        .map_err(|e| AppError::Other(format!("无法定位文件图标资源：{e}")))?;
    Ok(plan::AssocTarget { exe, ico_dir })
}

/// Open the OS page where the user can pick a default program.
///
/// Only Windows has one; elsewhere this reports why rather than pretending.
pub fn open_default_apps_settings() -> Result<(), AppError> {
    #[cfg(windows)]
    {
        // `start` is a cmd builtin; the empty argument is its window title.
        std::process::Command::new("cmd")
            .args(["/C", "start", "", "ms-settings:defaultapps"])
            .spawn()
            .map(|_| ())
            .map_err(|e| AppError::Other(format!("无法打开系统「默认应用」设置：{e}")))
    }
    #[cfg(not(windows))]
    {
        Err(AppError::Unsupported(
            "当前平台没有统一的「默认应用」设置页".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_table_covers_every_extension_exactly_once() {
        let exts = all_extensions();
        assert_eq!(exts.len(), 10, "png jpg jpeg gif webp bmp tif tiff ico svg");

        let mut sorted = exts.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), exts.len(), "no extension may repeat");

        for ext in &exts {
            assert!(format_of(ext).is_some(), "{ext} has no format family");
        }
        assert!(format_of("avif").is_none(), "unsupported formats stay out");
    }

    #[test]
    fn aliases_share_a_wordmark_but_not_an_extension() {
        assert_eq!(format_of("jpeg").unwrap().key, "jpg");
        assert_eq!(format_of("tif").unwrap().key, "tiff");
        assert_eq!(format_of("jpg").unwrap().key, "jpg");
    }

    #[test]
    fn every_wordmark_has_a_generated_icon() {
        // The registry points Explorer at `icons/file-icons/<key>.ico`; a key
        // with no file on disk would silently fall back to a generic icon.
        for format in FORMATS {
            let ico = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("icons/file-icons")
                .join(format!("{}.ico", format.key));
            assert!(ico.exists(), "missing {}", ico.display());
        }
    }
}
