mod callback;
mod manager;
mod snapshot;

use super::import_session::SecretBackend;
use super::oauth::CodexOAuthClient;
use callback::{bind_callback_listener, run_listener};
pub(crate) use shared::callback_secret_ref;
use shared::{AttemptHost, CALLBACK_PATH, PendingSnapshot, SNAPSHOT_VERSION};
pub use shared::{
    OAuthExchangeMaterial, OAuthFlowDiagnostic, OAuthFlowError, OAuthFlowErrorCode, OAuthFlowEvent,
    OAuthFlowEventSink, OAuthFlowFailureReason, OAuthFlowProgress, OAuthFlowStage, OAuthFlowStart,
    OAuthFlowStatus,
};
use snapshot::{
    callback_port, load_snapshots, read_snapshot, remove_snapshot, snapshot_path,
    validate_login_id, write_snapshot,
};
use std::{
    collections::HashMap,
    fmt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpListener, sync::oneshot};
use uuid::Uuid;
use zenith_relay_core::poison::mutex as lock;
use zenith_relay_core::providers::chatgpt::connector::oauth_flow as shared;

pub struct OAuthFlowManager<B, E> {
    inner: Arc<OAuthFlowInner<B, E>>,
}

impl<B, E> Clone for OAuthFlowManager<B, E> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<B, E> fmt::Debug for OAuthFlowManager<B, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthFlowManager")
            .field("root", &self.inner.root)
            .field("listener_count", &lock(&self.inner.listeners).len())
            .finish()
    }
}

struct OAuthFlowInner<B, E> {
    root: PathBuf,
    secrets: B,
    events: E,
    listeners: Mutex<HashMap<String, ListenerControl>>,
    mutation: Mutex<()>,
}

impl<B: SecretBackend, E: OAuthFlowEventSink> AttemptHost for OAuthFlowInner<B, E> {
    type Guard<'a>
        = std::sync::MutexGuard<'a, ()>
    where
        Self: 'a;
    fn lock(&self) -> Self::Guard<'_> {
        lock(&self.mutation)
    }
    fn read(&self, login_id: &str) -> Result<PendingSnapshot, OAuthFlowError> {
        read_snapshot(&self.root, login_id)
    }
    fn write(&self, snapshot: &PendingSnapshot) -> Result<(), OAuthFlowError> {
        write_snapshot(&self.root, snapshot)
    }
    fn remove(&self, login_id: &str) -> Result<(), OAuthFlowError> {
        remove_snapshot(&snapshot_path(&self.root, login_id)?).map_err(|_| {
            OAuthFlowError::new(
                OAuthFlowErrorCode::CleanupIncomplete,
                "OAuth pending snapshot cleanup is incomplete",
            )
            .for_login(login_id)
        })
    }
    fn load_secret(&self, key: &str) -> Result<Option<String>, OAuthFlowError> {
        self.secrets.load(key).map_err(|_| {
            OAuthFlowError::new(
                OAuthFlowErrorCode::SecretStoreUnavailable,
                "OAuth callback secret store is unavailable",
            )
        })
    }
    fn save_secret(&self, key: &str, value: &str) -> Result<(), OAuthFlowError> {
        self.secrets.save(key, value).map_err(|_| {
            OAuthFlowError::new(
                OAuthFlowErrorCode::SecretStoreUnavailable,
                "OAuth callback secret could not be saved",
            )
        })
    }
    fn delete_secret(&self, key: &str) -> Result<(), OAuthFlowError> {
        self.secrets.delete(key).map_err(|_| {
            OAuthFlowError::new(
                OAuthFlowErrorCode::SecretStoreUnavailable,
                "OAuth callback secret could not be cleared",
            )
        })
    }
    fn emit(&self, event: OAuthFlowEvent) {
        self.events.emit(event);
    }
}

impl<B: SecretBackend, E: OAuthFlowEventSink> OAuthFlowInner<B, E> {
    fn accept_callback(&self, login_id: &str, callback_url: &str) -> Result<(), OAuthFlowError> {
        shared::accept_callback(self, login_id, callback_url, now_ms())
    }
    fn cleanup(&self, login_id: &str) -> Result<(), OAuthFlowError> {
        shared::cleanup(self, login_id)
    }
    fn emit(&self, login_id: &str, status: OAuthFlowStatus) {
        self.events.emit(OAuthFlowEvent {
            login_id: login_id.into(),
            status,
            diagnostic: None,
        });
    }
}

struct ListenerControl {
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(1)
}

#[cfg(test)]
mod tests;
