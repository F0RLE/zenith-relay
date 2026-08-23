use std::{
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[cfg(not(target_os = "windows"))]
use std::path::PathBuf;

#[cfg(target_os = "windows")]
use std::ffi::OsStr;

#[cfg(any(not(target_os = "windows"), test))]
use crate::codex_config::load_api_key_for_launch;
use sysinfo::{ProcessesToUpdate, System};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(not(target_os = "windows"))]
const CODEX_PROCESS_NAMES: &[&str] = &[
    "ChatGPT",
    "ChatGPT.exe",
    "codex",
    "codex.exe",
    "Codex",
    "Codex.exe",
    "OpenAI.Codex.exe",
];
const CODEX_STOP_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(target_os = "windows")]
const CODEX_STOP_STABLE_WINDOW: Duration = Duration::from_millis(750);
#[cfg(target_os = "windows")]
const CODEX_START_TIMEOUT: Duration = Duration::from_secs(8);

pub fn launch_codex() -> String {
    launch_codex_checked(true)
        .map(|_| "ChatGPT запущен.".to_string())
        .unwrap_or_else(|error| format!("Ключ сохранен, но ChatGPT не запустился: {error}"))
}

pub fn launch_codex_with_profile() -> Result<(), String> {
    launch_codex_checked(false)
}

pub fn is_codex_running() -> bool {
    let system = codex_process_system();
    system.processes().values().any(is_codex_process)
}

pub fn stop_codex_and_wait() -> Result<bool, String> {
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

#[cfg(not(target_os = "windows"))]
fn stop_codex_processes(pids: &[u32]) {
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
fn stop_codex_windows(initial_pids: &[u32], timeout: Duration) -> Result<(), String> {
    let started = Instant::now();
    let mut signaled = Vec::new();
    let mut forced = Vec::new();
    let mut empty_since = None;

    for pid in initial_pids {
        signal_windows_process(*pid, false);
        signaled.push(*pid);
    }

    loop {
        let running = codex_process_pids();
        let now = Instant::now();
        let elapsed = now.duration_since(started);
        if elapsed >= timeout {
            return Err("ChatGPT did not exit before the profile switch timeout".to_string());
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
    command.args(["/PID", &pid.to_string(), "/T"]);
    if force {
        command.arg("/F");
    }
    let _ = command.status();
}

fn launch_codex_checked(inject_saved_key: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let _ = inject_saved_key;
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
        start_detached(resolve_codex_cli_path(), inject_saved_key)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        start_detached(resolve_codex_cli_path(), inject_saved_key)
    }
}

#[cfg(not(target_os = "windows"))]
fn resolve_codex_cli_path() -> PathBuf {
    if let Some(path) = find_command_on_path("codex") {
        return path;
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return PathBuf::from("codex");
    };
    for candidate in [
        home.join(".local/bin/codex"),
        home.join(".volta/bin/codex"),
        home.join(".asdf/shims/codex"),
    ] {
        if candidate.is_file() {
            return candidate;
        }
    }
    for (root, suffix) in [
        (home.join(".nvm/versions/node"), "bin/codex"),
        (
            home.join(".local/share/fnm/node-versions"),
            "installation/bin/codex",
        ),
        (home.join(".fnm/node-versions"), "installation/bin/codex"),
        (home.join(".asdf/installs/nodejs"), "bin/codex"),
    ] {
        if let Some(path) = newest_versioned_command(&root, suffix) {
            return path;
        }
    }
    PathBuf::from("codex")
}

#[cfg(not(target_os = "windows"))]
fn find_command_on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(not(target_os = "windows"))]
fn newest_versioned_command(root: &Path, suffix: &str) -> Option<PathBuf> {
    let mut candidates = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join(suffix))
        .filter(|candidate| candidate.is_file())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.pop()
}

#[cfg(not(target_os = "windows"))]
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

#[cfg(any(not(target_os = "windows"), test))]
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
    let mut last_error = None;
    for target in windows_chatgpt_launch_targets() {
        match windows_hidden_command("explorer.exe").arg(&target).spawn() {
            Ok(_) if wait_for_codex_state(true, CODEX_START_TIMEOUT) => return Ok(()),
            Ok(_) => last_error = Some(format!("ChatGPT did not start via {target}")),
            Err(error) => last_error = Some(error.to_string()),
        }
    }
    Err(last_error.unwrap_or_else(|| "ChatGPT desktop process did not start".to_string()))
}

