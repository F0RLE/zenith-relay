use crate::state::{AccountCredential, AppState, ServerAccountRecord};
use futures_util::future::BoxFuture;
use reqwest::redirect::Policy;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use zenith_relay_core::accounts::{
    AccountAuthState, TokenPersistenceAdapter, TokenPersistenceFailure, TokenRefresh,
    TokenRefreshAdapter, TokenRefreshFailure, TokenRefreshFailureKind, TokenSet,
};
use zenith_relay_core::ProxyConfig;

const CODEX_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;

pub(crate) struct ServerTokenPersistence {
    pub(crate) state: Arc<AppState>,
}

impl TokenPersistenceAdapter for ServerTokenPersistence {
    fn persist<'a>(
        &'a self,
        account_id: &'a str,
        tokens: &'a TokenSet,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
        Box::pin(async move {
            let record = find_account(&self.state, account_id).map_err(persistence_error)?;
            let secret = self
                .state
                .vault
                .load(&record.secret_ref)
                .map_err(persistence_error)?
                .ok_or_else(|| TokenPersistenceFailure::new("secret_missing"))?;
            let mut credential: AccountCredential = serde_json::from_str(&secret)
                .map_err(|_| TokenPersistenceFailure::new("secret_invalid"))?;
            credential.access_token = tokens.access_token().to_string();
            credential.refresh_token = tokens.refresh_token().map(str::to_string);
            credential.id_token = tokens.id_token().map(str::to_string);
            credential.expires_at_ms = tokens.expires_at_ms();
            credential.issued_at_ms = tokens.issued_at_ms();
            credential.generation = tokens.generation();
            let encoded = serde_json::to_string(&credential)
                .map_err(|_| TokenPersistenceFailure::new("secret_serialize"))?;
            self.state
                .vault
                .save(&record.secret_ref, &encoded)
                .map_err(persistence_error)
        })
    }

    fn persist_auth_state<'a>(
        &'a self,
        account_id: &'a str,
        auth_state: AccountAuthState,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
        Box::pin(async move {
            let mut record = find_account(&self.state, account_id).map_err(persistence_error)?;
            record.auth_state = auth_state;
            self.state
                .store
                .save_account(&record)
                .map_err(persistence_error)
        })
    }

    fn persist_agent_task_id<'a>(
        &'a self,
        account_id: &'a str,
        expected_task_id: Option<&'a str>,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<String, TokenPersistenceFailure>> {
        Box::pin(async move {
            let record = find_account(&self.state, account_id).map_err(persistence_error)?;
            let secret = self
                .state
                .vault
                .load(&record.secret_ref)
                .map_err(persistence_error)?
                .ok_or_else(|| TokenPersistenceFailure::new("secret_missing"))?;
            let mut credential: AccountCredential = serde_json::from_str(&secret)
                .map_err(|_| TokenPersistenceFailure::new("secret_invalid"))?;
            if !credential.is_agent_identity() {
                return Err(TokenPersistenceFailure::new("not_agent_identity"));
            }
            if let Some(current_task_id) = credential
                .agent_task_id
                .as_deref()
                .filter(|current_task_id| Some(*current_task_id) != expected_task_id)
            {
                return Ok(current_task_id.to_string());
            }
            credential.agent_task_id = Some(task_id.to_string());
            let encoded = serde_json::to_string(&credential)
                .map_err(|_| TokenPersistenceFailure::new("secret_serialize"))?;
            self.state
                .vault
                .save(&record.secret_ref, &encoded)
                .map_err(persistence_error)?;
            Ok(task_id.to_string())
        })
    }
}

pub(crate) fn find_account(state: &AppState, id: &str) -> Result<ServerAccountRecord, String> {
    state
        .store
        .accounts()?
        .into_iter()
        .find(|record| record.id == id)
        .ok_or_else(|| "account not found".to_string())
}

fn persistence_error(error: String) -> TokenPersistenceFailure {
    let _ = error;
    TokenPersistenceFailure::new("persistence_failed")
}

pub(crate) struct CodexRefreshClient {
    http: reqwest::Client,
}

impl CodexRefreshClient {
    pub(crate) fn new_with_proxy(proxy: Option<&ProxyConfig>) -> Result<Self, String> {
        let builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(20))
            .user_agent("Zenith Relay Server");
        let http = match proxy {
            Some(proxy) => proxy.apply(builder),
            None => builder,
        }
        .build()
        .map_err(|error| error.to_string())?;
        Ok(Self { http })
    }
}

pub(crate) struct ServerRefreshClients {
    pub(crate) direct: CodexRefreshClient,
    pub(crate) direct_accounts: HashSet<String>,
    pub(crate) clients: HashMap<String, CodexRefreshClient>,
}

impl TokenRefreshAdapter for ServerRefreshClients {
    fn refresh<'a>(
        &'a self,
        account_id: &'a str,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async move {
            let client = match self.clients.get(account_id) {
                Some(client) => client,
                None if self.direct_accounts.contains(account_id) => &self.direct,
                None => {
                    return Err(TokenRefreshFailure::new(
                        TokenRefreshFailureKind::Transient,
                        "proxy_client_missing",
                    ))
                }
            };
            client.refresh(account_id, refresh_token, now_ms).await
        })
    }
}

