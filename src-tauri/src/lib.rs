//! Lumina — lightweight cross-platform image viewer.
//!
//! Module map:
//! - [`imaging`]  : UI-agnostic decode / metadata / thumbnail / convert pipeline
//! - [`commands`] : Tauri IPC commands (the frontend contract)
//! - [`protocol`] : custom `lumina://` URI scheme used to stream image bytes
//! - [`error`]    : shared typed error

mod commands;
mod error;
mod imaging;
mod protocol;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .register_uri_scheme_protocol(protocol::SCHEME, |ctx, request| {
            protocol::handle(ctx, request)
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_image_metadata,
            commands::list_images,
            commands::get_thumbnail,
            commands::get_image_bytes,
            commands::save_image_as,
            commands::get_protocol_base,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
