use crate::state::{
    now_ms, AccountCredential, AppState, GatewayKeyRecord, ServerAccountRecord, SourceRecord,
    COMMON_PROXY_SECRET_REF, SERVER_SCHEMA_VERSION,
};
use futures_util::future::BoxFuture;
use reqwest::redirect::Policy;
use serde::Deserialize;
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
    time::Duration,
};
use zenith_relay_core::{
    accounts::{
        AccountAuthState, AccountHealthState, TokenPersistenceAdapter, TokenPersistenceFailure,
        TokenRefresh, TokenRefreshAdapter, TokenRefreshFailure, TokenRefreshFailureKind, TokenSet,
    },
    protocol::{
        AccountSummary, GatewaySummary, KeySummary, ProxyMode, RuntimeStateSnapshot,
        RuntimeTargetSummary, SourceSummary,
    },
    CandidateHealth, CandidateQuota, GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey,
    ProviderSource, ProxyConfig, RuntimeAccount, RuntimeAccountAuth, RuntimeMixedLocalKey,
    RuntimeSource,
};

const CODEX_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;

impl AppState {
    pub async fn prepare_account_tokens(
        self: &Arc<Self>,
        account_id: &str,
    ) -> Result<TokenSet, String> {
        let record = find_account(self, account_id)?;
        let secret = self
            .vault
            .load(&record.secret_ref)?
            .ok_or_else(|| "stored account credential is missing".to_string())?;
        let credential: AccountCredential = serde_json::from_str(&secret)
            .map_err(|_| "stored account credential is invalid".to_string())?;
        let proxy = account_proxy_config(self, &credential)?;
        let refresh = CodexRefreshClient::new_with_proxy(proxy.as_ref())?;
        let persistence = ServerTokenPersistence {
            state: self.clone(),
        };
        self.token_authority
            .prepare_and_persist(account_id, now_ms(), 60_000, &refresh, &persistence)
            .await
            .map(|prepared| prepared.tokens)
            .map_err(|error| error.to_string())
    }

