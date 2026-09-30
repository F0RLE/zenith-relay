#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod codex_config;
mod diagnostics;
mod files;
mod key_storage;
mod launcher;
mod local_pool;
mod platform;
mod portable_update;
mod ready_api;
mod storage_paths;
mod tray;

fn main() {
    diagnostics::install_panic_hook();
    portable_update::run_helper_if_requested();
    ready_api::run();
}
