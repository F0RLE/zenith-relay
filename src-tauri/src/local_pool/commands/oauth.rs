use crate::local_pool::{
    accounts::{
        credentials::{credential_local_error, CredentialStore},
        oauth::{CodexOAuthClient, OAuthClientKind},
        oauth_flow::{OAuthFlowStart, OAuthFlowStatus},
        proxy::{
            common_proxy_url, effective_proxy_url, ensure_account_proxy, is_proxy_id, ProxyPool,
        },
        NativeSecretBackend,
    },
    error::{CommandError, LocalPoolError},
    models::LocalAccountRecord,
    state::DesktopState,
};

use tauri::{AppHandle, State};
use zenith_relay_core::ProxyConfig;

mod account;
mod checkpoint;
mod completion;
mod flow;
mod window;

#[cfg(test)]
mod tests;

use completion::complete_oauth;
use flow::{flow_error, oauth_error, validate_oauth_target, validated_authorization_url};

type CommandResult<T> = std::result::Result<T, CommandError>;

#[tauri::command]
pub async fn start_codex_oauth(
    app: AppHandle,
    open_browser: Option<bool>,
    account_id: Option<String>,
    proxy_id: Option<String>,
    client_kind: Option<OAuthClientKind>,
    state: State<'_, DesktopState>,
) -> CommandResult<OAuthFlowStart> {
    let _mutation = state.setup_guard().await;
    crate::diagnostics::breadcrumb(
        "oauth",
        "start",
        &[(
            "account",
            account_id
                .as_deref()
                .map(crate::diagnostics::hash_identifier)
                .unwrap_or_else(|| "none".to_string()),
        )],
    );
    let requested_proxy_id = normalized_proxy_id(proxy_id.as_deref())?;
    let requested_proxy_url = match requested_proxy_id.as_deref() {
        Some(proxy_id) => Some(http_sign_in_proxy_url(proxy_id)?),
        None => None,
    };
    let target_account_id = validate_oauth_target(&state, account_id.as_deref())?;
    let client_kind = oauth_client_kind(target_account_id.as_deref(), client_kind)?;
    let client_proxy_url = match requested_proxy_url {
        Some(url) => Some(url),
        None => oauth_proxy_url(&state, target_account_id.as_deref())?,
    };
    let proxy = parsed_proxy(client_proxy_url.as_deref())?;
    let oauth = CodexOAuthClient::new_with_proxy_for_kind(client_kind, proxy.as_ref())
        .map_err(oauth_error)?;
    let flow = state.oauth_flow();
    let start = flow
        .start_for_account(
            &oauth,
            target_account_id.as_deref(),
            requested_proxy_id.as_deref(),
        )
        .await
        .map_err(flow_error)?;
    let authorization_url = validated_authorization_url(&start)?;
    let proxy_url = match sign_in_flow_proxy_url(&state, &start) {
        Ok(url) => url,
        Err(error) => {
            if start.status == OAuthFlowStatus::Pending {
                let _ = flow.cancel(&start.login_id).await;
            }
            return Err(error.into());
        }
    };
    if open_browser.unwrap_or(true) && start.status == OAuthFlowStatus::Pending {
        if let Err(error) =
            window::open_sign_in_window(&app, &authorization_url, &start, proxy_url.as_deref())
                .await
        {
            let _ = flow.cancel(&start.login_id).await;
            return Err(error.into());
        }
    }
    Ok(start)
}

