use super::{
    accounts::{
        authority::{ProcessAccountLocks, ProcessLockConfig},
        credentials::CredentialStore,
        import_session::SecretBackend,
        NativeSecretBackend,
    },
    models::LocalAccountRecord,
    state::DesktopOAuthEvents,
    store::{telemetry_db::TelemetryDb, LocalPoolStore},
};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use tokio::sync::Notify;
use zenith_relay_core::{
    accounts::{
        reduce_account_usage, AccountAccessState, AccountAuthState, AccountUsageObservation,
        AccountUsageState,
    },
    automations::WakeCoordinator,
    providers::chatgpt::subscription_refresh_due,
    scheduler::refresh::{
        passive_quota_age_ms, quota_reset_delay, service::RefreshService, RefreshIdentity,
        RefreshKind,
    },
    UsageCallback, UsageEvent,
};

pub(crate) struct DesktopUsageWriter {
    telemetry: Arc<TelemetryDb>,
    store: Arc<Mutex<LocalPoolStore>>,
    credentials: CredentialStore<NativeSecretBackend>,
    account_locks: ProcessAccountLocks,
    refresh: Arc<RefreshService<super::refresh::RefreshReadResult>>,
    wake: Arc<Mutex<WakeCoordinator>>,
    failed: Arc<AtomicU64>,
    wake_notify: Arc<Notify>,
    state_events: DesktopOAuthEvents,
}

pub(crate) struct DesktopUsageWriterParts {
    pub(crate) telemetry: Arc<TelemetryDb>,
    pub(crate) store: Arc<Mutex<LocalPoolStore>>,
    pub(crate) transient_root: PathBuf,
    pub(crate) refresh: Arc<RefreshService<super::refresh::RefreshReadResult>>,
    pub(crate) wake: Arc<Mutex<WakeCoordinator>>,
    pub(crate) failed: Arc<AtomicU64>,
    pub(crate) wake_notify: Arc<Notify>,
    pub(crate) state_events: DesktopOAuthEvents,
}

struct AccountRefreshHint {
    identity: RefreshIdentity,
    refresh_now: bool,
    passive_observation: Option<(u64, u64)>,
    reset_delay_ms: Option<u64>,
    retry_delay_ms: Option<u64>,
}

impl DesktopUsageWriter {
    pub(crate) fn new(parts: DesktopUsageWriterParts) -> Self {
        Self {
            telemetry: parts.telemetry,
            store: parts.store,
            credentials: CredentialStore::from_backend(NativeSecretBackend),
            // The default configuration is a fixed valid value. Constructing
            // the lock itself performs no filesystem I/O; acquisition remains
            // best-effort inside the synchronous usage callback.
            account_locks: ProcessAccountLocks::with_config(
                parts.transient_root,
                ProcessLockConfig::default(),
            )
            .expect("default account lock configuration is valid"),
            refresh: parts.refresh,
            wake: parts.wake,
            failed: parts.failed,
            wake_notify: parts.wake_notify,
            state_events: parts.state_events,
        }
    }

