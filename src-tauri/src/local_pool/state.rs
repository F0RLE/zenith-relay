mod adapters;
mod coordination;
mod snapshot;

pub(crate) use adapters::DesktopOAuthEvents;
use coordination::wake_coordinator;
#[cfg(test)]
use snapshot::{account_secret_available, SecretLookup};

use super::{
    accounts::{
        import_session::ImportSessionStore, oauth_flow::OAuthFlowManager, NativeSecretBackend,
    },
    error::{ErrorCode, LocalPoolError, Result},
    host::GatewayManager,
    profiles::repair,
    store::{telemetry_db::TelemetryDb, LocalPoolStore},
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{atomic::AtomicU64, Arc, Mutex, MutexGuard},
};
use tokio::sync::{Mutex as AsyncMutex, Notify};
use zenith_relay_core::{
    accounts::TokenAuthority, automations::WakeCoordinator, quota::QuotaRefreshQueue,
};

pub(super) use zenith_relay_core::unix_time_ms as now_ms;

#[cfg(test)]
use zenith_relay_core::DefaultServiceTier;

const MAX_QUOTA_REFRESH_ENTRIES: usize = crate::local_pool::models::MAX_LOCAL_ACCOUNTS;

pub struct DesktopState {
    pub(crate) root: PathBuf,
    pub(crate) gateway: GatewayManager,
    pub(crate) telemetry: Arc<TelemetryDb>,
    store: Arc<Mutex<LocalPoolStore>>,
    token_authority: Arc<TokenAuthority>,
    quota_refresh: Arc<Mutex<QuotaRefreshQueue>>,
    quota_refresh_notify: Arc<Notify>,
    wake: Arc<Mutex<WakeCoordinator>>,
    wake_notify: Arc<Notify>,
    oauth_flow: OAuthFlowManager<NativeSecretBackend, DesktopOAuthEvents>,
    oauth_events: DesktopOAuthEvents,
    failed_usage_writes: Arc<AtomicU64>,
    failed_affinity_writes: Arc<AtomicU64>,
    quota_account_locks: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
    subscription_refresh_lock: AsyncMutex<()>,
    setup_lock: tokio::sync::Mutex<()>,
}

impl DesktopState {
    pub fn open(root: PathBuf) -> Result<Self> {
        let transient_root = root.join("cache");
        if transient_root.exists() {
            let _ = std::thread::Builder::new()
                .name("transient-cleanup".to_string())
                .spawn(move || {
                    let _ = ImportSessionStore::new(transient_root.clone(), NativeSecretBackend)
                        .cleanup_expired();
                    let _ = repair::cleanup_expired_previews(&transient_root);
                });
        }
        let mut store = LocalPoolStore::open(root.clone())?;
        let telemetry = store.database();
        let mut quota_refresh =
            QuotaRefreshQueue::new(MAX_QUOTA_REFRESH_ENTRIES).map_err(invalid_core_state)?;
        let startup_due_at_ms = now_ms();
        for account in store.accounts().iter().filter(|account| {
            account.remote_location.is_none()
                && account.account.is_automatic_quota_monitoring_eligible()
        }) {
            quota_refresh
                .upsert(&account.account.id, startup_due_at_ms)
                .map_err(invalid_core_state)?;
        }
        let wake = wake_coordinator(store.automations())?;
        if &store.automations().state != wake.state() {
            let mut automations = store.automations().clone();
            automations.state = wake.state().clone();
            store.replace_automations(automations)?;
        }
        let failed_usage_writes = Arc::new(AtomicU64::new(0));
        let failed_affinity_writes = Arc::new(AtomicU64::new(0));
        let token_authority = Arc::new(
            TokenAuthority::new(crate::local_pool::models::MAX_LOCAL_ACCOUNTS)
                .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?,
        );
        let oauth_events = DesktopOAuthEvents::default();
        let oauth_flow = OAuthFlowManager::new(
            root.join("cache"),
            NativeSecretBackend,
            oauth_events.clone(),
        );
        Ok(Self {
            root,
            gateway: GatewayManager::default(),
            telemetry,
            store: Arc::new(Mutex::new(store)),
            token_authority,
            quota_refresh: Arc::new(Mutex::new(quota_refresh)),
            quota_refresh_notify: Arc::new(Notify::new()),
            wake: Arc::new(Mutex::new(wake)),
            wake_notify: Arc::new(Notify::new()),
            oauth_flow,
            oauth_events,
            failed_usage_writes,
            failed_affinity_writes,
            quota_account_locks: Arc::new(Mutex::new(HashMap::new())),
            subscription_refresh_lock: AsyncMutex::new(()),
            setup_lock: tokio::sync::Mutex::new(()),
        })
    }