#[tauri::command]
pub async fn resume_codex_oauth(
    app: AppHandle,
    login_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<OAuthFlowStart> {
    let _mutation = state.setup_guard().await;
    crate::diagnostics::breadcrumb(
        "oauth",
        "resume",
        &[("login", crate::diagnostics::hash_identifier(&login_id))],
    );
    let start = state
        .oauth_flow()
        .resume(&login_id)
        .await
        .map_err(flow_error)?;
    let authorization_url = validated_authorization_url(&start)?;
    if start.status == OAuthFlowStatus::Pending {
        let proxy_url = sign_in_flow_proxy_url(&state, &start)?;
        window::open_sign_in_window(&app, &authorization_url, &start, proxy_url.as_deref()).await?;
    }
    Ok(start)
}

fn oauth_client_kind(
    account_id: Option<&str>,
    requested: Option<OAuthClientKind>,
) -> Result<OAuthClientKind, LocalPoolError> {
    let stored = account_id
        .map(|account_id| CredentialStore::from_backend(NativeSecretBackend).load(account_id))
        .transpose()
        .map_err(credential_local_error)?
        .flatten();
    if let Some(stored) = stored {
        let kind = stored.oauth_client_kind();
        if requested.is_some_and(|requested| requested != kind) {
            return Err(LocalPoolError::new(
                crate::local_pool::error::ErrorCode::Conflict,
                "Reauthenticate with the original OAuth client. Add another connection to use a different client.",
            ));
        }
        return Ok(kind);
    }
    Ok(requested.unwrap_or_default())
}

fn oauth_proxy_url(
    state: &DesktopState,
    account_id: Option<&str>,
) -> Result<Option<String>, LocalPoolError> {
    let settings = state.store()?.gateway().clone();
    if let Some(account_id) = account_id {
        let credentials = CredentialStore::from_backend(NativeSecretBackend)
            .load(account_id)
            .map_err(credential_local_error)?;
        if let Some(credentials) = credentials.as_ref() {
            return effective_proxy_url(&settings, credentials);
        }
    }
    let proxy = common_proxy_url(&settings)?;
    ensure_account_proxy(&settings, proxy.as_ref())?;
    Ok(proxy)
}

fn normalized_proxy_id(proxy_id: Option<&str>) -> Result<Option<String>, LocalPoolError> {
    let Some(proxy_id) = proxy_id
        .map(str::trim)
        .filter(|proxy_id_text| !proxy_id_text.is_empty())
    else {
        return Ok(None);
    };
    if !is_proxy_id(proxy_id) {
        return Err(LocalPoolError::new(
            crate::local_pool::error::ErrorCode::InvalidState,
            "stored proxy id is invalid",
        ));
    }
    Ok(Some(proxy_id.to_string()))
}

pub(super) fn http_sign_in_proxy_url(proxy_id: &str) -> Result<String, LocalPoolError> {
    if !is_proxy_id(proxy_id) {
        return Err(LocalPoolError::new(
            crate::local_pool::error::ErrorCode::InvalidState,
            "stored proxy id is invalid",
        ));
    }
    let url = ProxyPool::load()?.stored_url(proxy_id)?;
    let parsed = url::Url::parse(&url).map_err(|_| {
        LocalPoolError::new(
            crate::local_pool::error::ErrorCode::InvalidState,
            "stored proxy URL is invalid",
        )
    })?;
    if parsed.scheme() != "http" {
        return Err(LocalPoolError::new(
            crate::local_pool::error::ErrorCode::InvalidState,
            "The sign-in window cannot use an HTTPS proxy. Use an HTTP proxy for this account.",
        ));
    }
    Ok(url)
}

fn sign_in_flow_proxy_url(
    state: &DesktopState,
    start: &OAuthFlowStart,
) -> Result<Option<String>, LocalPoolError> {
    match state
        .oauth_flow()
        .sign_in_proxy_id(&start.login_id)
        .map_err(flow_error)?
    {
        Some(proxy_id) => http_sign_in_proxy_url(&proxy_id).map(Some),
        None => oauth_proxy_url(state, start.target_account_id.as_deref()),
    }
}

fn parsed_proxy(proxy_url: Option<&str>) -> Result<Option<ProxyConfig>, LocalPoolError> {
    proxy_url
        .map(|proxy_url_text| {
            ProxyConfig::parse(proxy_url_text).map_err(|_| {
                LocalPoolError::new(
                    crate::local_pool::error::ErrorCode::InvalidState,
                    "stored proxy URL is invalid",
                )
            })
        })
        .transpose()
}

pub(crate) fn close_sign_in_window(app: &AppHandle) {
    window::close_sign_in_window(app);
}

#[tauri::command]
pub fn get_codex_oauth_status(
    login_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<OAuthFlowStart> {
    crate::diagnostics::breadcrumb(
        "oauth",
        "status",
        &[("login", crate::diagnostics::hash_identifier(&login_id))],
    );
    let start = state.oauth_flow().status(&login_id).map_err(flow_error)?;
    validated_authorization_url(&start)?;
    Ok(start)
}

#[tauri::command]
pub async fn submit_codex_oauth_callback(
    login_id: String,
    callback_url: String,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    let _mutation = state.setup_guard().await;
    crate::diagnostics::breadcrumb(
        "oauth",
        "callback",
        &[("login", crate::diagnostics::hash_identifier(&login_id))],
    );
    state
        .oauth_flow()
        .submit_manual_callback(&login_id, &callback_url)
        .await
        .map_err(flow_error)?;
    Ok(())
}

#[tauri::command]
pub async fn cancel_codex_oauth(
    login_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    let _mutation = state.setup_guard().await;
    crate::diagnostics::breadcrumb(
        "oauth",
        "cancel",
        &[("login", crate::diagnostics::hash_identifier(&login_id))],
    );
    state
        .oauth_flow()
        .cancel(&login_id)
        .await
        .map_err(flow_error)?;
    Ok(())
}

#[tauri::command]
pub async fn complete_codex_oauth(
    login_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalAccountRecord> {
    let _mutation = state.setup_guard().await;
    crate::diagnostics::breadcrumb(
        "oauth",
        "complete",
        &[("login", crate::diagnostics::hash_identifier(&login_id))],
    );
    complete_oauth(&login_id, &state).await.map_err(Into::into)
}
