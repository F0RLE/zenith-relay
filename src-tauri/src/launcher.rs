mod opencode_desktop;
mod process;

pub(crate) use process::is_codex_process;
use process::launch_codex_checked;

pub fn launch_codex() -> String {
    launch_codex_checked()
        .map(|_| "ChatGPT запущен.".to_string())
        .unwrap_or_else(|error| format!("Ключ сохранен, но ChatGPT не запустился: {error}"))
}

pub fn launch_codex_with_profile() -> Result<(), String> {
    launch_codex_checked()
}

/// Restart OpenCode after changing its global configuration. OpenCode's
/// desktop sidecar snapshots the provider catalog at startup, and launching
/// a second instance only focuses the existing single-instance process.
pub fn restart_opencode() -> Result<(), String> {
    opencode_desktop::restart_opencode()
}

pub fn is_codex_running() -> bool {
    process::is_codex_running()
}

pub fn stop_codex_and_wait() -> Result<bool, String> {
    process::stop_codex_and_wait()
}
#[cfg(test)]
mod tests;
