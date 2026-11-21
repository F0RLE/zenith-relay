use crate::local_pool::accounts::{
    oauth::OAuthClientKind,
    oauth_flow::{OAuthFlowStart, OAuthFlowStatus},
};
use crate::local_pool::error::{ErrorCode, LocalPoolError};
use crate::local_pool::state::DesktopState;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tauri::{
    AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
    utils::config::WindowConfig, webview::NewWindowResponse,
};
use url::Url;

pub(super) mod automatic_login;
#[cfg(windows)]
mod embedded;
#[cfg(windows)]
mod private_profile;
mod proxy;

#[cfg(windows)]
pub use embedded::SignInBounds;
#[cfg(windows)]
pub use embedded::{EditAction, EditResult};
use proxy::{ProxyBridge, webview_proxy};

const SIGN_IN_WINDOW: &str = "codex-sign-in";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Presentation {
    Detached,
    #[cfg(windows)]
    Embedded,
}

impl Presentation {
    pub(super) fn requested(_embedded: bool) -> Self {
        #[cfg(windows)]
        if _embedded {
            return Self::Embedded;
        }
        Self::Detached
    }

    pub(super) fn is_embedded(self) -> bool {
        self != Self::Detached
    }
}

#[derive(Clone)]
enum SignInSurface {
    Detached(Arc<WebviewWindow>),
    #[cfg(windows)]
    Embedded(embedded::SignInView),
}

impl SignInSurface {
    fn close(&self) {
        match self {
            Self::Detached(window) => {
                let _ = window.destroy();
            }
            #[cfg(windows)]
            Self::Embedded(view) => view.close(),
        }
    }

    fn reveal(&self) {
        match self {
            Self::Detached(window) => {
                let _ = window.show();
                let _ = window.set_focus();
            }
            #[cfg(windows)]
            Self::Embedded(_) => {} // The renderer owns pane visibility and layout.
        }
    }

    async fn url(&self) -> Option<Url> {
        match self {
            Self::Detached(window) => window.url().ok(),
            #[cfg(windows)]
            Self::Embedded(view) => view.url().await,
        }
    }

    fn evaluate(&self, script: String, callback: impl Fn(String) + Send + 'static) -> bool {
        match self {
            Self::Detached(window) => window.eval_with_callback(script, callback).is_ok(),
            #[cfg(windows)]
            Self::Embedded(view) => view.evaluate(script, callback),
        }
    }
}

struct SignInAttempt {
    login_id: String,
    _bridge: Option<ProxyBridge>,
    surface: Option<SignInSurface>,
    automation: Option<Arc<AtomicBool>>,
    #[cfg(windows)]
    presentation: Presentation,
}

impl Drop for SignInAttempt {
    fn drop(&mut self) {
        if let Some(active) = self.automation.take() {
            active.store(false, Ordering::Release);
        }
    }
}

struct SignInSession {
    generation: u64,
    attempt: Option<SignInAttempt>,
}

fn sign_in_session() -> std::sync::MutexGuard<'static, SignInSession> {
    static SESSION: Mutex<SignInSession> = Mutex::new(SignInSession {
        generation: 0,
        attempt: None,
    });
    zenith_relay_core::poison::mutex(&SESSION)
}

