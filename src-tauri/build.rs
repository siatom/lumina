fn main() {
    // Re-embed the icons when they change (otherwise the exe keeps the old ones).
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
