#[cfg(target_os = "windows")]
pub(super) const ACK_ENV: &str = "ZENITH_RELAY_PORTABLE_UPDATE_ACK";
#[cfg(target_os = "windows")]
pub(super) const HELPER_ENV: &str = "ZENITH_RELAY_PORTABLE_UPDATE_HELPER";
#[cfg(target_os = "windows")]
const HELPER_WAIT: Duration = Duration::from_secs(120);
#[cfg(any(target_os = "windows", test))]
const FILE_WAIT: Duration = Duration::from_secs(20);

#[cfg(target_os = "windows")]
use std::path::PathBuf;
#[cfg(target_os = "windows")]
use std::{env, process::Command};
#[cfg(any(target_os = "windows", test))]
use std::{
    fs,
    fs::OpenOptions,
    io,
    path::Path,
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "windows")]
#[derive(Debug, Clone)]
pub(super) struct UpdatePaths {
    pub(super) target: PathBuf,
    pub(super) helper: PathBuf,
    pub(super) ack: PathBuf,
    pub(super) temp: PathBuf,
}

#[cfg(target_os = "windows")]
pub(super) fn update_paths() -> io::Result<UpdatePaths> {
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
pub(super) fn prepare_update_directory(paths: &UpdatePaths) -> io::Result<()> {
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
pub(super) fn write_helper(path: &Path, bytes: &[u8]) -> io::Result<()> {
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
pub(super) fn replace_executable(source: &Path, target: &Path, backup: &Path) -> io::Result<()> {
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
pub(super) fn rollback_executable(target: &Path, backup: &Path) -> io::Result<()> {
    retry_io(|| remove_if_exists(target))?;
    retry_io(|| fs::rename(backup, target))
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn write_acknowledgement(path: &Path) -> io::Result<()> {
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
pub(super) fn run_helper(old_pid: u32, target: PathBuf, ack: PathBuf) -> io::Result<()> {
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