pub(super) async fn open_sign_in_window(
    app: &AppHandle,
    authorization_url: &str,
    start: &OAuthFlowStart,
    proxy_url: Option<&str>,
    automatic_login: Option<automatic_login::LoginNotes>,
    presentation: Presentation,
) -> Result<(), LocalPoolError> {
    static OPENING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _opening = OPENING.lock().await;
    #[cfg(windows)]
    if !tauri::webview_version().is_ok_and(|version| supports_private_sign_in(&version)) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "Update Microsoft Edge WebView2 Runtime to open a private sign-in page.",
        ));
    }
    let authorization_url = Url::parse(authorization_url).map_err(|_| window_open_error())?;
    let navigation = SignInNavigation::new(app, start)?;
    let (webview_proxy, bridge) = webview_proxy(proxy_url).await?;
    let automation = automatic_login
        .as_ref()
        .map(|_| Arc::new(AtomicBool::new(true)));
    let generation = begin_sign_in_session(SignInAttempt {
        login_id: start.login_id.clone(),
        _bridge: bridge,
        surface: None,
        automation: automation.clone(),
        #[cfg(windows)]
        presentation,
    });
    let surface = async {
        destroy_previous_sign_in_window(app).await?;
        if !session_is_current(generation) || !navigation.is_pending() {
            return Err(window_open_error());
        }
        match presentation {
            Presentation::Detached => {
                let window = build_sign_in_window(
                    app,
                    authorization_url,
                    navigation,
                    webview_proxy,
                    automatic_login.is_some(),
                )?;
                window.on_window_event(move |event| {
                    if matches!(
                        event,
                        WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
                    ) {
                        take_sign_in_attempt(|current, _| current == generation);
                    }
                });
                Ok(SignInSurface::Detached(Arc::new(window)))
            }
            #[cfg(windows)]
            Presentation::Embedded => embedded::SignInView::open(
                app,
                generation,
                authorization_url,
                navigation,
                webview_proxy,
            )
            .await
            .map(SignInSurface::Embedded),
        }
    }
    .await;
    let surface = match surface {
        Ok(surface) => surface,
        Err(error) => {
            close_sign_in_attempt(|current, _| current == generation);
            return Err(error);
        }
    };
    {
        let mut session = sign_in_session();
        let is_current = session.generation == generation;
        match session.attempt.as_mut().filter(|_| is_current) {
            Some(attempt) => attempt.surface = Some(surface.clone()),
            None => {
                drop(session);
                surface.close();
                return Err(window_open_error());
            }
        }
    }
    if let (Some(notes), Some(active)) = (automatic_login, automation) {
        tauri::async_runtime::spawn(automatic_login::run(
            app.clone(),
            surface,
            generation,
            start.login_id.clone(),
            active,
            notes,
        ));
    } else {
        surface.reveal();
    }
    let remaining = start
        .expires_at_ms
        .saturating_sub(crate::local_pool::commands::current_time_ms());
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(remaining)).await;
        close_sign_in_attempt(|current, _| current == generation);
    });
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

pub(super) fn close_sign_in_window(login_id: &str) {
    close_sign_in_attempt(|_, attempt| attempt.login_id == login_id);
}

#[cfg(windows)]
pub(super) fn close_embedded_sign_in() {
    close_sign_in_attempt(|_, attempt| attempt.presentation.is_embedded());
}

fn begin_sign_in_session(attempt: SignInAttempt) -> u64 {
    let (generation, previous) = {
        let mut session = sign_in_session();
        session.generation = session.generation.wrapping_add(1);
        (session.generation, session.attempt.replace(attempt))
    };
    if let Some(previous) = previous {
        // The detached window is removed (and awaited) by the opening path.
        #[cfg(windows)]
        if let Some(SignInSurface::Embedded(view)) = &previous.surface {
            view.close();
        }
        drop(previous);
    }
    generation
}

fn session_is_current(generation: u64) -> bool {
    let session = sign_in_session();
    session.generation == generation && session.attempt.is_some()
}

fn take_sign_in_attempt(
    matches: impl FnOnce(u64, &SignInAttempt) -> bool,
) -> Option<SignInAttempt> {
    let mut session = sign_in_session();
    if session
        .attempt
        .as_ref()
        .is_some_and(|attempt| matches(session.generation, attempt))
    {
        return session.attempt.take();
    }
    None
}

fn close_sign_in_attempt(matches: impl FnOnce(u64, &SignInAttempt) -> bool) {
    if let Some(attempt) = take_sign_in_attempt(matches)
        && let Some(surface) = &attempt.surface
    {
        surface.close();
    }
}

pub(super) fn continue_automatic_login_manually(app: &AppHandle, login_id: &str) -> bool {
    let Some(surface) = ({
        let mut session = sign_in_session();
        session
            .attempt
            .as_mut()
            .filter(|attempt| attempt.login_id == login_id)
            .and_then(|attempt| {
                let surface = attempt.surface.clone()?;
                attempt.automation.take()?.store(false, Ordering::Release);
                Some(surface)
            })
    }) else {
        return false;
    };
    automatic_login::notify_manual(app, login_id);
    surface.reveal();
    #[cfg(windows)]
    if let SignInSurface::Embedded(view) = &surface {
        view.focus();
    }
    true
}

