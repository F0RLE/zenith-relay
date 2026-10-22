use super::*;

struct Fixture {
    root: PathBuf,
    store: PreferenceStore,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("relay-launch-paths-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self {
            store: PreferenceStore {
                path: root.join("preferences/application-launch.json"),
                lock: Mutex::new(()),
            },
            root,
        }
    }

    fn executable(&self, application: Application) -> PathBuf {
        #[cfg(target_os = "windows")]
        let relative = match application {
            Application::Chatgpt => "Program Files/ChatGPT/ChatGPT.exe",
            Application::ClaudeDesktop => "Claude/app-1.0/Claude.exe",
            Application::Opencode => "Programs/OpenCode/OpenCode.exe",
        };
        #[cfg(target_os = "macos")]
        let relative = match application {
            Application::Chatgpt => "ChatGPT.app/Contents/MacOS/ChatGPT",
            Application::ClaudeDesktop => "Claude.app/Contents/MacOS/Claude",
            Application::Opencode => "OpenCode.app/Contents/MacOS/OpenCode",
        };
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let relative = match application {
            Application::Chatgpt => "chatgpt",
            Application::ClaudeDesktop => "claude",
            Application::Opencode => "opencode-desktop",
        };
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "synthetic desktop executable; never run").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn picker_starts_beside_the_executable_or_in_the_selected_directory() {
    let fixture = Fixture::new();
    for application in [
        Application::Chatgpt,
        Application::ClaudeDesktop,
        Application::Opencode,
    ] {
        let path = fixture.executable(application);
        #[cfg(not(target_os = "macos"))]
        let directory = path.parent().unwrap();
        #[cfg(target_os = "macos")]
        let directory = fixture.root.as_path();
        assert_eq!(picker_directory(path.to_str().unwrap()), Some(directory));
        fs::remove_file(&path).unwrap();
        assert_eq!(picker_directory(path.to_str().unwrap()), Some(directory));
    }
    assert_eq!(
        picker_directory(fixture.root.to_str().unwrap()),
        Some(fixture.root.as_path())
    );
    assert_eq!(
        picker_directory(&format!("  {}  ", fixture.root.display())),
        Some(fixture.root.as_path())
    );
}

#[test]
fn picker_ignores_non_filesystem_or_invalid_starting_paths() {
    for path in [
        "",
        "opencode",
        "relative/app.exe",
        "shell:AppsFolder\\App!App",
    ] {
        assert_eq!(picker_directory(path), None);
    }
    let fixture = Fixture::new();
    assert_eq!(
        picker_directory(fixture.root.join("missing/app.exe").to_str().unwrap()),
        None
    );
    assert_eq!(
        picker_directory(&format!("{}\0", fixture.root.display())),
        None
    );
    assert_eq!(picker_directory(&"/".repeat(8193)), None);
}

#[test]
fn default_detection_does_not_create_preferences() {
    let fixture = Fixture::new();
    for application in [
        Application::Chatgpt,
        Application::ClaudeDesktop,
        Application::Opencode,
    ] {
        assert!(fixture.store.path(application).unwrap().is_none());
        assert!(configured_path_from(&fixture.store, application)
            .unwrap()
            .is_none());
    }
    assert!(!fixture.store.path.exists());
}

#[cfg(target_os = "windows")]
#[test]
fn claude_desktop_msix_path_can_be_saved_and_resolved() {
    let fixture = Fixture::new();
    let path = fixture
        .root
        .join("WindowsApps/AnthropicPBC.Claude_2.37337.1.0_x64__test/app/Claude.exe");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "synthetic desktop executable; never run").unwrap();
    assert_eq!(
        fixture
            .store
            .set(Application::ClaudeDesktop, path.to_str())
            .unwrap(),
        Some(path.clone())
    );
    assert_eq!(
        configured_path_from(&fixture.store, Application::ClaudeDesktop).unwrap(),
        Some(fs::canonicalize(path).unwrap())
    );
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[test]
fn each_application_keeps_its_own_path_across_reopen_and_reset() {
    let fixture = Fixture::new();
    for application in [
        Application::Chatgpt,
        Application::ClaudeDesktop,
        Application::Opencode,
    ] {
        let path = fixture.executable(application);
        assert_eq!(
            fixture.store.set(application, path.to_str()).unwrap(),
            Some(path.clone())
        );
        assert_eq!(
            configured_path_from(&fixture.store, application).unwrap(),
            Some(fs::canonicalize(path).unwrap())
        );
    }
    let reopened = PreferenceStore {
        path: fixture.store.path.clone(),
        lock: Mutex::new(()),
    };
    reopened.set(Application::ClaudeDesktop, None).unwrap();
    assert!(reopened.path(Application::ClaudeDesktop).unwrap().is_none());
    assert!(reopened.path(Application::Chatgpt).unwrap().is_some());
    assert!(reopened.path(Application::Opencode).unwrap().is_some());
}