    pub fn store(&self) -> Result<MutexGuard<'_, LocalPoolStore>> {
        self.store
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "local pool store lock poisoned"))
    }

    pub(crate) fn token_authority(&self) -> Arc<TokenAuthority> {
        self.token_authority.clone()
    }

    pub(crate) fn record_performance(
        &self,
        name: &str,
        duration_ms: f64,
        context: Option<&str>,
    ) -> Result<()> {
        self.telemetry
            .record_performance(name, duration_ms, context)
    }

    pub async fn setup_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.setup_lock.lock().await
    }

    pub(crate) fn quota_account_lock(&self, account_id: &str) -> Result<Arc<AsyncMutex<()>>> {
        let mut locks = self
            .quota_account_locks
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "quota account lock poisoned"))?;
        Ok(locks
            .entry(account_id.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone())
    }

    pub(crate) fn remove_quota_account_lock(&self, account_id: &str) -> Result<bool> {
        Ok(self
            .quota_account_locks
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "quota account lock poisoned"))?
            .remove(account_id)
            .is_some())
    }

    pub(crate) async fn subscription_refresh_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.subscription_refresh_lock.lock().await
    }

    pub fn profile_backup_root(&self) -> PathBuf {
        self.recovery_root().join("profiles")
    }

    pub fn history_repair_backup_root(&self) -> PathBuf {
        self.recovery_root().join("history-repair")
    }

    pub fn ready_api_backup_root(&self) -> PathBuf {
        self.recovery_root().join("client-config")
    }

    pub fn data_root(&self) -> PathBuf {
        self.root.join("data")
    }

    pub fn recovery_root(&self) -> PathBuf {
        self.root.join("recovery")
    }

    pub fn transient_root(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn output_root(&self) -> PathBuf {
        self.cache_root()
    }

    pub fn cache_root(&self) -> PathBuf {
        self.root.join("cache")
    }
}

