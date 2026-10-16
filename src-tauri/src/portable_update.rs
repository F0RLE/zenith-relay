use serde::Serialize;
use tauri::{ipc::Channel, AppHandle};
use zenith_relay_core::error_codes;

#[cfg(target_os = "windows")]
use std::{env, process::Command};
#[cfg(any(target_os = "windows", test))]
use std::{fs, path::PathBuf};
#[cfg(target_os = "windows")]
use std::{io, thread, time::Duration};

#[cfg(target_os = "windows")]
const HELPER_ARG: &str = "--portable-update-helper";

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
        Err(error_codes::PORTABLE_UPDATE_UNSUPPORTED.to_string())
    }

    #[cfg(target_os = "windows")]
    {
        let target =
            get_portable_update_target().ok_or(error_codes::PORTABLE_UPDATE_UNSUPPORTED)?;
        let paths = helper::update_paths()
            .map_err(|error| format!("{}:{error}", error_codes::PORTABLE_NOT_WRITABLE))?;
        helper::prepare_update_directory(&paths)
            .map_err(|error| format!("{}:{error}", error_codes::PORTABLE_NOT_WRITABLE))?;

        use tauri_plugin_updater::UpdaterExt;
        let updater = app
            .updater_builder()
            .target(target)
            .build()
            .map_err(|error| format!("{}:{error}", error_codes::PORTABLE_UPDATE_FAILED))?;
        let update = updater
            .check()
            .await
            .map_err(|error| format!("{}:{error}", error_codes::PORTABLE_UPDATE_FAILED))?
            .ok_or(error_codes::PORTABLE_UPDATE_UNAVAILABLE)?;
        if update.version != expected_version {
            return Err(error_codes::PORTABLE_UPDATE_UNAVAILABLE.to_string());
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
            .map_err(|error| format!("{}:{error}", error_codes::PORTABLE_UPDATE_FAILED))?;

        helper::write_helper(&paths.helper, &bytes)
            .map_err(|error| format!("{}:{error}", error_codes::PORTABLE_NOT_WRITABLE))?;
        let pid = std::process::id();
        let mut helper = Command::new(&paths.helper);
        helper
            .arg(HELPER_ARG)
            .arg(pid.to_string())
            .arg(&paths.target)
            .arg(&paths.ack);
        helper
            .spawn()
            .map_err(|error| format!("{}:{error}", error_codes::PORTABLE_UPDATE_FAILED))?;

        app.exit(0);
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub fn acknowledge_startup() {
    let Some(ack) = env::var_os(helper::ACK_ENV).map(PathBuf::from) else {
        return;
    };
    if helper::write_acknowledgement(&ack).is_err() {
        return;
    }
    let Some(helper) = env::var_os(helper::HELPER_ENV).map(PathBuf::from) else {
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
        let Some(pid) = args
            .next()
            .and_then(|pid_argument| pid_argument.to_str()?.parse().ok())
        else {
            std::process::exit(2);
        };
        let Some(target) = args.next().map(PathBuf::from) else {
            std::process::exit(2);
        };
        let Some(ack) = args.next().map(PathBuf::from) else {
            std::process::exit(2);
        };
        let helper_result = helper::run_helper(pid, target, ack);
        std::process::exit(if helper_result.is_ok() { 0 } else { 1 });
    }
}

#[cfg(not(target_os = "windows"))]
pub fn run_helper_if_requested() {}
#[cfg(any(target_os = "windows", test))]
mod helper;

#[cfg(test)]
mod tests;
