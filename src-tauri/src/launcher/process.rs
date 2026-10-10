use std::env;
use std::path::PathBuf;
use std::time::Duration;

#[cfg(target_os = "windows")]
use std::ffi::OsStr;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::process::Command;
#[cfg(target_os = "windows")]
use std::thread;
#[cfg(target_os = "windows")]
use std::time::Instant;

const CODEX_STOP_TIMEOUT: Duration = Duration::from_secs(10);
const OPENCODE_STOP_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(target_os = "windows")]
const CODEX_STOP_STABLE_WINDOW: Duration = Duration::from_millis(750);
#[cfg(target_os = "windows")]
const CODEX_START_TIMEOUT: Duration = Duration::from_secs(8);

mod identity;
pub(crate) use identity::is_codex_process;
#[cfg(any(target_os = "windows", test))]
pub(super) use identity::process_stop_is_stable;
#[cfg(test)]
pub(super) use identity::running_target_pids;
#[cfg(target_os = "windows")]
use identity::wait_for_codex_state;
#[cfg(not(target_os = "windows"))]
pub(super) use identity::wait_for_pids_exit;
pub(super) use identity::{codex_process_pids, codex_process_system};
#[cfg(target_os = "windows")]
use identity::{codex_process_pids_for, opencode_process_pids_for};
#[cfg(all(test, target_os = "windows"))]
pub(super) use identity::{is_codex_process_identity, is_opencode_process_identity};
use identity::{is_opencode_process, opencode_process_pids};

pub(super) fn is_codex_running() -> bool {
    let system = codex_process_system();
    system.processes().values().any(is_codex_process)
}

pub(super) fn stop_codex_and_wait() -> Result<bool, String> {
    let pids = codex_process_pids();
    if pids.is_empty() {
        return Ok(false);
    }

    #[cfg(target_os = "windows")]
    {
        stop_codex_windows(&pids, CODEX_STOP_TIMEOUT)?;
        Ok(true)
    }

    #[cfg(not(target_os = "windows"))]
    {
        stop_codex_processes(&pids);
        if wait_for_pids_exit(&pids, CODEX_STOP_TIMEOUT) {
            Ok(true)
        } else {
            Err("ChatGPT did not exit before the profile switch timeout".to_string())
        }
    }
}

pub(super) fn is_opencode_running() -> bool {
    let system = codex_process_system();
    system.processes().values().any(is_opencode_process)
}

