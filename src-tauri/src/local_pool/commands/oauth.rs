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

#[derive(serde::Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum AutomaticLoginRequest {
    Enabled(bool),
    Route {
        #[serde(rename = "notesAccountId")]
        notes_account_id: String,
    },
}

#[tauri::command]
pub async fn start_codex_oauth(
    app: AppHandle,
    open_browser: Option<bool>,
    account_id: Option<String>,
    proxy_id: Option<String>,
    client_kind: Option<OAuthClientKind>,
    automatic_login: Option<AutomaticLoginRequest>,
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
    let (automatic_requested, notes_account_id) = match automatic_login {
        Some(AutomaticLoginRequest::Route { notes_account_id }) => (true, Some(notes_account_id)),
        Some(AutomaticLoginRequest::Enabled(enabled)) => (enabled, None),
        None => (false, None),
    };
    let target_account_id = validate_oauth_target(&state, account_id.as_deref())?;
    let notes_account_id = validate_oauth_target(&state, notes_account_id.as_deref())?;
    if target_account_id.is_some() && notes_account_id.is_some() {
        return Err(LocalPoolError::new(
            crate::local_pool::error::ErrorCode::InvalidState,
            "Route sign-in notes cannot replace a reauthentication target",
        )
        .into());
    }
    let client_kind = oauth_client_kind(target_account_id.as_deref(), client_kind)?;
    let automatic_login = automatic_requested
        .then(|| {
            window::automatic_login::LoginNotes::load(
                target_account_id.as_deref().or(notes_account_id.as_deref()),
                client_kind,
                &state,
                notes_account_id.is_some(),
            )
        })
        .transpose()?
        .flatten();
    let client_proxy_url = match requested_proxy_url {
        Some(url) => Some(url),
        None => oauth_proxy_url(
            &state,
            target_account_id.as_deref().or(notes_account_id.as_deref()),
        )?,
    };
    let proxy = parsed_proxy(client_proxy_url.as_deref())?;
    let mut oauth = CodexOAuthClient::new_with_proxy_for_kind(client_kind, proxy.as_ref())
        .map_err(oauth_error)?;
    if client_kind == OAuthClientKind::Siwc {
        let credentials = target_account_id
            .as_deref()
            .map(|id| CredentialStore::from_backend(NativeSecretBackend).load(id))
            .transpose()
            .map_err(credential_local_error)?
            .flatten();
        let registration = credentials.as_ref().and_then(|c| c.siwc_registration());
        let host_id = siwc_host_id()?;
        oauth = oauth
            .with_siwc_registration(host_id, registration)
            .map_err(oauth_error)?;
    }
    let flow = state.oauth_flow();
    let mut start = if let Some(notes_account_id) = notes_account_id.as_deref() {
        flow.start_for_account_with_notes(
            &oauth,
            None,
            requested_proxy_id.as_deref(),
            Some(notes_account_id),
        )
        .await
    } else {
        flow.start_for_account(
            &oauth,
            target_account_id.as_deref(),
            requested_proxy_id.as_deref(),
        )
        .await
    }
    .map_err(flow_error)?;
    if automatic_requested {
        start.automatic_login = Some(automatic_login.is_some());
    }
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
    if (open_browser.unwrap_or(true) || automatic_login.is_some())
        && start.status == OAuthFlowStatus::Pending
    {
        if let Err(error) = window::open_sign_in_window(
            &app,
            &authorization_url,
            &start,
            proxy_url.as_deref(),
            automatic_login,
        )
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
        if window::continue_automatic_login_manually(&app, &login_id) {
            return Ok(start);
        }
        let proxy_url = sign_in_flow_proxy_url(&state, &start)?;
        window::open_sign_in_window(&app, &authorization_url, &start, proxy_url.as_deref(), None)
            .await?;
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
    Ok(requested.unwrap_or(OAuthClientKind::Codex))
}

fn siwc_host_id() -> Result<String, LocalPoolError> {
    use crate::local_pool::accounts::import_session::SecretBackend;
    const KEY: &str = "siwc-host-id";
    let error = || {
        LocalPoolError::new(
            crate::local_pool::error::ErrorCode::SecretStoreUnavailable,
            "SIWC host identity storage is unavailable",
        )
    };
    if let Some(host) = NativeSecretBackend.load(KEY).map_err(|_| error())? {
        if host.starts_with("urn:uuid:") && uuid::Uuid::parse_str(&host[9..]).is_ok() {
            return Ok(host);
        }
        return Err(error());
    }
    let host = format!("urn:uuid:{}", uuid::Uuid::new_v4());
    NativeSecretBackend.save(KEY, &host).map_err(|_| error())?;
    if NativeSecretBackend
        .load(KEY)
        .map_err(|_| error())?
        .as_deref()
        != Some(&host)
    {
        return Err(error());
    }
    Ok(host)
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
        None => oauth_proxy_url(
            state,
            start
                .target_account_id
                .as_deref()
                .or(start.notes_account_id.as_deref()),
        ),
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
pub async fn cancel_codex_oauth(
    app: AppHandle,
    login_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    let _mutation = state.setup_guard().await;
    window::cancel_automatic_login(&app, &login_id);
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
