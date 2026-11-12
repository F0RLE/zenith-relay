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
pub(super) fn query_output(command: Command) -> Result<String, &'static str> {
    query_output_with_timeout(command, Duration::from_secs(5))
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn query_output_with_timeout(
    mut command: Command,
    timeout: Duration,
) -> Result<String, &'static str> {
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
            Ok(None) if started.elapsed() < timeout => {
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
    let packaged = selected.is_none();
    let mut command = super::super::process::windows_hidden_command("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-Sta", "-Command", r#"
$ErrorActionPreference = 'Stop'
$path = $env:RELAY_CLAUDE_VERSION_BIN
$appId = $null
$family = $null
if ([string]::IsNullOrEmpty($path)) {
    $appId = Get-StartApps | Where-Object {
        $_.Name -eq 'Claude' -and
        ($_.AppID -like 'AnthropicPBC.Claude_*!*' -or $_.AppID -like 'Anthropic.Claude_*!*') -and
        $_.AppID.Length -le 256 -and $_.AppID -cmatch '^[A-Za-z0-9._!-]+$'
    } | Select-Object -First 1 -ExpandProperty AppID
    if ([string]::IsNullOrEmpty($appId)) { throw 'Claude Desktop registered package identity is unavailable' }
    $shell = New-Object -ComObject Shell.Application
    $item = $shell.Namespace('shell:AppsFolder').ParseName($appId)
    if ($null -eq $item) { throw 'Claude Desktop registered package identity is unavailable' }
    $family = $item.ExtendedProperty('System.AppUserModel.PackageFamilyName')
    if ($family -ne $appId.Split('!')[0]) {
        throw 'Claude Desktop registered package identity is unavailable'
    }
    $root = $item.ExtendedProperty('System.AppUserModel.PackageInstallPath')
    if ([string]::IsNullOrEmpty($root)) { throw 'Claude Desktop registered package path is unavailable' }
    $path = Join-Path $root 'app\Claude.exe'
}
if (Test-Path -LiteralPath $path -PathType Leaf) {
    [pscustomobject]@{
        version = [Diagnostics.FileVersionInfo]::GetVersionInfo($path).ProductVersion
        appId = $appId
        family = $family
    } | ConvertTo-Json -Compress
}
"#])
        .env_remove("RELAY_CLAUDE_VERSION_BIN");
    if let Some(path) = selected {
        command.env("RELAY_CLAUDE_VERSION_BIN", path);
    }
    // Keep the previous combined discovery/version budget for registered packages.
    let timeout = Duration::from_secs(if packaged { 10 } else { 5 });
    windows_metadata_version(&query_output_with_timeout(command, timeout)?, packaged)
}

#[cfg(any(target_os = "windows", test))]
fn windows_metadata_version(output: &str, packaged: bool) -> Result<String, &'static str> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Metadata {
        version: String,
        app_id: Option<String>,
        family: Option<String>,
    }
    let metadata: Metadata =
        serde_json::from_str(output).map_err(|_| "Desktop publisher metadata was invalid")?;
    if release(&metadata.version).is_none() {
        return Err("Desktop publisher version was invalid");
    }
    match (
        packaged,
        metadata.app_id.as_deref(),
        metadata.family.as_deref(),
    ) {
        (true, Some(app_id), Some(family))
            if super::packaged_target(app_id) == Some(app_id)
                && app_id.split_once('!').map(|(prefix, _)| prefix) == Some(family) => {}
        (false, None, None) => {}
        _ => return Err("Desktop publisher package identity was invalid"),
    }
    Ok(metadata.version)
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

    #[test]
    fn publisher_metadata_requires_the_selected_binary_or_exact_registered_identity() {
        let package = serde_json::json!({
            "version":"4.4212.0",
            "appId":"AnthropicPBC.Claude_test!Claude",
            "family":"AnthropicPBC.Claude_test"
        });
        assert_eq!(
            windows_metadata_version(&package.to_string(), true),
            Ok("4.4212.0".into())
        );
        assert!(windows_metadata_version(&package.to_string(), false).is_err());
        for (field, value) in [
            (
                "appId",
                serde_json::json!("AnthropicPBC.ClaudeCode_test!Claude"),
            ),
            (
                "appId",
                serde_json::json!("AnthropicPBC.Claude_test!Claude & calc.exe"),
            ),
            (
                "appId",
                serde_json::json!("AnthropicPBC.Claude_test!Claude\n"),
            ),
            ("appId", serde_json::Value::Null),
            ("family", serde_json::json!("Other.Package_test")),
            ("family", serde_json::Value::Null),
            ("version", serde_json::json!("4.4212.0-beta")),
            ("version", serde_json::Value::Null),
            ("extra", serde_json::json!("unexpected")),
        ] {
            let mut changed = package.clone();
            changed[field] = value;
            assert!(
                windows_metadata_version(&changed.to_string(), true).is_err(),
                "accepted invalid field: {field}"
            );
        }
        let binary = r#"{"version":"4.29.0","appId":null,"family":null}"#;
        assert_eq!(windows_metadata_version(binary, false), Ok("4.29.0".into()));
        assert!(windows_metadata_version(binary, true).is_err());
        assert!(windows_metadata_version("4.29.0", false).is_err());
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
