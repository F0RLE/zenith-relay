use crate::local_pool::{
    accounts::{
        credentials::{CredentialError, StoredCodexCredentials},
        import_session::SecretBackend,
        oauth::{CodexOAuthClient, OAuthTokenSet},
        oauth_flow::callback_secret_ref,
        oauth_flow::{OAuthFlowEventSink, OAuthFlowManager, OAuthFlowStatus},
        records::{self},
        NativeSecretBackend,
    },
    error::{ErrorCode, LocalPoolError, Result as LocalResult},
};

use serde::{Deserialize, Serialize};
use std::fmt;

use zenith_relay_core::ProxyConfig;

use super::flow::{flow_error, oauth_error};

pub(super) const COMPLETION_CHECKPOINT_VERSION: u32 = 1;
const MAX_COMPLETION_CHECKPOINT_BYTES: usize = 256 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OAuthCompletionCheckpoint {
    pub(super) version: u32,
    pub(super) login_id: String,
    pub(super) access_token: String,
    pub(super) refresh_token: Option<String>,
    pub(super) id_token: Option<String>,
    pub(super) expires_at_ms: Option<u64>,
    pub(super) issued_at_ms: u64,
    pub(super) email: Option<String>,
    pub(super) provider_account_id: String,
    pub(super) provider_user_id: Option<String>,
    pub(super) plan_type: Option<String>,
    #[serde(default)]
    pub(super) subscription_active_until_ms: Option<u64>,
    pub(super) account_is_fedramp: bool,
}

impl OAuthCompletionCheckpoint {
    fn from_tokens(login_id: &str, tokens: OAuthTokenSet, issued_at_ms: u64) -> LocalResult<Self> {
        let claims = tokens
            .identity_claims()
            .map_err(oauth_error)?
            .ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "OAuth response did not contain identity claims",
                )
            })?;
        let provider_account_id = claims
            .account_id()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "OAuth response did not contain a ChatGPT account id",
                )
            })?
            .to_string();
        let checkpoint = Self {
            version: COMPLETION_CHECKPOINT_VERSION,
            login_id: login_id.to_string(),
            access_token: tokens.access_token().to_string(),
            refresh_token: tokens.refresh_token().map(str::to_string),
            id_token: tokens.id_token().map(str::to_string),
            expires_at_ms: tokens.expires_at_ms(),
            issued_at_ms,
            email: claims.email().map(str::to_string),
            provider_account_id,
            provider_user_id: claims.user_id().map(str::to_string),
            plan_type: claims.plan_type().map(str::to_string),
            subscription_active_until_ms: claims.subscription_active_until_ms(),
            account_is_fedramp: claims.account_is_fedramp(),
        };
        checkpoint.validate(login_id)?;
        Ok(checkpoint)
    }

    fn validate(&self, expected_login_id: &str) -> LocalResult<()> {
        if self.version != COMPLETION_CHECKPOINT_VERSION
            || self.login_id != expected_login_id
            || self.issued_at_ms == 0
            || self.id_token.is_none()
        {
            return Err(invalid_completion_checkpoint());
        }
        self.to_credentials("oauth_checkpoint", 1)
            .map(|_| ())
            .map_err(|_| invalid_completion_checkpoint())
    }

    pub(super) fn to_credentials(
        &self,
        local_account_id: &str,
        generation: u64,
    ) -> Result<StoredCodexCredentials, CredentialError> {
        StoredCodexCredentials::new(
            local_account_id,
            self.access_token.clone(),
            self.refresh_token.clone(),
            self.id_token.clone(),
            self.expires_at_ms,
            self.issued_at_ms,
            generation,
            self.email.clone(),
            Some(self.provider_account_id.clone()),
            self.provider_user_id.clone(),
            None,
            self.plan_type.clone(),
            self.account_is_fedramp,
        )
    }

    pub(super) fn identity_hash(&self) -> String {
        records::identity_hash(
            &self.provider_account_id,
            self.provider_user_id.as_deref(),
            self.email.as_deref(),
        )
    }
}

