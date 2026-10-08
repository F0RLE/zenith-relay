use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};
use sysinfo::{ProcessesToUpdate, System};

#[cfg(not(target_os = "windows"))]
use crate::launcher::opencode_desktop::is_macos_app_executable;
#[cfg(target_os = "windows")]
use sysinfo::Pid;

pub(in crate::launcher) fn codex_process_system() -> System {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    system
}

pub(in crate::launcher) fn codex_process_pids() -> Vec<u32> {
    let system = codex_process_system();
    system
        .processes()
        .values()
        .filter(|process| is_codex_process(process))
        .map(|process| process.pid().as_u32())
        .collect()
}

pub(super) fn opencode_process_pids() -> Vec<u32> {
    let system = codex_process_system();
    system
        .processes()
        .values()
        .filter(|process| is_opencode_process(process))
        .map(|process| process.pid().as_u32())
        .collect()
}

#[cfg(target_os = "windows")]
pub(super) fn codex_process_pids_for(targets: &[u32]) -> Vec<u32> {
    matching_process_pids(targets, is_codex_process)
}

#[cfg(target_os = "windows")]
pub(super) fn opencode_process_pids_for(targets: &[u32]) -> Vec<u32> {
    matching_process_pids(targets, is_opencode_process)
}

#[cfg(target_os = "windows")]
fn matching_process_pids(targets: &[u32], matcher: fn(&sysinfo::Process) -> bool) -> Vec<u32> {
    if targets.is_empty() {
        return Vec::new();
    }
    let pids = targets
        .iter()
        .copied()
        .map(Pid::from_u32)
        .collect::<Vec<_>>();
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&pids), true);
    system
        .processes()
        .values()
        .filter(|process| matcher(process))
        .map(|process| process.pid().as_u32())
        .collect()
}

#[cfg(any(not(target_os = "windows"), test))]
pub(in crate::launcher) fn running_target_pids(targets: &[u32]) -> Vec<u32> {
    let system = codex_process_system();
    system
        .processes()
        .keys()
        .map(|pid| pid.as_u32())
        .filter(|pid| targets.contains(pid))
        .collect()
}

pub(crate) fn is_codex_process(process: &sysinfo::Process) -> bool {
    let process_name = process.name().to_string_lossy();
    let executable = process.exe();
    let command = process
        .cmd()
        .iter()
        .map(|command_argument| command_argument.to_string_lossy())
        .collect::<Vec<_>>();
    is_codex_process_identity(&process_name, executable, &command)
}

pub(super) fn is_opencode_process(process: &sysinfo::Process) -> bool {
    let process_name = process.name().to_string_lossy();
    let executable = process.exe();
    let command = process
        .cmd()
        .iter()
        .map(|command_argument| command_argument.to_string_lossy())
        .collect::<Vec<_>>();
    is_opencode_process_identity(&process_name, executable, &command)
}

pub(in crate::launcher) fn is_opencode_process_identity(
    process_name: &str,
    executable: Option<&Path>,
    command: &[impl AsRef<str>],
) -> bool {
    #[cfg(target_os = "windows")]
    {
        // Chromium desktop wrappers create crashpad/GPU/renderer children with
        // the same executable; those helpers are not the app itself.
        if command
            .iter()
            .any(|command_argument| command_argument.as_ref().starts_with("--type="))
        {
            return false;
        }
        [
            "opencode.exe",
            "opencode-desktop.exe",
            "OpenCode Dev.exe",
            "OpenCode Beta.exe",
        ]
        .iter()
        .any(|candidate| process_name.eq_ignore_ascii_case(candidate))
            && executable
                .is_some_and(super::super::opencode_desktop::is_windows_opencode_desktop_path)
    }

    #[cfg(not(target_os = "windows"))]
    {
        !command
            .iter()
            .any(|command_argument| command_argument.as_ref().starts_with("--type="))
            && executable.is_some_and(|path| {
                if cfg!(target_os = "macos") {
                    process_name.eq_ignore_ascii_case("OpenCode")
                        && super::super::opencode_desktop::is_macos_opencode_desktop_path(path)
                } else {
                    super::super::opencode_desktop::is_linux_opencode_desktop_path(path)
                        && ["opencode", "opencode-desktop", "ai.opencode.desktop"]
                            .iter()
                            .any(|candidate| process_name.eq_ignore_ascii_case(candidate))
                }
            })
    }
}

pub(in crate::launcher) fn is_codex_process_identity(
    process_name: &str,
    executable: Option<&Path>,
    command: &[impl AsRef<str>],
) -> bool {
    #[cfg(target_os = "windows")]
    {
        let helper = command
            .iter()
            .any(|command_argument| command_argument.as_ref().starts_with("--type="));
        if helper {
            return false;
        }
        let path = executable
            .map(|executable_path| executable_path.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        // Keep this exclusion explicit. Relay may launch or own a process
        // tree that contains a compatible executable name in the future, but
        // profile switching must never classify Relay itself as Codex.
        if path.ends_with("\\zenith relay.exe") {
            return false;
        }
        if process_name.eq_ignore_ascii_case("ChatGPT.exe") {
            return path.contains("openai.codex_")
                || path.contains("openai.chatgpt_")
                || path.contains("\\chatgpt\\")
                || path.contains("\\codex\\");
        }
        // Do not match the standalone Codex CLI (`...\\OpenAI\\Codex\\bin\\codex.exe`):
        // profile switching must never terminate the active Relay/Codex task.
        let packaged_desktop = path.contains("\\windowsapps\\openai.")
            || path.contains("\\program files\\chatgpt\\")
            || path.contains("\\program files\\codex\\");
        (process_name.eq_ignore_ascii_case("OpenAI.Codex.exe")
            || (process_name.eq_ignore_ascii_case("Codex.exe") && packaged_desktop))
            && !path.contains("\\resources\\codex.exe")
    }

    #[cfg(not(target_os = "windows"))]
    {
        cfg!(target_os = "macos")
            && !command
                .iter()
                .any(|command_argument| command_argument.as_ref().starts_with("--type="))
            && executable.is_some_and(|path| {
                (process_name.eq_ignore_ascii_case("ChatGPT")
                    && is_macos_app_executable(path, "chatgpt", "chatgpt"))
                    || (process_name.eq_ignore_ascii_case("Codex")
                        && is_macos_app_executable(path, "codex", "codex"))
            })
    }
}

#[cfg(target_os = "windows")]
pub(super) fn wait_for_codex_state(running: bool, timeout: Duration) -> bool {
    let started = Instant::now();
    loop {
        if super::is_codex_running() == running {
            return true;
        }
        if started.elapsed() >= timeout {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(not(target_os = "windows"))]
pub(in crate::launcher) fn wait_for_pids_exit(pids: &[u32], timeout: Duration) -> bool {
    let started = Instant::now();
    loop {
        if running_target_pids(pids).is_empty() {
            return true;
        }
        if started.elapsed() >= timeout {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(any(target_os = "windows", test))]
pub(in crate::launcher) fn process_stop_is_stable(
    processes_running: bool,
    empty_since: &mut Option<Instant>,
    now: Instant,
    stable_window: Duration,
) -> bool {
    if processes_running {
        *empty_since = None;
        return false;
    }
    let since = empty_since.get_or_insert(now);
    now.duration_since(*since) >= stable_window
}
