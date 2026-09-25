use crate::state::{now_ms, AccountCredential, AppState};
use std::collections::BTreeMap;
use std::sync::{atomic::Ordering, mpsc, Arc};
use zenith_relay_core::accounts::{
    reduce_account_usage, AccountAccessState, AccountAuthState, AccountUsageObservation,
    AccountUsageState,
};
use zenith_relay_core::{
    accounts::automatic_quota_monitoring_eligible,
    providers::chatgpt::subscription_refresh_due,
    scheduler::refresh::{passive_quota_age_ms, quota_reset_delay, RefreshKind},
};
use zenith_relay_core::{UsageCallback, UsageEvent};

const USAGE_QUEUE_CAPACITY: usize = 16_384;
const USAGE_BATCH_SIZE: usize = 256;

struct QueuedUsage {
    event: UsageEvent,
    observed_at_ms: u64,
}

struct AccountUsageHints {
    natural_use_at_ms: Option<u64>,
    refresh_now: bool,
    passive_observation: Option<(u64, u64)>,
    reset_delay_ms: Option<u64>,
    retry_delay_ms: Option<u64>,
}

enum UsageWriterMessage {
    Event(Box<QueuedUsage>),
    Shutdown,
}

pub(crate) struct UsageWriter {
    callback: UsageCallback,
    sender: mpsc::SyncSender<UsageWriterMessage>,
    thread: std::thread::JoinHandle<()>,
}

