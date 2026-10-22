use super::session_is_current;
use crate::local_pool::{
    accounts::{oauth::OAuthClientKind, oauth_flow::OAuthFlowStatus},
    commands::current_time_ms,
    error::{ErrorCode, LocalPoolError},
    state::DesktopState,
};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use url::Url;

const DRIVER: &str = include_str!("automatic_login.js");
const EVALUATION_TIMEOUT_MS: u64 = 2_000;
const AUTOMATION_TIMEOUT_MS: u64 = 45_000;
const FORM_LOAD_GRACE_MS: u64 = 5_000;

// No Debug/Serialize: these values must never enter renderer state or diagnostics.
pub(in crate::local_pool::commands::oauth) struct LoginNotes {
    email: String,
    password: String,
    totp_secret: Option<String>,
}

impl LoginNotes {
    pub(in crate::local_pool::commands::oauth) fn load(
        account_id: Option<&str>,
        kind: OAuthClientKind,
        state: &DesktopState,
        route_sign_in: bool,
    ) -> Result<Option<Self>, LocalPoolError> {
        validate_request(account_id, kind, route_sign_in)?;
        let credentials = crate::local_pool::accounts::login::shared_login_credentials(
            account_id.ok_or_else(unavailable)?,
            state,
        )?;
        let email = credentials.email().filter(|value| !value.trim().is_empty());
        let password = credentials.password().filter(|value| !value.is_empty());
        let (Some(email), Some(password)) = (email, password) else {
            return if route_sign_in {
                Ok(None)
            } else {
                Err(unavailable())
            };
        };
        Ok(Some(Self {
            email: email.to_owned(),
            password: password.to_owned(),
            totp_secret: credentials.totp_secret().map(str::to_owned),
        }))
    }
}

fn unavailable() -> LocalPoolError {
    LocalPoolError::new(ErrorCode::InvalidState, "Automatic sign-in requires an existing account with saved email and password, and a supported sign-in window. SIWC uses manual sign-in.")
}

fn validate_request(
    account_id: Option<&str>,
    kind: OAuthClientKind,
    route_sign_in: bool,
) -> Result<(), LocalPoolError> {
    if !(kind == OAuthClientKind::ExcelBps || route_sign_in && kind == OAuthClientKind::Codex)
        || !account_id.is_some_and(|id| !id.trim().is_empty())
    {
        return Err(unavailable());
    }
    Ok(())
}

fn trusted_origin(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("auth.openai.com")
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
}

fn login_page(url: &Url) -> bool {
    trusted_origin(url)
        && matches!(
            url.path(),
            "/log-in"
                | "/log-in/password"
                | "/u/login/identifier"
                | "/u/login/password"
                | "/mfa/totp"
                | "/u/mfa-otp-challenge"
        )
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Stage {
    Email,
    Password,
    Totp,
    Wait,
    Manual,
    Submitted,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DriverInput<'a> {
    url: &'a str,
    expires_at_ms: u64,
    wait_until_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    stage: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<&'a str>,
}

fn script(
    url: &Url,
    stage: Option<&str>,
    value: Option<&str>,
    email: Option<&str>,
    expires_at_ms: u64,
    wait_until_ms: u64,
) -> Option<String> {
    let input = serde_json::to_string(&DriverInput {
        url: url.as_str(),
        expires_at_ms,
        wait_until_ms,
        stage,
        value,
        email,
    })
    .ok()?;
    Some(format!("({DRIVER})({input})"))
}

async fn evaluate(window: &WebviewWindow, script: String) -> Option<Stage> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let send = Mutex::new(Some(send));
    window
        .eval_with_callback(script, move |result| {
            if let Some(send) = zenith_relay_core::poison::mutex(&send).take() {
                let _ = send.send(result);
            }
        })
        .ok()?;
    let result = tokio::time::timeout(Duration::from_millis(EVALUATION_TIMEOUT_MS), receive)
        .await
        .ok()?
        .ok()?;
    serde_json::from_str(&result).ok()
}

