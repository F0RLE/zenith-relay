use crate::state::{now_ms, AccountCredential, AppState};
use std::collections::BTreeMap;
use std::sync::{atomic::Ordering, mpsc, Arc};
use zenith_relay_core::accounts::{
    reduce_account_usage, AccountAccessState, AccountAuthState, AccountUsageObservation,
    AccountUsageState,
};
use zenith_relay_core::quota::{quota_reference_value, quota_valuation_revision};
use zenith_relay_core::{UsageCallback, UsageEvent};

const USAGE_QUEUE_CAPACITY: usize = 16_384;
const USAGE_BATCH_SIZE: usize = 256;

struct QueuedUsage {
    event: UsageEvent,
    observed_at_ms: u64,
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
        let natural_use_at_ms = match state.store.update_account(&account_id, |account| {
            let credential = state
                .vault
                .load(&account.secret_ref)
                .ok()
                .flatten()
                .and_then(|value| serde_json::from_str::<AccountCredential>(&value).ok());
            let access_state = credential
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
            for queued in &events {
                let event = &queued.event;
                account
                    .economics
                    .set_account_context("chatgpt", account.subscription.plan_type.as_deref());
                account
                    .economics
                    .set_value_revision(quota_valuation_revision());
                account.economics.observe_event_at(
                    event,
                    quota_reference_value(event),
                    queued.observed_at_ms,
                );
                if let Some(snapshot) = event.quota_snapshot.as_ref().filter(|snapshot| {
                    snapshot.updated_at_ms.unwrap_or_default()
                        >= account.quota.updated_at_ms.unwrap_or_default()
                }) {
                    account.quota = snapshot.clone();
                }
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
                if update.reset_runtime_failures {
                    account.cooldowns.clear();
                    account.consecutive_failures = 0;
                }
                if event.success {
                    natural_use_at_ms = Some(queued.observed_at_ms);
                }
            }
            Ok(natural_use_at_ms)
        }) {
            Ok(Some(value)) => value,
            _ => {
                state.failed_usage_writes.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        };
        if let Some(observed_at_ms) = natural_use_at_ms {
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
