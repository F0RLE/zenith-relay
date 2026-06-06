use std::{ffi::OsStr, path::PathBuf, process::Command, thread, time::Duration};

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
    #[cfg(target_os = "windows")]
    {
        if launch_codex_desktop().is_ok() {
            return "Codex запущен.".to_string();
        }
    }

    #[cfg(target_os = "windows")]
    {
        return match start_detached(PathBuf::from("codex")) {
            Ok(_) => "Codex запущен.".to_string(),
            Err(err) => format!("Ключ сохранен, но Codex не запустился: {err}"),
        };
    }

    #[cfg(target_os = "macos")]
    {
        if Command::new("open").args(["-a", "Codex"]).spawn().is_ok() {
            return "Codex запущен.".to_string();
        }
        return match start_detached(PathBuf::from("codex")) {
            Ok(_) => "Codex запущен.".to_string(),
            Err(err) => format!("Ключ сохранен, но Codex не запустился: {err}"),
        };
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return match start_detached(PathBuf::from("codex")) {
            Ok(_) => "Codex запущен.".to_string(),
            Err(err) => format!("Ключ сохранен, но Codex не запустился: {err}"),
        };
    }
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

fn start_detached(path: PathBuf) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let mut command = windows_hidden_command(path);
        if let Some(api_key) = load_api_key_for_launch() {
            command.env("OPENAI_API_KEY", api_key);
        }
        return command.spawn().map(|_| ()).map_err(|err| err.to_string());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut command = Command::new(path);
        if let Some(api_key) = load_api_key_for_launch() {
            command.env("OPENAI_API_KEY", api_key);
        }
        command.spawn().map(|_| ()).map_err(|err| err.to_string())
    }
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
