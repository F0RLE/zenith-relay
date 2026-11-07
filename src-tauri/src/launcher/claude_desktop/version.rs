#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::{
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Read publisher version metadata without launching or stopping the client.
/// Unknown versions remain unknown so refresh can retain a working profile.
pub(crate) fn model_identity_support() -> Option<bool> {
    release(&installed_version().ok()?).map(|version| version >= (4, 29, 0))
}

fn release(version: &str) -> Option<(u32, u32, u32)> {
    let version = version.trim();
    if version.len() > 64 {
        return None;
    }
    let parts = version
        .split('.')
        .map(|part| {
            (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| part.parse::<u32>().ok())
                .flatten()
        })
        .collect::<Option<Vec<_>>>()?;
    matches!(parts.len(), 3 | 4).then(|| (parts[0], parts[1], parts[2]))
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(super) fn query_output(mut command: Command) -> Result<String, &'static str> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(if cfg!(test) {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .spawn()
        .map_err(|_| "metadata helper could not start")?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < Duration::from_secs(5) => {
                thread::sleep(Duration::from_millis(20));
            }
            _ => break None,
        }
    };
    let Some(status) = status else {
        // Only this newly created metadata helper is terminated on timeout.
        let _ = child.kill();
        let _ = child.wait();
        return Err("metadata helper timed out or could not be waited");
    };
    if !status.success() {
        return Err("metadata helper exited unsuccessfully");
    }
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .ok_or("metadata helper output was unavailable")?
        .take(16_385)
        .read_to_end(&mut bytes)
        .map_err(|_| "metadata helper output could not be read")?;
    if bytes.len() > 16_384 {
        return Err("metadata helper output exceeded its limit");
    }
    String::from_utf8(bytes).map_err(|_| "metadata helper output was not UTF-8")
}

#[cfg(target_os = "windows")]
fn installed_version() -> Result<String, &'static str> {
    let selected =
        super::super::settings::configured_path(super::super::settings::Application::ClaudeDesktop)
            .map_err(|_| "configured Desktop path could not be resolved")?
            .or_else(super::desktop_executable);
    let mut command = super::super::process::windows_hidden_command("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-Sta", "-Command", r#"
$ErrorActionPreference = 'Stop'
$path = $env:RELAY_CLAUDE_VERSION_BIN
if ([string]::IsNullOrEmpty($path)) {
    $appId = $env:RELAY_CLAUDE_VERSION_APPID
    $shell = New-Object -ComObject Shell.Application
    $item = $shell.Namespace('shell:AppsFolder').ParseName($appId)
    if ($null -eq $item -or $item.ExtendedProperty('System.AppUserModel.PackageFamilyName') -ne $appId.Split('!')[0]) {
        throw 'Claude Desktop registered package identity is unavailable'
    }
    $root = $item.ExtendedProperty('System.AppUserModel.PackageInstallPath')
    if ([string]::IsNullOrEmpty($root)) { throw 'Claude Desktop registered package path is unavailable' }
    $path = Join-Path $root 'app\Claude.exe'
}
if (Test-Path -LiteralPath $path -PathType Leaf) {
    [Diagnostics.FileVersionInfo]::GetVersionInfo($path).ProductVersion
}
"#])
        .env_remove("RELAY_CLAUDE_VERSION_BIN")
        .env_remove("RELAY_CLAUDE_VERSION_APPID");
    if let Some(path) = selected {
        command.env("RELAY_CLAUDE_VERSION_BIN", path);
    } else {
        let target = super::packaged_launch_target()
            .map_err(|_| "installed Desktop package could not be resolved")?;
        let app_id = target
            .strip_prefix("shell:AppsFolder\\")
            .ok_or("installed Desktop package identity was invalid")?;
        command.env("RELAY_CLAUDE_VERSION_APPID", app_id);
    }
    query_output(command)
}

#[cfg(target_os = "macos")]
fn installed_version() -> Result<String, &'static str> {
    let path =
        super::super::settings::configured_path(super::super::settings::Application::ClaudeDesktop)
            .map_err(|_| "configured Desktop path could not be resolved")?
            .or_else(super::desktop_executable)
            .ok_or("installed Desktop bundle was not found")?;
    let info = path
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or("installed Desktop bundle path was invalid")?
        .join("Info.plist");
    let mut command = Command::new("/usr/libexec/PlistBuddy");
    command
        .args(["-c", "Print :CFBundleShortVersionString"])
        .arg(info);
    query_output(command)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn installed_version() -> Result<String, &'static str> {
    Err("Desktop publisher metadata is unsupported on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_gate_is_numeric_and_requires_complete_publisher_versions() {
        for (version, supported) in [
            ("3.99999.0", false),
            ("4.28.999", false),
            ("4.29.0", true),
            ("4.4212.0\r\n", true),
            ("4.29.0.1", true),
            ("5.0.0", true),
        ] {
            assert_eq!(release(version).unwrap() >= (4, 29, 0), supported);
        }
        for version in [
            "",
            "4.29",
            "4.29.0-beta",
            "4.29.invalid",
            "4.29.0.0.1",
            "4.29.0\n5.0.0",
        ] {
            assert!(release(version).is_none(), "{version:?}");
        }
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    #[ignore = "requires installed Claude Desktop; reads publisher metadata only"]
    fn installed_desktop_model_identity_version_is_read_only() {
        #[cfg(target_os = "windows")]
        {
            let mut command =
                super::super::super::process::windows_hidden_command("powershell.exe");
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Write-Output '4.29.0'",
            ]);
            assert_eq!(
                query_output(command).ok().as_deref().map(str::trim),
                Some("4.29.0")
            );
            assert!(
                super::super::desktop_executable().is_some()
                    || super::super::packaged_launch_target().is_ok(),
                "installed Desktop launch target metadata"
            );
        }
        let version = installed_version().expect("installed Desktop version metadata");
        assert!(
            release(&version).is_some(),
            "invalid publisher version: {version:?}"
        );
    }
}
