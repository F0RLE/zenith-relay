mod callback;
mod snapshot;

use super::import_session::SecretBackend;
use super::oauth::{CodexOAuthClient, OAuthCallback, OAuthClientKind, OAuthPendingSession};
use callback::{bind_callback_listener, run_listener};
use serde::{Deserialize, Serialize};
pub(crate) use snapshot::callback_secret_ref;
use snapshot::{
    callback_port, load_snapshots, read_snapshot, remove_snapshot, snapshot_path,
    validate_login_id, write_snapshot,
};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use uuid::Uuid;
use zenith_relay_core::poison::mutex as lock;

const SNAPSHOT_VERSION: u32 = 1;
const CALLBACK_PATH: &str = "/auth/callback";

pub trait OAuthFlowEventSink: Send + Sync + 'static {
    fn emit(&self, event: OAuthFlowEvent);
}

impl<F> OAuthFlowEventSink for F
where
    F: Fn(OAuthFlowEvent) + Send + Sync + 'static,
{
    fn emit(&self, event: OAuthFlowEvent) {
        self(event);
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthFlowEvent {
    pub login_id: String,
    pub status: OAuthFlowStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthFlowStatus {
    Pending,
    CallbackReceived,
    CallbackRejected,
    Canceled,
    Completed,
    Expired,
    Failed,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthFlowStart {
    pub login_id: String,
    pub authorization_url: String,
    pub redirect_uri: String,
    pub expires_at_ms: u64,
    pub status: OAuthFlowStatus,
    pub client_kind: OAuthClientKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_account_id: Option<String>,
}

impl fmt::Debug for OAuthFlowStart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthFlowStart")
            .field("login_id", &self.login_id)
            .field("authorization_url", &"[redacted]")
            .field("redirect_uri", &self.redirect_uri)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("status", &self.status)
            .field("client_kind", &self.client_kind)
            .finish()
    }
}

pub struct OAuthExchangeMaterial {
    pending: OAuthPendingSession,
    callback: OAuthCallback,
}

impl OAuthExchangeMaterial {
    pub fn into_parts(self) -> (OAuthPendingSession, OAuthCallback) {
        (self.pending, self.callback)
    }
}

impl fmt::Debug for OAuthExchangeMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthExchangeMaterial")
            .field("pending", &"[redacted]")
            .field("callback", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthFlowErrorCode {
    CallbackAlreadyReceived,
    CallbackInvalid,
    CallbackPortUnavailable,
    CleanupIncomplete,
    Expired,
    InvalidLoginId,
    ListenerUnavailable,
    RecoveryRequired,
    SecretMissing,
    SecretStoreUnavailable,
    SnapshotIo,
    UnsupportedSnapshotVersion,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthFlowError {
    pub code: OAuthFlowErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_id: Option<String>,
}

impl OAuthFlowError {
    fn new(code: OAuthFlowErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
            login_id: None,
        }
    }

    fn for_login(mut self, login_id: &str) -> Self {
        self.login_id = Some(login_id.to_string());
        self
    }
}

impl fmt::Display for OAuthFlowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OAuthFlowError {}

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

mod manager;

struct OAuthFlowInner<B, E> {
    root: PathBuf,
    secrets: B,
    events: E,
    listeners: Mutex<HashMap<String, ListenerControl>>,
    mutation: Mutex<()>,
}

impl<B, E> OAuthFlowInner<B, E>
where
    B: SecretBackend,
    E: OAuthFlowEventSink,
{
    fn accept_callback(&self, login_id: &str, callback_url: &str) -> Result<(), OAuthFlowError> {
        let _mutation = lock(&self.mutation);
        let mut snapshot = read_snapshot(&self.root, login_id)?;
        if snapshot.status == OAuthFlowStatus::CallbackReceived {
            return Err(OAuthFlowError::new(
                OAuthFlowErrorCode::CallbackAlreadyReceived,
                "OAuth callback was already received",
            )
            .for_login(login_id));
        }
        snapshot
            .pending
            .parse_callback(callback_url, now_ms())
            .map_err(|_| {
                OAuthFlowError::new(
                    OAuthFlowErrorCode::CallbackInvalid,
                    "OAuth callback is invalid",
                )
                .for_login(login_id)
            })?;
        self.secrets
            .save(&snapshot.callback_secret_ref, callback_url)
            .map_err(|_| {
                OAuthFlowError::new(
                    OAuthFlowErrorCode::SecretStoreUnavailable,
                    "OAuth callback secret could not be saved",
                )
                .for_login(login_id)
            })?;
        snapshot.status = OAuthFlowStatus::CallbackReceived;
        if let Err(error) = write_snapshot(&self.root, &snapshot) {
            if self.secrets.delete(&snapshot.callback_secret_ref).is_err() {
                return Err(OAuthFlowError::new(
                    OAuthFlowErrorCode::RecoveryRequired,
                    "OAuth callback cleanup requires recovery",
                )
                .for_login(login_id));
            }
            return Err(error.for_login(login_id));
        }
        self.emit(login_id, OAuthFlowStatus::CallbackReceived);
        Ok(())
    }

    fn cleanup(&self, login_id: &str) -> Result<(), OAuthFlowError> {
        let _mutation = lock(&self.mutation);
        let secret_ref = callback_secret_ref(login_id);
        self.secrets.delete(&secret_ref).map_err(|_| {
            OAuthFlowError::new(
                OAuthFlowErrorCode::SecretStoreUnavailable,
                "OAuth callback secret could not be cleared",
            )
            .for_login(login_id)
        })?;
        remove_snapshot(&snapshot_path(&self.root, login_id)?).map_err(|_| {
            OAuthFlowError::new(
                OAuthFlowErrorCode::CleanupIncomplete,
                "OAuth pending snapshot cleanup is incomplete",
            )
            .for_login(login_id)
        })?;
        Ok(())
    }

    fn emit(&self, login_id: &str, status: OAuthFlowStatus) {
        self.events.emit(OAuthFlowEvent {
            login_id: login_id.to_string(),
            status,
        });
    }
}

struct ListenerControl {
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingSnapshot {
    version: u32,
    login_id: String,
    authorization_url: String,
    callback_secret_ref: String,
    status: OAuthFlowStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sign_in_proxy_id: Option<String>,
    pending: OAuthPendingSession,
}

impl PendingSnapshot {
    fn start(&self) -> OAuthFlowStart {
        OAuthFlowStart {
            login_id: self.login_id.clone(),
            authorization_url: self.authorization_url.clone(),
            redirect_uri: self.pending.redirect_uri().to_string(),
            expires_at_ms: self.pending.expires_at_ms(),
            status: self.status,
            client_kind: self.pending.client_kind(),
            target_account_id: self.target_account_id.clone(),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(1)
}

#[cfg(test)]
mod tests;
