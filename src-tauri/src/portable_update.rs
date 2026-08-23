use serde::Serialize;
use tauri::{ipc::Channel, AppHandle};

#[cfg(target_os = "windows")]
use std::{env, process::Command};
#[cfg(any(target_os = "windows", test))]
use std::{
    fs,
    fs::OpenOptions,
    io,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "windows")]
const HELPER_ARG: &str = "--portable-update-helper";
#[cfg(target_os = "windows")]
const ACK_ENV: &str = "ZENITH_RELAY_PORTABLE_UPDATE_ACK";
#[cfg(target_os = "windows")]
const HELPER_ENV: &str = "ZENITH_RELAY_PORTABLE_UPDATE_HELPER";
#[cfg(target_os = "windows")]
const HELPER_WAIT: Duration = Duration::from_secs(120);
#[cfg(any(target_os = "windows", test))]
const FILE_WAIT: Duration = Duration::from_secs(20);

#[derive(Clone, Serialize)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[serde(tag = "event", content = "data")]
pub enum DownloadEvent {
    #[serde(rename_all = "camelCase")]
    Started {
        content_length: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Progress {
        chunk_length: usize,
    },
    Finished,
}

#[tauri::command]
pub fn get_portable_update_target() -> Option<&'static str> {
    if cfg!(debug_assertions) {
        return None;
    }

    #[cfg(target_os = "windows")]
    {
        if tauri::utils::platform::bundle_type().is_none() {
            portable_target()
        } else {
            None
        }
    }

    #[cfg(not(target_os = "windows"))]
    None
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn portable_target() -> Option<&'static str> {
    Some("windows-x86_64-portable")
}

#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
fn portable_target() -> Option<&'static str> {
    Some("windows-aarch64-portable")
}

#[cfg(all(
    target_os = "windows",
    not(any(target_arch = "x86_64", target_arch = "aarch64"))
))]
fn portable_target() -> Option<&'static str> {
    None
}

#[tauri::command]
pub async fn install_portable_update(
    app: AppHandle,
    expected_version: String,
    on_event: Channel<DownloadEvent>,
) -> Result<(), String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, expected_version, on_event);
        Err("portable_update_unsupported".to_string())
    }

    #[cfg(target_os = "windows")]
    {
        let target = get_portable_update_target().ok_or("portable_update_unsupported")?;
        let paths = update_paths().map_err(|error| format!("portable_not_writable:{error}"))?;
        prepare_update_directory(&paths)
            .map_err(|error| format!("portable_not_writable:{error}"))?;

        use tauri_plugin_updater::UpdaterExt;
        let updater = app
            .updater_builder()
            .target(target)
            .build()
            .map_err(|error| format!("portable_update_failed:{error}"))?;
        let update = updater
            .check()
            .await
            .map_err(|error| format!("portable_update_failed:{error}"))?
            .ok_or("portable_update_unavailable")?;
        if update.version != expected_version {
            return Err("portable_update_unavailable".to_string());
        }

        let mut first_chunk = true;
        let bytes = update
            .download(
                |chunk_length, content_length| {
                    if first_chunk {
                        first_chunk = false;
                        let _ = on_event.send(DownloadEvent::Started { content_length });
                    }
                    let _ = on_event.send(DownloadEvent::Progress { chunk_length });
                },
                || {
                    let _ = on_event.send(DownloadEvent::Finished);
                },
            )
            .await
            .map_err(|error| format!("portable_update_failed:{error}"))?;

        write_helper(&paths.helper, &bytes)
            .map_err(|error| format!("portable_not_writable:{error}"))?;
        let pid = std::process::id();
        let mut helper = Command::new(&paths.helper);
        helper
            .arg(HELPER_ARG)
            .arg(pid.to_string())
            .arg(&paths.target)
            .arg(&paths.ack);
        helper
            .spawn()
            .map_err(|error| format!("portable_update_failed:{error}"))?;

        app.exit(0);
        Ok(())
    }
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone)]
struct UpdatePaths {
    target: PathBuf,
    helper: PathBuf,
    ack: PathBuf,
    temp: PathBuf,
}

