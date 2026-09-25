use crate::state::{AccountCredential, AppState, ServerAccountRecord};
use futures_util::{future::BoxFuture, StreamExt};
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
use zenith_relay_core::error_codes;
use zenith_relay_core::providers::chatgpt::{
    token_refresh_failure_kind, token_refresh_provider_error_code, AgentIdentityCredential,
};
use zenith_relay_core::scheduler::refresh::http::{management_http_gate, HttpClass};
use zenith_relay_core::ProxyConfig;

const CODEX_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;

pub(crate) struct ServerTokenPersistence {
    pub(crate) state: Arc<AppState>,
    /// Bound to the runtime's credential incarnation, not whatever login is
    /// currently stored under the same account id when a late refresh ends.
    pub(crate) secret_refs: HashMap<String, String>,
}

impl ServerTokenPersistence {
    pub(crate) fn for_account(state: Arc<AppState>, record: &ServerAccountRecord) -> Self {
        Self {
            state,
            secret_refs: HashMap::from([(record.id.clone(), record.secret_ref.clone())]),
        }
    }

    fn expected_ref(&self, account_id: &str) -> Result<&str, TokenPersistenceFailure> {
        self.secret_refs
            .get(account_id)
            .map(String::as_str)
            .ok_or_else(|| TokenPersistenceFailure::new(error_codes::PERSISTENCE_FAILED))
    }

    fn current_ref(&self, account_id: &str) -> Result<&str, TokenPersistenceFailure> {
        let expected = self.expected_ref(account_id)?;
        let record = find_account(&self.state, account_id).map_err(persistence_error)?;
        if record.secret_ref != expected {
            return Err(TokenPersistenceFailure::new(
                error_codes::PERSISTENCE_FAILED,
            ));
        }
        Ok(expected)
    }

    /// Call only under account_credential_lock so an import cannot change the
    /// record/reference between validation, vault load and the eventual save.
    fn load_current_credential(
        &self,
        account_id: &str,
    ) -> Result<(&str, AccountCredential), TokenPersistenceFailure> {
        let secret_ref = self.current_ref(account_id)?;
        let secret = self
            .state
            .vault
            .load(secret_ref)
            .map_err(persistence_error)?
            .ok_or_else(|| TokenPersistenceFailure::new(error_codes::SECRET_MISSING))?;
        let credential = serde_json::from_str(&secret)
            .map_err(|_| TokenPersistenceFailure::new(error_codes::SECRET_INVALID))?;
        Ok((secret_ref, credential))
    }

    async fn persist_agent_task_inner(
        &self,
        account_id: &str,
        expected_task_id: Option<&str>,
        expected_identity: Option<&AgentIdentityCredential>,
        task_id: &str,
    ) -> Result<String, TokenPersistenceFailure> {
        let _credential = self.state.account_credential_lock.lock().await;
        let (secret_ref, mut credential) = self.load_current_credential(account_id)?;
        let agent = credential
            .agent_identity()
            .map_err(persistence_error)?
            .ok_or_else(|| TokenPersistenceFailure::new(error_codes::NOT_AGENT_IDENTITY))?;
        if expected_identity.is_some_and(|expected| {
            agent.private_key() != expected.private_key()
                || agent.runtime_id() != expected.runtime_id()
                || expected.task_id() != expected_task_id
        }) {
            return Err(TokenPersistenceFailure::new(
                error_codes::PERSISTENCE_FAILED,
            ));
        }
        if let Some(current_task_id) = agent
            .task_id()
            .filter(|current_task_id| Some(*current_task_id) != expected_task_id)
        {
            return Ok(current_task_id.to_string());
        }
        credential.agent_task_id = Some(task_id.to_string());
        let encoded = serde_json::to_string(&credential)
            .map_err(|_| TokenPersistenceFailure::new(error_codes::SECRET_SERIALIZE))?;
        self.state
            .vault
            .save(secret_ref, &encoded)
            .map_err(persistence_error)?;
        Ok(task_id.to_string())
    }
}

