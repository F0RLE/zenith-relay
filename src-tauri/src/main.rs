#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod codex_config;
mod files;
mod key_storage;
mod launcher;
mod local_pool;
mod platform;
mod portable_update;
mod ready_api;
mod tray;

fn main() {
    portable_update::run_helper_if_requested();
    ready_api::run();
}
