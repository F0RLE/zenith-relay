use crate::state::{
    is_internal_gateway_key, now_ms, AccountCredential, AppState, ServerAccountRecord, SourceRecord,
};
use crate::token_refresh::{find_account, CodexRefreshClient, ServerTokenPersistence};
use crate::usage_writer::UsageWriter;
use std::{collections::BTreeSet, sync::Arc};
#[cfg(test)]
use zenith_relay_core::accounts::AccountHealthState;
use zenith_relay_core::{
    accounts::TokenSet, protocol::RuntimeStateSnapshot, CandidateScope, GatewayRuntime,
    UsageCallback, WireApi,
};

mod account_runtime;
mod runtime_build;
mod snapshot;

pub(crate) use account_runtime::{account_proxy_config, prepare_server_account_authorization};

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
        runtime_build::rebuild(self).await
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
        snapshot::build(self)
    }
}

#[cfg(test)]
mod tests {
    use super::account_runtime::{account_summary, runtime_account};
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
        protocol::{OperationalStatus, ProxyMode, UsageQuery},
        ApiEquivalentSummary, ApiModelPriceOverride, CandidateQuota, DefaultServiceTier,
        UsageEvent, WireApi,
    };

    fn snapshot_test_state(root: &TempDir) -> Arc<AppState> {
        let config = Config::for_test(root.path().to_path_buf(), "127.0.0.1:0".parse().unwrap());
        let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
        let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
        AppState::new(config, store, vault).unwrap()
    }

    fn snapshot_test_source(id: &str, model: &str) -> SourceRecord {
        SourceRecord {
            id: id.into(),
            name: "Snapshot source".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            base_url: "https://example.test/v1".into(),
            secret_ref: format!("source:{id}"),
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            models: vec![model.into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: BTreeMap::new(),
            detected_model_prices: BTreeMap::new(),
            last_error_code: None,
        }
    }

    fn snapshot_test_account(id: &str, model: &str) -> ServerAccountRecord {
        ServerAccountRecord {
            id: id.into(),
            label: "Snapshot account".into(),
            identity_hint: "snapshot-account".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            source_id: "openai_codex".into(),
            secret_ref: format!("account:{id}"),
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            models: vec![model.into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            subscription: Subscription::default(),
            quota: QuotaSnapshot::default(),
            purchase_cost_micro_usd: None,
            cooldowns: BTreeMap::new(),
            consecutive_failures: 0,
            created_at_ms: 1,
            last_used_at_ms: None,
            last_error_code: None,
            proxy_id: None,
            bypass_common_proxy: false,
        }
    }

    #[test]
    fn snapshot_preserves_persisted_model_policy_and_missing_secret_warning() {
        let root = TempDir::new().unwrap();
        let state = snapshot_test_state(&root);
        let source = snapshot_test_source("source-snapshot", "gpt-5.4");
        state.store.save_source(&source).unwrap();
        state
            .vault
            .save(&source.secret_ref, "synthetic-source-key")
            .unwrap();
        state
            .store
            .save_account(&snapshot_test_account(
                "account-missing",
                "gpt-account-test",
            ))
            .unwrap();
        state
            .store
            .set_hidden_models(vec!["gpt-5.4".into()])
            .unwrap();
        state
            .store
            .set_model_price_overrides(BTreeMap::from([(
                "gpt-5.4".into(),
                ApiModelPriceOverride {
                    input_micro_usd_per_million: 1_000,
                    cached_input_micro_usd_per_million: Some(100),
                    cache_write_5m_micro_usd_per_million: Some(1_500),
                    cache_write_1h_micro_usd_per_million: Some(2_000),
                    output_micro_usd_per_million: 3_000,
                },
            )]))
            .unwrap();

        let snapshot = state.snapshot().unwrap();

        assert_eq!(snapshot.runtime_target.kind, "remote");
        assert!(!snapshot.gateway.running);
        assert_eq!(snapshot.gateway.candidate_count, 1);
        assert!(snapshot.gateway.visible_model_ids.is_empty());
        assert_eq!(snapshot.sources.len(), 1);
        assert!(snapshot.sources[0].secret_available);
        assert_eq!(snapshot.accounts.len(), 1);
        assert!(!snapshot.accounts[0].secret_available);
        assert_eq!(
            snapshot.warnings,
            vec!["account_secret_missing:account-missing"]
        );

        let model = snapshot.gateway.models.first().unwrap();
        assert_eq!(model.id, "gpt-5.4");
        assert!(!model.enabled);
        assert!(model.custom_price);
        assert_eq!(model.input_micro_usd_per_million, Some(1_000));
        assert_eq!(model.cached_input_micro_usd_per_million, Some(100));
        assert_eq!(model.cache_write_5m_micro_usd_per_million, Some(1_500));
        assert_eq!(model.cache_write_1h_micro_usd_per_million, Some(2_000));
        assert_eq!(model.output_micro_usd_per_million, Some(3_000));
    }

    #[tokio::test]
    async fn snapshot_reports_the_active_runtime_candidate_order() {
        let root = TempDir::new().unwrap();
        let state = snapshot_test_state(&root);
        let source = snapshot_test_source("source-runtime", "gpt-runtime-test");
        state.store.save_source(&source).unwrap();
        state
            .vault
            .save(&source.secret_ref, "synthetic-source-key")
            .unwrap();
        state.rebuild_runtime().await.unwrap();

        let snapshot = state.snapshot().unwrap();

        assert!(snapshot.gateway.running);
        assert_eq!(snapshot.gateway.candidate_count, 1);
        assert_eq!(snapshot.gateway.visible_model_ids, ["gpt-runtime-test"]);
        assert_eq!(snapshot.gateway.routing_order.len(), 1);
        assert_eq!(snapshot.gateway.routing_order[0].candidate_id, source.id);
        assert!(snapshot.gateway.routing_order[0].available);
        assert_eq!(
            snapshot.sources[0].operational_status,
            OperationalStatus::Rotation
        );

        state.shutdown_runtime().await.unwrap();
    }

    #[tokio::test]
    async fn rebuild_runtime_requires_a_responses_pool_member_for_the_system_key() {
        let root = TempDir::new().unwrap();
        let state = snapshot_test_state(&root);
        let mut messages = snapshot_test_source("source-messages", "claude-native-test");
        messages.wire_api = WireApi::Messages;
        state.store.save_source(&messages).unwrap();
        state
            .vault
            .save(&messages.secret_ref, "synthetic-source-key")
            .unwrap();

        state.rebuild_runtime().await.unwrap();
        assert!(state.runtime().unwrap().is_none());

        let responses = snapshot_test_source("source-responses", "gpt-runtime-test");
        state.store.save_source(&responses).unwrap();
        state
            .vault
            .save(&responses.secret_ref, "synthetic-source-key")
            .unwrap();

        state.rebuild_runtime().await.unwrap();
        let snapshot = state.snapshot().unwrap();
        assert!(snapshot.gateway.running);
        assert_eq!(snapshot.gateway.candidate_count, 1);
        assert_eq!(snapshot.gateway.visible_model_ids, ["gpt-runtime-test"]);

        state.shutdown_runtime().await.unwrap();
    }

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
            requested_reasoning_effort: None,
            effective_reasoning_effort: None,
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
            purchase_cost_micro_usd: None,
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
            zenith_relay_core::QUOTA_STALE_AFTER_MS,
        );
        assert_eq!(summary.operational_status, OperationalStatus::Rotation);
        assert!(summary.enabled);
        assert!(summary.in_pool);
    }
}
