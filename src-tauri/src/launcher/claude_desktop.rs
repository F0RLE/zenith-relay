use std::{
    env,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

pub(crate) fn is_claude_desktop_running() -> bool {
    super::process::codex_process_system()
        .processes()
        .values()
        .any(|process| process.exe().is_some_and(is_desktop_executable))
}

pub(super) fn is_desktop_executable(path: &Path) -> bool {
    let path = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    (path.ends_with("/claude.exe")
        && (path.contains("/claude/")
            || path.contains("/anthropic.claude_")
            || path.contains("/anthropicpbc.claude_")))
        || path.ends_with("/claude.app/contents/macos/claude")
}

fn desktop_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("CLAUDE_DESKTOP_BIN").filter(|path| !path.is_empty()) {
        candidates.push(path.into());
    }
    if cfg!(target_os = "windows") {
        if let Some(local) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            for root in [local.join("Claude"), local.join("Programs/Claude")] {
                candidates.push(root.join("Claude.exe"));
                let mut versions = std::fs::read_dir(&root)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.file_name()
                            .is_some_and(|name| name.to_string_lossy().starts_with("app-"))
                    })
                    .collect::<Vec<_>>();
                versions.sort();
                candidates.extend(
                    versions
                        .into_iter()
                        .rev()
                        .map(|path| path.join("Claude.exe")),
                );
            }
        }
    } else if cfg!(target_os = "macos") {
        candidates.push("/Applications/Claude.app/Contents/MacOS/Claude".into());
        if let Some(home) = env::var_os("HOME") {
            candidates
                .push(PathBuf::from(home).join("Applications/Claude.app/Contents/MacOS/Claude"));
        }
    }
    candidates
}

fn desktop_executable() -> Option<PathBuf> {
    desktop_candidates()
        .into_iter()
        .find(|path| path.is_file() && is_desktop_executable(path))
}

pub(super) fn detected_launch_path() -> Option<String> {
    if let Some(path) = desktop_executable() {
        return Some(path.to_string_lossy().into_owned());
    }
    #[cfg(target_os = "windows")]
    {
        packaged_launch_target().ok()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[cfg(target_os = "windows")]
fn packaged_launch_target() -> Result<String, String> {
    let output = super::process::windows_hidden_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command",
            "Get-StartApps | Where-Object { $_.Name -eq 'Claude' -and ($_.AppID -like 'AnthropicPBC.Claude_*!*' -or $_.AppID -like 'Anthropic.Claude_*!*') } | ForEach-Object { $_.AppID }"])
        .output().map_err(|_| "Could not inspect installed Claude Desktop applications".to_string())?;
    if !output.status.success() {
        return Err("Could not inspect installed Claude Desktop applications".into());
    }
    let output_text = String::from_utf8_lossy(&output.stdout);
    let target = packaged_target(&output_text).ok_or_else(|| {
        "Claude Desktop was not found. Install Claude Desktop, not the Claude Code CLI".to_string()
    })?;
    Ok(format!("shell:AppsFolder\\{target}"))
}

#[cfg(target_os = "windows")]
fn start_packaged_desktop(target: &str) -> Result<(), String> {
    super::process::windows_hidden_command("explorer.exe")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|_| "Could not start Claude Desktop".into())
}

#[cfg(any(target_os = "windows", test))]
fn packaged_target(output: &str) -> Option<&str> {
    output.lines().map(str::trim).find(|id| {
        id.len() <= 256
            && (id.starts_with("AnthropicPBC.Claude_") || id.starts_with("Anthropic.Claude_"))
            && id.contains('!')
            && id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'!')
            })
    })
}