fn invalid_core_state(error: impl std::fmt::Display) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::InvalidState, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
    use crate::local_pool::models::{
        LocalAccountRecord, LocalGatewayKeyRecord, ProviderSourceRecord,
    };
    use crate::local_pool::usage_writer::apply_account_usage_state;
    use std::collections::{BTreeSet, HashMap};
    use zenith_relay_core::{
        accounts::{
            AccountAuthMode, AccountAuthState, AccountHealthState, AccountIdentity, AccountRecord,
        },
        automations::{
            AccountSelector, WakeAdapterPolicy, WakeCompletion, WakeDecision, WakeExecutionPolicy,
            WakeModel, WakeModelPolicy, WakeOutcome, WakeTask, WakeTrigger,
        },
        quota::{QuotaSnapshot, QuotaTransition, QuotaWindow, QuotaWindowKind, Subscription},
        UsageEvent, WireApi,
    };

    #[test]
    fn quota_refreshes_share_one_lock_per_account() {
        let root = temp_root("quota-locks");
        let state = DesktopState::open(root.clone()).unwrap();
        let first = state.quota_account_lock("account-1").unwrap();
        let same = state.quota_account_lock("account-1").unwrap();
        let other = state.quota_account_lock("account-2").unwrap();

        assert!(Arc::ptr_eq(&first, &same));
        assert!(!Arc::ptr_eq(&first, &other));
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn subscription_refreshes_share_one_global_lock() {
        let root = temp_root("subscription-lock");
        let state = DesktopState::open(root.clone()).unwrap();
        let first = state.subscription_refresh_guard().await;

        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(10),
            state.subscription_refresh_guard(),
        )
        .await
        .is_err());
        drop(first);
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(10),
            state.subscription_refresh_guard(),
        )
        .await
        .is_ok());

        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_callback_persists_before_returning() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-state-{}", uuid::Uuid::new_v4()));
        let state = DesktopState::open(root.clone()).unwrap();
        state
            .store()
            .unwrap()
            .upsert_source(ProviderSourceRecord {
                id: "source_1".into(),
                name: "Synthetic".into(),
                enabled: true,
                in_pool: true,
                draining: false,
                base_url: "https://example.test/v1".into(),
                secret_ref: "source:source_1".into(),
                wire_api: WireApi::Responses,
                protocol_bindings: Vec::new(),
                models: vec!["gpt-test".into()],
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                priority: 0,
                weight: 1,
                recovery_delay_seconds: 0,
                model_price_overrides: Default::default(),
                detected_model_prices: Default::default(),
                last_used_at: None,
                last_test_at: None,
                last_test_status: None,
                last_error: None,
            })
            .unwrap();
        state
            .store()
            .unwrap()
            .upsert_key(LocalGatewayKeyRecord {
                id: "key_1".into(),
                label: "Default".into(),
                enabled: true,
                system: false,
                secret_ref: "key:key_1".into(),
                created_at: "2026-07-10T00:00:00Z".into(),
                last_used_at: None,
            })
            .unwrap();

        (state.usage_callback())(UsageEvent {
            request_id: "req_callback".into(),
            attempt: 1,
            local_key_id: "key_1".into(),
            source_id: "source_1".into(),
            candidate_id: Some("source_1".into()),
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
            latency_ms: 7,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: Some(2),
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            reasoning_tokens: None,
            output_tokens: Some(3),
            total_tokens: Some(5),
            quota_snapshot: None,
        });

        let logs = state.telemetry.list(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].request_id, "req_callback");
        assert_eq!(logs[0].total_tokens, Some(5));
        let store = state.store().unwrap();
        assert!(store.source("source_1").unwrap().last_used_at.is_some());
        assert!(store.key("key_1").unwrap().last_used_at.is_some());
        drop(store);
        drop(state);
        let reopened = LocalPoolStore::open(root.clone()).unwrap();
        assert!(reopened.source("source_1").unwrap().last_used_at.is_some());
        assert!(reopened.key("key_1").unwrap().last_used_at.is_some());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unauthorized_access_only_account_is_saved_but_removed_from_routing() {
        let root = temp_root("usage-401");
        let account_id = format!("account-{}", uuid::Uuid::new_v4().simple());
        let state = DesktopState::open(root.clone()).unwrap();
        let mut account = account_record(&account_id);
        account.account.auth_state = AccountAuthState::DegradedAccessOnly;
        state.store().unwrap().upsert_account(account).unwrap();
        let credentials = CredentialStore::from_backend(NativeSecretBackend);
        credentials
            .save(
                &StoredCodexCredentials::new(
                    &account_id,
                    "access-private".into(),
                    None,
                    None,
                    Some(u64::MAX),
                    1,
                    1,
                    None,
                    Some("provider-private".into()),
                    None,
                    None,
                    None,
                    false,
                )
                .unwrap(),
            )
            .unwrap();
        assert!(credentials.require(&account_id).is_ok());
        let retry_at_ms = now_ms().saturating_add(30 * 60_000);

        (state.usage_callback())(account_status_event(
            &account_id,
            401,
            Some("*"),
            Some(retry_at_ms),
            1,
        ));

        let observed_after = now_ms();
        let stored = credentials.require(&account_id).unwrap();
        assert!(!stored.is_access_usable(observed_after, 0));
        let account = state.store().unwrap().account(&account_id).unwrap().clone();
        assert_eq!(account.account.auth_state, AccountAuthState::Error);
        assert_eq!(account.account.health, AccountHealthState::Unhealthy);
        assert_eq!(
            account.account.last_error_code.as_deref(),
            Some("upstream_unauthorized")
        );
        assert!(account.cooldowns.is_empty());
        assert_eq!(account.consecutive_failures, 0);
        assert!(state.next_quota_refresh_due().unwrap().is_none());
        drop(state);

        let reopened = LocalPoolStore::open(root.clone()).unwrap();
        let account = reopened.account(&account_id).unwrap();
        assert!(account.cooldowns.is_empty());
        assert_eq!(account.consecutive_failures, 0);
        drop(reopened);
        credentials.delete(&account_id).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn forbidden_account_is_blocked_until_an_actual_success() {
        let root = temp_root("usage-403");
        let account_id = "account-forbidden";
        let state = DesktopState::open(root.clone()).unwrap();
        state
            .store()
            .unwrap()
            .upsert_account(account_record(account_id))
            .unwrap();
        let retry_at_ms = now_ms().saturating_add(30 * 60_000);

        (state.usage_callback())(account_status_event(
            account_id,
            403,
            Some("*"),
            Some(retry_at_ms),
            2,
        ));
        {
            let store = state.store().unwrap();
            let account = store.account(account_id).unwrap();
            assert_eq!(account.account.health, AccountHealthState::Blocked);
            assert_eq!(
                account.account.last_error_code.as_deref(),
                Some("upstream_forbidden")
            );
            assert!(account.cooldowns.is_empty());
            assert_eq!(account.consecutive_failures, 0);
        }
        drop(state);

        let reopened = DesktopState::open(root.clone()).unwrap();
        assert_eq!(
            reopened
                .store()
                .unwrap()
                .account(account_id)
                .unwrap()
                .account
                .health,
            AccountHealthState::Blocked
        );
        (reopened.usage_callback())(account_success_event(account_id));
        let store = reopened.store().unwrap();
        let account = store.account(account_id).unwrap();
        assert_eq!(account.account.health, AccountHealthState::Healthy);
        assert_eq!(account.account.last_error_code, None);
        assert!(account.cooldowns.is_empty());
        assert_eq!(account.consecutive_failures, 0);
        drop(store);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn account_results_update_health_without_persisting_cooldowns() {
        let mut account = account_record("account-race");
        let failure = account_status_event("account-race", 429, Some("*"), Some(500), 2);
        assert!(apply_account_usage_state(
            &mut account,
            &failure,
            100,
            None,
            None,
        ));

        let mut late_success = account_success_event("account-race");
        late_success.consecutive_failures = None;
        assert!(!apply_account_usage_state(
            &mut account,
            &late_success,
            200,
            None,
            Some(AccountAuthState::Active),
        ));
        let older_failure = account_status_event("account-race", 429, Some("*"), Some(300), 1);
        assert!(apply_account_usage_state(
            &mut account,
            &older_failure,
            250,
            None,
            None,
        ));

        assert!(account.cooldowns.is_empty());
        assert_eq!(account.consecutive_failures, 0);
        assert_eq!(account.account.health, AccountHealthState::Degraded);
        assert_eq!(
            account.account.last_error_code.as_deref(),
            Some("upstream_rate_limited")
        );
    }

    #[test]
    fn neutral_request_failure_does_not_degrade_the_account() {
        let mut account = account_record("account-neutral");
        let mut event = account_status_event("account-neutral", 400, None, None, 0);
        event.consecutive_failures = None;
        event.error_category = Some("response_affinity_miss".into());

        assert!(!apply_account_usage_state(
            &mut account,
            &event,
            100,
            None,
            None,
        ));
        assert_eq!(account.account.health, AccountHealthState::Healthy);
        assert_eq!(account.account.last_error_code, None);
        assert!(account.cooldowns.is_empty());
        assert_eq!(account.consecutive_failures, 0);
    }

    #[test]
    fn quota_classification_queues_refresh_without_blocking_the_account() {
        let mut account = account_record("account-quota");
        let mut event = account_status_event("account-quota", 403, Some("*"), Some(60_000), 1);
        event.error_category = Some("upstream_quota_exhausted".into());

        assert!(apply_account_usage_state(
            &mut account,
            &event,
            100,
            None,
            None,
        ));
        assert_eq!(account.account.health, AccountHealthState::Degraded);
        assert_eq!(
            account.account.last_error_code.as_deref(),
            Some("upstream_quota_exhausted")
        );
    }

    #[test]
    fn model_entitlement_is_local_but_edge_challenges_degrade_the_account() {
        for (category, expected_health, expected_error) in [
            (
                "upstream_usage_not_included",
                AccountHealthState::Healthy,
                None,
            ),
            (
                "upstream_edge_challenge",
                AccountHealthState::Degraded,
                Some("upstream_edge_challenge"),
            ),
        ] {
            let mut account = account_record(category);
            let mut event = account_status_event(category, 403, Some("*"), Some(60_000), 1);
            event.error_category = Some(category.into());

            assert!(!apply_account_usage_state(
                &mut account,
                &event,
                100,
                None,
                None,
            ));
            assert_eq!(account.account.health, expected_health);
            assert_eq!(account.account.last_error_code.as_deref(), expected_error);
        }
    }

    #[test]
    fn startup_accounts_are_queued_due_now() {
        let root = temp_root("refresh-startup");
        {
            let mut store = LocalPoolStore::open(root.clone()).unwrap();
            let mut outside_pool = account_record("account-3");
            outside_pool.account.in_pool = false;
            store
                .replace_accounts_and_keys(
                    vec![
                        account_record("account-1"),
                        account_record("account-2"),
                        outside_pool,
                    ],
                    Vec::new(),
                )
                .unwrap();
        }
        let before_open_ms = now_ms();
        let state = DesktopState::open(root.clone()).unwrap();
        let next_due = state.next_quota_refresh_due().unwrap().unwrap();
        assert!((before_open_ms..=now_ms()).contains(&next_due));

        let mut permits = state.claim_due_quota_refreshes(next_due, 8).unwrap();
        permits.sort_by(|left, right| left.account_id.cmp(&right.account_id));
        assert_eq!(
            permits
                .iter()
                .map(|permit| permit.account_id.as_str())
                .collect::<Vec<_>>(),
            vec!["account-1", "account-2", "account-3"]
        );
        let first = permits.remove(0);
        assert!(state
            .reschedule_quota_refresh(first, next_due + 1_000)
            .unwrap());
        assert!(state.complete_quota_refresh(permits.remove(0)).unwrap());
        assert!(state.complete_quota_refresh(permits.remove(0)).unwrap());
        assert!(state
            .mark_quota_refresh("account-1", next_due + 10)
            .unwrap());
        assert_eq!(state.next_quota_refresh_due().unwrap(), Some(next_due + 10));
        assert!(state.remove_quota_refresh("account-1").unwrap());
        assert!(state.next_quota_refresh_due().unwrap().is_none());
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn account_usage_persists_natural_wake_completion_across_restart() {
        let root = temp_root("natural-use");
        let task = wake_task("task-1", WakeExecutionPolicy::Automatic);
        let state = DesktopState::open(root.clone()).unwrap();
        {
            let mut store = state.store().unwrap();
            let mut automations = store.automations().clone();
            automations.tasks = vec![task.clone()];
            store
                .replace_account_state(
                    vec![account_record("account-1")],
                    vec![key_record("key-1")],
                    automations,
                )
                .unwrap();
        }
        let account = state
            .store()
            .unwrap()
            .account("account-1")
            .unwrap()
            .account
            .clone();
        assert!(matches!(
            state
                .evaluate_wake_transition(&task, &account, &wake_transition(), &wake_policy(), 110,)
                .unwrap(),
            WakeDecision::Scheduled(_)
        ));
        let permit = state
            .claim_due_automatic_wakes(110, 1)
            .unwrap()
            .pop()
            .unwrap();
        assert!(state.is_wake_permit_active(&permit).unwrap());

        (state.usage_callback())(account_usage_event("req-failed", false));
        {
            let store = state.store().unwrap();
            assert!(store
                .account("account-1")
                .unwrap()
                .account
                .last_used_at_ms
                .is_none());
            assert!(wake_coordinator(store.automations())
                .unwrap()
                .pending()
                .is_empty());
        }
        assert!(state.is_wake_permit_active(&permit).unwrap());
        (state.usage_callback())(account_usage_event("req-natural-use", true));
        assert!(!state.is_wake_permit_active(&permit).unwrap());
        assert!(!state
            .complete_wake(
                permit,
                WakeCompletion {
                    outcome: zenith_relay_core::automations::WakeCompletionOutcome::Unconfirmed,
                    completed_at_ms: now_ms(),
                    latency_ms: Some(1),
                    input_tokens: Some(1),
                    output_tokens: Some(1),
                    error_code: None,
                },
            )
            .unwrap());
        drop(state);

        let reopened = DesktopState::open(root.clone()).unwrap();
        let store = reopened.store().unwrap();
        assert!(store
            .account("account-1")
            .unwrap()
            .account
            .last_used_at_ms
            .is_some());
        assert!(store.key("key-1").unwrap().last_used_at.is_some());
        let coordinator = wake_coordinator(store.automations()).unwrap();
        assert!(coordinator.pending().is_empty());
        let history = coordinator.state().history().back().unwrap();
        assert_eq!(history.outcome, WakeOutcome::SkippedAlreadyStarted);
        assert!(history.model_id.is_none());
        assert!(history.input_tokens.is_none());
        assert!(history.output_tokens.is_none());
        drop(store);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn automatic_wake_claim_does_not_claim_confirmation_cycle() {
        let root = temp_root("wake-confirmation");
        let automatic = wake_task("task-auto", WakeExecutionPolicy::Automatic);
        let confirmation = wake_task("task-confirm", WakeExecutionPolicy::RequireConfirmation);
        let state = DesktopState::open(root.clone()).unwrap();
        let account = account_record("account-1");
        {
            let mut store = state.store().unwrap();
            let mut automations = store.automations().clone();
            automations.tasks = vec![automatic.clone(), confirmation.clone()];
            store
                .replace_account_state(vec![account.clone()], Vec::new(), automations)
                .unwrap();
        }
        assert!(matches!(
            state
                .evaluate_wake_transition(
                    &automatic,
                    &account.account,
                    &wake_transition(),
                    &wake_policy(),
                    110,
                )
                .unwrap(),
            WakeDecision::Scheduled(_)
        ));
        assert!(matches!(
            state
                .evaluate_wake_transition(
                    &confirmation,
                    &account.account,
                    &wake_transition_with_fingerprint("cycle-2"),
                    &wake_policy(),
                    110,
                )
                .unwrap(),
            WakeDecision::Scheduled(_)
        ));

        let mut permits = state.claim_due_automatic_wakes(110, 8).unwrap();
        assert_eq!(permits.len(), 1);
        assert_eq!(permits[0].task_id, automatic.id);
        assert_eq!(state.next_automatic_wake_due().unwrap(), None);
        let mut confirmation_permits = state.claim_due_confirmation_wakes(110, 8).unwrap();
        assert_eq!(confirmation_permits.len(), 1);
        assert_eq!(confirmation_permits[0].task_id, confirmation.id);
        assert!(state
            .complete_wake(
                permits.remove(0),
                WakeCompletion {
                    outcome: zenith_relay_core::automations::WakeCompletionOutcome::Confirmed,
                    completed_at_ms: 120,
                    latency_ms: Some(10),
                    input_tokens: Some(1),
                    output_tokens: Some(1),
                    error_code: None,
                },
            )
            .unwrap());
        assert!(state
            .complete_wake(
                confirmation_permits.remove(0),
                WakeCompletion {
                    outcome: zenith_relay_core::automations::WakeCompletionOutcome::Confirmed,
                    completed_at_ms: 120,
                    latency_ms: Some(10),
                    input_tokens: Some(1),
                    output_tokens: Some(1),
                    error_code: None,
                },
            )
            .unwrap());
        assert_eq!(
            state
                .remove_pending_wakes_for_account("missing-account")
                .unwrap(),
            0
        );
        assert_eq!(
            state
                .remove_pending_wakes_for_task(&confirmation.id)
                .unwrap(),
            0
        );
        assert!(state.next_automatic_wake_due().unwrap().is_none());
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn setup_guard_serializes_mutations() {
        let root = std::env::temp_dir().join(format!("zenith-relay-lock-{}", uuid::Uuid::new_v4()));
        let state = Arc::new(DesktopState::open(root.clone()).unwrap());
        let first = state.setup_guard().await;
        let waiting_state = state.clone();
        let waiting = tokio::spawn(async move {
            let _guard = waiting_state.setup_guard().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!waiting.is_finished());
        drop(first);
        waiting.await.unwrap();
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    struct MemorySecrets(HashMap<String, String>);

    impl SecretLookup for MemorySecrets {
        fn load(&self, secret_ref: &str) -> Result<Option<String>> {
            Ok(self.0.get(secret_ref).cloned())
        }
    }

    #[test]
    fn canonical_account_credential_controls_availability() {
        let mut account = account_record("account_credential");
        account.account.secret_refs.clear();

        assert!(!account_secret_available(&account, &MemorySecrets(HashMap::new())).unwrap());
        assert!(account_secret_available(
            &account,
            &MemorySecrets(HashMap::from([(
                "account:codex:account_credential".into(),
                "credential".into(),
            )]))
        )
        .unwrap());
    }

    fn key_record(id: &str) -> LocalGatewayKeyRecord {
        LocalGatewayKeyRecord {
            id: id.into(),
            label: id.into(),
            enabled: true,
            system: false,
            secret_ref: format!("key:{id}"),
            created_at: "2026-07-10T00:00:00Z".into(),
            last_used_at: None,
        }
    }

    fn temp_root(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("zenith-relay-{prefix}-{}", uuid::Uuid::new_v4()))
    }

    fn account_record(id: &str) -> LocalAccountRecord {
        LocalAccountRecord {
            account: AccountRecord {
                id: id.into(),
                label: id.into(),
                identity: AccountIdentity::from_hashed_parts(
                    "openai",
                    "chatgpt.com/backend-api/codex",
                    &format!("identity-{id}"),
                    &format!("secret-{id}"),
                    "default",
                    None,
                )
                .unwrap(),
                auth_mode: AccountAuthMode::OAuth,
                auth_state: AccountAuthState::Active,
                health: AccountHealthState::Healthy,
                source_id: "openai_codex".into(),
                secret_refs: vec![format!("account:{id}")],
                subscription: Subscription::default(),
                quota: QuotaSnapshot {
                    primary: Some(QuotaWindow {
                        kind: QuotaWindowKind::Primary,
                        available_basis_points: Some(10_000),
                        explicitly_full: Some(true),
                        reset_at_ms: Some(10_000),
                        window_minutes: Some(300),
                        observed_at_ms: 100,
                        full_transition_fingerprint: Some("cycle-1".into()),
                    }),
                    ..QuotaSnapshot::default()
                },
                token_generation: 1,
                token_updated_at_ms: Some(1),
                tags: BTreeSet::new(),
                enabled: true,
                in_pool: true,
                draining: false,
                created_at_ms: 1,
                last_used_at_ms: None,
                last_error_code: None,
            },
            economics: Default::default(),
            remote_location: None,
            wire_api: WireApi::Responses,
            models: vec!["gpt-test".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            cooldowns: Default::default(),
            consecutive_failures: 0,
        }
    }

    fn wake_task(id: &str, execution_policy: WakeExecutionPolicy) -> WakeTask {
        WakeTask {
            id: id.into(),
            name: id.into(),
            enabled: true,
            account_selector: AccountSelector::AllEligible,
            window_kinds: [QuotaWindowKind::Primary].into(),
            model_policy: WakeModelPolicy::LightestSupported,
            trigger: WakeTrigger::QuotaFull,
            fallback_schedule: None,
            execution_policy,
            jitter_seconds: 0,
            max_attempts_per_cycle: 1,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    fn wake_transition() -> QuotaTransition {
        wake_transition_with_fingerprint("cycle-1")
    }

    fn wake_transition_with_fingerprint(fingerprint: &str) -> QuotaTransition {
        QuotaTransition {
            window_kind: QuotaWindowKind::Primary,
            fingerprint: fingerprint.into(),
            transitioned_at_ms: 100,
        }
    }

    fn wake_policy() -> WakeAdapterPolicy {
        WakeAdapterPolicy {
            windows_requiring_activity: [QuotaWindowKind::Primary].into(),
            models: vec![WakeModel {
                id: "gpt-test".into(),
                lightness_rank: 1,
                wake_capable: true,
            }],
            verification_delay_ms: 1_000,
            output_token_cap: 8,
        }
    }

    fn account_usage_event(request_id: &str, success: bool) -> UsageEvent {
        UsageEvent {
            request_id: request_id.into(),
            attempt: 1,
            local_key_id: "key-1".into(),
            source_id: "openai_codex".into(),
            candidate_id: Some("account-1".into()),
            account_id: Some("account-1".into()),
            routing: None,
            requested_model: Some("gpt-test".into()),
            resolved_model: Some("gpt-test".into()),
            requested_reasoning_effort: None,
            effective_reasoning_effort: None,
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success,
            http_status: if success { 200 } else { 500 },
            error_category: (!success).then(|| "upstream".into()),
            tool_use: zenith_relay_core::ToolUseDiagnostics::default(),
            cooldown_scope: (!success).then(|| "*".into()),
            retry_at_ms: (!success).then_some(60_000),
            consecutive_failures: Some(u32::from(!success)),
            latency_ms: 7,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: success.then_some(2),
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            reasoning_tokens: None,
            output_tokens: success.then_some(3),
            total_tokens: success.then_some(5),
            quota_snapshot: None,
        }
    }

    fn account_status_event(
        account_id: &str,
        http_status: u16,
        cooldown_scope: Option<&str>,
        retry_at_ms: Option<u64>,
        consecutive_failures: u32,
    ) -> UsageEvent {
        UsageEvent {
            request_id: format!("req-{account_id}-{http_status}"),
            attempt: 1,
            local_key_id: "key-1".into(),
            source_id: "openai_codex".into(),
            candidate_id: Some(account_id.into()),
            account_id: Some(account_id.into()),
            routing: None,
            requested_model: Some("gpt-test".into()),
            resolved_model: Some("gpt-test".into()),
            requested_reasoning_effort: None,
            effective_reasoning_effort: None,
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: false,
            http_status,
            error_category: Some("upstream_status".into()),
            tool_use: zenith_relay_core::ToolUseDiagnostics::default(),
            cooldown_scope: cooldown_scope.map(str::to_string),
            retry_at_ms,
            consecutive_failures: Some(consecutive_failures),
            latency_ms: 7,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            reasoning_tokens: None,
            output_tokens: None,
            total_tokens: None,
            quota_snapshot: None,
        }
    }

    fn account_success_event(account_id: &str) -> UsageEvent {
        let mut event = account_status_event(account_id, 200, None, None, 0);
        event.success = true;
        event.error_category = None;
        event
    }
}