    pub(crate) fn callback(&self) -> UsageCallback {
        let telemetry = self.telemetry.clone();
        let store = self.store.clone();
        let credentials = self.credentials.clone();
        let account_locks = self.account_locks.clone();
        let account_refresh = self.refresh.clone();
        let wake = self.wake.clone();
        let failed = self.failed.clone();
        let wake_notify = self.wake_notify.clone();
        let state_events = self.state_events.clone();
        Arc::new(move |event| {
            // Keep the local write synchronous so the usage callback's state contract remains durable.
            let observed_at = chrono::Utc::now();
            let observed_at_ms = u64::try_from(observed_at.timestamp_millis()).unwrap_or_default();
            let observed_at = observed_at.to_rfc3339();
            let account_id = event.account_id.clone();
            // A 401 may arrive after an OAuth refresh or a manual sign-in has
            // replaced the credentials used by the original request. Acquire
            // the same per-account lock as credential writers, then compare
            // the request's provenance before changing either the secret or
            // the visible account state. Unknown/locked provenance is safer
            // as a telemetry-only observation than invalidating a fresh login.
            let (access_expiry, ignore_account_observation, _account_lock) =
                if event.http_status == 401 {
                    match (account_id.as_deref(), event.account_token_generation) {
                        (Some(account_id), Some(expected_generation)) => {
                            match account_locks.try_acquire(account_id) {
                                Ok(Some(lock)) => match expire_account_access(
                                    &credentials,
                                    account_id,
                                    expected_generation,
                                    observed_at_ms,
                                ) {
                                    Some(access_state) => (Some(access_state), false, Some(lock)),
                                    None => (None, true, Some(lock)),
                                },
                                Ok(None) | Err(_) => (None, true, None),
                            }
                        }
                        _ => (None, true, None),
                    }
                } else {
                    (None, false, None)
                };
            let successful_auth_state = if event.success {
                account_id
                    .as_deref()
                    .and_then(|account_id| persisted_auth_state(&credentials, account_id))
            } else {
                None
            };
            let recorded = telemetry.record(&event).is_ok();
            let update = store.lock().map_err(|_| ()).and_then(|mut store| {
                let Some(account_id) = account_id.as_deref() else {
                    store
                        .touch_usage(&event.local_key_id, &event.source_id, None, observed_at)
                        .map_err(|_| ())?;
                    return Ok((0, None, false));
                };

                let mut accounts = store.accounts().to_vec();
                let account = accounts
                    .iter_mut()
                    .find(|account| account.account.id == account_id)
                    .ok_or(())?;
                let visible_state = (
                    account.account.quota.clone(),
                    account.account.auth_state,
                    account.account.health,
                    account.account.last_error_code.clone(),
                );
                let refresh_now = apply_account_usage_state(
                    account,
                    &event,
                    observed_at_ms,
                    access_expiry,
                    successful_auth_state,
                    ignore_account_observation,
                ) && account.account.is_automatic_quota_monitoring_eligible()
                    && account.remote_location.is_none();
                let new_passive = event.success
                    && !ignore_account_observation
                    && event.quota_snapshot.as_ref().is_some_and(|snapshot| {
                        snapshot != &visible_state.0
                            && snapshot.updated_at_ms.is_some()
                            && snapshot.updated_at_ms >= visible_state.0.updated_at_ms
                            && snapshot == &account.account.quota
                    });
                let passive_observation = new_passive
                    .then(|| {
                        (!subscription_refresh_due(
                            account.account.subscription.active_until_ms,
                            account.account.subscription.updated_at_ms,
                            observed_at_ms,
                        ))
                        .then(|| {
                            passive_quota_age_ms(&account.account.quota, observed_at_ms)
                                .zip(account.account.quota.updated_at_ms)
                        })
                        .flatten()
                    })
                    .flatten();
                let reset_delay_ms = new_passive
                    .then(|| quota_reset_delay(account_id, &account.account.quota, observed_at_ms))
                    .flatten();
                let visible_state_changed = visible_state
                    != (
                        account.account.quota.clone(),
                        account.account.auth_state,
                        account.account.health,
                        account.account.last_error_code.clone(),
                    );
                let mut keys = store.keys().to_vec();
                if let Some(key) = keys.iter_mut().find(|key| key.id == event.local_key_id) {
                    key.last_used_at = Some(observed_at);
                }
                let mut coordinator = wake.lock().map_err(|_| ())?;
                let mut next = coordinator.clone();
                let mut automations = store.automations().clone();
                let natural_use = if event.success {
                    next.mark_natural_use_for_account(account_id, observed_at_ms)
                } else {
                    0
                };
                automations.state = next.state().clone();
                store
                    .replace_account_state(accounts, keys, automations)
                    .map_err(|_| ())?;
                *coordinator = next;
                let (_, fence) = store.account_refresh_scope(account_id).map_err(|_| ())?;
                Ok((
                    natural_use,
                    Some(AccountRefreshHint {
                        identity: fence.identity(),
                        refresh_now,
                        passive_observation,
                        reset_delay_ms,
                        retry_delay_ms: (refresh_now && event.http_status == 429)
                            .then_some(event.retry_at_ms)
                            .flatten()
                            .map(|at| at.saturating_sub(observed_at_ms))
                            .filter(|delay| *delay > 0),
                    }),
                    visible_state_changed,
                ))
            });
            if update
                .as_ref()
                .is_ok_and(|(completed, _, _)| *completed > 0)
            {
                wake_notify.notify_one();
            }
            if recorded {
                state_events.emit_usage_recorded();
            }
            if update
                .as_ref()
                .is_ok_and(|(_, _, visible_state_changed)| *visible_state_changed)
            {
                state_events.emit_state_changed();
            }
            if let Some(hint) = update.as_ref().ok().and_then(|(_, hint, _)| hint.as_ref()) {
                account_refresh.set_member_active(&hint.identity.member_id);
                if let Some((age_ms, observed_at_ms)) = hint.passive_observation {
                    account_refresh.observe_passive_quota(&hint.identity, age_ms, observed_at_ms);
                }
                if let Some(delay_ms) = hint.reset_delay_ms {
                    account_refresh.schedule_after(&hint.identity, RefreshKind::Quota, delay_ms);
                }
                if let Some(delay_ms) = hint.retry_delay_ms {
                    account_refresh.respect_retry_after(
                        &hint.identity,
                        RefreshKind::Quota,
                        delay_ms,
                    );
                }
                if hint.refresh_now {
                    account_refresh.mark_dirty(&hint.identity, RefreshKind::Quota);
                }
            } else if let Some(account_id) = account_id.as_deref() {
                account_refresh.set_member_active(&format!("account:{account_id}"));
            }
            if !recorded || update.is_err() {
                failed.fetch_add(1, Ordering::Relaxed);
            }
        })
    }
}