#[cfg(target_os = "windows")]
#[test]
fn custom_installation_directory_keeps_desktop_identity() {
    let fixture = Fixture::new();
    let original = fixture.executable(Application::Opencode);
    let custom = fixture.root.join("Custom Apps/OpenCode/OpenCode.exe");
    fs::create_dir_all(custom.parent().unwrap()).unwrap();
    fs::rename(original, &custom).unwrap();
    fixture
        .store
        .set(Application::Opencode, custom.to_str())
        .unwrap();
    assert_eq!(
        configured_path_from(&fixture.store, Application::Opencode).unwrap(),
        Some(fs::canonicalize(custom).unwrap())
    );
}

#[test]
fn saving_a_path_does_not_modify_the_executable() {
    let fixture = Fixture::new();
    let path = fixture.executable(Application::Opencode);
    let original = fs::read(&path).unwrap();
    fixture
        .store
        .set(Application::Opencode, path.to_str())
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(fs::read_dir(&fixture.root).unwrap().count(), 2);
}

#[test]
fn missing_selected_file_blocks_launch_instead_of_falling_back() {
    let fixture = Fixture::new();
    let path = fixture.executable(Application::Opencode);
    fixture
        .store
        .set(Application::Opencode, path.to_str())
        .unwrap();
    fs::remove_file(&path).unwrap();
    assert!(configured_path_from(&fixture.store, Application::Opencode).is_err());
    assert_eq!(
        fixture.store.path(Application::Opencode).unwrap(),
        Some(path)
    );
    fixture
        .store
        .set(Application::Opencode, Some("  "))
        .unwrap();
    assert!(configured_path_from(&fixture.store, Application::Opencode)
        .unwrap()
        .is_none());
}

#[test]
fn explicit_environment_path_is_validated_without_fallback() {
    let fixture = Fixture::new();
    let application = Application::Opencode;
    assert_eq!(environment_path(application, None).unwrap(), None);
    assert_eq!(
        environment_path(application, Some("".as_ref())).unwrap(),
        None
    );
    let desktop = fixture.executable(application);
    assert_eq!(
        environment_path(application, Some(desktop.as_os_str())).unwrap(),
        Some(fs::canonicalize(&desktop).unwrap())
    );
    let cli = fixture.root.join(if cfg!(target_os = "windows") {
        "bin/opencode.exe"
    } else {
        "bin/opencode"
    });
    fs::create_dir_all(cli.parent().unwrap()).unwrap();
    fs::copy(&desktop, &cli).unwrap();
    for path in [Path::new("opencode"), cli.as_path(), fixture.root.as_path()] {
        assert!(environment_path(application, Some(path.as_os_str())).is_err());
    }
    fs::remove_file(&desktop).unwrap();
    assert!(environment_path(application, Some(desktop.as_os_str())).is_err());
    assert!(!fixture.store.path.exists());
}

#[test]
fn earlier_cli_on_path_does_not_hide_later_desktop() {
    let fixture = Fixture::new();
    let desktop = fixture.executable(Application::Opencode);
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let desktop = {
        let installed = fixture.root.join("opt/opencode/opencode");
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        fs::copy(desktop, &installed).unwrap();
        installed
    };
    let cli_directory = fixture.root.join("bin");
    let cli = cli_directory.join(desktop.file_name().unwrap());
    fs::create_dir_all(&cli_directory).unwrap();
    fs::copy(&desktop, &cli).unwrap();
    let paths = std::env::join_paths([cli_directory.as_path(), desktop.parent().unwrap()]).unwrap();
    #[cfg(target_os = "windows")]
    let command = desktop.file_stem().unwrap();
    #[cfg(not(target_os = "windows"))]
    let command = desktop.file_name().unwrap();
    let candidates =
        super::super::process::find_commands_on_path(&paths, command.to_str().unwrap());
    assert_eq!(candidates, vec![cli, desktop.clone()]);
    assert_eq!(
        super::super::opencode_desktop::first_desktop_executable(candidates),
        Some(fs::canonicalize(desktop).unwrap())
    );
}

