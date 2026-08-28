use super::{
    account_id_from_check_response, imported_identity, CODEX_ACCOUNT_CHECK_ENDPOINT,
    MAX_ACCOUNT_PROFILE_RESPONSE_BYTES,
};
use crate::local_pool::accounts::credentials::{bearer_authorization, StoredCodexCredentials};
use crate::local_pool::accounts::import_orchestrator::{
    credential_item_error, ImportItemError, ItemResult,
};
use crate::local_pool::accounts::oauth::CodexOAuthClient;
use crate::local_pool::accounts::{collect_limited, LimitedBodyError};
use reqwest::header::HeaderValue;
use reqwest::redirect::Policy;
use std::time::Duration;
use url::Url;
use zenith_relay_core::accounts::ParsedImportItem;
use zenith_relay_core::providers::chatgpt::AgentIdentityCredential;
use zenith_relay_core::ProxyConfig;

pub(in crate::local_pool::accounts) struct ImportedCredentialMaterial {
    pub(in crate::local_pool::accounts) access_token: String,
    pub(in crate::local_pool::accounts) agent_identity: Option<AgentIdentityCredential>,
    pub(in crate::local_pool::accounts) refresh_token: Option<String>,
    pub(in crate::local_pool::accounts) id_token: Option<String>,
    pub(in crate::local_pool::accounts) expires_at_ms: Option<u64>,
    pub(in crate::local_pool::accounts) email: Option<String>,
    pub(in crate::local_pool::accounts) provider_account_id: Option<String>,
    pub(in crate::local_pool::accounts) provider_user_id: Option<String>,
    pub(in crate::local_pool::accounts) organization_id: Option<String>,
    pub(in crate::local_pool::accounts) plan_type: Option<String>,
    pub(in crate::local_pool::accounts) subscription_active_until_ms: Option<u64>,
    pub(in crate::local_pool::accounts) account_is_fedramp: bool,
}

impl ImportedCredentialMaterial {
    pub(in crate::local_pool::accounts) fn authorization(
        &self,
        now_ms: u64,
    ) -> ItemResult<HeaderValue> {
        if let Some(agent) = self.agent_identity.as_ref() {
            return agent.authorization(now_ms).map_err(|_| {
                ImportItemError::new(
                    "agent_identity_invalid",
                    "Agent Identity credential is invalid",
                )
            });
        }
        bearer_authorization(&self.access_token).map_err(|_| {
            ImportItemError::new("access_token_rejected", "imported access token is invalid")
        })
    }

    pub(in crate::local_pool::accounts) fn subscription_authorization(
        &self,
    ) -> ItemResult<Option<HeaderValue>> {
        if self.access_token.is_empty() {
            return Ok(None);
        }
        bearer_authorization(&self.access_token)
            .map(Some)
            .map_err(|_| {
                ImportItemError::new("access_token_rejected", "imported access token is invalid")
            })
    }

    pub(in crate::local_pool::accounts) fn into_stored(
        self,
        local_account_id: &str,
        issued_at_ms: u64,
        generation: u64,
    ) -> ItemResult<StoredCodexCredentials> {
        if self.access_token.is_empty() {
            let agent_identity = self.agent_identity.ok_or_else(|| {
                ImportItemError::new(
                    "access_token_missing",
                    "ChatGPT account import has no authorization method",
                )
            })?;
            return StoredCodexCredentials::new_agent_identity(
                local_account_id,
                agent_identity,
                issued_at_ms,
                generation,
                self.email,
                self.provider_account_id,
                self.provider_user_id,
                self.organization_id,
                self.plan_type,
                self.account_is_fedramp,
            )
            .map_err(credential_item_error);
        }
        let agent_identity = self.agent_identity;
        let mut stored = StoredCodexCredentials::new(
            local_account_id,
            self.access_token,
            self.refresh_token,
            self.id_token,
            self.expires_at_ms,
            issued_at_ms,
            generation,
            self.email,
            self.provider_account_id,
            self.provider_user_id,
            self.organization_id,
            self.plan_type,
            self.account_is_fedramp,
        )
        .map_err(credential_item_error)?;
        if let Some(agent_identity) = agent_identity {
            stored = stored.with_agent_identity(agent_identity);
        }
        Ok(stored)
    }
}