pub(super) async fn run(
    window: WebviewWindow,
    generation: u64,
    login_id: String,
    active: Arc<AtomicBool>,
    notes: LoginNotes,
) {
    let flow = window.app_handle().state::<DesktopState>().oauth_flow();
    let expires_at_ms = current_time_ms().saturating_add(AUTOMATION_TIMEOUT_MS);
    let current = || {
        active.load(Ordering::Acquire)
            && session_is_current(generation)
            && flow.status(&login_id).is_ok_and(|start| {
                start.status == OAuthFlowStatus::Pending
                    && start.expires_at_ms > current_time_ms()
                    && start.notes_account_id.as_deref().is_none_or(|id| {
                        super::super::flow::validate_oauth_target(
                            &window.app_handle().state::<DesktopState>(),
                            Some(id),
                        )
                        .is_ok()
                    })
            })
    };
    let automate = async {
        let mut submitted = [false; 3];
        let mut previous_url = None;
        let mut wait_until_ms = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if !current() {
                return;
            }
            let Ok(url) = window.url() else { break };
            if url.as_str() == "about:blank"
                || (trusted_origin(&url)
                    && matches!(url.path(), "/api/accounts/authorize" | "/oauth/authorize"))
            {
                continue;
            }
            if !login_page(&url) {
                break;
            }
            if previous_url.as_ref() != Some(&url) {
                previous_url = Some(url.clone());
                wait_until_ms = current_time_ms().saturating_add(FORM_LOAD_GRACE_MS);
            }
            let evaluation_deadline =
                || expires_at_ms.min(current_time_ms().saturating_add(EVALUATION_TIMEOUT_MS));
            let Some(detect) = script(&url, None, None, None, evaluation_deadline(), wait_until_ms)
            else {
                break;
            };
            let Some(stage) = evaluate(&window, detect).await else {
                break;
            };
            let (index, name, value) = match stage {
                Stage::Wait => continue,
                Stage::Email => (0, "email", notes.email.clone()),
                Stage::Password => (1, "password", notes.password.clone()),
                Stage::Totp => {
                    let Some(secret) = notes.totp_secret.as_ref() else {
                        break;
                    };
                    let preview =
                        crate::local_pool::accounts::login::preview_totp_code(secret.clone());
                    if preview
                        .expires_at_ms
                        .is_some_and(|expires| expires.saturating_sub(current_time_ms()) < 4_000)
                    {
                        continue;
                    }
                    let Some(code) = preview.code else { break };
                    (2, "totp", code)
                }
                Stage::Manual | Stage::Submitted => break,
            };
            if !current() || window.url().ok().as_ref() != Some(&url) {
                continue;
            }
            if !claim_attempt(&mut submitted, index) {
                continue;
            }
            let Some(fill) = script(
                &url,
                Some(name),
                Some(&value),
                (stage == Stage::Password).then_some(notes.email.as_str()),
                evaluation_deadline(),
                wait_until_ms,
            ) else {
                break;
            };
            if evaluate(&window, fill).await != Some(Stage::Submitted) {
                break;
            }
            wait_until_ms = current_time_ms().saturating_add(FORM_LOAD_GRACE_MS);
        }
    };
    // One bounded native task, no page timers, external IPC or background browser process.
    let _ = tokio::time::timeout(Duration::from_millis(AUTOMATION_TIMEOUT_MS), automate).await;
    if current() && active.swap(false, Ordering::AcqRel) {
        notify_manual(window.app_handle(), &login_id);
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub(super) fn notify_manual(app: &AppHandle, login_id: &str) {
    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct AutomationStopped<'a> {
        login_id: &'a str,
        status: OAuthFlowStatus,
        automatic_login: bool,
    }
    let _ = app.emit(
        "relay-oauth-status",
        AutomationStopped {
            login_id,
            status: OAuthFlowStatus::Pending,
            automatic_login: false,
        },
    );
}

fn claim_attempt(submitted: &mut [bool; 3], index: usize) -> bool {
    !std::mem::replace(&mut submitted[index], true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_requires_original_bps_account_on_every_platform() {
        assert!(validate_request(Some("synthetic"), OAuthClientKind::ExcelBps, false).is_ok());
        for (account, kind) in [
            (None, OAuthClientKind::ExcelBps),
            (Some(" "), OAuthClientKind::ExcelBps),
            (Some("synthetic"), OAuthClientKind::Codex),
            (Some("synthetic"), OAuthClientKind::Siwc),
        ] {
            assert!(validate_request(account, kind, false).is_err());
        }
    }

    #[test]
    fn route_automation_supports_codex_and_bps_but_never_siwc() {
        for kind in [OAuthClientKind::Codex, OAuthClientKind::ExcelBps] {
            assert!(validate_request(Some("synthetic-source"), kind, true).is_ok());
            assert!(validate_request(None, kind, true).is_err());
            assert!(validate_request(Some(" "), kind, true).is_err());
        }
        assert!(validate_request(Some("synthetic-source"), OAuthClientKind::Siwc, true).is_err());
    }

    #[test]
    fn only_exact_https_login_and_authenticator_pages_receive_notes() {
        for path in [
            "/log-in",
            "/log-in/password",
            "/u/login/identifier",
            "/u/login/password",
            "/mfa/totp",
            "/u/mfa-otp-challenge",
        ] {
            assert!(login_page(
                &Url::parse(&format!("https://auth.openai.com{path}?synthetic=true")).unwrap()
            ));
        }
        for url in [
            "http://auth.openai.com/log-in",
            "https://auth.openai.com:444/log-in",
            "https://auth.openai.com.evil.test/log-in",
            "https://user@auth.openai.com/log-in",
            "https://auth.openai.com/create-account/password",
            "https://auth.openai.com/email-verification",
            "https://auth.openai.com/consent",
            "https://auth.openai.com/log-in/",
        ] {
            assert!(!login_page(&Url::parse(url).unwrap()));
        }
    }

    #[test]
    fn detector_contains_no_notes_and_fill_values_are_json_encoded() {
        let url = Url::parse("https://auth.openai.com/log-in").unwrap();
        let detect = script(&url, None, None, None, 2_000, 5_000).unwrap();
        assert!(!detect.contains("synthetic-secret"));
        assert!(detect.contains("\"expiresAtMs\":2000,\"waitUntilMs\":5000"));
        let value = "synthetic-secret\"\\\n);throw new Error('not code')";
        let filled = script(&url, Some("password"), Some(value), None, 2_000, 5_000).unwrap();
        assert!(filled.ends_with(&format!(
            "\"value\":{}}})",
            serde_json::to_string(value).unwrap()
        )));
    }

    #[test]
    fn stages_cannot_be_retried_after_navigation_or_error() {
        let mut submitted = [false; 3];
        for index in [0, 1, 2] {
            assert!(claim_attempt(&mut submitted, index));
            assert!(!claim_attempt(&mut submitted, index));
        }
        for index in [2, 0, 1] {
            assert!(!claim_attempt(&mut submitted, index));
        }
    }
}