pub(super) fn stop_opencode_and_wait() -> Result<bool, String> {
    let pids = opencode_process_pids();
    if pids.is_empty() {
        return Ok(false);
    }

    #[cfg(target_os = "windows")]
    {
        stop_opencode_windows(&pids, OPENCODE_STOP_TIMEOUT)?;
        Ok(true)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let system = codex_process_system();
        for process in system
            .processes()
            .values()
            .filter(|process| pids.contains(&process.pid().as_u32()))
        {
            let _ = process.kill();
        }
        if wait_for_pids_exit(&pids, OPENCODE_STOP_TIMEOUT) {
            Ok(true)
        } else {
            Err("OpenCode did not exit before the restart timeout".to_string())
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn stop_codex_processes(pids: &[u32]) {
    let system = codex_process_system();
    for process in system
        .processes()
        .values()
        .filter(|process| pids.contains(&process.pid().as_u32()))
    {
        let _ = process.kill();
    }
}

#[cfg(target_os = "windows")]
pub(super) fn stop_codex_windows(initial_pids: &[u32], timeout: Duration) -> Result<(), String> {
    stop_windows_processes(initial_pids, timeout, codex_process_pids_for, "ChatGPT")
}

#[cfg(target_os = "windows")]
fn stop_opencode_windows(initial_pids: &[u32], timeout: Duration) -> Result<(), String> {
    stop_windows_processes(initial_pids, timeout, opencode_process_pids_for, "OpenCode")
}

#[cfg(target_os = "windows")]
fn stop_windows_processes(
    initial_pids: &[u32],
    timeout: Duration,
    current_pids: fn(&[u32]) -> Vec<u32>,
    product: &str,
) -> Result<(), String> {
    let started = Instant::now();
    let mut signaled = Vec::new();
    let mut forced = Vec::new();
    let mut empty_since = None;

    for pid in initial_pids {
        signal_windows_process(*pid, false);
        signaled.push(*pid);
    }

    loop {
        // Probe only those exact main-process PIDs instead of enumerating every
        // process on the machine on each 100 ms stop-loop iteration. The
        // signal deliberately does not use taskkill's `/T`: a desktop Codex
        // process can own unrelated children (for example the Codex
        // app-server used by another host), and killing that whole tree can
        // terminate Relay or another user's process.
        let running = current_pids(initial_pids);
        let now = Instant::now();
        let elapsed = now.duration_since(started);
        if elapsed >= timeout {
            return Err(format!("{product} did not exit before the restart timeout"));
        }
        if process_stop_is_stable(
            !running.is_empty(),
            &mut empty_since,
            now,
            CODEX_STOP_STABLE_WINDOW,
        ) {
            return Ok(());
        }

        let force = elapsed >= Duration::from_secs(2);
        for pid in running {
            let attempted = if force { &mut forced } else { &mut signaled };
            if !attempted.contains(&pid) {
                signal_windows_process(pid, force);
                attempted.push(pid);
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(target_os = "windows")]
fn signal_windows_process(pid: u32, force: bool) {
    let mut command = windows_hidden_command("taskkill");
    command.args(windows_taskkill_arguments(pid, force));
    let _ = command.status();
}

#[cfg(target_os = "windows")]
pub(super) fn windows_taskkill_arguments(pid: u32, force: bool) -> Vec<String> {
    let mut arguments = vec!["/PID".to_string(), pid.to_string()];
    if force {
        arguments.push("/F".to_string());
    }
    arguments
}

pub(super) fn launch_codex_checked() -> Result<(), String> {
    // Opening an already running desktop app is a no-op. Besides avoiding a
    // duplicate process, this prevents Chromium from reinitializing its
    // profile and touching the large on-disk cache on every click.
    if is_codex_running() {
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        launch_codex_desktop()
    }

    #[cfg(target_os = "macos")]
    {
        for app in ["ChatGPT", "Codex"] {
            if Command::new("open")
                .args(["-a", app])
                .status()
                .is_ok_and(|status| status.success())
            {
                return Ok(());
            }
        }
        Err("ChatGPT desktop was not found. Install the desktop app; the terminal CLI cannot be opened from Relay without a terminal".to_string())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Err("ChatGPT desktop launch is unavailable on this platform. Open a supported desktop app manually; Relay will not start the terminal CLI in the background".to_string())
    }
}

pub(super) fn find_command_on_path(command_name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH").or_else(|| env::var_os("Path"))?;
    let names = if cfg!(target_os = "windows") {
        vec![
            command_name.to_string(),
            format!("{command_name}.exe"),
            format!("{command_name}.cmd"),
            format!("{command_name}.bat"),
        ]
    } else {
        vec![command_name.to_string()]
    };
    env::split_paths(&paths)
        .flat_map(|directory| names.iter().map(move |entry| directory.join(entry)))
        .find(|candidate| candidate.is_file())
}

#[cfg(target_os = "windows")]
fn launch_codex_desktop() -> Result<(), String> {
    let mut last_error = None;
    for target in windows_chatgpt_launch_targets() {
        match windows_hidden_command("explorer.exe").arg(&target).spawn() {
            Ok(_) if wait_for_codex_state(true, CODEX_START_TIMEOUT) => return Ok(()),
            Ok(_) => last_error = Some(format!("ChatGPT did not start via {target}")),
            Err(error) => last_error = Some(error.to_string()),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        "ChatGPT was not found in Windows installed apps. Repair or reinstall ChatGPT, then try again.".to_string()
    }))
}

#[cfg(target_os = "windows")]
fn windows_chatgpt_launch_targets() -> Vec<String> {
    let command_result = windows_hidden_command("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$OutputEncoding=[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); Get-StartApps | Where-Object { ($_.Name -eq 'ChatGPT' -or $_.Name -eq 'Codex') -and $_.AppID -like 'OpenAI.*!*' } | ForEach-Object { \"$($_.Name)`t$($_.AppID)\" }",
        ])
        .output()
        .ok();
    command_result
        .filter(|command_result| command_result.status.success())
        .map(|command_result| {
            parse_windows_start_apps_output(&String::from_utf8_lossy(&command_result.stdout))
        })
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
pub(super) fn parse_windows_start_apps_output(command_output: &str) -> Vec<String> {
    let mut targets = command_output
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter_map(|(display_name, app_id)| {
            let display_name = display_name.trim();
            let app_id = app_id.trim();
            let valid_name = display_name.eq_ignore_ascii_case("ChatGPT")
                || display_name.eq_ignore_ascii_case("Codex");
            let valid_id = app_id.len() <= 256
                && app_id.starts_with("OpenAI.")
                && app_id.contains('!')
                && app_id.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'!')
                });
            (valid_name && valid_id).then(|| {
                (
                    !display_name.eq_ignore_ascii_case("ChatGPT"),
                    format!(r"shell:AppsFolder\{app_id}"),
                )
            })
        })
        .collect::<Vec<_>>();
    targets.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    targets.dedup_by(|left, right| left.1.eq_ignore_ascii_case(&right.1));
    targets.into_iter().map(|(_, target)| target).collect()
}

#[cfg(target_os = "windows")]
pub(super) fn windows_hidden_command(program: impl AsRef<OsStr>) -> Command {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}
