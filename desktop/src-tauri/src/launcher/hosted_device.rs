use crate::key_storage;
use serde::Serialize;
use std::sync::Mutex;
use tauri::WebviewWindow;
use uuid::Uuid;
use zenith_relay_hosted_device::{Destination, DeviceKey, DevicePage};
use zeroize::Zeroizing;
pub(crate) mod copy;

const SLOT: &str = "hosted/desktop-pairing/v1";
const STORAGE: &str = "device_storage_unavailable";
const CHANGED: &str = "device_target_changed";
static CHANGE: Mutex<()> = Mutex::new(());

#[derive(Serialize)]
pub(crate) struct Review {
    destination: Destination,
    invitation: String,
    approval_code: String,
}

fn review(k: &DeviceKey) -> Review {
    Review {
        destination: k.target().clone(),
        invitation: k.target().invitation(),
        approval_code: k.approval_code(),
    }
}

fn load() -> Result<Option<DeviceKey>, &'static str> {
    key_storage::load_named_secret_read_only(SLOT)
        .map_err(|_| STORAGE)?
        .map(|s| DeviceKey::restore(&Zeroizing::new(s)).ok_or(STORAGE))
        .transpose()
}

fn prepare(invitation: &str) -> Result<Review, &'static str> {
    let dest = Destination::parse(invitation)?;
    let _lock = CHANGE.lock().map_err(|_| STORAGE)?;
    if let Some(k) = load()? {
        return if k.target() == &dest {
            Ok(review(&k))
        } else {
            Err(CHANGED)
        };
    }
    let k = DeviceKey::generate(dest)?;
    key_storage::save_named_secret(SLOT, &k.protected_record()).map_err(|_| STORAGE)?;
    Ok(review(&k))
}

fn expected(invitation: &str) -> Result<DeviceKey, &'static str> {
    load()?
        .filter(|k| k.target().invitation() == invitation)
        .ok_or(CHANGED)
}

fn forget(invitation: &str) -> Result<(), &'static str> {
    let _lock = CHANGE.lock().map_err(|_| STORAGE)?;
    expected(invitation)?;
    key_storage::delete_named_secret_result(SLOT).map_err(|_| STORAGE)
}

async fn local<T: Send + 'static>(
    window: &WebviewWindow,
    work: impl FnOnce() -> Result<T, &'static str> + Send + 'static,
) -> Result<T, &'static str> {
    if !crate::tray::is_main_window_label(window.label()) {
        return Err("device_window_forbidden");
    }
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|_| STORAGE)?
}

#[tauri::command]
pub(crate) async fn get_hosted_device(
    window: WebviewWindow,
) -> Result<Option<Review>, &'static str> {
    local(&window, || Ok(load()?.as_ref().map(review))).await
}

#[tauri::command]
pub(crate) async fn prepare_hosted_device(
    window: WebviewWindow,
    invitation: String,
) -> Result<Review, &'static str> {
    local(&window, move || prepare(&invitation)).await
}

#[tauri::command]
pub(crate) async fn forget_hosted_device(
    window: WebviewWindow,
    state: tauri::State<'_, crate::local_pool::DesktopState>,
    invitation: String,
) -> Result<(), &'static str> {
    let _copy = copy::COPYING.lock().await;
    let state = state.inner().clone();
    local(&window, move || {
        if crate::local_pool::hosted_copy::load(&state)?.is_some() {
            return Err("copy_review_pending");
        }
        forget(&invitation)
    })
    .await
}

#[tauri::command]
pub(crate) async fn read_hosted_connections(
    window: WebviewWindow,
    invitation: String,
    after: Option<String>,
) -> Result<DevicePage, &'static str> {
    let cursor = after
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| "invalid_device_cursor")?;
    let target = invitation.clone();
    let key = local(&window, move || expected(&target)).await?;
    let page = key.connections(cursor).await?;
    let same = local(&window, move || {
        Ok(expected(&invitation)?.verifier() == key.verifier())
    })
    .await?;
    if same { Ok(page) } else { Err(CHANGED) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_survives_reload_but_never_overwrites_another_target() {
        let url = |port| {
            format!(
                "http://localhost:{port}/admin/connections#relay-device=00000000-0000-4000-8000-000000000001.00000000-0000-4000-8000-000000000002"
            )
        };
        let a = url(1443);
        let b = url(1444);
        let first = prepare(&a).unwrap();
        assert_eq!(prepare(&a).unwrap().approval_code, first.approval_code);
        assert_eq!(prepare(&b).err(), Some(CHANGED));
        assert_eq!(forget(&b).err(), Some(CHANGED));
        let dto = serde_json::to_string(&first).unwrap();
        let stored = load().unwrap().unwrap().protected_record();
        assert!(!dto.contains(stored.split_once('\n').unwrap().1));
        assert_eq!(expected(&a).unwrap().approval_code(), first.approval_code);
        forget(&a).unwrap();
        assert!(load().unwrap().is_none());
    }
}
