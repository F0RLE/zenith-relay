use crate::local_pool::accounts::{oauth::OAuthClientKind, oauth_flow::OAuthFlowStart};
use crate::local_pool::error::{ErrorCode, LocalPoolError};
use crate::local_pool::state::DesktopState;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tauri::{
    AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent, utils::config::WindowConfig,
    webview::NewWindowResponse,
};
use url::Url;

pub(super) mod automatic_login;
#[cfg(target_os = "windows")]
mod private_profile;
mod proxy;

use proxy::{ProxyBridge, webview_proxy};

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
    static OPENING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _opening = OPENING.lock().await;
    let (webview_proxy, bridge) = webview_proxy(proxy_url).await?;
    let generation = begin_sign_in_session();
    if let Err(error) = destroy_previous_sign_in_window(app).await {
        release_sign_in_session(generation);
        return Err(error);
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
    if !session_is_current(generation) {
        let _ = window.destroy();
        return Err(window_open_error());
    }
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

async fn destroy_previous_sign_in_window(app: &AppHandle) -> Result<(), LocalPoolError> {
    let Some(window) = app.get_webview_window(SIGN_IN_WINDOW) else {
        return Ok(());
    };
    window.destroy().map_err(|_| window_open_error())?;
    // destroy() queues work on the UI thread. Do not race the old label while
    // creating its replacement or silently continue after a failed teardown.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while app.get_webview_window(SIGN_IN_WINDOW).is_some() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| window_open_error())
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
    webview_proxy: Option<Url>,
    automatic_login: bool,
) -> Result<tauri::WebviewWindow, LocalPoolError> {
    #[cfg(target_os = "windows")]
    if !tauri::webview_version().is_ok_and(|version| supports_private_sign_in(&version)) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "Update Microsoft Edge WebView2 Runtime to open a private sign-in window.",
        ));
    }
    let authorization_url = Url::parse(authorization_url).map_err(|_| window_open_error())?;
    let mut builder =
        WebviewWindowBuilder::from_config(app, &sign_in_window_config(authorization_url))
            .map_err(|_| window_prepare_error())?
            .title(crate::platform::localization::text(
                |copy| &copy.sign_in,
                "Sign in",
            ))
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
    #[cfg(target_os = "windows")]
    let profile = {
        let cache = app
            .path()
            .app_cache_dir()
            .map_err(|_| window_prepare_error())?;
        let profile =
            private_profile::PrivateProfile::create(&cache).map_err(|_| window_prepare_error())?;
        // WebView2 shares an InPrivate profile when its data directory matches.
        // A unique directory also isolates us from any open Hosted preview.
        builder = builder.data_directory(profile.path().to_path_buf());
        profile
    };
    let window = builder.build().map_err(|_| window_open_error())?;
    #[cfg(target_os = "windows")]
    profile.attach(&window);
    Ok(window)
}

fn sign_in_window_config(authorization_url: Url) -> WindowConfig {
    // Only browser state is ephemeral. OAuth state/PKCE and saved credentials
    // remain owned by the existing flow and credential authorities.
    WindowConfig {
        label: SIGN_IN_WINDOW.into(),
        url: WebviewUrl::External(authorization_url),
        incognito: true,
        ..Default::default()
    }
}

#[cfg(any(test, target_os = "windows"))]
fn supports_private_sign_in(version: &str) -> bool {
    let Ok(parts) = version
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
    else {
        return false;
    };
    // Older WebView2 silently ignores the private-mode option.
    parts.len() == 4 && parts.as_slice() >= [101, 0, 1210, 39].as_slice()
}

fn captures_callback_url(client_kind: OAuthClientKind, redirect: &Url, url: &Url) -> bool {
    // Keep loopback callbacks out of the proxy; the flow manager validates state and code.
    matches!(
        client_kind,
        OAuthClientKind::ExcelBps | OAuthClientKind::Siwc
    ) && url.origin() == redirect.origin()
        && url.path() == redirect.path()
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
    fn every_sign_in_requests_private_mode_without_a_saved_data_store() {
        for path in [
            "/authorize?client=codex",
            "/authorize?client=bps",
            "/authorize?client=siwc",
        ] {
            let url = Url::parse(&format!("https://auth.openai.com{path}")).unwrap();
            for _ in 0..2 {
                let config = sign_in_window_config(url.clone());
                assert!(config.incognito);
                assert!(config.data_directory.is_none());
                assert!(config.data_store_identifier.is_none());
                assert_eq!(config.label, SIGN_IN_WINDOW);
                assert_eq!(config.url, WebviewUrl::External(url.clone()));
            }
        }
    }

    #[test]
    fn old_or_unknown_webview_runtimes_cannot_fall_back_to_persistent_sign_in() {
        for version in [
            "100.0.1200.0",
            "101.0.1210.38",
            "",
            "unknown",
            "101.0.1210",
            "101.0.1210.39.1",
        ] {
            assert!(!supports_private_sign_in(version), "{version}");
        }
        for version in ["101.0.1210.39", "101.0.1210.40", "142.0.3595.94"] {
            assert!(supports_private_sign_in(version), "{version}");
        }
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