#[cfg(target_os = "windows")]
fn update_paths() -> io::Result<UpdatePaths> {
    let target = env::current_exe()?;
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::other("executable has no parent directory"))?;
    let stem = target
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io::Error::other("executable has no valid file name"))?;
    let helper = parent.join(format!("{stem}.update.exe"));
    let temp = parent.join(format!("{stem}.update.exe.tmp"));
    let ack = parent.join(format!(".zenith-relay-update-{}.ack", std::process::id()));
    Ok(UpdatePaths {
        target,
        helper,
        ack,
        temp,
    })
}

#[cfg(target_os = "windows")]
fn prepare_update_directory(paths: &UpdatePaths) -> io::Result<()> {
    remove_if_exists(&paths.helper)?;
    remove_if_exists(&paths.temp)?;
    remove_if_exists(&paths.ack)?;
    let probe = paths
        .target
        .parent()
        .ok_or_else(|| io::Error::other("executable has no parent directory"))?
        .join(format!(".zenith-relay-write-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe)?;
    use std::io::Write;
    file.write_all(b"ok")?;
    file.sync_all()?;
    drop(file);
    remove_if_exists(&probe)
}

#[cfg(target_os = "windows")]
fn write_helper(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension("exe.tmp");
    remove_if_exists(&temp)?;
    fs::write(&temp, bytes)?;
    OpenOptions::new().write(true).open(&temp)?.sync_all()?;
    fs::rename(temp, path)
}

#[cfg(target_os = "windows")]
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    value.into()
}

#[cfg(any(target_os = "windows", test))]
fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(any(target_os = "windows", test))]
fn retry_io<T, F>(mut operation: F) -> io::Result<T>
where
    F: FnMut() -> io::Result<T>,
{
    let deadline = Instant::now() + FILE_WAIT;
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(_error) if Instant::now() < deadline => thread::sleep(Duration::from_millis(150)),
            Err(error) => return Err(error),
        }
    }
}

#[cfg(any(target_os = "windows", test))]
fn replace_executable(source: &Path, target: &Path, backup: &Path) -> io::Result<()> {
    retry_io(|| remove_if_exists(backup))?;
    retry_io(|| fs::rename(target, backup))?;
    let copy_result = retry_io(|| {
        fs::copy(source, target)?;
        let file = OpenOptions::new().write(true).open(target)?;
        file.sync_all()
    });
    if let Err(error) = copy_result {
        return match rollback_executable(target, backup) {
            Ok(()) => Err(error),
            Err(rollback) => Err(io::Error::other(format!(
                "copy failed ({error}); rollback failed ({rollback})"
            ))),
        };
    }
    Ok(())
}

#[cfg(any(target_os = "windows", test))]
fn rollback_executable(target: &Path, backup: &Path) -> io::Result<()> {
    retry_io(|| remove_if_exists(target))?;
    retry_io(|| fs::rename(backup, target))
}

#[cfg(any(target_os = "windows", test))]
fn write_acknowledgement(path: &Path) -> io::Result<()> {
    fs::write(path, b"ready")
}

#[cfg(target_os = "windows")]
fn relaunch(path: &Path) {
    let _ = Command::new(path).spawn();
}

#[cfg(target_os = "windows")]
fn process_is_running(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).is_some()
}

#[cfg(target_os = "windows")]
fn wait_for_process_exit(pid: u32) -> bool {
    let deadline = Instant::now() + HELPER_WAIT;
    while process_is_running(pid) {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(200));
    }
    true
}