impl UsageWriter {
    pub(crate) fn start(state: &Arc<AppState>) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel(USAGE_QUEUE_CAPACITY);
        let weak_state = Arc::downgrade(state);
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| "usage writer requires an async runtime".to_string())?;
        let thread = std::thread::Builder::new()
            .name("relay-usage-writer".to_string())
            .spawn(move || {
                while let Ok(message) = receiver.recv() {
                    let mut batch = Vec::with_capacity(USAGE_BATCH_SIZE);
                    let mut stopping = false;
                    match message {
                        UsageWriterMessage::Event(event) => batch.push(*event),
                        UsageWriterMessage::Shutdown => stopping = true,
                    }
                    for message in receiver.try_iter().take(USAGE_BATCH_SIZE - 1) {
                        match message {
                            UsageWriterMessage::Event(event) => batch.push(*event),
                            UsageWriterMessage::Shutdown => {
                                stopping = true;
                                break;
                            }
                        }
                    }
                    if !batch.is_empty() {
                        let Some(state) = weak_state.upgrade() else {
                            break;
                        };
                        persist_usage_batch(&state, &batch, &runtime);
                    }
                    if stopping {
                        break;
                    }
                }
            })
            .map_err(|error| format!("failed to start usage writer: {error}"))?;

        let callback_sender = sender.clone();
        let weak_state = Arc::downgrade(state);
        let callback = Arc::new(move |event| {
            if callback_sender
                .try_send(UsageWriterMessage::Event(Box::new(QueuedUsage {
                    event,
                    observed_at_ms: now_ms(),
                })))
                .is_err()
            {
                if let Some(state) = weak_state.upgrade() {
                    state.failed_usage_writes.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
        Ok(Self {
            callback,
            sender,
            thread,
        })
    }

    pub(crate) fn callback(&self) -> UsageCallback {
        self.callback.clone()
    }

    pub(crate) async fn shutdown(self) -> Result<(), String> {
        let Self {
            callback,
            sender,
            thread,
        } = self;
        drop(callback);
        tokio::task::spawn_blocking(move || {
            sender
                .send(UsageWriterMessage::Shutdown)
                .map_err(|_| "usage writer stopped before flush".to_string())?;
            thread
                .join()
                .map_err(|_| "usage writer panicked during shutdown".to_string())
        })
        .await
        .map_err(|_| "usage writer shutdown task failed".to_string())?
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
    let mut natural_uses = Vec::new();
    for (account_id, events) in account_events {
        let (hints, identity) =
            match state
                .store
                .update_account_with_refresh_identity(&account_id, |account| {
                    let credential = state
                        .vault
                        .load(&account.secret_ref)
                        .ok()
                        .flatten()
                        .and_then(|value| serde_json::from_str::<AccountCredential>(&value).ok());
                    let access_state =
                        credential
                            .as_ref()
                            .map_or(AccountAccessState::Failed, |value| {
                                if value.refresh_token.is_some() {
                                    AccountAccessState::Refreshable
                                } else {
                                    AccountAccessState::AccessOnly
                                }
                            });
                    let successful_auth_state = credential.as_ref().map(|value| {
                        if value.refresh_token.is_some() {
                            AccountAuthState::Active
                        } else {
                            AccountAuthState::DegradedAccessOnly
                        }
                    });
                    let mut natural_use_at_ms = None;
                    let mut refresh_now = false;
                    let mut new_passive = false;
                    let mut retry_at_ms = None;
                    for queued in &events {
                        let event = &queued.event;
                        let previous_quota = account.quota.clone();
                        if let Some(snapshot) = event.quota_snapshot.as_ref().filter(|snapshot| {
                            snapshot.updated_at_ms.unwrap_or_default()
                                >= account.quota.updated_at_ms.unwrap_or_default()
                        }) {
                            account.quota = snapshot.clone();
                        }
                        new_passive |= event.success
                            && event.quota_snapshot.as_ref().is_some_and(|snapshot| {
                                snapshot != &previous_quota
                                    && snapshot.updated_at_ms.is_some()
                                    && snapshot.updated_at_ms >= previous_quota.updated_at_ms
                                    && snapshot == &account.quota
                            });
                        let update = reduce_account_usage(
                            AccountUsageState {
                                auth_state: account.auth_state,
                                health: account.health,
                                last_error_code: account.last_error_code.clone(),
                                last_used_at_ms: account.last_used_at_ms,
                            },
                            AccountUsageObservation {
                                success: event.success,
                                http_status: event.http_status,
                                error_category: event.error_category.as_deref(),
                                affects_account: event.affects_account_state(),
                            },
                            queued.observed_at_ms,
                            (event.http_status == 401).then_some(access_state),
                            if event.success {
                                successful_auth_state
                            } else {
                                None
                            },
                        );
                        account.auth_state = update.state.auth_state;
                        account.health = update.state.health;
                        account.last_error_code = update.state.last_error_code;
                        account.last_used_at_ms = update.state.last_used_at_ms;
                        refresh_now |= update.refresh_quota;
                        if update.refresh_quota && event.http_status == 429 {
                            retry_at_ms = retry_at_ms.max(event.retry_at_ms);
                        }
                        if update.reset_runtime_failures {
                            account.cooldowns.clear();
                            account.consecutive_failures = 0;
                        }
                        if event.success {
                            natural_use_at_ms = Some(queued.observed_at_ms);
                        }
                    }
                    let now = now_ms();
                    let eligible =
                        automatic_quota_monitoring_eligible(account.enabled, account.auth_state);
                    Ok(AccountUsageHints {
                        natural_use_at_ms,
                        refresh_now: eligible && refresh_now,
                        passive_observation: (eligible
                            && new_passive
                            && !subscription_refresh_due(
                                account.subscription.active_until_ms,
                                account.subscription.updated_at_ms,
                                now,
                            ))
                        .then(|| {
                            passive_quota_age_ms(&account.quota, now)
                                .zip(account.quota.updated_at_ms)
                        })
                        .flatten(),
                        reset_delay_ms: (eligible && new_passive)
                            .then(|| quota_reset_delay(&account_id, &account.quota, now))
                            .flatten(),
                        retry_delay_ms: eligible
                            .then_some(retry_at_ms)
                            .flatten()
                            .map(|at| at.saturating_sub(now))
                            .filter(|delay| *delay > 0),
                    })
                }) {
                Ok(Some(value)) => value,
                Ok(None) => continue,
                Err(_) => {
                    state.failed_usage_writes.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };
        state.refresh.set_member_active(&identity.member_id);
        if let Some((age_ms, observed_at_ms)) = hints.passive_observation {
            state
                .refresh
                .observe_passive_quota(&identity, age_ms, observed_at_ms);
        }
        if let Some(delay_ms) = hints.reset_delay_ms {
            state
                .refresh
                .schedule_after(&identity, RefreshKind::Quota, delay_ms);
        }
        if let Some(delay_ms) = hints.retry_delay_ms {
            state
                .refresh
                .respect_retry_after(&identity, RefreshKind::Quota, delay_ms);
        }
        if hints.refresh_now {
            state.refresh.mark_dirty(&identity, RefreshKind::Quota);
        }
        if let Some(observed_at_ms) = hints.natural_use_at_ms {
            natural_uses.push((account_id, observed_at_ms));
        }
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
    runtime.block_on(async move {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        state::{AccountCredential, ServerAccountRecord},
        store::{Store, Vault},
    };
    use std::sync::atomic::Ordering;
    use tempfile::TempDir;
    use zenith_relay_core::{
        accounts::{AccountAuthState, AccountHealthState},
        quota::{QuotaSnapshot, QuotaWindow, QuotaWindowKind, Subscription},
        scheduler::refresh::{
            service::RefreshRegistration, service::RefreshResult, RefreshFreshness, RefreshOutcome,
        },
        DefaultServiceTier, ToolUseDiagnostics, UsageEvent, WireApi,
    };

    fn test_account(id: &str) -> ServerAccountRecord {
        ServerAccountRecord {
            id: id.to_string(),
            label: id.to_string(),
            identity_hint: id.to_string(),
            enabled: true,
            in_pool: true,
            draining: false,
            source_id: "openai_codex".to_string(),
            secret_ref: format!("account:{id}"),
            provider_family: Some("openai".to_string()),
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            models: vec!["gpt-test".to_string()],
            discovered_models: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            subscription: Subscription::default(),
            quota: Default::default(),
            purchase_cost_micro_usd: None,
            cooldowns: Default::default(),
            consecutive_failures: 0,
            created_at_ms: 1,
            last_used_at_ms: None,
            last_error_code: None,
            proxy_id: None,
            bypass_common_proxy: false,
        }
    }

    fn usage_event(request_id: &str, account_id: &str) -> UsageEvent {
        UsageEvent {
            request_id: request_id.to_string(),
            attempt: 1,
            local_key_id: "key_test".to_string(),
            source_id: "openai_codex".to_string(),
            candidate_id: Some(account_id.to_string()),
            account_id: Some(account_id.to_string()),
            account_token_generation: None,
            client_context_id: None,
            routing: None,
            requested_model: Some("gpt-test".to_string()),
            resolved_model: Some("gpt-test".to_string()),
            requested_reasoning_effort: None,
            effective_reasoning_effort: None,
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: true,
            http_status: 200,
            error_category: None,
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 1,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: Some(1),
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            cache_write_ttl: None,
            reasoning_tokens: None,
            output_tokens: Some(1),
            total_tokens: Some(2),
            upstream_error: None,
            quota_snapshot: None,
        }
    }

    #[test]
    fn missing_account_does_not_block_other_usage_updates_or_count_as_a_write_failure() {
        let root = TempDir::new().unwrap();
        let config = Config::for_test(root.path().to_path_buf(), "127.0.0.1:0".parse().unwrap());
        let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
        let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
        let state = AppState::new(config, store.clone(), vault.clone()).unwrap();
        for id in ["account_a", "account_b"] {
            let account = test_account(id);
            store.save_account(&account).unwrap();
            vault
                .save(
                    &account.secret_ref,
                    &serde_json::to_string(&AccountCredential {
                        access_token: "test-token".to_string(),
                        refresh_token: None,
                        id_token: None,
                        expires_at_ms: None,
                        issued_at_ms: 0,
                        generation: 0,
                        chatgpt_account_id: id.to_string(),
                        responses_url: "https://example.test/v1/responses".to_string(),
                        proxy_url: None,
                        agent_private_key: None,
                        agent_runtime_id: None,
                        agent_task_id: None,
                    })
                    .unwrap(),
                )
                .unwrap();
        }
        let batch = [
            QueuedUsage {
                event: usage_event("request_a", "account_a"),
                observed_at_ms: 10,
            },
            QueuedUsage {
                event: usage_event("request_missing", "account_missing"),
                observed_at_ms: 20,
            },
            QueuedUsage {
                event: usage_event("request_b", "account_b"),
                observed_at_ms: 30,
            },
        ];
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        persist_usage_batch(&state, &batch, runtime.handle());

        assert_eq!(
            store.account("account_a").unwrap().unwrap().last_used_at_ms,
            Some(10)
        );
        assert_eq!(
            store.account("account_b").unwrap().unwrap().last_used_at_ms,
            Some(30)
        );
        assert_eq!(state.failed_usage_writes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn persisted_passive_quota_is_fresh_for_the_registered_account_only() {
        let root = TempDir::new().unwrap();
        let config = Config::for_test(root.path().to_path_buf(), "127.0.0.1:0".parse().unwrap());
        let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
        let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
        let state = AppState::new(config, store.clone(), vault).unwrap();
        let now = now_ms();
        let mut account = test_account("account_a");
        account.subscription.active_until_ms = Some(now + 3_600_000);
        account.subscription.updated_at_ms = Some(now);
        store.save_account(&account).unwrap();
        let (_, fence) = store.account_refresh_scope(&account.id).unwrap();
        let identity = fence.identity();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let enter = runtime.enter();
        state
            .refresh
            .register(
                RefreshRegistration {
                    identity: identity.clone(),
                    kind: RefreshKind::Quota,
                    origin: "https://provider.example.test".into(),
                    active: true,
                    automatic: true,
                    due_now: false,
                },
                |_| {
                    Box::pin(async {
                        RefreshResult {
                            value: Err("synthetic read".into()),
                            outcome: RefreshOutcome::Success,
                        }
                    })
                },
            )
            .unwrap();
        drop(enter);
        let mut event = usage_event("request_passive", &account.id);
        event.quota_snapshot = Some(QuotaSnapshot {
            primary: Some(QuotaWindow {
                kind: QuotaWindowKind::Primary,
                provider_cycle_id: None,
                window_start_ms: None,
                available_basis_points: Some(2_000),
                explicitly_full: None,
                reset_at_ms: None,
                window_minutes: None,
                observed_at_ms: now,
                full_transition_fingerprint: None,
                exhaustion_transition_fingerprint: None,
            }),
            updated_at_ms: Some(now),
            ..QuotaSnapshot::default()
        });
        let batch = [QueuedUsage {
            event,
            observed_at_ms: now,
        }];
        persist_usage_batch(&state, &batch, runtime.handle());
        assert_eq!(
            store.account(&account.id).unwrap().unwrap().quota,
            batch[0].event.quota_snapshot.clone().unwrap()
        );
        assert!(matches!(
            state.refresh.freshness(&identity, RefreshKind::Quota),
            RefreshFreshness::Fresh { .. }
        ));
        assert_eq!(
            state.refresh.freshness(&identity, RefreshKind::Models),
            RefreshFreshness::Unknown
        );
        runtime.block_on(state.refresh.shutdown());
    }
}