pub(in crate::local_pool::accounts) async fn build_import_credential_material(
    item: ParsedImportItem,
    issued_at_ms: u64,
    plan_hint: Option<&str>,
    subscription_active_until_hint: Option<u64>,
    proxy: Option<&ProxyConfig>,
    request_timeout_seconds: u64,
) -> ItemResult<ImportedCredentialMaterial> {
    let email = item.email().map(str::to_string);
    let item_account_id = item.account_id.clone();
    let item_user_id = item.chatgpt_user_id.clone();
    let organization_id = item.organization_id.clone();
    let secrets = item.into_secrets();
    let original_refresh = secrets.refresh_token().map(str::to_string);
    let imported_identity = imported_identity(secrets.id_token(), secrets.access_token());

    let agent_identity = match (secrets.agent_private_key(), secrets.agent_runtime_id()) {
        (Some(private_key), Some(runtime_id)) => Some(
            match secrets.agent_task_id() {
                Some(task_id) => AgentIdentityCredential::new(
                    private_key.to_string(),
                    runtime_id.to_string(),
                    task_id.to_string(),
                ),
                None => AgentIdentityCredential::unregistered(
                    private_key.to_string(),
                    runtime_id.to_string(),
                ),
            }
            .map_err(|_| {
                ImportItemError::new(
                    "agent_identity_invalid",
                    "Agent Identity credential is invalid",
                )
            })?,
        ),
        (None, None) => None,
        _ => {
            return Err(ImportItemError::new(
                "agent_identity_invalid",
                "Agent Identity credential is incomplete",
            ))
        }
    };

    if let Some(access_token) = secrets.access_token() {
        let material = ImportedCredentialMaterial {
            access_token: access_token.to_string(),
            agent_identity,
            refresh_token: original_refresh,
            id_token: secrets.id_token().map(str::to_string),
            expires_at_ms: imported_identity.access_expires_at_ms,
            email: email.or(imported_identity.email),
            provider_account_id: imported_identity.provider_account_id.or(item_account_id),
            provider_user_id: imported_identity.provider_user_id.or(item_user_id),
            organization_id,
            plan_type: imported_identity
                .plan_type
                .or_else(|| plan_hint.map(str::to_string)),
            subscription_active_until_ms: imported_identity
                .subscription_active_until_ms
                .or(subscription_active_until_hint),
            account_is_fedramp: imported_identity.account_is_fedramp,
        };
        return resolve_import_account_identity(material, proxy, request_timeout_seconds).await;
    }

    let Some(refresh_token) = original_refresh else {
        let agent_identity = agent_identity.ok_or_else(|| {
            ImportItemError::new(
                "access_token_missing",
                "ChatGPT account import requires an access or refresh token",
            )
        })?;
        return Ok(ImportedCredentialMaterial {
            access_token: String::new(),
            agent_identity: Some(agent_identity),
            refresh_token: None,
            id_token: None,
            expires_at_ms: None,
            email: email.or(imported_identity.email),
            provider_account_id: imported_identity.provider_account_id.or(item_account_id),
            provider_user_id: imported_identity.provider_user_id.or(item_user_id),
            organization_id,
            plan_type: imported_identity
                .plan_type
                .or_else(|| plan_hint.map(str::to_string)),
            subscription_active_until_ms: imported_identity
                .subscription_active_until_ms
                .or(subscription_active_until_hint),
            account_is_fedramp: imported_identity.account_is_fedramp,
        });
    };
    let oauth = CodexOAuthClient::new_with_proxy(proxy).map_err(|_| {
        ImportItemError::new(
            "refresh_exchange_unavailable",
            "refresh-token exchange is unavailable",
        )
    })?;
    let tokens = oauth
        .exchange_refresh_token(&refresh_token, issued_at_ms)
        .await
        .map_err(|failure| ImportItemError::new(&failure.code, "refresh-token exchange failed"))?;
    let oauth_claims = tokens.identity_claims().map_err(|_| {
        ImportItemError::new(
            "invalid_identity_token",
            "refreshed identity token is invalid",
        )
    })?;
    let oauth_email = oauth_claims
        .as_ref()
        .and_then(|claims| claims.email().map(str::to_string));
    let oauth_account_id = oauth_claims
        .as_ref()
        .and_then(|claims| claims.account_id().map(str::to_string));
    let oauth_user_id = oauth_claims
        .as_ref()
        .and_then(|claims| claims.user_id().map(str::to_string));
    let oauth_plan = oauth_claims
        .as_ref()
        .and_then(|claims| claims.plan_type().map(str::to_string));
    let oauth_subscription_active_until_ms = oauth_claims
        .as_ref()
        .and_then(|claims| claims.subscription_active_until_ms());
    let account_is_fedramp = oauth_claims
        .as_ref()
        .is_some_and(|claims| claims.account_is_fedramp());
    let (access_token, rotated_refresh, id_token, expires_at_ms) = tokens.into_secret_parts();
    let material = ImportedCredentialMaterial {
        access_token,
        agent_identity,
        refresh_token: rotated_refresh.or(Some(refresh_token)),
        id_token,
        expires_at_ms,
        email: email.or(oauth_email).or(imported_identity.email),
        provider_account_id: oauth_account_id
            .or(imported_identity.provider_account_id)
            .or(item_account_id),
        provider_user_id: oauth_user_id
            .or(imported_identity.provider_user_id)
            .or(item_user_id),
        organization_id,
        plan_type: oauth_plan
            .or(imported_identity.plan_type)
            .or_else(|| plan_hint.map(str::to_string)),
        subscription_active_until_ms: imported_identity
            .subscription_active_until_ms
            .or(oauth_subscription_active_until_ms)
            .or(subscription_active_until_hint),
        account_is_fedramp: account_is_fedramp || imported_identity.account_is_fedramp,
    };
    resolve_import_account_identity(material, proxy, request_timeout_seconds).await
}