    pub async fn rebuild_runtime(self: &Arc<Self>) -> Result<(), String> {
        let source_records = self.store.sources()?;
        let account_records = self.store.accounts()?;
        let key_records = self.store.keys()?;
        if key_records.is_empty() || (source_records.is_empty() && account_records.is_empty()) {
            return self.replace_runtime(None);
        }

        let mut sources = Vec::new();
        for record in source_records {
            let Some(api_key) = self.vault.load(&record.secret_ref)? else {
                continue;
            };
            sources.push(runtime_source(record, api_key));
        }

        let mut accounts = Vec::new();
        let mut refresh_clients = HashMap::new();
        let common_proxy_configured = self.store.common_proxy_configured()?;
        let common_proxy = if common_proxy_configured {
            self.vault
                .load(COMMON_PROXY_SECRET_REF)?
                .and_then(|value| ProxyConfig::parse(&value).ok())
        } else {
            None
        };
        for record in account_records {
            let Some(secret) = self.vault.load(&record.secret_ref)? else {
                continue;
            };
            let credential: AccountCredential = serde_json::from_str(&secret)
                .map_err(|_| "stored account credential is invalid".to_string())?;
            let proxy = match credential.proxy_url.as_deref() {
                Some(value) => match ProxyConfig::parse(value) {
                    Ok(proxy) => Some(proxy),
                    Err(_) => continue,
                },
                None if common_proxy_configured => match common_proxy.clone() {
                    Some(proxy) => Some(proxy),
                    None => continue,
                },
                None => None,
            };
            self.token_authority
                .register(&record.id, credential.tokens()?, record.auth_state)
                .await
                .map_err(|error| error.to_string())?;
            if proxy.is_some() {
                refresh_clients.insert(
                    record.id.clone(),
                    CodexRefreshClient::new_with_proxy(proxy.as_ref())?,
                );
            }
            accounts.push(runtime_account(record, &credential, proxy));
        }

        let mut keys = Vec::new();
        for record in key_records {
            let Some(secret) = self.vault.load(&record.secret_ref)? else {
                continue;
            };
            keys.push(runtime_key(record, secret));
        }
        if keys.is_empty() || (sources.is_empty() && accounts.is_empty()) {
            return self.replace_runtime(None);
        }

        let refresh = Arc::new(ServerRefreshClients {
            direct: CodexRefreshClient::new_with_proxy(None)?,
            clients: refresh_clients,
        });
        let persistence = Arc::new(ServerTokenPersistence {
            state: self.clone(),
        });
        let weak_state = Arc::downgrade(self);
        let usage = Arc::new(move |event| {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            let observed_at_ms = now_ms();
            let _ = state.store.record_usage(&event, observed_at_ms);
            let Some(account_id) = event.account_id.clone() else {
                return;
            };
            if let Ok(mut accounts) = state.store.accounts() {
                if let Some(mut account) = accounts.drain(..).find(|value| value.id == account_id) {
                    if event.success {
                        account.last_used_at_ms = Some(observed_at_ms);
                        account.health = AccountHealthState::Healthy;
                        account.consecutive_failures = 0;
                        account.last_error_code = None;
                    } else {
                        account.consecutive_failures = event
                            .consecutive_failures
                            .unwrap_or_else(|| account.consecutive_failures.saturating_add(1));
                        account.health = AccountHealthState::Degraded;
                        account.last_error_code = event.error_category.clone();
                    }
                    let _ = state.store.save_account(&account);
                }
            }
            if event.success {
                tokio::spawn(async move {
                    let _guard = state.wake_lock.lock().await;
                    let Ok(wake_state) = state.store.wake_state() else {
                        return;
                    };
                    let Ok(mut coordinator) =
                        zenith_relay_core::automations::WakeCoordinator::from_state(wake_state)
                    else {
                        return;
                    };
                    if coordinator.mark_natural_use_for_account(&account_id, observed_at_ms) > 0 {
                        let _ = state.store.save_wake_state(coordinator.state());
                    }
                });
            }
        });
        let runtime = GatewayRuntime::from_mixed_pool(
            sources,
            accounts,
            keys,
            RuntimeAccountAuth {
                token_authority: self.token_authority.clone(),
                refresh_adapter: refresh,
                persistence_adapter: persistence,
                refresh_skew_ms: 60_000,
            },
            GatewayRuntimeOptions::default(),
            usage,
        )
        .map_err(|error| error.to_string())?;
        self.replace_runtime(Some(Arc::new(runtime)))
    }

