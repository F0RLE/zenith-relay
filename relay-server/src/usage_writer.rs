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

mod persist;

use persist::persist_usage_batch;

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
#[cfg(test)]
mod tests;
