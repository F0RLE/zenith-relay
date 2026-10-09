use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialRefreshStatus {
    Refreshed,
    RetryableFailure,
    RequiresReauth,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialRefreshResult {
    pub account_id: String,
    pub status: CredentialRefreshStatus,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
}

pub(in crate::local_pool::accounts) use zenith_relay_core::accounts::TOKEN_REFRESH_SKEW_MS;

pub(in crate::local_pool::accounts) const QUOTA_COMMAND_TIMEOUT_OVERHEAD: Duration =
    Duration::from_secs(5);

pub(in crate::local_pool::accounts) const QUOTA_REFRESH_BATCH_SIZE: usize = 5;

// A profile observation is best effort. It must never hold a request or quota
// refresh behind an automatic credential rotation for the normal five-second
// mutation-lock timeout.
pub(in crate::local_pool::accounts) const MANAGED_PROFILE_OBSERVATION_LOCK: ProcessLockConfig =
    ProcessLockConfig {
        wait_timeout_ms: 125,
        poll_interval_ms: 25,
        stale_after_ms: 120_000,
    };

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum AccountQuotaOutcome {
    Skipped,
    Updated {
        transitions: Vec<QuotaTransition>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        exhaustion_transitions: Vec<QuotaTransition>,
    },
    Failed {
        code: String,
        retryable: bool,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItemResult {
    pub item_id: String,
    pub status: ImportItemStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<LocalAccountRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ProviderSourceRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota: Option<AccountQuotaOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ImportItemError>,
}

impl ImportItemResult {
    pub(in crate::local_pool::accounts) fn account_success(
        item_id: String,
        account: LocalAccountRecord,
        quota: AccountQuotaOutcome,
    ) -> Self {
        Self {
            item_id,
            status: ImportItemStatus::Succeeded,
            account: Some(account),
            source: None,
            quota: Some(quota),
            error: None,
        }
    }

    pub(in crate::local_pool::accounts) fn source_success(
        item_id: String,
        source: ProviderSourceRecord,
    ) -> Self {
        Self {
            item_id,
            status: ImportItemStatus::Succeeded,
            account: None,
            source: Some(source),
            quota: None,
            error: None,
        }
    }

    pub(in crate::local_pool::accounts) fn failure(
        item_id: String,
        error: ImportItemError,
    ) -> Self {
        Self {
            item_id,
            status: ImportItemStatus::Failed,
            account: None,
            source: None,
            quota: None,
            error: Some(error),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmAccountImportResponse {
    pub session_id: String,
    pub results: Vec<ImportItemResult>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountQuotaRefreshResponse {
    pub account: LocalAccountRecord,
    pub quota: AccountQuotaOutcome,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exhaustion_transitions: Vec<QuotaTransition>,
}

pub(crate) struct PreparedAccountCredentials {
    pub(in crate::local_pool::accounts) oauth_client_kind:
        zenith_relay_core::providers::chatgpt::OAuthClientKind,
    pub(in crate::local_pool::accounts) tokens: TokenSet,
    pub(in crate::local_pool::accounts) provider_account_id: String,
    pub(in crate::local_pool::accounts) proxy: Option<ProxyConfig>,
}

#[derive(Clone)]
pub(in crate::local_pool) struct PreparedAccountAuthorization {
    pub(in crate::local_pool::accounts) authorization: HeaderValue,
    pub(in crate::local_pool::accounts) subscription_authorization: Option<HeaderValue>,
    pub(in crate::local_pool::accounts) tokens: Option<TokenSet>,
    pub(in crate::local_pool::accounts) agent_task_id: Option<String>,
    pub(in crate::local_pool::accounts) provider_account_id: String,
    pub(in crate::local_pool::accounts) proxy: Option<ProxyConfig>,
}

impl fmt::Debug for PreparedAccountAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedAccountAuthorization")
            .field("authorization", &"[redacted]")
            .field("subscription_authorization", &"[redacted]")
            .field("tokens", &"[redacted]")
            .field("agent_task_id", &"[redacted]")
            .field("provider_account_id", &"[redacted]")
            .field("proxy", &"[redacted]")
            .finish()
    }
}

impl PreparedAccountAuthorization {
    pub(in crate::local_pool::accounts) fn from_tokens(
        credentials: PreparedAccountCredentials,
    ) -> LocalResult<Self> {
        let authorization =
            bearer_authorization(credentials.tokens.access_token()).map_err(|_| {
                LocalPoolError::new(ErrorCode::InvalidState, "account token is invalid")
            })?;
        Ok(Self {
            subscription_authorization: Some(authorization.clone()),
            authorization,
            tokens: Some(credentials.tokens),
            agent_task_id: None,
            provider_account_id: credentials.provider_account_id,
            proxy: credentials.proxy,
        })
    }
}

impl PreparedAccountCredentials {
    pub(crate) fn supports_native_codex(&self) -> bool {
        self.oauth_client_kind == zenith_relay_core::providers::chatgpt::OAuthClientKind::Codex
    }

    pub(crate) fn tokens(&self) -> &TokenSet {
        &self.tokens
    }

    pub(crate) fn provider_account_id(&self) -> &str {
        &self.provider_account_id
    }

    pub(crate) fn proxy(&self) -> Option<&ProxyConfig> {
        self.proxy.as_ref()
    }
}

impl fmt::Debug for PreparedAccountCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedAccountCredentials")
            .field("tokens", &self.tokens)
            .field("provider_account_id", &"[redacted]")
            .field("proxy_configured", &self.proxy.is_some())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountQuotaRefreshStatus {
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountQuotaRefreshItemResult {
    pub account_id: String,
    pub status: AccountQuotaRefreshStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<AccountQuotaRefreshResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CommandError>,
}