#[cfg(target_os = "windows")]
fn windows_chatgpt_launch_targets() -> Vec<String> {
    let output = windows_hidden_command("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$OutputEncoding=[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); Get-StartApps | Where-Object { ($_.Name -eq 'ChatGPT' -or $_.Name -eq 'Codex') -and $_.AppID -like 'OpenAI.*!*' } | ForEach-Object { \"$($_.Name)`t$($_.AppID)\" }",
        ])
        .output()
        .ok();
    let mut targets = output
        .filter(|output| output.status.success())
        .map(|output| parse_windows_start_apps_output(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default();
    for fallback in [
        r"shell:AppsFolder\OpenAI.ChatGPT_2p2nqsd0c76g0!App",
        r"shell:AppsFolder\OpenAI.Codex_2p2nqsd0c76g0!App",
        "chatgpt:",
        "codex:",
    ] {
        if !targets.iter().any(|target| target == fallback) {
            targets.push(fallback.to_string());
        }
    }
    targets
}

#[cfg(target_os = "windows")]
fn parse_windows_start_apps_output(output: &str) -> Vec<String> {
    let mut targets = output
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter_map(|(name, app_id)| {
            let name = name.trim();
            let app_id = app_id.trim();
            let valid_name =
                name.eq_ignore_ascii_case("ChatGPT") || name.eq_ignore_ascii_case("Codex");
            let valid_id = app_id.len() <= 256
                && app_id.starts_with("OpenAI.")
                && app_id.contains('!')
                && app_id.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'!')
                });
            (valid_name && valid_id).then(|| {
                (
                    !name.eq_ignore_ascii_case("ChatGPT"),
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

fn codex_process_pids() -> Vec<u32> {
    let system = codex_process_system();
    system
        .processes()
        .values()
        .filter(|process| is_codex_process(process))
        .map(|process| process.pid().as_u32())
        .collect()
}

#[cfg(any(not(target_os = "windows"), test))]
fn running_target_pids(targets: &[u32]) -> Vec<u32> {
    let system = codex_process_system();
    system
        .processes()
        .keys()
        .map(|pid| pid.as_u32())
        .filter(|pid| targets.contains(pid))
        .collect()
}

fn is_codex_process(process: &sysinfo::Process) -> bool {
    let name = process.name().to_string_lossy();
    let executable = process.exe();
    let command = process
        .cmd()
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>();
    is_codex_process_identity(&name, executable, &command)
}

fn is_codex_process_identity(
    name: &str,
    executable: Option<&Path>,
    command: &[impl AsRef<str>],
) -> bool {
    #[cfg(target_os = "windows")]
    {
        let helper = command
            .iter()
            .any(|value| value.as_ref().starts_with("--type="));
        if helper {
            return false;
        }
        let path = executable
            .map(|value| value.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if name.eq_ignore_ascii_case("ChatGPT.exe") {
            return path.contains("openai.codex_")
                || path.contains("openai.chatgpt_")
                || path.contains("\\chatgpt\\")
                || path.contains("\\codex\\");
        }
        (name.eq_ignore_ascii_case("OpenAI.Codex.exe")
            || (name.eq_ignore_ascii_case("Codex.exe") && path.contains("openai")))
            && !path.contains("\\resources\\codex.exe")
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (executable, command);
        CODEX_PROCESS_NAMES
            .iter()
            .any(|candidate| name.eq_ignore_ascii_case(candidate))
    }
}

#[cfg(target_os = "windows")]
fn wait_for_codex_state(running: bool, timeout: Duration) -> bool {
    let started = Instant::now();
    loop {
        if is_codex_running() == running {
            return true;
        }
        if started.elapsed() >= timeout {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(not(target_os = "windows"))]
fn wait_for_pids_exit(pids: &[u32], timeout: Duration) -> bool {
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
fn process_stop_is_stable(
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

    #[test]
    fn target_pid_probe_ignores_unrelated_processes() {
        let current = std::process::id();
        assert_eq!(running_target_pids(&[current]), vec![current]);
        assert!(running_target_pids(&[u32::MAX]).is_empty());
    }

    #[test]
    fn stable_stop_wait_resets_when_process_reappears() {
        let started = Instant::now();
        let window = Duration::from_millis(500);
        let mut empty_since = None;

        assert!(!process_stop_is_stable(
            false,
            &mut empty_since,
            started,
            window
        ));
        assert!(!process_stop_is_stable(
            true,
            &mut empty_since,
            started + window,
            window
        ));
        assert!(!process_stop_is_stable(
            false,
            &mut empty_since,
            started + window,
            window
        ));
        assert!(process_stop_is_stable(
            false,
            &mut empty_since,
            started + window + window,
            window
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_store_codex_matches_only_the_desktop_root() {
        let executable = Path::new(
            r"C:\Program Files\WindowsApps\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe",
        );
        assert!(is_codex_process_identity(
            "ChatGPT.exe",
            Some(executable),
            &[""]
        ));
        assert!(!is_codex_process_identity(
            "ChatGPT.exe",
            Some(executable),
            &["--type=renderer"]
        ));
        assert!(!is_codex_process_identity(
            "codex.exe",
            Some(Path::new(r"C:\tools\codex.exe")),
            &["app-server"]
        ));
        assert!(is_codex_process_identity(
            "ChatGPT.exe",
            Some(Path::new(
                r"C:\Program Files\WindowsApps\OpenAI.ChatGPT_2.0.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe"
            )),
            &[""]
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn start_apps_parser_prefers_chatgpt_and_rejects_unrelated_apps() {
        let targets = parse_windows_start_apps_output(
            "Codex\tOpenAI.Codex_2p2nqsd0c76g0!App\nChatGPT\tOpenAI.ChatGPT_2p2nqsd0c76g0!App\nZenith Relay\tcom.zenith.codex\n",
        );
        assert_eq!(
            targets,
            vec![
                r"shell:AppsFolder\OpenAI.ChatGPT_2p2nqsd0c76g0!App",
                r"shell:AppsFolder\OpenAI.Codex_2p2nqsd0c76g0!App"
            ]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn detects_running_installed_codex_when_requested() {
        if std::env::var("ZENITH_TEST_RUNNING_CODEX").as_deref() == Ok("1") {
            assert!(is_codex_running(), "running Codex desktop was not detected");
        }
    }
}
