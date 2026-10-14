//! Tauri IPC commands — the contract consumed by the frontend.
//!
//! Every command returns `Result<T, String>` so failures surface as rejected
//! promises in JS with a readable message. See `docs/INTERFACES.md` for the
//! full, authoritative description of each command and its TypeScript types.

use std::path::{Path, PathBuf};

use crate::error::AppError;
use crate::imaging::{self, DirectoryListing, ImageMetadata};
use crate::protocol;

/// Metadata for a single image: dimensions, format, size and basic EXIF.
///
/// `path` is the *original* file path; the frontend turns it into a display URL
/// via the `lumina://` protocol (see [`get_protocol_base`]).
#[tauri::command]
pub fn get_image_metadata(path: String) -> Result<ImageMetadata, String> {
    imaging::read_metadata(Path::new(&path)).map_err(|e| e.to_string())
}

/// List the sibling images in `dir` (naturally sorted) plus the index of
/// `current`, so the frontend can implement previous/next navigation.
#[tauri::command]
pub fn list_images(dir: String, current: Option<String>) -> Result<DirectoryListing, String> {
    let current = current.map(PathBuf::from);
    imaging::list_directory(Path::new(&dir), current.as_deref()).map_err(|e| e.to_string())
}

/// Downscaled PNG thumbnail (fits inside `max_size` on both axes).
///
/// Returned as raw bytes (`tauri::ipc::Response`) so the frontend receives an
/// `ArrayBuffer`, not a base64 string.
#[tauri::command]
pub fn get_thumbnail(path: String, max_size: Option<u32>) -> Result<tauri::ipc::Response, String> {
    let max = max_size.unwrap_or(256).clamp(16, 4096);
    let bytes = imaging::make_thumbnail(Path::new(&path), max).map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Raw bytes for the original view. Same payload the `lumina://` protocol
/// serves; used as a fallback when the protocol is unavailable.
#[tauri::command]
pub fn get_image_bytes(path: String) -> Result<tauri::ipc::Response, String> {
    let (bytes, _mime) = imaging::read_for_display(Path::new(&path)).map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Save/convert an image to another format.
///
/// `format` is optional; when omitted the output format is inferred from the
/// `output_path` extension. Returns the written path.
#[tauri::command]
pub fn save_image_as(
    input_path: String,
    output_path: String,
    format: Option<String>,
) -> Result<String, String> {
    let fmt = format
        .as_deref()
        .map(parse_format)
        .transpose()
        .map_err(|e| e.to_string())?;
    imaging::convert_image(Path::new(&input_path), Path::new(&output_path), fmt)
        .map_err(|e| e.to_string())?;
    Ok(output_path)
}

/// Platform-correct base URL for the `lumina://` transport, e.g.
/// `http://lumina.localhost` on Windows.
#[tauri::command]
pub fn get_protocol_base() -> String {
    protocol::protocol_base()
}

fn parse_format(name: &str) -> Result<image::ImageFormat, AppError> {
    match name.to_ascii_lowercase().as_str() {
        "png" => Ok(image::ImageFormat::Png),
        "jpg" | "jpeg" => Ok(image::ImageFormat::Jpeg),
        "gif" => Ok(image::ImageFormat::Gif),
        "webp" => Ok(image::ImageFormat::WebP),
        "bmp" => Ok(image::ImageFormat::Bmp),
        "tif" | "tiff" => Ok(image::ImageFormat::Tiff),
        "ico" => Ok(image::ImageFormat::Ico),
        other => Err(AppError::Unsupported(other.to_string())),
    }
}
