use crate::state::{
    identity_hint, is_internal_gateway_key, now_ms, AccountCredential, AppState,
    ServerAccountRecord, SourceRecord, SERVER_SCHEMA_VERSION,
};
use crate::store::configuration_revision;
use crate::token_refresh::{
    find_account, CodexRefreshClient, ServerRefreshClients, ServerTokenPersistence,
};
use crate::usage_writer::UsageWriter;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::{atomic::Ordering, Arc},
};
#[cfg(test)]
use zenith_relay_core::accounts::AccountHealthState;
use zenith_relay_core::{
    accounts::TokenSet,
    protocol::{
        apply_model_reasoning_summary, GatewaySummary, OperationalStatus, ProxyMode,
        RuntimeStateSnapshot, RuntimeTargetSummary,
    },
    quota::{attach_quota_plan_benchmarks, quota_plan_benchmarks, quota_valuation_revision},
    CandidateKind, CandidateScope, GatewayRuntime, GatewayRuntimeOptions, RuntimeChatGptAuth,
    UsageCallback, WireApi, QUOTA_STALE_AFTER_MS,
};

mod account_runtime;

pub(crate) use account_runtime::{
    account_proxy_config, model_has_native_account_route, prepare_server_account_authorization,
};
use account_runtime::{
    account_proxy_status, account_summary, common_proxy_available, runtime_account, runtime_key,
    runtime_source, source_runtime_available, source_summary,
};

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
        let proxy = account_proxy_config(self, &record, &credential)?;
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

    pub async fn recover_account_tokens_after_unauthorized(
        self: &Arc<Self>,
        account_id: &str,
    ) -> Result<TokenSet, String> {
        let persistence = ServerTokenPersistence {
            state: self.clone(),
        };
        self.token_authority
            .invalidate_access_and_persist(account_id, now_ms(), &persistence)
            .await
            .map_err(|error| error.to_string())?;
        self.prepare_account_tokens(account_id).await
    }

    pub async fn rebuild_runtime(self: &Arc<Self>) -> Result<(), String> {
        let source_records = self.store.sources()?;
        let account_records = self.store.accounts()?;
        let key_records = self
            .store
            .keys()?
            .into_iter()
            .filter(|key| key.enabled && is_internal_gateway_key(key))
            .collect::<Vec<_>>();
        let hidden_models = self.store.hidden_models()?;
        let model_reasoning_allowed_levels = self.store.model_reasoning_allowed_levels()?;
        let quota_stale_after_ms = QUOTA_STALE_AFTER_MS;
        let routing_policy = self.store.routing_policy()?;
        // Candidate state remains available to management, while the internal
        // profile credential derives its request scope solely from pool membership.
        let mut pool_source_ids = source_records
            .iter()
            .filter(|record| {
                record.in_pool
                    && record
                        .supports_wire_api(WireApi::Responses)
                        .unwrap_or(false)
            })
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        let mut pool_account_ids = account_records
            .iter()
            .filter(|record| record.in_pool)
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
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
        let mut agent_identities = HashMap::new();
        for record in account_records {
            let Some(secret) = self.vault.load(&record.secret_ref)? else {
                continue;
            };
            let credential: AccountCredential = serde_json::from_str(&secret)
                .map_err(|_| "stored account credential is invalid".to_string())?;
            let proxy = match account_proxy_config(self, &record, &credential) {
                Ok(proxy) => proxy,
                Err(_) => continue,
            };
            if let Some(agent) = credential.agent_identity()? {
                agent_identities.insert(record.id.clone(), agent);
            }
            if credential.has_oauth() {
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
            }
            accounts.push(runtime_account(
                record,
                &credential,
                proxy,
                quota_stale_after_ms,
            ));
        }

        if !sources
            .iter()
            .any(|source| source.enabled && !source.draining)
            && !accounts
                .iter()
                .any(|account| account.enabled && !account.draining)
        {
            return self.replace_runtime(None);
        }
        let active_source_ids = sources
            .iter()
            .filter(|source| source.enabled && !source.draining)
            .map(|source| source.source.id.as_str())
            .collect::<HashSet<_>>();
        pool_source_ids.retain(|id| active_source_ids.contains(id.as_str()));
        let active_account_ids = accounts
            .iter()
            .filter(|account| account.enabled && !account.draining)
            .map(|account| account.id.as_str())
            .collect::<HashSet<_>>();
        pool_account_ids.retain(|id| active_account_ids.contains(id.as_str()));

        let mut keys = Vec::new();
        for record in key_records {
            let Some(secret) = self.vault.load(&record.secret_ref)? else {
                continue;
            };
            if pool_source_ids.is_empty() && pool_account_ids.is_empty() {
                continue;
            }
            keys.push(runtime_key(
                record,
                secret,
                &pool_source_ids,
                &pool_account_ids,
            ));
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
            RuntimeChatGptAuth {
                token_authority: self.token_authority.clone(),
                refresh_adapter: refresh,
                persistence_adapter: persistence,
                refresh_skew_ms: 60_000,
                agent_identities,
            },
            GatewayRuntimeOptions {
                max_retry_candidates: usize::from(routing_policy.max_retry_candidates),
                cooldown_after_failures: routing_policy.cooldown_after_failures,
                keep_last_candidate_available: routing_policy.keep_last_candidate_available,
                routing_strategy: routing_policy.routing_strategy,
                subscription_plan_order: routing_policy.subscription_plan_order,
                hidden_models,
                default_service_tier: routing_policy.default_service_tier,
                quota_stale_after_ms,
                image_base_model: None,
                model_reasoning_allowed_levels,
                response_affinity_store: Some(self.store.clone()),
                provider_storm_breaker: true,
            },
            usage,
        )
        .map_err(|error| error.to_string())?;
        self.replace_runtime(Some(Arc::new(runtime)))
    }

    /// Rebuilds the runtime from persisted state and restores the previous
    /// configuration if activation fails. Callers use this for mutations that
    /// have already been committed to the store or vault.
    pub(crate) async fn rebuild_runtime_or_rollback<F>(
        self: &Arc<Self>,
        rollback: F,
    ) -> Result<(), String>
    where
        F: FnOnce() -> Result<(), String>,
    {
        let Err(error) = self.rebuild_runtime().await else {
            return Ok(());
        };
        rollback().map_err(|rollback_error| {
            format!("{error}; failed to restore persisted state: {rollback_error}")
        })?;
        self.rebuild_runtime().await.map_err(|restore_error| {
            format!("{error}; failed to rebuild previous runtime: {restore_error}")
        })?;
        Err(error)
    }

    /// Restores persisted state after an in-place activation failed, then
    /// rebuilds the runtime from that restored state.
    pub(crate) async fn rollback_and_rebuild_runtime<F>(
        self: &Arc<Self>,
        rollback: F,
    ) -> Result<(), String>
    where
        F: FnOnce() -> Result<(), String>,
    {
        rollback().map_err(|error| format!("failed to restore persisted state: {error}"))?;
        self.rebuild_runtime()
            .await
            .map_err(|error| format!("failed to rebuild previous runtime: {error}"))
    }

    /// Updates the scopes of all active internal profile keys after a candidate
    /// policy changes in place. This keeps an enabled or un-drained pool member
    /// reachable without replacing the runtime that owns active streams.
    pub(crate) fn refresh_internal_gateway_key_scopes(
        &self,
        runtime: &GatewayRuntime,
    ) -> Result<bool, String> {
        let sources = self.store.sources()?;
        let accounts = self.store.accounts()?;
        let keys = self
            .store
            .keys()?
            .into_iter()
            .filter(|key| key.enabled && is_internal_gateway_key(key))
            .collect::<Vec<_>>();
        if keys.is_empty() {
            // There is no internal profile scope to synchronize. The policy
            // has already been applied to the live runtime, so this is a
            // successful no-op rather than a reason to replace it.
            return Ok(true);
        }
        let scope = Self::active_internal_gateway_scope(&sources, &accounts, runtime);
        Ok(keys
            .iter()
            .all(|key| runtime.update_key_scope(&key.id, scope.clone())))
    }

    fn active_internal_gateway_scope(
        sources: &[SourceRecord],
        accounts: &[ServerAccountRecord],
        runtime: &GatewayRuntime,
    ) -> CandidateScope {
        let source_ids = sources
            .iter()
            .filter(|source| {
                source.in_pool
                    && source
                        .supports_wire_api(WireApi::Responses)
                        .unwrap_or(false)
            })
            .map(|source| source.id.clone())
            .collect::<BTreeSet<_>>();
        let account_ids = accounts
            .iter()
            .filter(|account| account.in_pool)
            .map(|account| account.id.clone())
            .collect::<BTreeSet<_>>();
        runtime.active_responses_scope(&source_ids, &account_ids)
    }

    fn usage_callback(self: &Arc<Self>) -> Result<UsageCallback, String> {
        let mut writer = self
            .usage_writer
            .lock()
            .map_err(|_| "usage writer lock poisoned".to_string())?;
        if writer.is_none() {
            *writer = Some(UsageWriter::start(self)?);
        }
        Ok(writer
            .as_ref()
            .expect("usage writer initialized")
            .callback())
    }

    pub async fn shutdown_runtime(self: &Arc<Self>) -> Result<(), String> {
        self.replace_runtime(None)?;
        let writer = self
            .usage_writer
            .lock()
            .map_err(|_| "usage writer lock poisoned".to_string())?
            .take();
        if let Some(writer) = writer {
            writer.shutdown().await?;
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Result<RuntimeStateSnapshot, String> {
        let sources = self.store.sources()?;
        let accounts = self.store.accounts()?;
        let common_proxy_configured = self.store.common_proxy_configured()?;
        let common_proxy_id = self.store.common_proxy_id()?;
        let common_proxy_available = common_proxy_available(self, common_proxy_configured);
        let account_proxy_required = self.store.account_proxy_required()?;
        let quota_request_timeout_seconds = self.store.quota_request_timeout_seconds()?;
        let routing_policy = self.store.routing_policy()?;
        let hidden_models = self.store.hidden_models()?;
        let model_price_overrides = self.store.model_price_overrides()?;
        let model_reasoning_allowed_levels = self.store.model_reasoning_allowed_levels()?;
        let configuration_revision = configuration_revision(&self.store.configuration_settings()?)?;
        let equivalents = self.store.api_equivalents()?;
        let runtime = self.runtime()?;
        let running = self.store.gateway_enabled()? && runtime.is_some();
        let routing_order = runtime
            .as_ref()
            .map(|runtime| runtime.candidate_runtime_order())
            .unwrap_or_default();
        let source_runtime = routing_order
            .iter()
            .filter(|candidate| candidate.kind == CandidateKind::ApiSource)
            .map(|candidate| (candidate.candidate_id.as_str(), candidate.available))
            .collect::<HashMap<_, _>>();
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
                    (running && record.enabled)
                        .then(|| source_runtime_available(&source_runtime, &record.id)),
                    equivalents
                        .get(&identity_hint(&record.id))
                        .copied()
                        .unwrap_or_default(),
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut account_summaries = accounts
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
                            self,
                            record,
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
                    equivalents
                        .get(&identity_hint(&record.id))
                        .copied()
                        .unwrap_or_default(),
                    routing_policy.default_service_tier,
                    QUOTA_STALE_AFTER_MS,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let economics_revision = quota_valuation_revision();
        let plan_benchmarks = quota_plan_benchmarks(
            accounts
                .iter()
                .map(|account| (account.id.as_str(), &account.economics)),
            now_ms(),
            economics_revision,
        );
        for (record, summary) in accounts.iter().zip(&mut account_summaries) {
            attach_quota_plan_benchmarks(
                &mut summary.economics,
                "chatgpt",
                record.subscription.plan_type.as_deref(),
                &record.quota,
                routing_policy.default_service_tier,
                economics_revision,
                &plan_benchmarks,
            );
        }
        let mut models = zenith_relay_core::protocol::pool_model_summaries(
            &source_summaries,
            &account_summaries,
            &hidden_models,
        );
        for model in &mut models {
            let model_id = model.id.clone();
            if let Some(price) = model_price_overrides.get(&model_id.to_ascii_lowercase()) {
                model.input_micro_usd_per_million = Some(price.input_micro_usd_per_million);
                model.cached_input_micro_usd_per_million = Some(
                    price
                        .cached_input_micro_usd_per_million
                        .unwrap_or(price.input_micro_usd_per_million),
                );
                model.cache_write_5m_micro_usd_per_million =
                    price.cache_write_5m_micro_usd_per_million;
                model.cache_write_1h_micro_usd_per_million =
                    price.cache_write_1h_micro_usd_per_million;
                model.output_micro_usd_per_million = Some(price.output_micro_usd_per_million);
                model.custom_price = true;
            }
            apply_model_reasoning_summary(
                model,
                runtime
                    .as_ref()
                    .map(|runtime| runtime.confirmed_source_reasoning_levels(&model_id))
                    .unwrap_or_default(),
                model_reasoning_allowed_levels
                    .get(&model_id.to_ascii_lowercase())
                    .map(Vec::as_slice),
                model_has_native_account_route(&account_summaries, &model_id),
            );
        }
        let visible_model_ids = models
            .iter()
            .filter(|model| model.enabled)
            .map(|model| model.id.clone())
            .collect::<Vec<_>>();
        Ok(RuntimeStateSnapshot {
            schema_version: SERVER_SCHEMA_VERSION,
            configuration_revision: Some(configuration_revision),
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
                        record.in_pool
                            && record.supports_wire_api(WireApi::Responses)
                            && record.operational_status == OperationalStatus::Rotation
                    })
                    .count()
                    + account_summaries
                        .iter()
                        .filter(|record| {
                            record.in_pool
                                && record.operational_status == OperationalStatus::Rotation
                        })
                        .count(),
                visible_model_ids,
                max_retry_candidates: routing_policy.max_retry_candidates,
                cooldown_after_failures: routing_policy.cooldown_after_failures,
                keep_last_candidate_available: routing_policy.keep_last_candidate_available,
                routing_strategy: routing_policy.routing_strategy,
                subscription_plan_order: routing_policy.subscription_plan_order,
                default_service_tier: routing_policy.default_service_tier,
                image_base_model: routing_policy.image_base_model,
                models,
                common_proxy_configured,
                common_proxy_available,
                common_proxy_id,
                account_proxy_required,
                quota_request_timeout_seconds,
                chatgpt_interface_quota_reserve_basis_points: None,
                routing_order,
            },
            platform: std::env::consts::OS.to_string(),
            capabilities: self.capabilities.clone(),
            sources: source_summaries,
            accounts: account_summaries,
            automations: self.store.wake_tasks()?,
            wake_history: self.store.wake_state()?.history().iter().cloned().collect(),
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        store::{Store, Vault},
    };
    use std::collections::BTreeMap;
    use tempfile::TempDir;
    use zenith_relay_core::accounts::AccountAuthState;
    use zenith_relay_core::quota::{QuotaSnapshot, QuotaWindow, QuotaWindowKind, Subscription};
    use zenith_relay_core::{
        protocol::UsageQuery, ApiEquivalentSummary, CandidateQuota, DefaultServiceTier, UsageEvent,
        WireApi,
    };

    #[tokio::test]
    async fn usage_writer_is_reused_and_flushes_before_shutdown() {
        let root = TempDir::new().unwrap();
        let config = Config::for_test(root.path().to_path_buf(), "127.0.0.1:0".parse().unwrap());
        let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
        let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
        let state = AppState::new(config, store.clone(), vault).unwrap();
        let first = state.usage_callback().unwrap();
        let second = state.usage_callback().unwrap();
        assert!(Arc::ptr_eq(&first, &second));

        first(UsageEvent {
            request_id: "req_shutdown_flush".into(),
            attempt: 1,
            local_key_id: "key_test".into(),
            source_id: "source_test".into(),
            candidate_id: Some("source_test".into()),
            account_id: None,
            routing: None,
            requested_model: Some("gpt-test".into()),
            resolved_model: Some("gpt-test".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: true,
            http_status: 200,
            error_category: None,
            tool_use: zenith_relay_core::ToolUseDiagnostics::default(),
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 10,
            ttft_ms: Some(3),
            generation_ms: Some(7),
            input_tokens: Some(2),
            cached_input_tokens: Some(1),
            cache_write_input_tokens: None,
            reasoning_tokens: None,
            output_tokens: Some(3),
            total_tokens: Some(5),
            quota_snapshot: None,
        });
        state.shutdown_runtime().await.unwrap();

        let usage = store.usage_page(&UsageQuery::default()).unwrap();
        assert_eq!(usage.total, 1);
        assert_eq!(usage.events[0].request_id, "req_shutdown_flush");
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
            CandidateQuota::from_snapshot(&quota, 2_000, zenith_relay_core::QUOTA_STALE_AFTER_MS,),
            CandidateQuota::Available(2_500)
        );
        assert_eq!(
            CandidateQuota::from_snapshot(
                &quota,
                zenith_relay_core::QUOTA_STALE_AFTER_MS + 1_001,
                zenith_relay_core::QUOTA_STALE_AFTER_MS,
            ),
            CandidateQuota::Stale
        );
    }

    #[test]
    fn free_accounts_route_like_other_pool_accounts() {
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
            economics: Default::default(),
            cooldowns: BTreeMap::new(),
            consecutive_failures: 0,
            created_at_ms: 1,
            last_used_at_ms: None,
            last_error_code: None,
            proxy_id: None,
            bypass_common_proxy: false,
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
            agent_private_key: None,
            agent_runtime_id: None,
            agent_task_id: None,
        };

        assert!(
            runtime_account(
                record.clone(),
                &credential,
                None,
                zenith_relay_core::QUOTA_STALE_AFTER_MS,
            )
            .enabled
        );
        let mut exhausted = record.clone();
        exhausted.quota = QuotaSnapshot {
            primary: Some(QuotaWindow {
                kind: QuotaWindowKind::Primary,
                available_basis_points: Some(0),
                explicitly_full: None,
                reset_at_ms: Some(60_000),
                window_minutes: None,
                observed_at_ms: 1_000,
                full_transition_fingerprint: None,
            }),
            updated_at_ms: Some(1_000),
            ..Default::default()
        };
        assert!(
            runtime_account(
                exhausted,
                &credential,
                None,
                zenith_relay_core::QUOTA_STALE_AFTER_MS,
            )
            .enabled,
            "a quota wait must stay instantiated so a refresh can re-enable it"
        );
        let summary = account_summary(
            &record,
            true,
            ProxyMode::Direct,
            true,
            ApiEquivalentSummary::default(),
            DefaultServiceTier::Standard,
            zenith_relay_core::QUOTA_STALE_AFTER_MS,
        );
        assert_eq!(summary.operational_status, OperationalStatus::Rotation);
        assert!(summary.enabled);
        assert!(summary.in_pool);
    }

    #[test]
    fn source_runtime_status_matches_protocol_candidates() {
        let runtime = HashMap::from([
            ("source::messages", true),
            ("source::responses", false),
            ("other", true),
        ]);

        assert!(source_runtime_available(&runtime, "source"));
        assert!(!source_runtime_available(&runtime, "missing"));
        assert!(!source_runtime_available(&runtime, "sour"));
    }
}
