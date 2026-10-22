use crate::local_pool::accounts::{oauth::OAuthClientKind, oauth_flow::OAuthFlowStart};
use crate::local_pool::error::{ErrorCode, LocalPoolError};
use crate::local_pool::state::DesktopState;
use crate::platform::ui_text;
use sha2::{Digest, Sha256};
#[cfg(not(target_os = "macos"))]
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{
    webview::NewWindowResponse, AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
use url::Url;

pub(super) mod automatic_login;
mod proxy;

use proxy::{webview_proxy, ProxyBridge};

const SIGN_IN_WINDOW: &str = "codex-sign-in";
struct SignInSession {
    generation: u64,
    bridge: Option<ProxyBridge>,
    automation: Option<(String, Arc<AtomicBool>)>,
}

fn sign_in_session() -> std::sync::MutexGuard<'static, SignInSession> {
    static SESSION: Mutex<SignInSession> = Mutex::new(SignInSession {
        generation: 0,
        bridge: None,
        automation: None,
    });
    zenith_relay_core::poison::mutex(&SESSION)
}

pub(super) async fn open_sign_in_window(
    app: &AppHandle,
    authorization_url: &str,
    start: &OAuthFlowStart,
    proxy_url: Option<&str>,
    automatic_login: Option<automatic_login::LoginNotes>,
) -> Result<(), LocalPoolError> {
    let profile = sign_in_profile(
        start.client_kind,
        start
            .target_account_id
            .as_deref()
            .or(start.notes_account_id.as_deref()),
    );
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
    let window = match build_sign_in_window(
        app,
        authorization_url,
        start,
        &profile,
        webview_proxy,
        automatic_login.is_some(),
    ) {
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
    if let Some(notes) = automatic_login {
        let active = Arc::new(AtomicBool::new(true));
        {
            let mut session = sign_in_session();
            if session.generation != generation {
                return Err(window_open_error());
            }
            session.automation = Some((start.login_id.clone(), active.clone()));
        }
        let login_id = start.login_id.clone();
        tauri::async_runtime::spawn(automatic_login::run(
            window, generation, login_id, active, notes,
        ));
    } else {
        let _ = window.show();
        let _ = window.set_focus();
    }
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
    stop_automation(&mut session);
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
        stop_automation(&mut session);
    }
}

fn stop_automation(session: &mut SignInSession) {
    if let Some((_, active)) = session.automation.take() {
        active.store(false, Ordering::Release);
    }
}

pub(super) fn continue_automatic_login_manually(app: &AppHandle, login_id: &str) -> bool {
    {
        let session = sign_in_session();
        let Some((current, active)) = &session.automation else {
            return false;
        };
        if current != login_id {
            return false;
        }
        active.store(false, Ordering::Release);
    }
    let Some(window) = app.get_webview_window(SIGN_IN_WINDOW) else {
        return false;
    };
    automatic_login::notify_manual(app, login_id);
    let _ = window.show();
    let _ = window.set_focus();
    true
}

pub(super) fn cancel_automatic_login(app: &AppHandle, login_id: &str) {
    let matches = sign_in_session()
        .automation
        .as_ref()
        .is_some_and(|(current, _)| current == login_id);
    if matches {
        close_sign_in_window(app);
    }
}

