use std::cell::RefCell;

use tauri::{AppHandle, Manager, WebviewWindow};
use url::Url;
use wry::{
    NewWindowResponse, PermissionResponse, ProxyConfig, ProxyEndpoint, Rect, WebContext, WebView,
    WebViewBuilder,
    dpi::{PhysicalPosition, PhysicalSize},
};

use super::{
    SignInNavigation, create_private_profile, private_profile::PrivateProfile, session_is_current,
    window_open_error,
};
use crate::local_pool::{accounts::oauth::OAuthClientKind, error::LocalPoolError};

mod edit_menu;
pub use edit_menu::{EditAction, EditResult};

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignInBounds {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl SignInBounds {
    fn within(self, parent: PhysicalSize<u32>) -> Option<Rect> {
        let width = self.width.min(parent.width.checked_sub(self.x)?);
        let height = self.height.min(parent.height.checked_sub(self.y)?);
        if width == 0 || height == 0 {
            return None;
        }
        Some(Rect {
            position: PhysicalPosition::new(
                i32::try_from(self.x).ok()?,
                i32::try_from(self.y).ok()?,
            )
            .into(),
            size: PhysicalSize::new(width, height).into(),
        })
    }
}

struct NativeView {
    generation: u64,
    // Drop the controller before starting directory cleanup.
    view: WebView,
    _profile: PrivateProfile,
    visible: bool,
    revision: u64,
    menu: Option<edit_menu::Pending>,
}

impl NativeView {
    fn show(&mut self, bounds: Option<Rect>) -> Result<(), LocalPoolError> {
        if let Some(bounds) = bounds
            && self.view.set_bounds(bounds).is_err()
        {
            // A failed resize must not leave the provider over the Relay controls.
            self.set_visible(false)?;
            return Err(window_open_error());
        }
        self.set_visible(bounds.is_some())
    }

    fn set_visible(&mut self, visible: bool) -> Result<(), LocalPoolError> {
        if self.visible != visible {
            self.view
                .set_visible(visible)
                .map_err(|_| window_open_error())?;
            self.visible = visible;
        }
        Ok(())
    }
}

thread_local! {
    static SIGN_IN_VIEW: RefCell<Option<NativeView>> = const { RefCell::new(None) };
}

#[derive(Clone)]
pub(super) struct SignInView {
    app: AppHandle,
    generation: u64,
}

impl SignInView {
    pub(super) async fn open(
        app: &AppHandle,
        generation: u64,
        authorization_url: Url,
        navigation: SignInNavigation,
        proxy: Option<Url>,
    ) -> Result<Self, LocalPoolError> {
        let profile = create_private_profile(app)?;
        let handle = Self {
            app: app.clone(),
            generation,
        };
        let app = app.clone();
        let login_id = navigation.login_id.clone();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        handle
            .app
            .run_on_main_thread(move || {
                let result = (|| {
                    let parent = main_window(&app).ok_or_else(window_open_error)?;
                    if !session_is_current(generation) {
                        return Err(window_open_error());
                    }
                    // The provider has no Tauri command access. Its only message
                    // is bounded edit-menu metadata; actions originate in Relay.
                    let menu_app = app.clone();
                    let view = build(
                        &parent,
                        profile.path(),
                        authorization_url,
                        proxy,
                        move |raw| {
                            if !session_is_current(generation) {
                                return false;
                            }
                            let Ok(url) = Url::parse(&raw) else {
                                return false;
                            };
                            !navigation.intercept_callback(&url)
                                && allowed_page(navigation.client_kind, &navigation.redirect, &url)
                        },
                        move |raw| edit_menu::requested(&menu_app, generation, &login_id, raw),
                    )?;
                    if !session_is_current(generation) {
                        return Err(window_open_error());
                    }
                    let previous = SIGN_IN_VIEW.with(|slot| {
                        slot.replace(Some(NativeView {
                            generation,
                            view,
                            _profile: profile,
                            visible: false,
                            revision: 0,
                            menu: None,
                        }))
                    });
                    drop(previous);
                    Ok(())
                })();
                let _ = sender.send(result);
            })
            .map_err(|_| window_open_error())?;
        receiver.await.map_err(|_| window_open_error())??;
        Ok(handle)
    }

    pub(super) fn close(&self) {
        let generation = self.generation;
        let _ = self.app.run_on_main_thread(move || {
            let previous = SIGN_IN_VIEW.with_borrow_mut(|slot| {
                if slot
                    .as_ref()
                    .is_some_and(|view| view.generation == generation)
                {
                    slot.take()
                } else {
                    None
                }
            });
            drop(previous);
        });
    }

    pub(super) async fn place(
        &self,
        revision: u64,
        bounds: Option<SignInBounds>,
    ) -> Result<bool, LocalPoolError> {
        let app = self.app.clone();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        if !self.dispatch(move |native| {
            let result = (|| {
                if revision <= native.revision {
                    return Ok(true);
                }
                native.revision = revision;
                if let Some(bounds) = bounds {
                    let parent = main_window(&app).ok_or_else(window_open_error)?;
                    let parent_size = parent.inner_size().map_err(|_| window_open_error())?;
                    let clipped = bounds.within(parent_size);
                    native.show(clipped)?;
                    return Ok(clipped.is_some());
                }
                native.show(None)?;
                Ok(true)
            })();
            let _ = sender.send(result);
        }) {
            return Err(window_open_error());
        }
        receiver.await.unwrap_or(Ok(false))
    }

    pub(super) fn focus(&self) {
        self.dispatch(|native| {
            if native.visible {
                let _ = native.view.focus();
            }
        });
    }

    pub(super) async fn url(&self) -> Option<Url> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.dispatch(move |native| {
            let _ = sender.send(native.view.url().ok().and_then(|url| Url::parse(&url).ok()));
        });
        receiver.await.ok()?
    }

    pub(super) fn evaluate(
        &self,
        script: String,
        callback: impl Fn(String) + Send + 'static,
    ) -> bool {
        self.dispatch(move |native| {
            let _ = native.view.evaluate_script_with_callback(&script, callback);
        })
    }

    pub(super) async fn edit(
        &self,
        request_id: String,
        action: EditAction,
        value: Option<String>,
    ) -> Result<EditResult, LocalPoolError> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.dispatch(move |native| {
            if action == EditAction::Dismiss
                && native
                    .menu
                    .as_ref()
                    .is_none_or(|menu| !menu.matches(&request_id))
            {
                let _ = sender.send(Ok(EditResult {
                    ok: true,
                    text: None,
                }));
                return;
            }
            let script = native
                .menu
                .as_ref()
                .ok_or_else(edit_menu::unavailable)
                .and_then(|menu| menu.script(&request_id, action, value.as_deref()));
            let script = match script {
                Ok(script) => script,
                Err(error) => {
                    let _ = sender.send(Err(error));
                    return;
                }
            };
            if action != EditAction::Read {
                native.menu = None;
            }
            let sender = std::sync::Mutex::new(Some(sender));
            let _ = native
                .view
                .evaluate_script_with_callback(&script, move |raw| {
                    let result = serde_json::from_str::<EditResult>(&raw)
                        .ok()
                        .filter(|result| {
                            result.ok
                                && result
                                    .text
                                    .as_ref()
                                    .is_none_or(|text| text.len() <= 1_048_576)
                        })
                        .ok_or_else(edit_menu::unavailable);
                    if let Some(sender) = zenith_relay_core::poison::mutex(&sender).take() {
                        let _ = sender.send(result);
                    }
                });
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), receiver)
            .await
            .map_err(|_| edit_menu::unavailable())?
            .map_err(|_| edit_menu::unavailable())?
    }

    fn dispatch(&self, update: impl FnOnce(&mut NativeView) + Send + 'static) -> bool {
        let generation = self.generation;
        self.app
            .run_on_main_thread(move || {
                if !session_is_current(generation) {
                    return;
                }
                SIGN_IN_VIEW.with_borrow_mut(|slot| {
                    if let Some(view) = slot.as_mut().filter(|view| view.generation == generation) {
                        update(view);
                    }
                });
            })
            .is_ok()
    }
}

fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(crate::tray::MAIN_WINDOW_LABEL)
}

pub(super) fn build(
    parent: &WebviewWindow,
    profile: &std::path::Path,
    url: Url,
    proxy: Option<Url>,
    navigate: impl Fn(String) -> bool + 'static,
    edit_menu: impl Fn(&str) + 'static,
) -> Result<WebView, LocalPoolError> {
    let mut context = WebContext::new(Some(profile.to_path_buf()));
    let mut builder = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(url.as_str())
        .with_incognito(true)
        .with_general_autofill_enabled(false)
        .with_visible(false)
        .with_focused(false)
        .with_devtools(false)
        .with_initialization_script(edit_menu::SCRIPT)
        .with_ipc_handler(move |request| edit_menu(request.body()))
        .with_navigation_handler(navigate)
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_download_started_handler(|_, _| false)
        .with_permission_handler(|_| PermissionResponse::Deny);
    if let Some(proxy) = proxy {
        let host = proxy.host_str().ok_or_else(window_open_error)?;
        let port = proxy
            .port_or_known_default()
            .ok_or_else(window_open_error)?;
        builder = builder.with_proxy_config(ProxyConfig::Http(ProxyEndpoint {
            host: host.into(),
            port: port.to_string(),
        }));
    }
    builder
        .build_as_child(parent)
        .map_err(|_| window_open_error())
}

