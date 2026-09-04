use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "windows")]
use std::ffi::OsStr;

#[cfg(any(not(target_os = "windows"), test))]
use crate::codex_config::load_api_key_for_launch;
#[cfg(target_os = "windows")]
use sysinfo::Pid;
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
#[cfg(not(target_os = "windows"))]
const OPENCODE_PROCESS_NAMES: &[&str] = &[
    "opencode",
    "opencode.exe",
    "OpenCode",
    "OpenCode.exe",
    "opencode-desktop",
    "opencode-desktop.exe",
];
const CODEX_STOP_TIMEOUT: Duration = Duration::from_secs(10);
const OPENCODE_STOP_TIMEOUT: Duration = Duration::from_secs(10);
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

/// Restart OpenCode after changing its global configuration. OpenCode's
/// desktop sidecar snapshots the provider catalog at startup, and launching
/// a second instance only focuses the existing single-instance process.
pub fn restart_opencode() -> Result<(), String> {
    let executable = resolve_opencode_command().ok_or_else(|| {
        "OpenCode executable was not found. Relay checks Desktop, the official installer, package managers, and PATH; restart Relay after installing it".to_string()
    })?;

    // Resolve the executable before stopping the current instance. A broken
    // installation must not leave a working OpenCode session closed.
    if is_opencode_running() {
        stop_opencode_and_wait()?;
    }

    #[cfg(target_os = "windows")]
    {
        spawn_opencode_windows(&executable)
    }

    #[cfg(not(target_os = "windows"))]
    {
        Command::new(executable)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("failed to start OpenCode: {error}"))
    }
}

