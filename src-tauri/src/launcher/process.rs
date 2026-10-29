use std::env;
use std::path::{Path, PathBuf};
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
use identity::claude_desktop_pids;
#[cfg(target_os = "windows")]
use identity::claude_desktop_pids_for;
pub(super) use identity::is_claude_desktop_process;
#[cfg(test)]
pub(super) use identity::is_claude_desktop_process_identity;
pub(crate) use identity::is_codex_process;
pub(super) use identity::is_codex_process_identity;
#[cfg(all(test, target_os = "windows"))]
pub(super) use identity::is_opencode_process_identity;
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

const CLAUDE_STOP_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn stop_claude_desktop_and_wait() -> Result<bool, String> {
    let pids = claude_desktop_pids();
    if pids.is_empty() {
        return Ok(false);
    }

    #[cfg(target_os = "windows")]
    {
        stop_windows_processes(
            &pids,
            CLAUDE_STOP_TIMEOUT,
            claude_desktop_pids_for,
            "Claude Desktop",
        )?;
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
        if wait_for_pids_exit(&pids, CLAUDE_STOP_TIMEOUT) {
            Ok(true)
        } else {
            Err("Claude Desktop did not exit before the restart timeout".to_string())
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
    let mut last_signal_failure = None;

    for pid in initial_pids {
        if let Some(failure) = signal_windows_process(*pid, false) {
            last_signal_failure = Some(failure);
        }
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
            let message = format!("{product} did not exit before the restart timeout");
            let mut details = vec![("product", product.to_string())];
            if let Some(failure) = last_signal_failure {
                details.push(("last_signal_failure", failure));
            }
            crate::diagnostics::record_error(
                "launcher-stop",
                Some("process_stop_timeout"),
                &message,
                &details,
            );
            return Err(message);
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
                if let Some(failure) = signal_windows_process(pid, force) {
                    last_signal_failure = Some(failure);
                }
                attempted.push(pid);
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(target_os = "windows")]
fn signal_windows_process(pid: u32, force: bool) -> Option<String> {
    let mut command = windows_hidden_command("taskkill");
    command.args(windows_taskkill_arguments(pid, force));
    // taskkill uses localized console output that is not necessarily UTF-8.
    // Capture both streams without decoding them; the stop loop still checks
    // process identity and exit, so a rejected graceful signal is not fatal.
    windows_taskkill_failure(command.output())
        .map(|failure| format!("PID {pid}, force={force}: {failure}"))
}

#[cfg(target_os = "windows")]
pub(super) fn windows_taskkill_failure(
    result: std::io::Result<std::process::Output>,
) -> Option<String> {
    match result {
        Ok(output) if output.status.success() => None,
        Ok(output) => Some(format!("taskkill exit code: {:?}", output.status.code())),
        Err(error) => Some(format!(
            "taskkill I/O error: {:?} (OS code: {:?})",
            error.kind(),
            error.raw_os_error()
        )),
    }
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
    let executable = super::settings::configured_path(super::settings::Application::Chatgpt)?;
    // Opening an already running desktop app is a no-op. Besides avoiding a
    // duplicate process, this prevents Chromium from reinitializing its
    // profile and touching the large on-disk cache on every click.
    if is_codex_running() {
        return Ok(());
    }

    if let Some(executable) = executable {
        return launch_configured_chatgpt(&executable);
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

fn launch_configured_chatgpt(executable: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        windows_hidden_command(executable)
            .spawn()
            .map_err(|_| "Could not start the selected ChatGPT desktop application".to_string())?;
        if wait_for_codex_state(true, CODEX_START_TIMEOUT) {
            Ok(())
        } else {
            Err("The selected ChatGPT desktop application did not start".to_string())
        }
    }
    #[cfg(target_os = "macos")]
    {
        let bundle = executable
            .ancestors()
            .nth(3)
            .ok_or("Invalid ChatGPT application path")?;
        Command::new("open")
            .arg("-a")
            .arg(bundle)
            .status()
            .map_err(|_| "Could not start the selected ChatGPT desktop application".to_string())?
            .success()
            .then_some(())
            .ok_or_else(|| "The selected ChatGPT desktop application did not start".to_string())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = executable;
        Err("ChatGPT desktop launch is unavailable on this platform".to_string())
    }
}

pub(super) fn detected_chatgpt_launch_path() -> Option<String> {
    if let Some(path) = codex_process_system()
        .processes()
        .values()
        .filter(|process| is_codex_process(process))
        .find_map(|process| process.exe())
    {
        return Some(path.to_string_lossy().into_owned());
    }
    #[cfg(target_os = "windows")]
    {
        windows_chatgpt_launch_targets().into_iter().next()
    }
    #[cfg(target_os = "macos")]
    {
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Applications"));
        }
        roots
            .into_iter()
            .flat_map(|root| {
                ["ChatGPT", "Codex"]
                    .map(|name| root.join(format!("{name}.app/Contents/MacOS/{name}")))
            })
            .find(|path| path.is_file())
            .map(|path| path.to_string_lossy().into_owned())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

pub(super) fn find_commands_on_path(paths: &std::ffi::OsStr, command_name: &str) -> Vec<PathBuf> {
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
    env::split_paths(paths)
        .flat_map(|directory| names.iter().map(move |entry| directory.join(entry)))
        .filter(|candidate| candidate.is_file())
        .collect()
}

pub(super) fn wait_for_application_start(
    mut running: impl FnMut() -> bool,
    timeout: Duration,
    product: &str,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if running() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "{product} did not start before the launch timeout; the saved profile was kept"
            ));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
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