#[cfg(unix)]
#[test]
fn non_executable_autodetected_desktop_is_rejected_before_restart() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let desktop = fixture.executable(Application::Opencode);
    fs::set_permissions(&desktop, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(super::super::opencode_desktop::first_desktop_executable([desktop.clone()]).is_none());
    assert!(environment_path(Application::Opencode, Some(desktop.as_os_str())).is_err());
}

#[test]
fn invalid_paths_do_not_replace_a_saved_path() {
    let fixture = Fixture::new();
    let path = fixture.executable(Application::Opencode);
    fixture
        .store
        .set(Application::Opencode, path.to_str())
        .unwrap();
    let original = fs::read(&fixture.store.path).unwrap();
    for input in [
        "opencode",
        "../opencode",
        "https://example.test/app.exe",
        "shell:AppsFolder\\Other!App",
        "x\n.exe",
    ] {
        assert!(fixture
            .store
            .set(Application::Opencode, Some(input))
            .is_err());
        assert_eq!(fs::read(&fixture.store.path).unwrap(), original);
    }
    let with_arguments = format!("{} --argument", path.display());
    assert!(fixture
        .store
        .set(Application::Opencode, Some(&with_arguments))
        .is_err());
    assert_eq!(fs::read(&fixture.store.path).unwrap(), original);
}

#[test]
fn terminal_clients_scripts_and_other_applications_are_rejected() {
    let fixture = Fixture::new();
    let paths = [
        "bin/claude.exe",
        "bin/opencode.exe",
        "bin/codex.exe",
        "start.cmd",
        "other.exe",
    ];
    for relative in paths {
        let path = fixture.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "synthetic; never run").unwrap();
        for application in [
            Application::Chatgpt,
            Application::ClaudeDesktop,
            Application::Opencode,
        ] {
            assert!(fixture.store.set(application, path.to_str()).is_err());
        }
    }
    assert!(!fixture.store.path.exists());
}

#[test]
fn malformed_or_oversized_preferences_are_not_overwritten() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.store.path.parent().unwrap()).unwrap();
    for content in ["not json".to_string(), " ".repeat(33 * 1024)] {
        fs::write(&fixture.store.path, &content).unwrap();
        assert!(fixture.store.path(Application::Opencode).is_err());
        assert!(fixture.store.set(Application::Opencode, None).is_err());
        assert_eq!(fs::read_to_string(&fixture.store.path).unwrap(), content);
    }
}

#[cfg(unix)]
#[test]
fn preferences_do_not_follow_symlinks() {
    let fixture = Fixture::new();
    let target = fixture.root.join("unrelated.json");
    fs::write(&target, b"{}").unwrap();
    fs::create_dir_all(fixture.store.path.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &fixture.store.path).unwrap();
    assert!(fixture.store.set(Application::Opencode, None).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"{}");
}

#[cfg(unix)]
#[test]
fn launcher_resolves_selected_alias_to_validated_desktop_target() {
    let fixture = Fixture::new();
    let target = fixture.executable(Application::Opencode);
    let alias = fixture.root.join("selected-application");
    std::os::unix::fs::symlink(&target, &alias).unwrap();
    fixture
        .store
        .set(Application::Opencode, alias.to_str())
        .unwrap();
    assert_eq!(
        fixture.store.path(Application::Opencode).unwrap(),
        Some(alias)
    );
    assert_eq!(
        configured_path_from(&fixture.store, Application::Opencode).unwrap(),
        Some(fs::canonicalize(target).unwrap())
    );
}

#[cfg(target_os = "macos")]
#[test]
fn application_bundle_is_normalized_to_its_desktop_executable() {
    let fixture = Fixture::new();
    for application in [
        Application::Chatgpt,
        Application::ClaudeDesktop,
        Application::Opencode,
    ] {
        let executable = fixture.executable(application);
        let bundle = executable.ancestors().nth(3).unwrap();
        assert_eq!(
            fixture.store.set(application, bundle.to_str()).unwrap(),
            Some(executable)
        );
    }
    for bundle in ["OpenCode Beta.app", "OpenCode Dev.app"] {
        let executable = fixture.root.join(bundle).join("Contents/MacOS/OpenCode");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::copy(fixture.executable(Application::Opencode), &executable).unwrap();
        assert_eq!(
            fixture
                .store
                .set(Application::Opencode, fixture.root.join(bundle).to_str())
                .unwrap(),
            Some(executable)
        );
    }
}
