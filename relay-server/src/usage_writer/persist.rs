use super::*;
use crate::state::ServerAccountRecord;

pub(super) fn persist_usage_batch(
    state: &Arc<AppState>,
    batch: &[QueuedUsage],
    runtime: &tokio::runtime::Handle,
) {
    let usage_records = batch
        .iter()
        .map(|queued| (&queued.event, queued.observed_at_ms))
        .collect::<Vec<_>>();
    if state.store.record_usage_batch(&usage_records).is_err() {
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
        let (hints, identity) = match state
            .store
            .update_account_with_refresh_identity(&account_id, |account| {
                apply_queued_account_usage(state, &account_id, account, &events)
            }) {
            Ok(Some(account_usage_hints)) => account_usage_hints,
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

fn apply_queued_account_usage(
    state: &AppState,
    account_id: &str,
    account: &mut ServerAccountRecord,
    events: &[&QueuedUsage],
) -> Result<AccountUsageHints, String> {
    let credential = state
        .vault
        .load(&account.secret_ref)
        .ok()
        .flatten()
        .and_then(|credential_json| {
            serde_json::from_str::<AccountCredential>(&credential_json).ok()
        });
    let access_state =
        credential
            .as_ref()
            .map_or(AccountAccessState::Failed, |account_credential| {
                if account_credential.refresh_token.is_some() {
                    AccountAccessState::Refreshable
                } else {
                    AccountAccessState::AccessOnly
                }
            });
    let successful_auth_state = credential.as_ref().map(|account_credential| {
        if account_credential.refresh_token.is_some() {
            AccountAuthState::Active
        } else {
            AccountAuthState::DegradedAccessOnly
        }
    });
    let mut natural_use_at_ms = None;
    let mut refresh_now = false;
    let mut new_passive = false;
    let mut retry_at_ms = None;
    for queued in events {
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
    let eligible = automatic_quota_monitoring_eligible(account.enabled, account.auth_state);
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
        .then(|| passive_quota_age_ms(&account.quota, now).zip(account.quota.updated_at_ms))
        .flatten(),
        reset_delay_ms: (eligible && new_passive)
            .then(|| quota_reset_delay(account_id, &account.quota, now))
            .flatten(),
        retry_delay_ms: eligible
            .then_some(retry_at_ms)
            .flatten()
            .map(|at| at.saturating_sub(now))
            .filter(|delay| *delay > 0),
    })
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