#[cfg(windows)]
fn embedded_view(login_id: &str) -> Option<embedded::SignInView> {
    match sign_in_session()
        .attempt
        .as_ref()
        .filter(|attempt| attempt.login_id == login_id)?
        .surface
        .as_ref()?
    {
        SignInSurface::Embedded(view) => Some(view.clone()),
        SignInSurface::Detached(_) => None,
    }
}

pub(super) fn focus_embedded_sign_in(_login_id: &str) -> bool {
    #[cfg(windows)]
    if let Some(view) = embedded_view(_login_id) {
        view.focus();
        return true;
    }
    false
}

#[cfg(windows)]
pub(super) async fn place_embedded_sign_in(
    login_id: &str,
    revision: u64,
    bounds: Option<SignInBounds>,
) -> Result<bool, LocalPoolError> {
    let Some(view) = embedded_view(login_id) else {
        return Ok(false);
    };
    view.place(revision, bounds).await
}

#[cfg(windows)]
pub(super) async fn edit_embedded_sign_in(
    login_id: &str,
    request_id: String,
    action: EditAction,
    value: Option<String>,
) -> Result<EditResult, LocalPoolError> {
    let view = embedded_view(login_id).ok_or_else(window_open_error)?;
    view.edit(request_id, action, value).await
}

fn build_sign_in_window(
    app: &AppHandle,
    authorization_url: Url,
    navigation: SignInNavigation,
    webview_proxy: Option<Url>,
    automatic_login: bool,
) -> Result<WebviewWindow, LocalPoolError> {
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
            .on_new_window(|_url, _features| NewWindowResponse::Deny)
            .on_navigation(move |url| !navigation.intercept_callback(url));
    if let Some(proxy) = webview_proxy {
        builder = builder.proxy_url(proxy);
    }
    #[cfg(target_os = "windows")]
    let profile = {
        let profile = create_private_profile(app)?;
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

#[cfg(windows)]
fn create_private_profile(
    app: &AppHandle,
) -> Result<private_profile::PrivateProfile, LocalPoolError> {
    let cache = app
        .path()
        .app_cache_dir()
        .map_err(|_| window_prepare_error())?;
    private_profile::PrivateProfile::create(&cache).map_err(|_| window_prepare_error())
}

struct SignInNavigation {
    app: AppHandle,
    login_id: String,
    redirect: Url,
    client_kind: OAuthClientKind,
}

impl SignInNavigation {
    fn new(app: &AppHandle, start: &OAuthFlowStart) -> Result<Self, LocalPoolError> {
        Ok(Self {
            app: app.clone(),
            login_id: start.login_id.clone(),
            redirect: Url::parse(&start.redirect_uri).map_err(|_| window_open_error())?,
            client_kind: start.client_kind,
        })
    }

    fn is_pending(&self) -> bool {
        self.app
            .state::<DesktopState>()
            .oauth_flow()
            .status(&self.login_id)
            .is_ok_and(|start| {
                start.status == OAuthFlowStatus::Pending
                    && start.expires_at_ms > crate::local_pool::commands::current_time_ms()
            })
    }

    fn intercept_callback(&self, url: &Url) -> bool {
        if !captures_callback_url(self.client_kind, &self.redirect, url) {
            return false;
        }
        let flow = self.app.state::<DesktopState>().oauth_flow();
        let login_id = self.login_id.clone();
        let callback_url = url.to_string();
        tauri::async_runtime::spawn(async move {
            let _ = flow.submit_manual_callback(&login_id, &callback_url).await;
        });
        true
    }
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
    fn dropping_the_attempt_revokes_automation() {
        let active = Arc::new(AtomicBool::new(true));
        let attempt = SignInAttempt {
            login_id: "synthetic-login".into(),
            _bridge: None,
            surface: None,
            automation: Some(active.clone()),
            #[cfg(windows)]
            presentation: Presentation::Embedded,
        };
        drop(attempt);
        assert!(!active.load(Ordering::Acquire));
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