pub(super) fn expire_account_access<B: SecretBackend>(
    credentials: &CredentialStore<B>,
    account_id: &str,
    expected_generation: u64,
    now_ms: u64,
) -> Option<AccountAccessState> {
    let Ok(Some(mut stored)) = credentials.load(account_id) else {
        return Some(AccountAccessState::Failed);
    };
    if stored.generation() != expected_generation {
        return None;
    };
    let refreshable = stored.refresh_token().is_some();
    stored.expire_access_at(now_ms);
    if credentials.save(&stored).is_err() {
        Some(AccountAccessState::Failed)
    } else if refreshable {
        Some(AccountAccessState::Refreshable)
    } else {
        Some(AccountAccessState::AccessOnly)
    }
}

pub(super) fn persisted_auth_state(
    credentials: &CredentialStore<NativeSecretBackend>,
    account_id: &str,
) -> Option<AccountAuthState> {
    credentials.load(account_id).ok().flatten().map(|stored| {
        if stored.refresh_token().is_some() {
            AccountAuthState::Active
        } else {
            AccountAuthState::DegradedAccessOnly
        }
    })
}

pub(super) fn apply_account_usage_state(
    account: &mut LocalAccountRecord,
    event: &UsageEvent,
    observed_at_ms: u64,
    access_state: Option<AccountAccessState>,
    successful_auth_state: Option<AccountAuthState>,
    ignore_account_observation: bool,
) -> bool {
    if ignore_account_observation {
        return false;
    }
    if let Some(snapshot) = event.quota_snapshot.as_ref().filter(|snapshot| {
        snapshot.updated_at_ms.unwrap_or_default()
            >= account.account.quota.updated_at_ms.unwrap_or_default()
    }) {
        account.account.quota = snapshot.clone();
    }
    let update = reduce_account_usage(
        AccountUsageState {
            auth_state: account.account.auth_state,
            health: account.account.health,
            last_error_code: account.account.last_error_code.clone(),
            last_used_at_ms: account.account.last_used_at_ms,
        },
        AccountUsageObservation {
            success: event.success,
            http_status: event.http_status,
            error_category: event.error_category.as_deref(),
            affects_account: event.affects_account_state(),
        },
        observed_at_ms,
        access_state,
        successful_auth_state,
    );
    account.account.auth_state = update.state.auth_state;
    account.account.health = update.state.health;
    account.account.last_error_code = update.state.last_error_code;
    account.account.last_used_at_ms = update.state.last_used_at_ms;
    if update.reset_runtime_failures {
        account.cooldowns.clear();
        account.consecutive_failures = 0;
    }
    update.refresh_quota
}