impl TokenRefreshAdapter for CodexRefreshClient {
    fn refresh<'a>(
        &'a self,
        _account_id: &'a str,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async move {
            if refresh_token.is_empty()
                || refresh_token.len() > 64 * 1024
                || refresh_token.bytes().any(|byte| byte.is_ascii_control())
            {
                return Err(TokenRefreshFailure::new(
                    TokenRefreshFailureKind::InvalidatedRefreshToken,
                    "invalid_refresh_token",
                ));
            }
            let response = self
                .http
                .post(CODEX_TOKEN_ENDPOINT)
                .json(&serde_json::json!({
                    "client_id": CODEX_CLIENT_ID,
                    "grant_type": "refresh_token",
                    "refresh_token": refresh_token,
                }))
                .send()
                .await
                .map_err(|_| {
                    TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "transport")
                })?;
            let status = response.status();
            let body = response.bytes().await.map_err(|_| {
                TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "transport")
            })?;
            if body.len() > MAX_TOKEN_RESPONSE_BYTES {
                return Err(TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "response_too_large",
                ));
            }
            if !status.is_success() {
                let code = provider_error_code(&body)
                    .unwrap_or_else(|| "token_refresh_failed".to_string());
                let kind = token_refresh_failure_kind(&code);
                return Err(TokenRefreshFailure::new(kind, &code));
            }
            let payload: TokenResponse = serde_json::from_slice(&body).map_err(|_| {
                TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "invalid_response")
            })?;
            let expires_at_ms = payload.expires_in.and_then(|seconds| {
                u64::try_from(seconds)
                    .ok()
                    .map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1_000)))
            });
            TokenRefresh::new(
                payload.access_token,
                payload.refresh_token,
                payload.id_token,
                expires_at_ms,
            )
            .map_err(|_| {
                TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "invalid_response")
            })
        })
    }
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
}

pub(crate) fn token_refresh_failure_kind(code: &str) -> TokenRefreshFailureKind {
    match code.trim().to_ascii_lowercase().as_str() {
        "invalid_grant" => TokenRefreshFailureKind::InvalidGrant,
        "refresh_token_reused" => TokenRefreshFailureKind::ReusedRefreshToken,
        "refresh_token_expired" => TokenRefreshFailureKind::ExpiredRefreshToken,
        "invalid_refresh_token" | "refresh_token_invalidated" | "token_invalidated" => {
            TokenRefreshFailureKind::InvalidatedRefreshToken
        }
        _ => TokenRefreshFailureKind::Transient,
    }
}

pub(crate) fn provider_error_code(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let code = [
        value
            .pointer("/error/code")
            .and_then(serde_json::Value::as_str),
        value.get("code").and_then(serde_json::Value::as_str),
        value.get("error").and_then(serde_json::Value::as_str),
        value
            .pointer("/error/type")
            .and_then(serde_json::Value::as_str),
    ]
    .into_iter()
    .flatten()
    .find_map(safe_provider_code);
    code
}

fn safe_provider_code(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    .then(|| value.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenith_relay_core::accounts::{TokenRefreshAdapter, TokenRefreshFailureKind};

    #[test]
    fn refresh_errors_keep_distinct_reauthentication_reasons() {
        assert_eq!(
            token_refresh_failure_kind("invalid_grant"),
            TokenRefreshFailureKind::InvalidGrant
        );
        assert_eq!(
            token_refresh_failure_kind("refresh_token_reused"),
            TokenRefreshFailureKind::ReusedRefreshToken
        );
        assert_eq!(
            token_refresh_failure_kind("refresh_token_expired"),
            TokenRefreshFailureKind::ExpiredRefreshToken
        );
        assert_eq!(
            token_refresh_failure_kind("refresh_token_invalidated"),
            TokenRefreshFailureKind::InvalidatedRefreshToken
        );
        assert_eq!(
            token_refresh_failure_kind("unsupported_country_region_territory"),
            TokenRefreshFailureKind::Transient
        );
    }

    #[test]
    fn provider_refresh_error_prefers_specific_rotation_code() {
        assert_eq!(
            provider_error_code(br#"{"error":"invalid_grant","code":"refresh_token_reused"}"#)
                .as_deref(),
            Some("refresh_token_reused")
        );
        assert_eq!(
            provider_error_code(
                br#"{"error":{"type":"invalid_request_error","code":"refresh_token_expired"}}"#
            )
            .as_deref(),
            Some("refresh_token_expired")
        );
    }

    #[tokio::test]
    async fn refresh_client_never_falls_back_to_direct_for_unknown_account() {
        let clients = ServerRefreshClients {
            direct: CodexRefreshClient::new_with_proxy(None).unwrap(),
            direct_accounts: HashSet::new(),
            clients: HashMap::new(),
        };
        let failure = clients
            .refresh("proxy-required", "unused-refresh-token", 1)
            .await
            .unwrap_err();
        assert_eq!(failure.code, "proxy_client_missing");
    }
}
