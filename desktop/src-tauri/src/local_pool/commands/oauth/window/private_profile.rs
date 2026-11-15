use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// Owns only the cache directory exclusively created for one private WebView.
/// Never points at saved accounts, old sign-in profiles or another window.
pub(super) struct PrivateProfile {
    path: PathBuf,
}

impl PrivateProfile {
    pub(super) fn create(cache: &Path) -> io::Result<Self> {
        let root = cache.join("sign-in-sessions");
        fs::create_dir_all(&root)?;
        let path = root.join(uuid::Uuid::new_v4().simple().to_string());
        // Fail rather than attach to an existing profile, even on a collision.
        fs::create_dir(&path)?;
        Ok(Self { path })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn attach(self, window: &tauri::WebviewWindow) {
        let profile = Mutex::new(Some(self));
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                zenith_relay_core::poison::mutex(&profile).take();
            }
        });
    }
}

impl Drop for PrivateProfile {
    fn drop(&mut self) {
        let path = self.path.clone();
        // Destroyed does not mean Chromium has released the directory: its
        // browser/GPU processes can still write caches during shutdown. Keep
        // cleanup bounded and off the UI thread, without unsafe native FFI.
        tauri::async_runtime::spawn_blocking(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while webview_uses_profile(&path).unwrap_or(true) {
                if Instant::now() >= deadline {
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            for attempt in 0..8 {
                match fs::remove_dir_all(&path) {
                    Ok(()) => return,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return,
                    Err(_) if attempt < 7 => {
                        std::thread::sleep(Duration::from_millis(50 << attempt));
                    }
                    Err(_) => return,
                }
            }
        });
    }
}

fn webview_uses_profile(path: &Path) -> Result<bool, sysinfo::Error> {
    let mut system = System::new()?;
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let webviews = system
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            process
                .name()
                .to_str()
                .is_some_and(|name| name.eq_ignore_ascii_case("msedgewebview2.exe"))
                .then_some(*pid)
        })
        .collect::<Vec<_>>();
    if webviews.is_empty() {
        return Ok(false);
    }
    // Read only WebView2 command lines, only during this owned profile's cleanup.
    // No arguments or process metadata leave this function or enter diagnostics.
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&webviews),
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    Ok(webviews
        .into_iter()
        .filter_map(|pid| system.process(pid))
        .any(|process| {
            process
                .cmd()
                .iter()
                .any(|argument| uses_profile(&argument.to_string_lossy(), path))
        }))
}

fn uses_profile(argument: &str, profile: &Path) -> bool {
    let Some(value) = argument.strip_prefix("--user-data-dir=") else {
        return false;
    };
    let value = value.trim_matches('"').replace('/', "\\").to_lowercase();
    let profile = profile.to_string_lossy().replace('/', "\\").to_lowercase();
    value == profile
        || value
            .strip_prefix(&profile)
            .is_some_and(|tail| tail.starts_with('\\'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_matching_is_limited_to_the_owned_profile_directory() {
        let profile = Path::new(r"C:\cache\sign-in-sessions\synthetic");
        assert!(uses_profile(
            r"--user-data-dir=C:\cache\sign-in-sessions\synthetic",
            profile
        ));
        assert!(uses_profile(
            r#"--user-data-dir="c:/CACHE/sign-in-sessions/synthetic/EBWebView""#,
            profile
        ));
        for argument in [
            r"--user-data-dir=C:\cache\sign-in-sessions\synthetic-other",
            r"--user-data-dir=C:\saved-accounts",
            r"--other=C:\cache\sign-in-sessions\synthetic",
        ] {
            assert!(!uses_profile(argument, profile));
        }
    }

    #[tokio::test]
    async fn openings_never_share_storage_and_cleanup_preserves_other_profiles() {
        let root = crate::files::test_temp_root("private-sign-in");
        let saved = root.join("saved-account.json");
        fs::write(&saved, "synthetic saved account").unwrap();
        let first = PrivateProfile::create(&root).unwrap();
        let second = PrivateProfile::create(&root).unwrap();
        assert_ne!(first.path(), second.path());
        assert_eq!(
            first.path().parent().unwrap(),
            root.join("sign-in-sessions")
        );
        fs::write(first.path().join("synthetic-cookie"), "first opening").unwrap();
        assert!(!second.path().join("synthetic-cookie").exists());
        let first_path = first.path().to_path_buf();
        drop(first);
        wait_for_cleanup(&first_path).await;
        assert!(second.path().is_dir());
        assert_eq!(
            fs::read_to_string(&saved).unwrap(),
            "synthetic saved account"
        );
        let second_path = second.path().to_path_buf();
        drop(second);
        wait_for_cleanup(&second_path).await;
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn cleanup_waits_for_webview_file_handles_to_release() {
        let root = crate::files::test_temp_root("private-sign-in-lock");
        let profile = PrivateProfile::create(&root).unwrap();
        let path = profile.path().to_path_buf();
        let locked_path = path.join("synthetic-browser-lock");
        use std::os::windows::fs::OpenOptionsExt;
        let locked = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(locked_path)
            .unwrap();
        drop(profile);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(path.exists());
        drop(locked);
        wait_for_cleanup(&path).await;
        fs::remove_dir_all(root).unwrap();
    }

    async fn wait_for_cleanup(path: &Path) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while path.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}

#[cfg(test)]
mod native_test;