fn build_sign_in_window(
    app: &AppHandle,
    authorization_url: &str,
    start: &OAuthFlowStart,
    profile: &str,
    webview_proxy: Option<Url>,
    automatic_login: bool,
) -> Result<tauri::WebviewWindow, LocalPoolError> {
    let authorization_url = Url::parse(authorization_url).map_err(|_| window_open_error())?;
    let mut builder =
        WebviewWindowBuilder::new(app, SIGN_IN_WINDOW, WebviewUrl::External(authorization_url))
            .title(ui_text("Sign in", "Вход"))
            .inner_size(520.0, 780.0)
            .center()
            .resizable(true)
            .focused(!automatic_login)
            .visible(!automatic_login)
            .enable_clipboard_access()
            .devtools(false)
            .on_new_window(|_url, _features| NewWindowResponse::Deny);
    let redirect = Url::parse(&start.redirect_uri).map_err(|_| window_open_error())?;
    let flow = app.state::<DesktopState>().oauth_flow();
    let login_id = start.login_id.clone();
    let client_kind = start.client_kind;
    builder = builder.on_navigation(move |url| {
        if !captures_callback_url(client_kind, &redirect, url) {
            return true;
        }
        let flow = flow.clone();
        let login_id = login_id.clone();
        let callback_url = url.to_string();
        tauri::async_runtime::spawn(async move {
            let _ = flow.submit_manual_callback(&login_id, &callback_url).await;
        });
        false
    });
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

fn captures_callback_url(client_kind: OAuthClientKind, redirect: &Url, url: &Url) -> bool {
    // Keep loopback callbacks out of the proxy; the flow manager validates state and code.
    matches!(
        client_kind,
        OAuthClientKind::ExcelBps | OAuthClientKind::Siwc
    ) && url.origin() == redirect.origin()
        && url.path() == redirect.path()
}

fn sign_in_profile(client_kind: OAuthClientKind, account_id: Option<&str>) -> String {
    let profile = account_profile(account_id);
    match client_kind {
        OAuthClientKind::Codex => profile,
        OAuthClientKind::Siwc => format!("siwc-{profile}"),
        OAuthClientKind::ExcelBps => format!("excel-bps-{profile}"),
    }
}

fn account_profile(account_id: Option<&str>) -> String {
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
    fn stopping_automation_revokes_the_task_without_changing_the_proxy_session() {
        let active = Arc::new(AtomicBool::new(true));
        let mut session = SignInSession {
            generation: 42,
            bridge: None,
            automation: Some(("synthetic-login".into(), active.clone())),
        };
        stop_automation(&mut session);
        assert!(!active.load(Ordering::Acquire));
        assert!(session.automation.is_none());
        assert_eq!(session.generation, 42);
        stop_automation(&mut session);
    }

    #[test]
    fn sign_in_profile_names_do_not_escape_the_profile_directory() {
        assert_eq!(account_profile(None), "new");
        assert_eq!(account_profile(Some(" account_abc ")), "account_abc");
        let escaped = account_profile(Some("../secret"));
        assert!(!escaped.contains('/'));
        assert!(!escaped.contains('\\'));
        assert!(!escaped.contains("secret"));
        assert_ne!(
            profile_store_id("account_one"),
            profile_store_id("account_two")
        );
    }

    #[test]
    fn siwc_webview_profiles_are_separate_without_moving_existing_client_profiles() {
        for (kind, prefix) in [
            (OAuthClientKind::Codex, ""),
            (OAuthClientKind::Siwc, "siwc-"),
            (OAuthClientKind::ExcelBps, "excel-bps-"),
        ] {
            assert_eq!(sign_in_profile(kind, None), format!("{prefix}new"));
            assert_eq!(
                sign_in_profile(kind, Some("account_one")),
                format!("{prefix}account_one")
            );
        }
        assert_ne!(
            profile_store_id(&sign_in_profile(OAuthClientKind::Siwc, Some("account_one"))),
            profile_store_id(&sign_in_profile(
                OAuthClientKind::Codex,
                Some("account_one")
            ))
        );
    }

    #[test]
    fn webview_callbacks_match_only_the_issuing_clients_exact_redirect() {
        let redirect = Url::parse("http://127.0.0.1:1455/auth/callback").unwrap();
        let callback =
            Url::parse("http://127.0.0.1:1455/auth/callback?code=synthetic&state=test").unwrap();
        for kind in [OAuthClientKind::Siwc, OAuthClientKind::ExcelBps] {
            assert!(captures_callback_url(kind, &redirect, &callback));
            for foreign in [
                "https://127.0.0.1:1455/auth/callback",
                "http://localhost:1455/auth/callback",
                "http://127.0.0.1:1456/auth/callback",
                "http://127.0.0.1:1455/callback",
                "https://auth.openai.com/log-in",
            ] {
                assert!(!captures_callback_url(
                    kind,
                    &redirect,
                    &Url::parse(foreign).unwrap()
                ));
            }
        }
        assert!(!captures_callback_url(
            OAuthClientKind::Codex,
            &redirect,
            &callback
        ));
    }
}
