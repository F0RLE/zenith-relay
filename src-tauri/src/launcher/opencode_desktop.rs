use std::env;
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "windows"))]
use std::process::Command;

#[cfg(target_os = "windows")]
use super::process::windows_hidden_command;
use super::process::{find_command_on_path, is_opencode_running, stop_opencode_and_wait};

#[cfg(target_os = "windows")]
pub(super) fn spawn_opencode_windows(executable: &Path) -> Result<(), String> {
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

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(super) fn resolve_opencode_desktop_command() -> Option<PathBuf> {
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

    candidates
        .into_iter()
        .filter(|candidate| candidate.is_file())
        .map(|candidate| std::fs::canonicalize(&candidate).unwrap_or(candidate))
        .find(|candidate| is_opencode_desktop_path(candidate))
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

fn is_opencode_desktop_path(path: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        is_windows_opencode_desktop_path(path)
    }
    #[cfg(target_os = "macos")]
    {
        is_macos_opencode_desktop_path(path)
    }
    #[cfg(target_os = "linux")]
    {
        is_linux_opencode_desktop_path(path)
    }
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn is_windows_opencode_desktop_path(path: &Path) -> bool {
    let path = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    let desktop_name = [
        "opencode.exe",
        "opencode-desktop.exe",
        "opencode dev.exe",
        "opencode beta.exe",
    ]
    .iter()
    .any(|name| path.ends_with(&format!("\\{name}")));
    desktop_name
        && !path.contains("\\bin\\")
        && !path.contains("\\resources\\")
        && [
            "\\programs\\@opencode-aidesktop\\",
            "\\programs\\opencode\\",
            "\\programs\\opencode desktop\\",
            "\\programs\\opencode dev\\",
            "\\programs\\opencode beta\\",
            "\\program files\\opencode\\",
            "\\program files\\opencode desktop\\",
            "\\program files\\opencode dev\\",
            "\\program files\\opencode beta\\",
            "\\scoop\\apps\\opencode-desktop\\",
        ]
        .iter()
        .any(|root| path.contains(root))
}

#[cfg(any(not(target_os = "windows"), test))]
pub(super) fn is_macos_opencode_desktop_path(path: &Path) -> bool {
    ["opencode", "opencode beta", "opencode dev"]
        .iter()
        .any(|bundle| is_macos_app_executable(path, bundle, "opencode"))
}

#[cfg(any(not(target_os = "windows"), test))]
pub(super) fn is_macos_app_executable(path: &Path, bundle: &str, executable: &str) -> bool {
    let path = path.to_string_lossy().to_ascii_lowercase();
    path.ends_with(&format!("/{bundle}.app/contents/macos/{executable}"))
}

#[cfg(any(not(target_os = "windows"), test))]
pub(super) fn is_linux_opencode_desktop_path(path: &Path) -> bool {
    let path = path.to_string_lossy().to_ascii_lowercase();
    path.ends_with(".appimage") && path.contains("opencode")
        || path.ends_with("/opencode-desktop")
        || path.ends_with("/ai.opencode.desktop")
        || (path.ends_with("/opencode")
            && (path.contains("/opt/opencode/")
                || (path.contains("/.mount_") && path.contains("opencode"))))
}

/// Restart OpenCode after changing its global configuration. OpenCode's
/// desktop sidecar snapshots the provider catalog at startup, and launching
/// a second instance only focuses the existing single-instance process.
pub(super) fn restart_opencode() -> Result<(), String> {
    let executable = resolve_opencode_desktop_command().ok_or_else(|| {
        "OpenCode desktop was not found. Install the desktop app or open it manually; Relay does not launch the terminal CLI without a terminal".to_string()
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
        #[cfg(target_os = "macos")]
        {
            let app = executable
                .ancestors()
                .find(|path| path.extension().is_some_and(|extension| extension == "app"))
                .ok_or_else(|| "OpenCode desktop bundle was not found".to_string())?;
            Command::new("open")
                .arg("-a")
                .arg(app)
                .status()
                .map_err(|error| format!("failed to open OpenCode: {error}"))
                .and_then(|status| {
                    status
                        .success()
                        .then_some(())
                        .ok_or_else(|| "OpenCode desktop did not open".to_string())
                })
        }

        #[cfg(not(target_os = "macos"))]
        {
            Command::new(executable)
                .spawn()
                .map(|_| ())
                .map_err(|error| format!("failed to start OpenCode: {error}"))
        }
    }
}