#[cfg(target_os = "windows")]
fn spawn_opencode_windows(executable: &Path) -> Result<(), String> {
    let is_script = executable.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
    });
    let mut command = if is_script {
        let mut command = windows_hidden_command("cmd.exe");
        // Pass the resolved path as a real process argument. Building a
        // quoted `start` command by hand makes cmd.exe treat the final quote
        // as a path separator (`OpenCode.exe\\`) on some Windows builds.
        command.args(["/D", "/C"]).arg(executable);
        command
    } else {
        windows_hidden_command(executable)
    };

    // GUI builds ignore CREATE_NEW_CONSOLE; CLI/TUI builds receive their own
    // console instead of inheriting Relay's hidden process window.
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    command.creation_flags(CREATE_NEW_CONSOLE);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn resolve_opencode_command() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(configured) = env::var_os("OPENCODE_BIN").filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(configured));
    }
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    if let Some(home) = home {
        for directory in [
            home.join(".opencode").join("bin"),
            home.join(".local").join("bin"),
            home.join("bin"),
            home.join(".bun").join("bin"),
            home.join(".local").join("share").join("mise").join("shims"),
            home.join(".config").join("mise").join("shims"),
            home.join("scoop").join("shims"),
            home.join("scoop")
                .join("apps")
                .join("opencode")
                .join("current"),
            home.join("scoop")
                .join("apps")
                .join("opencode-desktop")
                .join("current"),
        ] {
            push_opencode_commands(&mut candidates, &directory);
        }
        for directory in [
            home.join("Applications"),
            home.join("Downloads"),
            home.join(".local").join("share").join("applications"),
        ] {
            push_desktop_files(&mut candidates, &directory);
            push_macos_app_bundles(&mut candidates, &directory);
        }
    }

    for variable in [
        "OPENCODE_INSTALL_DIR",
        "XDG_BIN_DIR",
        "BUN_INSTALL",
        "VOLTA_HOME",
    ] {
        if let Some(directory) = env::var_os(variable).map(PathBuf::from) {
            push_opencode_commands(&mut candidates, &directory);
            push_opencode_commands(&mut candidates, &directory.join("bin"));
        }
    }

    if let Some(app_data) = env::var_os("APPDATA").map(PathBuf::from) {
        push_opencode_commands(&mut candidates, &app_data.join("npm"));
        push_opencode_commands(
            &mut candidates,
            &app_data
                .join(".local")
                .join("share")
                .join("mise")
                .join("shims"),
        );
    }

    if let Some(chocolatey_root) = env::var_os("ChocolateyInstall").map(PathBuf::from) {
        push_opencode_commands(&mut candidates, &chocolatey_root.join("bin"));
    }

    if cfg!(target_os = "windows") {
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            // Official NSIS builds and the current desktop beta have used
            // these roots over time. Keep all channel names so a beta/dev
            // install is not mistaken for a missing OpenCode installation.
            for directory in [
                local_app_data.join("Programs").join("@opencode-aidesktop"),
                local_app_data.join("Programs").join("OpenCode"),
                local_app_data.join("Programs").join("OpenCode Desktop"),
                local_app_data.join("Programs").join("OpenCode Dev"),
                local_app_data.join("Programs").join("OpenCode Beta"),
                local_app_data.join("OpenCode"),
                local_app_data.join("OpenCode Desktop"),
            ] {
                push_opencode_commands(&mut candidates, &directory);
            }
            push_opencode_commands(
                &mut candidates,
                &local_app_data
                    .join("Microsoft")
                    .join("WinGet")
                    .join("Links"),
            );
            push_opencode_commands(&mut candidates, &local_app_data.join("mise").join("shims"));
        }
        for variable in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            if let Some(program_files) = env::var_os(variable).map(PathBuf::from) {
                for directory in [
                    program_files.join("OpenCode"),
                    program_files.join("OpenCode Desktop"),
                    program_files.join("OpenCode Dev"),
                    program_files.join("OpenCode Beta"),
                ] {
                    push_opencode_commands(&mut candidates, &directory);
                }
            }
        }
    } else if cfg!(target_os = "macos") {
        for directory in [
            PathBuf::from("/Applications"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ] {
            push_opencode_commands(&mut candidates, &directory);
            push_macos_app_bundles(&mut candidates, &directory);
        }
    } else {
        // Official Linux packages use the app id as the executable name and
        // install under /opt; Flatpak exports the same launcher into one of
        // these two standard export directories.
        for directory in [
            PathBuf::from("/usr/bin"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/home/linuxbrew/.linuxbrew/bin"),
            PathBuf::from("/opt/OpenCode"),
            PathBuf::from("/nix/profile/bin"),
            PathBuf::from("/nix/var/nix/profiles/default/bin"),
            PathBuf::from("/run/current-system/sw/bin"),
            PathBuf::from("/var/lib/flatpak/exports/bin"),
            PathBuf::from("/usr/local/share/flatpak/exports/bin"),
        ] {
            push_opencode_commands(&mut candidates, &directory);
        }
    }

    // Homebrew exposes the desktop app as `opencode-desktop`, while the
    // terminal package and all distro packages expose `opencode`.
    for name in ["opencode", "opencode-desktop", "ai.opencode.desktop"] {
        if let Some(path) = find_command_on_path(name) {
            candidates.push(path);
        }
    }

    // Discover package-manager global bin directories. This covers npm,
    // pnpm, Bun and Yarn installations even when their shims are not in the
    // environment inherited by the desktop app.
    for (manager, args) in [
        ("npm", ["prefix", "-g"].as_slice()),
        ("pnpm", ["bin", "-g"].as_slice()),
        ("bun", ["pm", "bin", "-g"].as_slice()),
        ("yarn", ["global", "bin"].as_slice()),
    ] {
        let Some(manager_path) = find_command_on_path(manager) else {
            continue;
        };
        let Ok(output) = run_command_output(&manager_path, args) else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let Ok(directory) = String::from_utf8(output.stdout) else {
            continue;
        };
        let directory = PathBuf::from(directory.trim());
        if !directory.as_os_str().is_empty() {
            push_opencode_commands(&mut candidates, &directory);
            // npm's global prefix is the parent of its bin directory on
            // Unix, but the prefix itself is the bin directory on Windows.
            push_opencode_commands(&mut candidates, &directory.join("bin"));
        }
    }

    candidates.into_iter().find(|candidate| candidate.is_file())
}

fn push_opencode_commands(candidates: &mut Vec<PathBuf>, directory: &Path) {
    for name in [
        "opencode",
        "opencode.exe",
        "opencode.cmd",
        "opencode.bat",
        "opencode-desktop",
        "opencode-desktop.exe",
        "opencode-desktop.cmd",
        "opencode-desktop.bat",
        "ai.opencode.desktop",
        "ai.opencode.desktop.exe",
        "OpenCode.exe",
        "OpenCode Dev.exe",
        "OpenCode Beta.exe",
    ] {
        candidates.push(directory.join(name));
    }
}

fn push_desktop_files(candidates: &mut Vec<PathBuf>, directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if name.contains("opencode") && name.ends_with(".appimage") {
            candidates.push(path);
        }
    }
}

#[cfg(target_os = "macos")]
fn push_macos_app_bundles(candidates: &mut Vec<PathBuf>, directory: &Path) {
    for name in ["OpenCode.app", "OpenCode Beta.app", "OpenCode Dev.app"] {
        candidates.push(
            directory
                .join(name)
                .join("Contents")
                .join("MacOS")
                .join("OpenCode"),
        );
    }
}

#[cfg(not(target_os = "macos"))]
fn push_macos_app_bundles(_candidates: &mut Vec<PathBuf>, _directory: &Path) {}