    pub fn snapshot(&self) -> Result<RuntimeStateSnapshot, String> {
        let sources = self.store.sources()?;
        let accounts = self.store.accounts()?;
        let keys = self.store.keys()?;
        let common_proxy_configured = self.store.common_proxy_configured()?;
        let common_proxy_available = common_proxy_available(self, common_proxy_configured);
        let mut warnings = Vec::new();
        let source_summaries = sources
            .iter()
            .map(|record| {
                let secret_available = self.vault.load(&record.secret_ref)?.is_some();
                if !secret_available {
                    warnings.push(format!("source_secret_missing:{}", record.id));
                }
                Ok(source_summary(record, secret_available))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let account_summaries = accounts
            .iter()
            .map(|record| {
                let secret = self.vault.load(&record.secret_ref)?;
                let secret_available = secret.is_some();
                if !secret_available {
                    warnings.push(format!("account_secret_missing:{}", record.id));
                }
                let (proxy_mode, proxy_available) = secret
                    .as_deref()
                    .and_then(|value| serde_json::from_str::<AccountCredential>(value).ok())
                    .map(|credential| {
                        account_proxy_status(
                            &credential,
                            common_proxy_configured,
                            common_proxy_available,
                        )
                    })
                    .unwrap_or((ProxyMode::Direct, false));
                Ok(account_summary(
                    record,
                    secret_available,
                    proxy_mode,
                    proxy_available,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let key_summaries = keys.iter().map(key_summary).collect::<Vec<_>>();
        let visible_model_ids = sources
            .iter()
            .filter(|record| record.enabled && !record.draining)
            .flat_map(|record| record.models.iter().cloned())
            .chain(
                accounts
                    .iter()
                    .filter(|record| record.enabled && !record.draining)
                    .flat_map(|record| record.models.iter().cloned()),
            )
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let running = self.store.gateway_enabled()? && self.runtime()?.is_some();
        Ok(RuntimeStateSnapshot {
            schema_version: SERVER_SCHEMA_VERSION,
            runtime_target: RuntimeTargetSummary {
                kind: "remote".to_string(),
                connected: true,
                origin: Some(self.config.public_base_url.origin().ascii_serialization()),
                server_id: Some(self.capabilities.server_id.clone()),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            },
            gateway: GatewaySummary {
                running,
                base_url: format!(
                    "{}/v1",
                    self.config.public_base_url.as_str().trim_end_matches('/')
                ),
                candidate_count: sources.len() + accounts.len(),
                visible_model_ids,
                common_proxy_configured,
                common_proxy_available,
            },
            platform: std::env::consts::OS.to_string(),
            capabilities: self.capabilities.clone(),
            sources: source_summaries,
            accounts: account_summaries,
            keys: key_summaries,
            automations: self.store.wake_tasks()?,
            wake_history: self.store.wake_state()?.history().iter().cloned().collect(),
            warnings,
        })
    }
}

pub(crate) fn account_proxy_config(
    state: &AppState,
    credential: &AccountCredential,
) -> Result<Option<ProxyConfig>, String> {
    if let Some(value) = credential.proxy_url.as_deref() {
        return ProxyConfig::parse(value)
            .map(Some)
            .map_err(|_| "stored account proxy URL is invalid".to_string());
    }
    if !state.store.common_proxy_configured()? {
        return Ok(None);
    }
    let value = state
        .vault
        .load(COMMON_PROXY_SECRET_REF)?
        .ok_or_else(|| "common account proxy is configured but unavailable".to_string())?;
    ProxyConfig::parse(&value)
        .map(Some)
        .map_err(|_| "stored common proxy URL is invalid".to_string())
}

fn common_proxy_available(state: &AppState, configured: bool) -> bool {
    configured
        && state
            .vault
            .load(COMMON_PROXY_SECRET_REF)
            .ok()
            .flatten()
            .is_some_and(|value| ProxyConfig::parse(&value).is_ok())
}

fn account_proxy_status(
    credential: &AccountCredential,
    common_configured: bool,
    common_available: bool,
) -> (ProxyMode, bool) {
    if let Some(value) = credential.proxy_url.as_deref() {
        return (ProxyMode::Account, ProxyConfig::parse(value).is_ok());
    }
    if common_configured {
        return (ProxyMode::Common, common_available);
    }
    (ProxyMode::Direct, true)
}

fn runtime_source(record: SourceRecord, api_key: String) -> RuntimeSource {
    RuntimeSource {
        source: ProviderSource {
            id: record.id,
            name: record.name,
            base_url: record.base_url,
            api_key,
            wire_api: record.wire_api,
            models: record.models,
        },
        enabled: record.enabled,
        draining: record.draining,
        priority: record.priority,
        weight: record.weight,
        allowed_models: record.allowed_models,
        excluded_models: record.excluded_models,
        last_used_at_ms: None,
    }
}

fn runtime_account(
    record: ServerAccountRecord,
    credential: &AccountCredential,
    proxy: Option<ProxyConfig>,
) -> RuntimeAccount {
    let quota = candidate_quota(&record);
    RuntimeAccount {
        id: record.id,
        source_id: record.source_id,
        chatgpt_account_id: credential.chatgpt_account_id.clone(),
        responses_url: credential.responses_url.clone(),
        models: record.models,
        enabled: record.enabled,
        draining: record.draining,
        priority: record.priority,
        weight: record.weight,
        allowed_models: record.allowed_models,
        excluded_models: record.excluded_models,
        health: candidate_health(record.auth_state, record.health),
        quota,
        last_used_at_ms: record.last_used_at_ms,
        cooldowns: record.cooldowns,
        consecutive_failures: record.consecutive_failures,
        proxy,
    }
}

fn runtime_key(record: GatewayKeyRecord, secret: String) -> RuntimeMixedLocalKey {
    RuntimeMixedLocalKey {
        key: LocalGatewayKey {
            id: record.id,
            secret,
        },
        enabled: record.enabled,
        source_ids: record.source_ids,
        account_ids: record.account_ids,
        allowed_models: record.allowed_models,
        excluded_models: record.excluded_models,
        model_prefix: record.model_prefix,
    }
}

fn candidate_health(auth: AccountAuthState, health: AccountHealthState) -> CandidateHealth {
    match auth {
        AccountAuthState::RequiresReauth(_) => return CandidateHealth::ReauthRequired,
        AccountAuthState::Error => return CandidateHealth::Unhealthy,
        _ => {}
    }
    match health {
        AccountHealthState::Unknown => CandidateHealth::Unknown,
        AccountHealthState::Healthy => CandidateHealth::Healthy,
        AccountHealthState::Degraded => CandidateHealth::Degraded,
        AccountHealthState::Unhealthy => CandidateHealth::Unhealthy,
        AccountHealthState::Blocked => CandidateHealth::Blocked,
    }
}

fn candidate_quota(record: &ServerAccountRecord) -> CandidateQuota {
    let available = record
        .quota
        .primary
        .as_ref()
        .and_then(|window| window.available_basis_points);
    match available {
        Some(0) => CandidateQuota::Exhausted,
        Some(value) => CandidateQuota::Available(u64::from(value)),
        None => CandidateQuota::Unknown,
    }
}

fn source_summary(record: &SourceRecord, secret_available: bool) -> SourceSummary {
    SourceSummary {
        id: record.id.clone(),
        name: record.name.clone(),
        enabled: record.enabled,
        draining: record.draining,
        base_url: record.base_url.clone(),
        wire_api: record.wire_api,
        models: record.models.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        priority: record.priority,
        weight: record.weight,
        secret_available,
        last_error_code: record.last_error_code.clone(),
    }
}

fn account_summary(
    record: &ServerAccountRecord,
    secret_available: bool,
    proxy_mode: ProxyMode,
    proxy_available: bool,
) -> AccountSummary {
    AccountSummary {
        id: record.id.clone(),
        label: record.label.clone(),
        identity_hint: record.identity_hint.clone(),
        enabled: record.enabled,
        draining: record.draining,
        auth_state: record.auth_state,
        health: format!("{:?}", record.health).to_ascii_lowercase(),
        models: record.models.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        priority: record.priority,
        weight: record.weight,
        subscription: record.subscription.clone(),
        quota: record.quota.clone(),
        secret_available,
        proxy_mode,
        proxy_available,
        last_error_code: record.last_error_code.clone(),
    }
}

fn key_summary(record: &GatewayKeyRecord) -> KeySummary {
    KeySummary {
        id: record.id.clone(),
        label: record.label.clone(),
        enabled: record.enabled,
        source_ids: record.source_ids.clone(),
        account_ids: record.account_ids.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        model_prefix: record.model_prefix.clone(),
        created_at_ms: record.created_at_ms,
        last_used_at_ms: record.last_used_at_ms,
    }
}

struct ServerTokenPersistence {
    state: Arc<AppState>,
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
}

fn find_account(state: &AppState, id: &str) -> Result<ServerAccountRecord, String> {
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

struct CodexRefreshClient {
    http: reqwest::Client,
}

impl CodexRefreshClient {
    fn new_with_proxy(proxy: Option<&ProxyConfig>) -> Result<Self, String> {
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

struct ServerRefreshClients {
    direct: CodexRefreshClient,
    clients: HashMap<String, CodexRefreshClient>,
}

impl TokenRefreshAdapter for ServerRefreshClients {
    fn refresh<'a>(
        &'a self,
        account_id: &'a str,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async move {
            let client = self.clients.get(account_id).unwrap_or(&self.direct);
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
                let code = serde_json::from_slice::<TokenError>(&body)
                    .ok()
                    .and_then(|value| value.error)
                    .unwrap_or_else(|| "token_refresh_failed".to_string());
                let kind = match code.as_str() {
                    "invalid_grant" => TokenRefreshFailureKind::InvalidGrant,
                    _ => TokenRefreshFailureKind::Transient,
                };
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

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
}

#[derive(Deserialize)]
struct TokenError {
    error: Option<String>,
}
