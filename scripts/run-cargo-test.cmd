@echo off
set CARGO_HOME=D:\DevEnv\Runtimes\rust\.cargo
cd /d d:\CodeProjects\dev-team-test\src-tauri
cargo test --all-targets %*
