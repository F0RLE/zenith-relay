use std::{path::PathBuf, process::Command, thread, time::Duration};

#[cfg(target_os = "windows")]
use std::ffi::OsStr;

use crate::codex_config::load_api_key_for_launch;
use sysinfo::{ProcessesToUpdate, System};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const CODEX_PROCESS_NAMES: &[&str] = &[
    "codex",
    "codex.exe",
    "Codex",
    "Codex.exe",
    "OpenAI.Codex.exe",
];

pub fn launch_codex() -> String {
    launch_codex_checked(true)
        .map(|_| "Codex запущен.".to_string())
        .unwrap_or_else(|error| format!("Ключ сохранен, но Codex не запустился: {error}"))
}

pub fn launch_codex_with_profile() -> Result<(), String> {
    if is_codex_running() {
        stop_codex();
        thread::sleep(Duration::from_millis(600));
    }
    launch_codex_checked(false)
}

pub fn restart_codex_if_running() -> Option<String> {
    if !is_codex_running() {
        return None;
    }

    stop_codex();
    thread::sleep(Duration::from_millis(600));
    let _ = launch_codex();
    Some("Ключ сохранен.".to_string())
}

pub fn is_codex_running() -> bool {
    let system = codex_process_system();
    system.processes().values().any(is_codex_process)
}

fn stop_codex() {
    let system = codex_process_system();
    for process in system
        .processes()
        .values()
        .filter(|process| is_codex_process(process))
    {
        let _ = process.kill();
    }
}

fn launch_codex_checked(inject_saved_key: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        if launch_codex_desktop().is_ok() {
            return Ok(());
        }
        start_detached(PathBuf::from("codex"), inject_saved_key)
    }

    #[cfg(target_os = "macos")]
    {
        if Command::new("open").args(["-a", "Codex"]).spawn().is_ok() {
            return Ok(());
        }
        start_detached(PathBuf::from("codex"), inject_saved_key)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        start_detached(PathBuf::from("codex"), inject_saved_key)
    }
}

fn start_detached(path: PathBuf, inject_saved_key: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let mut command = windows_hidden_command(path);
        configure_launch_environment(&mut command, inject_saved_key);
        command.spawn().map(|_| ()).map_err(|err| err.to_string())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut command = Command::new(path);
        configure_launch_environment(&mut command, inject_saved_key);
        command.spawn().map(|_| ()).map_err(|err| err.to_string())
    }
}

fn configure_launch_environment(command: &mut Command, inject_saved_key: bool) {
    if inject_saved_key {
        if let Some(api_key) = load_api_key_for_launch() {
            command.env("OPENAI_API_KEY", api_key);
        }
        return;
    }
    command.env_remove("OPENAI_API_KEY");
    command.env_remove("OPENAI_BASE_URL");
}

#[cfg(target_os = "windows")]
fn launch_codex_desktop() -> Result<(), String> {
    windows_hidden_command("explorer.exe")
        .arg(r"shell:AppsFolder\OpenAI.Codex_2p2nqsd0c76g0!App")
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(target_os = "windows")]
fn windows_hidden_command(program: impl AsRef<OsStr>) -> Command {
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn codex_process_system() -> System {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    system
}

fn is_codex_process(process: &sysinfo::Process) -> bool {
    let name = process.name().to_string_lossy();
    CODEX_PROCESS_NAMES
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn account_profile_launch_clears_api_environment_overrides() {
        let mut command = Command::new("codex");
        command.env("OPENAI_API_KEY", "old-key");
        command.env("OPENAI_BASE_URL", "https://old.example/v1");
        configure_launch_environment(&mut command, false);
        let environment = command.get_envs().collect::<Vec<_>>();
        assert!(environment
            .iter()
            .any(|(key, value)| { *key == OsStr::new("OPENAI_API_KEY") && value.is_none() }));
        assert!(environment
            .iter()
            .any(|(key, value)| { *key == OsStr::new("OPENAI_BASE_URL") && value.is_none() }));
    }
}