fn run_command_output(path: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    #[cfg(target_os = "windows")]
    if path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
    }) {
        return windows_hidden_command("cmd.exe")
            .arg("/D")
            .arg("/S")
            .arg("/C")
            .arg(path)
            .args(args)
            .output();
    }
    Command::new(path).args(args).output()
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

fn is_opencode_running() -> bool {
    let system = codex_process_system();
    system.processes().values().any(is_opencode_process)
}

fn stop_opencode_and_wait() -> Result<bool, String> {
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
        // The initial taskkill uses `/T`, so all children are already covered.
        // Probe only those exact main-process PIDs instead of enumerating every
        // process on the machine on each 100 ms stop-loop iteration.
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
    command.args(["/PID", &pid.to_string(), "/T"]);
    if force {
        command.arg("/F");
    }
    let _ = command.status();
}

fn launch_codex_checked(inject_saved_key: bool) -> Result<(), String> {
    // Opening an already running desktop app is a no-op. Besides avoiding a
    // duplicate process, this prevents Chromium from reinitializing its
    // profile and touching the large on-disk cache on every click.
    if is_codex_running() {
        return Ok(());
    }

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

fn find_command_on_path(name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH").or_else(|| env::var_os("Path"))?;
    let names = if cfg!(target_os = "windows") {
        vec![
            name.to_string(),
            format!("{name}.exe"),
            format!("{name}.cmd"),
            format!("{name}.bat"),
        ]
    } else {
        vec![name.to_string()]
    };
    env::split_paths(&paths)
        .flat_map(|directory| names.iter().map(move |entry| directory.join(entry)))
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
    Err(last_error.unwrap_or_else(|| {
        "ChatGPT was not found in Windows installed apps. Repair or reinstall ChatGPT, then try again.".to_string()
    }))
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
    output
        .filter(|output| output.status.success())
        .map(|output| parse_windows_start_apps_output(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default()
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
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
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

fn opencode_process_pids() -> Vec<u32> {
    let system = codex_process_system();
    system
        .processes()
        .values()
        .filter(|process| is_opencode_process(process))
        .map(|process| process.pid().as_u32())
        .collect()
}

#[cfg(target_os = "windows")]
fn codex_process_pids_for(targets: &[u32]) -> Vec<u32> {
    matching_process_pids(targets, is_codex_process)
}

#[cfg(target_os = "windows")]
fn opencode_process_pids_for(targets: &[u32]) -> Vec<u32> {
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

fn is_opencode_process(process: &sysinfo::Process) -> bool {
    let name = process.name().to_string_lossy();
    let executable = process.exe();
    let command = process
        .cmd()
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>();
    is_opencode_process_identity(&name, executable, &command)
}

fn is_opencode_process_identity(
    name: &str,
    executable: Option<&Path>,
    command: &[impl AsRef<str>],
) -> bool {
    #[cfg(target_os = "windows")]
    {
        // Electron creates crashpad/GPU/renderer children with the same
        // executable. Only the main OpenCode process may be terminated; the
        // normal task-kill tree then cleans up its children.
        if command
            .iter()
            .any(|value| value.as_ref().starts_with("--type="))
        {
            return false;
        }
        let path = executable
            .map(|value| value.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let name_matches = ["opencode.exe", "opencode-desktop.exe"]
            .iter()
            .any(|candidate| name.eq_ignore_ascii_case(candidate));
        name_matches && path.contains("opencode")
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (executable, command);
        OPENCODE_PROCESS_NAMES
            .iter()
            .any(|candidate| name.eq_ignore_ascii_case(candidate))
    }
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
        // Do not match the standalone Codex CLI (`...\\OpenAI\\Codex\\bin\\codex.exe`):
        // profile switching must never terminate the active Relay/Codex task.
        let packaged_desktop = path.contains("\\windowsapps\\openai.")
            || path.contains("\\program files\\chatgpt\\")
            || path.contains("\\program files\\codex\\");
        (name.eq_ignore_ascii_case("OpenAI.Codex.exe")
            || (name.eq_ignore_ascii_case("Codex.exe") && packaged_desktop))
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
        assert!(!is_codex_process_identity(
            "codex.exe",
            Some(Path::new(
                r"C:\Users\FORLE\AppData\Local\OpenAI\Codex\bin\codex.exe"
            )),
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
    fn windows_opencode_matches_desktop_but_ignores_electron_helpers() {
        let executable =
            Path::new(r"C:\Users\test\AppData\Local\Programs\@opencode-aidesktop\opencode.exe");
        assert!(is_opencode_process_identity(
            "opencode.exe",
            Some(executable),
            &[""]
        ));
        assert!(!is_opencode_process_identity(
            "opencode.exe",
            Some(executable),
            &["--type=renderer"]
        ));
        assert!(!is_opencode_process_identity(
            "other.exe",
            Some(executable),
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