pub(crate) fn launch_claude_desktop() -> Result<(), String> {
    if !cfg!(any(target_os = "windows", target_os = "macos")) {
        return Err("Automatic Claude Desktop launch is supported on Windows and macOS".into());
    }
    let executable = super::settings::configured_path(super::settings::Application::ClaudeDesktop)?
        .or_else(desktop_executable);
    // Resolve and validate the launch target before stopping an existing client.
    #[cfg(target_os = "windows")]
    let packaged = if executable.is_none() {
        Some(packaged_launch_target()?)
    } else {
        None
    };
    #[cfg(target_os = "macos")]
    let application = executable
        .as_ref()
        .ok_or("Claude Desktop was not found. Install Claude Desktop, not the Claude Code CLI")?
        .ancestors()
        .nth(3)
        .ok_or("Invalid Claude Desktop application path")?
        .to_path_buf();
    super::process::stop_claude_desktop_and_wait()?;
    if let Some(_executable) = executable {
        #[cfg(target_os = "windows")]
        super::process::windows_hidden_command(_executable)
            .spawn()
            .map_err(|_| "Could not start Claude Desktop".to_string())?;
        #[cfg(target_os = "macos")]
        std::process::Command::new("open")
            .arg("-a")
            .arg(application)
            .spawn()
            .map_err(|_| "Could not start Claude Desktop".to_string())?;
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = _executable;
            return Err("Automatic Claude Desktop launch is supported on Windows and macOS".into());
        }
    } else {
        #[cfg(target_os = "windows")]
        start_packaged_desktop(
            packaged
                .as_deref()
                .ok_or("Claude Desktop launch target is unavailable")?,
        )?;
        #[cfg(not(target_os = "windows"))]
        return Err(
            "Claude Desktop was not found. Install Claude Desktop, not the Claude Code CLI".into(),
        );
    }
    wait_for_start(is_claude_desktop_running, Duration::from_secs(10))
}

fn wait_for_start(mut running: impl FnMut() -> bool, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        if running() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("Claude Desktop did not start before the launch timeout; the saved profile was kept".into());
        }
        thread::sleep(Duration::from_millis(150));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_confirmation_requires_a_running_desktop() {
        assert!(wait_for_start(|| true, Duration::ZERO).is_ok());
        assert!(wait_for_start(|| false, Duration::ZERO).is_err());
    }

    #[test]
    fn distinguishes_desktop_from_code_cli() {
        assert!(is_desktop_executable(Path::new(
            "C:/Users/Test/AppData/Local/Claude/app-1.0/Claude.exe"
        )));
        assert!(is_desktop_executable(Path::new(
            "C:/Program Files/WindowsApps/Anthropic.Claude_1.0_x64__test/app/Claude.exe"
        )));
        assert!(is_desktop_executable(Path::new(
            "C:/Program Files/WindowsApps/AnthropicPBC.Claude_2.37337.1.0_x64__test/app/Claude.exe"
        )));
        assert!(is_desktop_executable(Path::new(
            "/Applications/Claude.app/Contents/MacOS/Claude"
        )));
        assert!(!is_desktop_executable(Path::new(
            "C:/Users/Test/.local/bin/claude.exe"
        )));
        assert!(!is_desktop_executable(Path::new("/usr/local/bin/claude")));
        assert!(!is_desktop_executable(Path::new(
            "C:/Claude/claude-code.exe"
        )));
    }

    #[test]
    fn only_accepts_claude_desktop_package_ids() {
        assert_eq!(
            packaged_target("Other.App!App\nAnthropic.Claude_test!Claude\n"),
            Some("Anthropic.Claude_test!Claude")
        );
        assert_eq!(
            packaged_target("Other.App!App\r\nAnthropicPBC.Claude_test!Claude\r\n"),
            Some("AnthropicPBC.Claude_test!Claude")
        );
        assert!(packaged_target("Anthropic.Claude_test!App & calc.exe").is_none());
        assert!(packaged_target("AnthropicPBC.Claude_test!App & calc.exe").is_none());
        assert!(packaged_target("Anthropic.ClaudeCode_test!App").is_none());
        assert!(packaged_target("AnthropicPBC.ClaudeCode_test!App").is_none());
    }
}
