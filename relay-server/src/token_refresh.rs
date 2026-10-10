use crate::state::{AccountCredential, AppState, ServerAccountRecord};
use futures_util::{future::BoxFuture, StreamExt};
use reqwest::redirect::Policy;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use zenith_relay_core::accounts::{
    AccountAuthState, TokenPersistenceAdapter, TokenPersistenceFailure, TokenRefresh,
    TokenRefreshAdapter, TokenRefreshFailure, TokenRefreshFailureKind, TokenSet,
};
use zenith_relay_core::error_codes;
use zenith_relay_core::providers::chatgpt::{
    token_refresh_failure_kind, token_refresh_provider_error_code, AgentIdentityCredential,
    OAuthClientKind,
};
use zenith_relay_core::scheduler::refresh::http::{management_http_gate, HttpClass};
use zenith_relay_core::ProxyConfig;

const CODEX_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;

pub(crate) struct ServerTokenPersistence {
    pub(crate) state: Arc<AppState>,
    /// Bound to the runtime's credential incarnation, not whatever login is
    /// currently stored under the same account id when a late refresh ends.
    pub(crate) secret_refs: HashMap<String, String>,
}

pub(crate) fn find_account(
    state: &AppState,
    account_id: &str,
) -> Result<ServerAccountRecord, String> {
    state
        .store
        .accounts()?
        .into_iter()
        .find(|account_record| account_record.id == account_id)
        .ok_or_else(|| "account not found".to_string())
}

mod client;
mod persistence;
pub(crate) use client::{CodexRefreshClient, ServerRefreshClients};
#[cfg(test)]
mod tests;
