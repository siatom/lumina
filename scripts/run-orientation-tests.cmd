@echo off
set CARGO_HOME=D:\DevEnv\Runtimes\rust\.cargo
cd /d d:\CodeProjects\dev-team-test\src-tauri
cargo test --all-targets orientation -- --nocapture
cargo test --all-targets convert_and_transform_use_oriented_pixels -- --nocapture
cargo test --all-targets webview_native_passthrough -- --nocapture
