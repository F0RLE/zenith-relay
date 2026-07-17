use crate::state::{
    identity_hint, now_ms, AccountCredential, AppState, GatewayKeyRecord, ServerAccountRecord,
    SourceRecord, COMMON_PROXY_SECRET_REF, SERVER_SCHEMA_VERSION,
};
use futures_util::future::BoxFuture;
use reqwest::redirect::Policy;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{atomic::Ordering, mpsc, Arc},
    time::Duration,
};
use zenith_relay_core::{
    accounts::{
        AccountAuthState, AccountHealthState, TokenPersistenceAdapter, TokenPersistenceFailure,
        TokenRefresh, TokenRefreshAdapter, TokenRefreshFailure, TokenRefreshFailureKind, TokenSet,
    },
    protocol::{
        AccountRoutingExclusion, AccountSummary, GatewaySummary, KeySummary, ProxyMode,
        RuntimeStateSnapshot, RuntimeTargetSummary, SourceSummary,
    },
    ApiEquivalentSummary, CandidateHealth, CandidateQuota, GatewayRuntime, GatewayRuntimeOptions,
    LocalGatewayKey, ProviderSource, ProxyConfig, RuntimeAccount, RuntimeAccountAuth,
    RuntimeMixedLocalKey, RuntimeSource, UsageCallback, UsageEvent,
};

const CODEX_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;
const QUOTA_STALE_AFTER_MS: u64 = 15 * 60 * 1_000;
const USAGE_QUEUE_CAPACITY: usize = 16_384;
const USAGE_BATCH_SIZE: usize = 256;