fn allowed_page(kind: OAuthClientKind, redirect: &Url, url: &Url) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    if kind == OAuthClientKind::Codex
        && url.origin() == redirect.origin()
        && url.path() == redirect.path()
    {
        return true;
    }
    url.as_str() == "about:blank"
        || (url.scheme() == "https"
            && url.port_or_known_default() == Some(443)
            && url
                .domain()
                .is_some_and(|domain| domain.contains('.') && !domain.ends_with(".localhost")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_pane_is_clipped_to_the_current_client_area() {
        let small = SignInBounds {
            x: 500,
            y: 150,
            width: 600,
            height: 650,
        };
        let clipped = small.within(PhysicalSize::new(800, 550)).unwrap();
        assert_eq!(clipped.position, PhysicalPosition::new(500, 150).into());
        assert_eq!(clipped.size, PhysicalSize::new(300, 400).into());
        assert!(small.within(PhysicalSize::new(500, 150)).is_none());
        assert!(small.within(PhysicalSize::new(499, 600)).is_none());
    }

    #[test]
    fn only_codex_can_navigate_to_its_exact_http_callback() {
        let redirect = Url::parse("http://localhost:1455/auth/callback").unwrap();
        for (raw, expected) in [
            ("about:blank", true),
            ("https://auth.openai.com/log-in", true),
            ("https://accounts.google.com/", true),
            ("http://localhost:1455/auth/callback?code=synthetic", true),
            ("http://127.0.0.1:1420/", false),
            ("http://localhost:1455/other", false),
            ("https://tauri.localhost/", false),
            ("tauri://localhost/", false),
            ("file:///C:/local", false),
            ("https://username@example.test/", false),
        ] {
            assert_eq!(
                allowed_page(OAuthClientKind::Codex, &redirect, &Url::parse(raw).unwrap()),
                expected,
                "{raw}"
            );
        }
        assert!(!allowed_page(OAuthClientKind::Siwc, &redirect, &redirect));
        assert!(!allowed_page(
            OAuthClientKind::ExcelBps,
            &redirect,
            &redirect
        ));
    }
}
