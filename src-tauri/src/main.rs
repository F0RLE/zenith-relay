#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod codex_config;
mod desktop;
mod diagnostics;
mod files;
mod key_storage;
mod launcher;
mod local_pool;
mod platform;
mod portable_update;
mod storage_paths;
#[cfg(test)]
mod tests;
mod tray;

fn main() {
    if let Some(code) = diagnostics::transport::run_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = local_pool::accounts::proxy::diagnose::run_if_requested() {
        std::process::exit(code);
    }
    diagnostics::install_panic_hook();
    portable_update::run_helper_if_requested();
    desktop::run();
}
