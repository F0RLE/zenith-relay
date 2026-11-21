//! Persisted OAuth attempt lifecycle, independent of native listeners and windows.
use super::oauth::{CodexOAuthClient, OAuthCallback, OAuthClientKind, OAuthPendingSession};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const SNAPSHOT_VERSION: u32 = 1;
pub const CALLBACK_PATH: &str = "/auth/callback";

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<OAuthFlowDiagnostic>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthFlowStage {
    Callback,
    TokenExchange,
    IdentityValidation,
    Saving,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthFlowFailureReason {
    CallbackInvalid,
    TokenExchangeFailed,
    IdentityInvalid,
    SaveFailed,
    StorageUnavailable,
    Conflict,
    RecoveryRequired,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthFlowDiagnostic {
    pub stage: OAuthFlowStage,
    pub elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<OAuthFlowFailureReason>,
}

pub struct OAuthFlowProgress {
    stage: OAuthFlowStage,
    started: std::time::Instant,
}

impl OAuthFlowProgress {
    pub fn new() -> Self {
        Self {
            stage: OAuthFlowStage::Callback,
            started: std::time::Instant::now(),
        }
    }

    pub fn stage(&self) -> OAuthFlowStage {
        self.stage
    }

    pub fn advance(&mut self, stage: OAuthFlowStage) {
        self.stage = stage;
        self.started = std::time::Instant::now();
    }

    pub fn diagnostic(&self, reason: Option<OAuthFlowFailureReason>) -> OAuthFlowDiagnostic {
        OAuthFlowDiagnostic {
            stage: self.stage,
            elapsed_ms: self.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            reason,
        }
    }
}

impl Default for OAuthFlowProgress {
    fn default() -> Self {
        Self::new()
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes_account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automatic_login: Option<bool>,
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
    AuthorizationDenied,
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
    pub fn new(code: OAuthFlowErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
            login_id: None,
        }
    }

    pub fn for_login(mut self, login_id: &str) -> Self {
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

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingSnapshot {
    pub version: u32,
    pub login_id: String,
    pub authorization_url: String,
    pub callback_secret_ref: String,
    pub status: OAuthFlowStatus,
    #[serde(default)]
    pub exchange_started: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in_proxy_id: Option<String>,
    pub pending: OAuthPendingSession,
}

impl PendingSnapshot {
    pub fn start(&self) -> OAuthFlowStart {
        OAuthFlowStart {
            login_id: self.login_id.clone(),
            authorization_url: self.authorization_url.clone(),
            redirect_uri: self.pending.redirect_uri().to_string(),
            expires_at_ms: self.pending.expires_at_ms(),
            status: self.status,
            client_kind: self.pending.client_kind(),
            target_account_id: self.target_account_id.clone(),
            notes_account_id: self.notes_account_id.clone(),
            automatic_login: None,
        }
    }
}

mod lifecycle;
pub use lifecycle::*;