impl TokenPersistenceAdapter for ServerTokenPersistence {
    fn persist<'a>(
        &'a self,
        account_id: &'a str,
        tokens: &'a TokenSet,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
        Box::pin(async move {
            let _credential = self.state.account_credential_lock.lock().await;
            let (secret_ref, mut credential) = self.load_current_credential(account_id)?;
            credential.access_token = tokens.access_token().to_string();
            credential.refresh_token = tokens.refresh_token().map(str::to_string);
            credential.id_token = tokens.id_token().map(str::to_string);
            credential.expires_at_ms = tokens.expires_at_ms();
            credential.issued_at_ms = tokens.issued_at_ms();
            credential.generation = tokens.generation();
            let encoded = serde_json::to_string(&credential)
                .map_err(|_| TokenPersistenceFailure::new(error_codes::SECRET_SERIALIZE))?;
            self.state
                .vault
                .save(secret_ref, &encoded)
                .map_err(persistence_error)
        })
    }

    fn persist_auth_state<'a>(
        &'a self,
        account_id: &'a str,
        auth_state: AccountAuthState,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
        Box::pin(async move {
            let _credential = self.state.account_credential_lock.lock().await;
            let expected = self.expected_ref(account_id)?;
            self.state
                .store
                .update_account(account_id, |record| {
                    if record.secret_ref != expected {
                        return Err("account login changed during token refresh".into());
                    }
                    record.auth_state = auth_state;
                    Ok(())
                })
                .map_err(persistence_error)?
                .ok_or_else(|| TokenPersistenceFailure::new(error_codes::ACCOUNT_MISSING))
        })
    }

    fn persist_agent_task_id<'a>(
        &'a self,
        account_id: &'a str,
        expected_task_id: Option<&'a str>,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<String, TokenPersistenceFailure>> {
        Box::pin(self.persist_agent_task_inner(account_id, expected_task_id, None, task_id))
    }

    fn persist_agent_task_id_for_identity<'a>(
        &'a self,
        account_id: &'a str,
        expected: &'a AgentIdentityCredential,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<String, TokenPersistenceFailure>> {
        Box::pin(self.persist_agent_task_inner(
            account_id,
            expected.task_id(),
            Some(expected),
            task_id,
        ))
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
    TokenPersistenceFailure::new(error_codes::PERSISTENCE_FAILED)
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
                    error_codes::INVALID_REFRESH_TOKEN,
                ));
            }
            let (response, permit) = management_http_gate()
                .send(
                    &self.http,
                    self.http
                        .post(CODEX_TOKEN_ENDPOINT)
                        .json(&serde_json::json!({
                            "client_id": CODEX_CLIENT_ID,
                            "grant_type": "refresh_token",
                            "refresh_token": refresh_token,
                        })),
                    HttpClass::Auth,
                )
                .await
                .map_err(|_| {
                    TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "transport")
                })?;
            let status = response.status();
            let body = collect_token_response(response).await?;
            drop(permit);
            if !status.is_success() {
                let code = token_refresh_provider_error_code(&body)
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

async fn collect_token_response(
    response: reqwest::Response,
) -> Result<Vec<u8>, TokenRefreshFailure> {
    let oversized =
        || TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "response_too_large");
    if response
        .content_length()
        .is_some_and(|length| length > MAX_TOKEN_RESPONSE_BYTES as u64)
    {
        return Err(oversized());
    }
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| {
            TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "transport")
        })?;
        if body.len().saturating_add(chunk.len()) > MAX_TOKEN_RESPONSE_BYTES {
            return Err(oversized());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        store::{Store, Vault},
    };
    use tempfile::TempDir;
    use zenith_relay_core::accounts::TokenPersistenceAdapter;
    use zenith_relay_core::accounts::TokenRefreshAdapter;

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

    #[tokio::test]
    async fn old_persistence_cannot_write_a_replaced_login_or_auth_state() {
        let root = TempDir::new().unwrap();
        let config = Config::for_test(root.path().into(), "127.0.0.1:0".parse().unwrap());
        let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
        let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
        let state = AppState::new(config, store, vault).unwrap();
        let mut record: ServerAccountRecord = serde_json::from_value(serde_json::json!({
            "id": "synthetic", "label": "Synthetic", "identityHint": "synthetic",
            "enabled": true, "inPool": false, "draining": false,
            "sourceId": "openai_codex", "secretRef": "account:synthetic:old",
            "authState": AccountAuthState::Active, "health": "healthy",
            "models": [], "allowedModels": [], "excludedModels": [], "priority": 0,
            "weight": 1, "subscription": zenith_relay_core::quota::Subscription::default(),
            "quota": zenith_relay_core::quota::QuotaSnapshot::default(),
            "cooldowns": {}, "consecutiveFailures": 0
        }))
        .unwrap();
        let old_record = record.clone();
        let old_persistence = ServerTokenPersistence::for_account(state.clone(), &record);
        record.secret_ref = "account:synthetic:new".into();
        state.store.save_account(&record).unwrap();
        let credential = crate::state::AccountCredential {
            access_token: "synthetic-new-access".into(),
            refresh_token: Some("synthetic-new-refresh".into()),
            id_token: None,
            expires_at_ms: Some(crate::state::now_ms() + 3_600_000),
            issued_at_ms: 10,
            generation: 1,
            chatgpt_account_id: "synthetic-provider-account".into(),
            responses_url: "https://provider.example.test/v1/responses".into(),
            proxy_url: None,
            agent_private_key: None,
            agent_runtime_id: None,
            agent_task_id: None,
        };
        let encoded = serde_json::to_string(&credential).unwrap();
        state.vault.save(&record.secret_ref, &encoded).unwrap();
        let rejected = TokenSet::new(
            "synthetic-old-access",
            Some("synthetic-old-refresh".into()),
            None,
            Some(60_000),
            10,
            2,
        )
        .unwrap();

        assert!(old_persistence
            .persist(&record.id, &rejected)
            .await
            .is_err());
        assert!(old_persistence
            .persist_auth_state(&record.id, AccountAuthState::Refreshing)
            .await
            .is_err());
        assert!(old_persistence
            .persist_agent_task_id(&record.id, None, "synthetic-task")
            .await
            .is_err());
        assert_eq!(
            state.vault.load(&record.secret_ref).unwrap().as_deref(),
            Some(encoded.as_str())
        );
        assert_eq!(
            state.store.account(&record.id).unwrap().unwrap().auth_state,
            AccountAuthState::Active
        );
        assert!(state.prepare_account_tokens(&old_record).await.is_err());
        assert!(state.token_authority.tokens(&record.id).await.is_none());
        assert_eq!(
            state
                .prepare_account_tokens(&record)
                .await
                .unwrap()
                .access_token(),
            "synthetic-new-access"
        );
        // Runtime rebuilds may hold configuration_lock while waiting for the
        // token authority slot. Persistence must use only the credential lock.
        let configuration = state.configuration_lock.lock().await;
        tokio::time::timeout(
            Duration::from_secs(2),
            ServerTokenPersistence::for_account(state.clone(), &record)
                .persist(&record.id, &credential.tokens().unwrap()),
        )
        .await
        .expect("token persistence must not wait for configuration_lock")
        .unwrap();
        drop(configuration);

        // Even when the account and secret reference remain the same, a late
        // task registration belongs to the old Agent Identity, not a new one
        // that also has no task id yet.
        const TEST_KEY: &str = "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";
        let old_agent =
            AgentIdentityCredential::unregistered(TEST_KEY.into(), "old-runtime".into()).unwrap();
        let mut replacement = credential.clone();
        replacement.agent_private_key = Some(TEST_KEY.into());
        replacement.agent_runtime_id = Some("new-runtime".into());
        let encoded_replacement = serde_json::to_string(&replacement).unwrap();
        state
            .vault
            .save(&record.secret_ref, &encoded_replacement)
            .unwrap();
        assert!(ServerTokenPersistence::for_account(state.clone(), &record)
            .persist_agent_task_id_for_identity(&record.id, &old_agent, "old-task")
            .await
            .is_err());
        assert_eq!(
            state.vault.load(&record.secret_ref).unwrap().as_deref(),
            Some(encoded_replacement.as_str())
        );
        state.refresh.shutdown().await;
    }

    #[tokio::test]
    async fn oauth_response_body_is_bounded_while_streaming() {
        use axum::{
            body::{Body, Bytes},
            routing::get,
            Router,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let service = Router::new().route(
            "/token",
            get(|| async {
                Body::from_stream(futures_util::stream::iter(vec![
                    Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; MAX_TOKEN_RESPONSE_BYTES])),
                    Ok(Bytes::from_static(b"overflow")),
                ]))
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, service).await.unwrap() });
        let response = reqwest::get(format!("http://{address}/token"))
            .await
            .unwrap();
        let failure = collect_token_response(response).await.unwrap_err();
        assert_eq!(failure.code, "response_too_large");
        server.abort();
    }
}
