use std::path::Path;
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use super::is_codex_running;
use super::opencode_desktop::{
    is_linux_opencode_desktop_path, is_macos_app_executable, is_macos_opencode_desktop_path,
    is_windows_opencode_desktop_path,
};
#[cfg(target_os = "windows")]
use super::process::{
    is_codex_process_identity, is_opencode_process_identity, parse_windows_start_apps_output,
    windows_taskkill_arguments,
};
use super::process::{process_stop_is_stable, running_target_pids};

#[test]
fn unix_desktop_paths_do_not_include_terminal_clients_or_helpers() {
    assert!(is_macos_app_executable(
        Path::new("/Applications/Codex.app/Contents/MacOS/Codex"),
        "codex",
        "codex"
    ));
    assert!(is_macos_app_executable(
        Path::new("/Users/test/Applications/ChatGPT.app/Contents/MacOS/ChatGPT"),
        "chatgpt",
        "chatgpt"
    ));
    assert!(!is_macos_app_executable(
        Path::new("/Applications/Codex.app/Contents/Resources/codex"),
        "codex",
        "codex"
    ));
    assert!(!is_macos_app_executable(
        Path::new("/usr/local/bin/codex"),
        "codex",
        "codex"
    ));
    assert!(is_macos_opencode_desktop_path(Path::new(
        "/Applications/OpenCode.app/Contents/MacOS/OpenCode"
    )));
    assert!(!is_macos_opencode_desktop_path(Path::new(
        "/Users/test/.opencode/bin/opencode"
    )));
    assert!(is_linux_opencode_desktop_path(Path::new(
        "/usr/bin/ai.opencode.desktop"
    )));
    assert!(is_linux_opencode_desktop_path(Path::new(
        "/opt/OpenCode/opencode"
    )));
    assert!(!is_linux_opencode_desktop_path(Path::new(
        "/usr/local/bin/opencode"
    )));
    assert!(!is_linux_opencode_desktop_path(Path::new(
        "/home/test/.opencode/bin/opencode"
    )));
}

#[test]
fn windows_opencode_desktop_path_excludes_cli_installation() {
    assert!(is_windows_opencode_desktop_path(Path::new(
        r"C:\Users\test\AppData\Local\Programs\@opencode-aidesktop\opencode.exe"
    )));
    assert!(!is_windows_opencode_desktop_path(Path::new(
        r"C:\Users\test\AppData\Local\Programs\OpenCode\bin\opencode.exe"
    )));
    assert!(!is_windows_opencode_desktop_path(Path::new(
        r"C:\Users\test\.opencode\bin\opencode.exe"
    )));
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
fn windows_relay_is_never_classified_as_codex() {
    assert!(!is_codex_process_identity(
        "Zenith Relay.exe",
        Some(Path::new(r"C:\Users\FORLE\Desktop\Zenith Relay.exe")),
        &[""]
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn windows_taskkill_never_targets_a_process_tree() {
    assert_eq!(
        windows_taskkill_arguments(1234, false),
        vec!["/PID".to_string(), "1234".to_string()]
    );
    assert_eq!(
        windows_taskkill_arguments(1234, true),
        vec!["/PID".to_string(), "1234".to_string(), "/F".to_string()]
    );
}

#[cfg(target_os = "windows")]
#[test]
fn windows_opencode_matches_desktop_but_ignores_chromium_helpers() {
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
        "Codex\tOpenAI.Codex_2p2nqsd0c76g0!App\nChatGPT\tOpenAI.ChatGPT_2p2nqsd0c76g0!App\nZenith Relay\tcom.zenith.relay\n",
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
