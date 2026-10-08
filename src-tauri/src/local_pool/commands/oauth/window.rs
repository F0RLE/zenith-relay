use crate::local_pool::error::{ErrorCode, LocalPoolError};
use crate::platform::ui_text;
use sha2::{Digest, Sha256};
#[cfg(not(target_os = "macos"))]
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{
    webview::NewWindowResponse, AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
use url::Url;

mod proxy;

use proxy::{webview_proxy, ProxyBridge};

const SIGN_IN_WINDOW: &str = "codex-sign-in";
struct SignInSession {
    generation: u64,
    bridge: Option<ProxyBridge>,
}

fn sign_in_session() -> std::sync::MutexGuard<'static, SignInSession> {
    static SESSION: Mutex<SignInSession> = Mutex::new(SignInSession {
        generation: 0,
        bridge: None,
    });
    zenith_relay_core::poison::mutex(&SESSION)
}

pub(super) async fn open_sign_in_window(
    app: &AppHandle,
    authorization_url: &str,
    account_id: Option<&str>,
    proxy_url: Option<&str>,
) -> Result<(), LocalPoolError> {
    let profile = sign_in_profile(account_id);
    let (webview_proxy, bridge) = webview_proxy(proxy_url).await?;
    let generation = begin_sign_in_session();
    if let Some(window) = app.get_webview_window(SIGN_IN_WINDOW) {
        let _ = window.destroy();
    }
    if let Some(bridge) = bridge {
        if !store_sign_in_bridge(generation, bridge) {
            return Err(window_open_error());
        }
    } else if !session_is_current(generation) {
        return Err(window_open_error());
    }
    let window = match build_sign_in_window(app, authorization_url, &profile, webview_proxy) {
        Ok(window) => window,
        Err(error) => {
            release_sign_in_session(generation);
            return Err(error);
        }
    };
    window.on_window_event(move |event| {
        if matches!(
            event,
            WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
        ) {
            release_sign_in_session(generation);
        }
    });
    let _ = window.show();
    let _ = window.set_focus();
    Ok(())
}

pub(crate) fn close_sign_in_window(app: &AppHandle) {
    let _generation = begin_sign_in_session();
    if let Some(window) = app.get_webview_window(SIGN_IN_WINDOW) {
        let _ = window.destroy();
    }
}

fn begin_sign_in_session() -> u64 {
    let mut session = sign_in_session();
    session.generation = session.generation.wrapping_add(1);
    session.bridge.take();
    session.generation
}

fn session_is_current(generation: u64) -> bool {
    sign_in_session().generation == generation
}

fn store_sign_in_bridge(generation: u64, bridge: ProxyBridge) -> bool {
    let mut session = sign_in_session();
    if session.generation != generation {
        drop(bridge);
        return false;
    }
    session.bridge = Some(bridge);
    true
}

fn release_sign_in_session(generation: u64) {
    let mut session = sign_in_session();
    if session.generation == generation {
        session.bridge.take();
    }
}

fn build_sign_in_window(
    app: &AppHandle,
    authorization_url: &str,
    profile: &str,
    webview_proxy: Option<Url>,
) -> Result<tauri::WebviewWindow, LocalPoolError> {
    let authorization_url = Url::parse(authorization_url).map_err(|_| window_open_error())?;
    let mut builder =
        WebviewWindowBuilder::new(app, SIGN_IN_WINDOW, WebviewUrl::External(authorization_url))
            .title(ui_text("Sign in", "Вход"))
            .inner_size(520.0, 780.0)
            .center()
            .resizable(true)
            .focused(true)
            .visible(true)
            .enable_clipboard_access()
            .devtools(false)
            .on_new_window(|_url, _features| NewWindowResponse::Deny);
    if let Some(proxy) = webview_proxy {
        builder = builder.proxy_url(proxy);
    }
    builder = apply_sign_in_profile(builder, app, profile)?;
    builder.build().map_err(|_| window_open_error())
}

fn apply_sign_in_profile<'a>(
    builder: WebviewWindowBuilder<'a, tauri::Wry, AppHandle>,
    app: &AppHandle,
    profile: &str,
) -> Result<WebviewWindowBuilder<'a, tauri::Wry, AppHandle>, LocalPoolError> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        Ok(builder.data_store_identifier(profile_store_id(profile)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(builder.data_directory(sign_in_profile_directory(app, profile)?))
    }
}

#[cfg(not(target_os = "macos"))]
fn sign_in_profile_directory(app: &AppHandle, profile: &str) -> Result<PathBuf, LocalPoolError> {
    let root = crate::platform::relay_dir(app).map_err(|_| window_prepare_error())?;
    let directory = crate::storage_paths::StoragePaths::from_root(root)
        .cache_root()
        .join("sign-in")
        .join(profile);
    crate::platform::ensure_real_directory(&directory).map_err(|_| window_prepare_error())?;
    Ok(directory)
}

fn sign_in_profile(account_id: Option<&str>) -> String {
    let Some(account_id) = account_id
        .map(str::trim)
        .filter(|account_id_text| !account_id_text.is_empty())
    else {
        return "new".to_string();
    };
    if (1..=80).contains(&account_id.len())
        && account_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return account_id.to_string();
    }
    format!(
        "account_{}",
        hex::encode(Sha256::digest(account_id.as_bytes()))
    )
}

#[cfg(any(test, target_os = "macos"))]
fn profile_store_id(profile: &str) -> [u8; 16] {
    let digest = Sha256::digest(profile.as_bytes());
    let mut identifier = [0_u8; 16];
    identifier.copy_from_slice(&digest[..16]);
    identifier
}

pub(super) fn window_prepare_error() -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, "The sign-in window could not be prepared.")
}

fn window_open_error() -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, "The sign-in window could not be opened.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_profile_names_do_not_escape_the_profile_directory() {
        assert_eq!(sign_in_profile(None), "new");
        assert_eq!(sign_in_profile(Some(" account_abc ")), "account_abc");
        let escaped = sign_in_profile(Some("../secret"));
        assert!(!escaped.contains('/'));
        assert!(!escaped.contains('\\'));
        assert!(!escaped.contains("secret"));
        assert_ne!(
            profile_store_id("account_one"),
            profile_store_id("account_two")
        );
    }
}