impl fmt::Debug for OAuthCompletionCheckpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthCompletionCheckpoint")
            .field("version", &self.version)
            .field("login_id", &self.login_id)
            .field("access_token", &"[redacted]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "[redacted]"))
            .field("expires_at_ms", &self.expires_at_ms)
            .field("email", &self.email.as_ref().map(|_| "[redacted]"))
            .field("provider_account_id", &"[redacted]")
            .field(
                "provider_user_id",
                &self.provider_user_id.as_ref().map(|_| "[redacted]"),
            )
            .field("plan_type", &self.plan_type)
            .field(
                "subscription_active_until_ms",
                &self.subscription_active_until_ms,
            )
            .field("account_is_fedramp", &self.account_is_fedramp)
            .finish()
    }
}

pub(super) async fn completion_checkpoint<E>(
    flow: &OAuthFlowManager<NativeSecretBackend, E>,
    login_id: &str,
    now_ms: u64,
    proxy: Option<&ProxyConfig>,
) -> LocalResult<(OAuthCompletionCheckpoint, String, Option<String>)>
where
    E: OAuthFlowEventSink,
{
    let start = flow.status(login_id).map_err(flow_error)?;
    if start.status != OAuthFlowStatus::CallbackReceived {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "OAuth callback has not been received",
        ));
    }
    let secret_ref = callback_secret_ref(&start.login_id);
    let stored = NativeSecretBackend
        .load(&secret_ref)
        .map_err(|_| completion_secret_error())?
        .ok_or_else(completion_secret_error)?;
    if let Some(checkpoint) = decode_completion_checkpoint(&stored, &start.login_id)? {
        return Ok((checkpoint, stored, start.target_account_id));
    }
    drop(stored);

    let material = flow
        .exchange_material(&start.login_id)
        .map_err(flow_error)?;
    let (pending, callback) = material.into_parts();
    let tokens = CodexOAuthClient::new_with_proxy(proxy)
        .map_err(oauth_error)?
        .exchange_code(&pending, callback, now_ms)
        .await
        .map_err(oauth_error)?;
    let checkpoint = OAuthCompletionCheckpoint::from_tokens(&start.login_id, tokens, now_ms)?;
    let encoded = encode_completion_checkpoint(&checkpoint)?;
    store_completion_checkpoint(&start.login_id, &encoded)?;
    Ok((checkpoint, encoded, start.target_account_id))
}

pub(super) fn decode_completion_checkpoint(
    value: &str,
    expected_login_id: &str,
) -> LocalResult<Option<OAuthCompletionCheckpoint>> {
    if !value.trim_start().starts_with('{') {
        return Ok(None);
    }
    if value.len() > MAX_COMPLETION_CHECKPOINT_BYTES {
        return Err(invalid_completion_checkpoint());
    }
    let checkpoint: OAuthCompletionCheckpoint =
        serde_json::from_str(value).map_err(|_| invalid_completion_checkpoint())?;
    checkpoint.validate(expected_login_id)?;
    Ok(Some(checkpoint))
}

pub(super) fn encode_completion_checkpoint(
    checkpoint: &OAuthCompletionCheckpoint,
) -> LocalResult<String> {
    let encoded = serde_json::to_string(checkpoint).map_err(|_| invalid_completion_checkpoint())?;
    if encoded.len() > MAX_COMPLETION_CHECKPOINT_BYTES {
        Err(invalid_completion_checkpoint())
    } else {
        Ok(encoded)
    }
}

pub(super) fn store_completion_checkpoint(login_id: &str, encoded: &str) -> LocalResult<()> {
    let secret_ref = callback_secret_ref(login_id);
    NativeSecretBackend
        .save(&secret_ref, encoded)
        .map_err(|_| completion_secret_error())?;
    let stored = NativeSecretBackend
        .load(&secret_ref)
        .map_err(|_| completion_secret_error())?;
    if stored.as_deref() != Some(encoded) {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "OAuth completion checkpoint could not be verified",
        ));
    }
    Ok(())
}

pub(super) fn restore_completion_checkpoint(login_id: &str, encoded: &str) -> LocalResult<()> {
    store_completion_checkpoint(login_id, encoded)
}

pub(super) fn invalid_completion_checkpoint() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        "OAuth completion checkpoint requires recovery",
    )
}

pub(super) fn completion_secret_error() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::SecretStoreUnavailable,
        "OAuth completion secret storage is unavailable",
    )
}
