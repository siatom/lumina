//! Lumina — lightweight cross-platform image viewer.
//!
//! Module map:
//! - [`imaging`]     : UI-agnostic decode / metadata / thumbnail / convert pipeline
//! - [`commands`]    : Tauri IPC commands (the frontend contract)
//! - [`protocol`]    : custom `lumina://` URI scheme used to stream image bytes
//! - [`cache`]       : process-wide LRU for generated thumbnails
//! - [`watcher`]     : filesystem watch on the browsed folder
//! - [`file_assoc`]  : operating-system file association + per-format icons
//! - [`error`]       : shared typed error

mod cache;
mod commands;
mod error;
mod file_assoc;
mod imaging;
mod protocol;
mod watcher;

/// Format × result verification matrix (real fixtures on disk).
#[cfg(test)]
mod format_matrix;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol(protocol::SCHEME, |ctx, request, responder| {
            protocol::handle(ctx, request, responder)
        })
        .manage(watcher::WatcherState::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_image_metadata,
            commands::list_images,
            commands::get_thumbnail,
            commands::get_image_bytes,
            commands::save_image_as,
            commands::transform_image,
            commands::get_image_pages,
            commands::get_image_frame,
            commands::rename_file,
            commands::delete_file,
            commands::watch_directory,
            commands::unwatch_directory,
            commands::get_association_status,
            commands::set_file_association,
            commands::open_default_apps_settings,
            commands::get_protocol_base,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
