//! Tauri IPC commands — the contract consumed by the frontend.
//!
//! Every command returns `Result<T, String>` so failures surface as rejected
//! promises in JS with a readable message. See `docs/INTERFACES.md` for the
//! full, authoritative description of each command and its TypeScript types.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, State};

use crate::file_assoc::{self, AssocStatus};
use crate::imaging::{self, DirectoryListing, ImageMetadata, TransformSpec};
use crate::protocol;
use crate::watcher::WatcherState;

/// Metadata for a single image: dimensions, format, size and EXIF.
///
/// `path` is the *original* file path; the frontend turns it into a display URL
/// via the `lumina://` protocol (see [`get_protocol_base`]).
///
/// Runs on the async runtime (`async`), not the main thread, so header reads
/// and EXIF parsing never freeze the UI.
#[tauri::command(async)]
pub fn get_image_metadata(path: String) -> Result<ImageMetadata, String> {
    imaging::read_metadata(Path::new(&path)).map_err(|e| e.to_string())
}

/// List the sibling images in `dir` (naturally sorted) plus the index of
/// `current`, so the frontend can implement previous/next navigation.
#[tauri::command(async)]
pub fn list_images(dir: String, current: Option<String>) -> Result<DirectoryListing, String> {
    let current = current.map(PathBuf::from);
    imaging::list_directory(Path::new(&dir), current.as_deref()).map_err(|e| e.to_string())
}

/// Downscaled PNG thumbnail (fits inside `max_size` on both axes).
///
/// Returned as raw bytes (`tauri::ipc::Response`) so the frontend receives an
/// `ArrayBuffer`, not a base64 string.
#[tauri::command(async)]
pub fn get_thumbnail(path: String, max_size: Option<u32>) -> Result<tauri::ipc::Response, String> {
    let max = max_size.unwrap_or(256).clamp(16, 4096);
    let bytes = imaging::make_thumbnail(Path::new(&path), max).map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Raw bytes for the original view. Same payload the `lumina://` protocol
/// serves; used as a fallback when the protocol is unavailable.
#[tauri::command(async)]
pub fn get_image_bytes(path: String) -> Result<tauri::ipc::Response, String> {
    let (bytes, _mime) = imaging::read_for_display(Path::new(&path)).map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Save/convert an image without geometry changes.
///
/// `format` is optional; when omitted the output format is inferred from the
/// `output_path` extension. `quality` (`1..=100`) only affects JPEG. Returns
/// the written path.
#[tauri::command(async)]
pub fn save_image_as(
    input_path: String,
    output_path: String,
    format: Option<String>,
    quality: Option<u8>,
) -> Result<String, String> {
    let fmt = format
        .as_deref()
        .map(imaging::parse_format)
        .transpose()
        .map_err(|e| e.to_string())?;
    imaging::convert_image(
        Path::new(&input_path),
        Path::new(&output_path),
        fmt,
        quality,
    )
    .map_err(|e| e.to_string())?;
    Ok(output_path)
}

/// Apply an edit (crop / flip / rotate / resize) and write the result to
/// `output_path`. See [`TransformSpec`] for the field contract and the fixed
/// operation order (crop → flip → rotate → resize).
#[tauri::command(async)]
pub fn transform_image(
    input_path: String,
    output_path: String,
    options: TransformSpec,
) -> Result<String, String> {
    imaging::transform_image(Path::new(&input_path), Path::new(&output_path), &options)
        .map_err(|e| e.to_string())?;
    Ok(output_path)
}

/// Number of pages / frames (GIF, animated WebP, multi-page TIFF). Single
/// image formats report `1`.
#[tauri::command(async)]
pub fn get_image_pages(path: String) -> Result<u32, String> {
    imaging::page_count(Path::new(&path)).map_err(|e| e.to_string())
}

/// Extract one page / frame as a PNG (`ArrayBuffer`).
#[tauri::command(async)]
pub fn get_image_frame(path: String, index: u32) -> Result<tauri::ipc::Response, String> {
    let bytes = imaging::read_frame_png(Path::new(&path), index).map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Rename a file in place; returns the new absolute path. `new_name` must be a
/// plain file name (no path separators) and must not already exist.
#[tauri::command]
pub fn rename_file(path: String, new_name: String) -> Result<String, String> {
    let renamed = imaging::rename_file(Path::new(&path), &new_name).map_err(|e| e.to_string())?;
    Ok(renamed.to_string_lossy().into_owned())
}

/// Delete a single file. The backend refuses directories and symlinks; the UI
/// is responsible for asking the user to confirm first.
#[tauri::command]
pub fn delete_file(path: String) -> Result<(), String> {
    imaging::delete_file(Path::new(&path)).map_err(|e| e.to_string())
}

/// Watch `dir` for image changes, replacing any directory watched before.
///
/// Nothing is returned inline: changes arrive as `directory-changed` events
/// carrying the watched directory, and the frontend answers each one by
/// re-listing. Re-watching the current directory is a no-op.
#[tauri::command]
pub fn watch_directory(
    app: AppHandle,
    watcher: State<'_, WatcherState>,
    dir: String,
) -> Result<(), String> {
    watcher.watch(&app, &dir)
}

/// Stop watching. Safe to call when nothing is watched.
#[tauri::command]
pub fn unwatch_directory(watcher: State<'_, WatcherState>) -> Result<(), String> {
    watcher.unwatch()
}

/// Platform-correct base URL for the `lumina://` transport, e.g.
/// `http://lumina.localhost` on Windows.
#[tauri::command]
pub fn get_protocol_base() -> String {
    protocol::protocol_base()
}

/// Return the image path passed by the operating system when Lumina was
/// launched through a file association.
///
/// Explorer invokes the registered command as `Lumina.exe "%1"`. Returning
/// only an existing, supported file keeps unrelated process arguments (for
/// example development flags) from being treated as an image path.
#[tauri::command]
pub fn get_launch_path() -> Option<String> {
    std::env::args_os().skip(1).find_map(|arg| {
        let path = PathBuf::from(arg);
        if path.is_file() && imaging::is_supported(&path) {
            Some(path.to_string_lossy().into_owned())
        } else {
            None
        }
    })
}

/* --------------------------- file association ---------------------------- */

/// Current operating-system association state for every format Lumina opens.
///
/// Also carries the platform's capability flags and a ready-to-show note, so
/// the settings panel never has to hard-code what Windows/macOS/Linux allow.
#[tauri::command(async)]
pub fn get_association_status() -> Result<AssocStatus, String> {
    Ok(file_assoc::status())
}

/// Register or withdraw Lumina as a handler for `exts`.
///
/// `exts` is the exact set to change rather than the desired end state: the
/// settings panel sends only the rows the user actually toggled. Returns the
/// refreshed status so the panel does not need a second round-trip.
#[tauri::command(async)]
pub fn set_file_association(
    app: AppHandle,
    exts: Vec<String>,
    enabled: bool,
) -> Result<AssocStatus, String> {
    file_assoc::set_enabled(&app, &exts, enabled).map_err(|e| e.to_string())
}

/// Open the operating system's "default apps" page.
///
/// Windows-only: it is the one place the user can confirm Lumina as the default
/// handler, which Windows 10/11 will not let an application do on its own.
#[tauri::command]
pub fn open_default_apps_settings() -> Result<(), String> {
    file_assoc::open_default_apps_settings().map_err(|e| e.to_string())
}