struct QueuedUsage {
    event: UsageEvent,
    observed_at_ms: u64,
}

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
        self.token_authority
            .register_if_absent(account_id, credential.tokens()?, record.auth_state)
            .map_err(|error| error.to_string())?;
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
        let hidden_models = self.store.hidden_models()?;
        let use_free_accounts = self.store.quota_policy()?.2;
        let (max_retry_candidates, routing_strategy, default_service_tier, image_base_model) =
            self.store.routing_policy()?;
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
        let mut direct_refresh_accounts = HashSet::new();
        let mut refresh_clients = HashMap::new();
        let common_proxy_configured = self.store.common_proxy_configured()?;
        let account_proxy_required = self.store.account_proxy_required()?;
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
                None if account_proxy_required => continue,
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
            } else {
                direct_refresh_accounts.insert(record.id.clone());
            }
            accounts.push(runtime_account(
                record,
                &credential,
                proxy,
                use_free_accounts,
            ));
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
            direct_accounts: direct_refresh_accounts,
            clients: refresh_clients,
        });
        let persistence = Arc::new(ServerTokenPersistence {
            state: self.clone(),
        });
        let usage = self.usage_callback()?;
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
            GatewayRuntimeOptions {
                max_retry_candidates: usize::from(max_retry_candidates),
                routing_strategy,
                hidden_models,
                default_service_tier,
                quota_stale_after_ms: QUOTA_STALE_AFTER_MS,
                image_base_model,
                response_affinity_store: Some(self.store.clone()),
            },
            usage,
        )
        .map_err(|error| error.to_string())?;
        self.replace_runtime(Some(Arc::new(runtime)))
    }

    fn usage_callback(self: &Arc<Self>) -> Result<UsageCallback, String> {
        let (sender, receiver) = mpsc::sync_channel::<QueuedUsage>(USAGE_QUEUE_CAPACITY);
        let weak_state = Arc::downgrade(self);
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| "usage writer requires an async runtime".to_string())?;
        std::thread::Builder::new()
            .name("relay-usage-writer".to_string())
            .spawn(move || {
                while let Ok(first) = receiver.recv() {
                    let Some(state) = weak_state.upgrade() else {
                        break;
                    };
                    let mut batch = Vec::with_capacity(USAGE_BATCH_SIZE);
                    batch.push(first);
                    batch.extend(receiver.try_iter().take(USAGE_BATCH_SIZE - 1));
                    persist_usage_batch(&state, &batch, &runtime);
                }
            })
            .map_err(|error| format!("failed to start usage writer: {error}"))?;

        let weak_state = Arc::downgrade(self);
        Ok(Arc::new(move |event| {
            if sender
                .try_send(QueuedUsage {
                    event,
                    observed_at_ms: now_ms(),
                })
                .is_err()
            {
                if let Some(state) = weak_state.upgrade() {
                    state.failed_usage_writes.fetch_add(1, Ordering::Relaxed);
                }
            }
        }))
    }

    pub fn snapshot(&self) -> Result<RuntimeStateSnapshot, String> {
        let sources = self.store.sources()?;
        let accounts = self.store.accounts()?;
        let keys = self.store.keys()?;
        let common_proxy_configured = self.store.common_proxy_configured()?;
        let common_proxy_available = common_proxy_available(self, common_proxy_configured);
        let account_proxy_required = self.store.account_proxy_required()?;
        let (quota_refresh_interval_seconds, quota_request_timeout_seconds, use_free_accounts) =
            self.store.quota_policy()?;
        let (max_retry_candidates, routing_strategy, default_service_tier, image_base_model) =
            self.store.routing_policy()?;
        let hidden_models = self.store.hidden_models()?;
        let equivalents = self.store.api_equivalents()?;
        let mut warnings = Vec::new();
        if self.failed_usage_writes.load(Ordering::Relaxed) > 0 {
            warnings.push("usage_persistence_failed".to_string());
        }
        let source_summaries = sources
            .iter()
            .map(|record| {
                let secret_available = self.vault.load(&record.secret_ref)?.is_some();
                if !secret_available {
                    warnings.push(format!("source_secret_missing:{}", record.id));
                }
                Ok(source_summary(
                    record,
                    secret_available,
                    equivalents
                        .get(&identity_hint(&record.id))
                        .copied()
                        .unwrap_or_default(),
                ))
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
                            account_proxy_required,
                        )
                    })
                    .unwrap_or((ProxyMode::Direct, false));
                Ok(account_summary(
                    record,
                    secret_available,
                    proxy_mode,
                    proxy_available,
                    use_free_accounts,
                    equivalents
                        .get(&identity_hint(&record.id))
                        .copied()
                        .unwrap_or_default(),
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let key_summaries = keys.iter().map(key_summary).collect::<Vec<_>>();
        let models = zenith_relay_core::protocol::pool_model_summaries(
            &source_summaries,
            &account_summaries,
            &hidden_models,
        );
        let visible_model_ids = models
            .iter()
            .filter(|model| model.enabled)
            .map(|model| model.id.clone())
            .collect::<Vec<_>>();
        let runtime = self.runtime()?;
        let running = self.store.gateway_enabled()? && runtime.is_some();
        let routing_order = runtime
            .as_ref()
            .map(|runtime| runtime.candidate_runtime_order())
            .unwrap_or_default();
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
                candidate_count: source_summaries
                    .iter()
                    .filter(|record| {
                        record.enabled
                            && record.in_pool
                            && !record.draining
                            && record.secret_available
                    })
                    .count()
                    + account_summaries
                        .iter()
                        .filter(|record| {
                            record.enabled
                                && record.in_pool
                                && !record.draining
                                && record.secret_available
                                && record.proxy_available
                                && record.routing_exclusion.is_none()
                        })
                        .count(),
                visible_model_ids,
                max_retry_candidates: Some(max_retry_candidates),
                routing_strategy: Some(routing_strategy),
                default_service_tier: Some(default_service_tier),
                image_base_model,
                models,
                common_proxy_configured,
                common_proxy_available,
                account_proxy_required,
                quota_refresh_interval_seconds,
                quota_request_timeout_seconds,
                use_free_accounts,
                routing_order,
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

fn persist_usage_batch(
    state: &Arc<AppState>,
    batch: &[QueuedUsage],
    runtime: &tokio::runtime::Handle,
) {
    let records = batch
        .iter()
        .map(|queued| (&queued.event, queued.observed_at_ms))
        .collect::<Vec<_>>();
    if state.store.record_usage_batch(&records).is_err() {
        state
            .failed_usage_writes
            .fetch_add(batch.len() as u64, Ordering::Relaxed);
    }

    let mut account_events = BTreeMap::<String, Vec<&QueuedUsage>>::new();
    for queued in batch {
        if let Some(account_id) = queued.event.account_id.as_ref() {
            account_events
                .entry(account_id.clone())
                .or_default()
                .push(queued);
        }
    }
    let mut updated_accounts = Vec::with_capacity(account_events.len());
    let mut natural_uses = Vec::new();
    for (account_id, events) in account_events {
        let Ok(Some(mut account)) = state.store.account(&account_id) else {
            state.failed_usage_writes.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let mut natural_use_at_ms = None;
        for queued in events {
            let event = &queued.event;
            if event.success {
                account.last_used_at_ms = Some(queued.observed_at_ms);
                account.health = AccountHealthState::Healthy;
                account.consecutive_failures = 0;
                account.last_error_code = None;
                natural_use_at_ms = Some(queued.observed_at_ms);
            } else if event.cooldown_scope.is_some()
                || event.retry_at_ms.is_some()
                || event.consecutive_failures.is_some()
            {
                account.consecutive_failures = event
                    .consecutive_failures
                    .unwrap_or_else(|| account.consecutive_failures.saturating_add(1));
                account.health = AccountHealthState::Degraded;
                account.last_error_code = event.error_category.clone();
            }
        }
        updated_accounts.push(account);
        if let Some(observed_at_ms) = natural_use_at_ms {
            natural_uses.push((account_id, observed_at_ms));
        }
    }
    if !updated_accounts.is_empty() && state.store.save_accounts(&updated_accounts).is_err() {
        state
            .failed_usage_writes
            .fetch_add(updated_accounts.len() as u64, Ordering::Relaxed);
    }
    mark_natural_use(state.clone(), natural_uses, runtime);
}

fn mark_natural_use(
    state: Arc<AppState>,
    events: Vec<(String, u64)>,
    runtime: &tokio::runtime::Handle,
) {
    if events.is_empty() {
        return;
    }
    runtime.spawn(async move {
        let _guard = state.wake_lock.lock().await;
        let Ok(wake_state) = state.store.wake_state() else {
            return;
        };
        let Ok(mut coordinator) =
            zenith_relay_core::automations::WakeCoordinator::from_state(wake_state)
        else {
            return;
        };
        let changed = events
            .into_iter()
            .map(|(account_id, observed_at_ms)| {
                coordinator.mark_natural_use_for_account(&account_id, observed_at_ms)
            })
            .sum::<usize>();
        if changed > 0 {
            let _ = state.store.save_wake_state(coordinator.state());
        }
    });
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
        if state.store.account_proxy_required()? {
            return Err("an account proxy is required; direct account traffic is blocked".into());
        }
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
    account_proxy_required: bool,
) -> (ProxyMode, bool) {
    if let Some(value) = credential.proxy_url.as_deref() {
        return (ProxyMode::Account, ProxyConfig::parse(value).is_ok());
    }
    if common_configured {
        return (ProxyMode::Common, common_available);
    }
    (ProxyMode::Direct, !account_proxy_required)
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
        enabled: record.enabled && record.in_pool,
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
    use_free_accounts: bool,
) -> RuntimeAccount {
    let quota = candidate_quota(&record.quota, now_ms());
    // Older server records predate the persisted creation timestamp; token issue time is the
    // only stable ordering signal available for those records.
    let created_at_ms = if record.created_at_ms > 0 {
        record.created_at_ms
    } else {
        credential.issued_at_ms
    };
    let enabled = record.enabled
        && record.in_pool
        && (use_free_accounts || !record.subscription.is_free_plan());
    RuntimeAccount {
        id: record.id,
        source_id: record.source_id,
        chatgpt_account_id: credential.chatgpt_account_id.clone(),
        responses_url: credential.responses_url.clone(),
        models: record.models,
        enabled,
        draining: record.draining,
        priority: record.priority,
        weight: record.weight,
        allowed_models: record.allowed_models,
        excluded_models: record.excluded_models,
        health: candidate_health(record.auth_state, record.health),
        quota,
        quota_updated_at_ms: record.quota.updated_at_ms,
        created_at_ms,
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

fn candidate_quota(quota: &zenith_relay_core::quota::QuotaSnapshot, now_ms: u64) -> CandidateQuota {
    if quota
        .updated_at_ms
        .is_some_and(|updated_at| now_ms.saturating_sub(updated_at) > QUOTA_STALE_AFTER_MS)
    {
        return CandidateQuota::Stale;
    }
    let available = quota
        .primary
        .iter()
        .chain(quota.secondary.iter())
        .filter_map(|window| window.available_basis_points)
        .min();
    match available {
        Some(0) => CandidateQuota::Exhausted,
        Some(value) => CandidateQuota::Available(u64::from(value)),
        None => CandidateQuota::Unknown,
    }
}

fn source_summary(
    record: &SourceRecord,
    secret_available: bool,
    api_equivalent: ApiEquivalentSummary,
) -> SourceSummary {
    SourceSummary {
        id: record.id.clone(),
        name: record.name.clone(),
        enabled: record.enabled,
        in_pool: record.in_pool,
        draining: record.draining,
        base_url: record.base_url.clone(),
        wire_api: record.wire_api,
        models: record.models.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        priority: record.priority,
        weight: record.weight,
        api_equivalent,
        secret_available,
        last_error_code: record.last_error_code.clone(),
    }
}

fn account_summary(
    record: &ServerAccountRecord,
    secret_available: bool,
    proxy_mode: ProxyMode,
    proxy_available: bool,
    use_free_accounts: bool,
    api_equivalent: ApiEquivalentSummary,
) -> AccountSummary {
    let routing_exclusion = (!use_free_accounts && record.subscription.is_free_plan())
        .then_some(AccountRoutingExclusion::FreePlanPolicy);
    AccountSummary {
        id: record.id.clone(),
        label: record.label.clone(),
        identity_hint: record.identity_hint.clone(),
        enabled: record.enabled,
        in_pool: record.in_pool,
        draining: record.draining,
        auth_state: record.auth_state,
        health: format!("{:?}", record.health).to_ascii_lowercase(),
        models: record.models.clone(),
        allowed_models: record.allowed_models.clone(),
        excluded_models: record.excluded_models.clone(),
        priority: record.priority,
        weight: record.weight,
        api_equivalent,
        subscription: record.subscription.clone(),
        quota: record.quota.clone(),
        secret_available,
        proxy_mode,
        proxy_available,
        routing_exclusion,
        last_error_code: record.last_error_code.clone(),
    }
}

fn key_summary(record: &GatewayKeyRecord) -> KeySummary {
    KeySummary {
        id: record.id.clone(),
        label: record.label.clone(),
        enabled: record.enabled,
        system: false,
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
    direct_accounts: HashSet<String>,
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

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
}

fn token_refresh_failure_kind(code: &str) -> TokenRefreshFailureKind {
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

fn provider_error_code(body: &[u8]) -> Option<String> {
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
    use zenith_relay_core::quota::{QuotaSnapshot, QuotaWindow, QuotaWindowKind, Subscription};

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

    #[test]
    fn scheduler_uses_the_tightest_fresh_quota_window() {
        let window = |kind, available_basis_points| QuotaWindow {
            kind,
            available_basis_points: Some(available_basis_points),
            explicitly_full: None,
            reset_at_ms: None,
            window_minutes: None,
            full_transition_fingerprint: None,
            observed_at_ms: 1_000,
        };
        let quota = QuotaSnapshot {
            primary: Some(window(QuotaWindowKind::Primary, 9_000)),
            secondary: Some(window(QuotaWindowKind::Secondary, 2_500)),
            updated_at_ms: Some(1_000),
            ..Default::default()
        };
        assert_eq!(
            candidate_quota(&quota, 2_000),
            CandidateQuota::Available(2_500)
        );
        assert_eq!(
            candidate_quota(&quota, QUOTA_STALE_AFTER_MS + 1_001),
            CandidateQuota::Stale
        );
    }

    #[test]
    fn free_accounts_require_explicit_routing_opt_in() {
        let record = ServerAccountRecord {
            id: "account-free".into(),
            label: "Free".into(),
            identity_hint: "free-account".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            source_id: "codex".into(),
            secret_ref: "account:free".into(),
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            models: vec!["gpt-test".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            subscription: Subscription {
                plan_type: Some("free".into()),
                ..Subscription::default()
            },
            quota: QuotaSnapshot::default(),
            cooldowns: BTreeMap::new(),
            consecutive_failures: 0,
            created_at_ms: 1,
            last_used_at_ms: None,
            last_error_code: None,
        };
        let credential = AccountCredential {
            access_token: "access".into(),
            refresh_token: None,
            id_token: None,
            expires_at_ms: None,
            issued_at_ms: 1,
            generation: 0,
            chatgpt_account_id: "provider-account".into(),
            responses_url: "https://example.test/responses".into(),
            proxy_url: None,
        };

        assert!(!runtime_account(record.clone(), &credential, None, false).enabled);
        assert!(runtime_account(record.clone(), &credential, None, true).enabled);
        let summary = account_summary(
            &record,
            true,
            ProxyMode::Direct,
            true,
            false,
            ApiEquivalentSummary::default(),
        );
        assert_eq!(
            summary.routing_exclusion,
            Some(AccountRoutingExclusion::FreePlanPolicy)
        );
        assert!(summary.enabled);
        assert!(summary.in_pool);
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