pub(in crate::local_pool::accounts) async fn resolve_import_account_identity(
    mut material: ImportedCredentialMaterial,
    proxy: Option<&ProxyConfig>,
    request_timeout_seconds: u64,
) -> ItemResult<ImportedCredentialMaterial> {
    if material.provider_account_id.is_some() {
        return Ok(material);
    }
    let endpoint = Url::parse(CODEX_ACCOUNT_CHECK_ENDPOINT).map_err(|_| {
        ImportItemError::new(
            "provider_account_lookup_failed",
            "ChatGPT account lookup is unavailable",
        )
    })?;
    material.provider_account_id = Some(
        lookup_import_account_id(
            endpoint,
            &material.access_token,
            proxy,
            Duration::from_secs(request_timeout_seconds.max(1)),
        )
        .await?,
    );
    Ok(material)
}

pub(in crate::local_pool::accounts) async fn lookup_import_account_id(
    endpoint: Url,
    access_token: &str,
    proxy: Option<&ProxyConfig>,
    timeout: Duration,
) -> ItemResult<String> {
    let authorization = HeaderValue::from_str(&format!("Bearer {access_token}")).map_err(|_| {
        ImportItemError::new("access_token_rejected", "imported access token is invalid")
    })?;
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(timeout)
        .user_agent("Zenith Relay");
    let http = match proxy {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .map_err(|_| {
        ImportItemError::new(
            "provider_account_lookup_failed",
            "ChatGPT account lookup client could not be created",
        )
    })?;
    let response = http
        .get(endpoint)
        .header(reqwest::header::AUTHORIZATION, authorization)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|_| {
            ImportItemError::new(
                "provider_account_lookup_failed",
                "ChatGPT account lookup request failed",
            )
        })?;
    let status = response.status();
    let body = collect_limited(response, MAX_ACCOUNT_PROFILE_RESPONSE_BYTES)
        .await
        .map_err(|error| match error {
            LimitedBodyError::Transport => ImportItemError::new(
                "provider_account_lookup_failed",
                "ChatGPT account lookup response could not be read",
            ),
            LimitedBodyError::TooLarge => ImportItemError::new(
                "provider_account_lookup_failed",
                "ChatGPT account lookup response was too large",
            ),
        })?;
    if !status.is_success() {
        let (code, message) = match status.as_u16() {
            401 | 403 => (
                "access_token_rejected",
                "ChatGPT rejected the imported access token",
            ),
            429 => (
                "account_profile_rate_limited",
                "ChatGPT rate limited the account lookup request",
            ),
            _ => (
                "provider_account_lookup_failed",
                "ChatGPT account lookup returned an unexpected status",
            ),
        };
        return Err(ImportItemError::new(code, message));
    }
    let payload: serde_json::Value = serde_json::from_slice(&body).map_err(|_| {
        ImportItemError::new(
            "provider_account_lookup_failed",
            "ChatGPT account lookup returned invalid JSON",
        )
    })?;
    account_id_from_check_response(&payload).ok_or_else(|| {
        ImportItemError::new(
            "provider_account_id_missing",
            "ChatGPT account lookup did not return an account id",
        )
    })
}