#[cfg(target_os = "windows")]
fn validate_helper_paths(helper: &Path, target: &Path, ack: &Path) -> io::Result<()> {
    let helper_parent = helper
        .parent()
        .ok_or_else(|| io::Error::other("helper has no parent directory"))?
        .canonicalize()?;
    let target_parent = target
        .parent()
        .ok_or_else(|| io::Error::other("target has no parent directory"))?
        .canonicalize()?;
    let ack_parent = ack
        .parent()
        .ok_or_else(|| io::Error::other("acknowledgement has no parent directory"))?
        .canonicalize()?;
    if helper_parent != target_parent || helper_parent != ack_parent {
        return Err(io::Error::other("update files must share one directory"));
    }
    if target.file_name() == helper.file_name() {
        return Err(io::Error::other("helper cannot replace itself"));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn wait_for_ack(child: &mut std::process::Child, ack: &Path) -> bool {
    let deadline = Instant::now() + HELPER_WAIT;
    loop {
        if ack.exists() {
            return true;
        }
        if child.try_wait().ok().flatten().is_some() {
            return false;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(target_os = "windows")]
fn run_helper(old_pid: u32, target: PathBuf, ack: PathBuf) -> io::Result<()> {
    let helper = env::current_exe()?;
    if !wait_for_process_exit(old_pid) {
        relaunch(&target);
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "the previous Relay process did not exit",
        ));
    }
    validate_helper_paths(&helper, &target, &ack).inspect_err(|_| {
        relaunch(&target);
    })?;
    let backup = with_suffix(&target, ".bak");
    replace_executable(&helper, &target, &backup).inspect_err(|_| {
        relaunch(&target);
    })?;

    let mut child = match Command::new(&target)
        .env(ACK_ENV, &ack)
        .env(HELPER_ENV, &helper)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = rollback_executable(&target, &backup);
            relaunch(&target);
            return Err(error);
        }
    };

    if wait_for_ack(&mut child, &ack) {
        let _ = remove_if_exists(&ack);
        let _ = remove_if_exists(&backup);
        return Ok(());
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = remove_if_exists(&ack);
    let rollback = rollback_executable(&target, &backup);
    relaunch(&target);
    rollback
}

#[cfg(target_os = "windows")]
pub fn acknowledge_startup() {
    let Some(ack) = env::var_os(ACK_ENV).map(PathBuf::from) else {
        return;
    };
    if write_acknowledgement(&ack).is_err() {
        return;
    }
    let Some(helper) = env::var_os(HELPER_ENV).map(PathBuf::from) else {
        return;
    };
    thread::spawn(move || {
        for _ in 0..240 {
            match fs::remove_file(&helper) {
                Ok(()) => break,
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Err(_) => thread::sleep(Duration::from_millis(250)),
            }
        }
    });
}

#[cfg(not(target_os = "windows"))]
pub fn acknowledge_startup() {}

#[cfg(target_os = "windows")]
pub fn run_helper_if_requested() {
    let mut args = env::args_os().skip(1);
    while let Some(argument) = args.next() {
        if argument != HELPER_ARG {
            continue;
        }
        let Some(pid) = args.next().and_then(|value| value.to_str()?.parse().ok()) else {
            std::process::exit(2);
        };
        let Some(target) = args.next().map(PathBuf::from) else {
            std::process::exit(2);
        };
        let Some(ack) = args.next().map(PathBuf::from) else {
            std::process::exit(2);
        };
        let result = run_helper(pid, target, ack);
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
}

#[cfg(not(target_os = "windows"))]
pub fn run_helper_if_requested() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_dir(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("zenith-relay-portable-{label}-{stamp}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn replacement_writes_new_bytes_and_keeps_backup_until_success() {
        let root = temp_dir("replace");
        let source = root.join("update.exe");
        let target = root.join("relay.exe");
        let backup = root.join("relay.exe.bak");
        fs::write(&source, b"new").unwrap();
        fs::write(&target, b"old").unwrap();

        replace_executable(&source, &target, &backup).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert_eq!(fs::read(&backup).unwrap(), b"old");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn acknowledgement_is_written_for_the_new_process() {
        let root = temp_dir("ack");
        let ack = root.join("ready.ack");

        write_acknowledgement(&ack).unwrap();

        assert_eq!(fs::read(&ack).unwrap(), b"ready");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rollback_restores_the_previous_executable() {
        let root = temp_dir("rollback");
        let source = root.join("update.exe");
        let target = root.join("relay.exe");
        let backup = root.join("relay.exe.bak");
        fs::write(&source, b"new").unwrap();
        fs::write(&target, b"old").unwrap();
        replace_executable(&source, &target, &backup).unwrap();

        rollback_executable(&target, &backup).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(!backup.exists());
        let _ = fs::remove_dir_all(root);
    }
}
