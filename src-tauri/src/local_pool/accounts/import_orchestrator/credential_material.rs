use crate::local_pool::accounts::credentials::{bearer_authorization, StoredCodexCredentials};
use crate::local_pool::accounts::import_orchestrator::{
    credential_item_error, ImportItemError, ItemResult,
};
use crate::local_pool::accounts::oauth::CodexOAuthClient;
use reqwest::header::HeaderValue;
use url::Url;
use zenith_relay_core::accounts::{ImportSecretMaterial, ParsedImportItem};
use zenith_relay_core::error_codes;
use zenith_relay_core::providers::chatgpt::{push_account_id_hint, AgentIdentityCredential};
use zenith_relay_core::ProxyConfig;
use zenith_relay_core::{is_http_endpoint, url_has_userinfo};

mod lookup;

#[cfg(test)]
pub(in crate::local_pool::accounts) use lookup::lookup_import_account_id;
pub(in crate::local_pool::accounts) use lookup::resolve_import_account_identity;
use lookup::{account_id_hints, ensure_account_id_hints_are_consistent};

struct ImportDocumentIdentity<'a> {
    email: Option<String>,
    phone: Option<String>,
    password: Option<String>,
    totp_secret: Option<String>,
    item_user_id: Option<String>,
    organization_id: Option<String>,
    plan_hint: Option<&'a str>,
    subscription_active_until_hint: Option<u64>,
    item_account_is_fedramp: bool,
    imported_identity: super::claims::ImportedIdentity,
    account_id_hints: Vec<String>,
}

pub(in crate::local_pool::accounts) struct ImportedCredentialMaterial {
    pub(in crate::local_pool::accounts) access_token: String,
    pub(in crate::local_pool::accounts) agent_identity: Option<AgentIdentityCredential>,
    pub(in crate::local_pool::accounts) refresh_token: Option<String>,
    pub(in crate::local_pool::accounts) id_token: Option<String>,
    pub(in crate::local_pool::accounts) expires_at_ms: Option<u64>,
    pub(in crate::local_pool::accounts) email: Option<String>,
    pub(in crate::local_pool::accounts) phone: Option<String>,
    pub(in crate::local_pool::accounts) password: Option<String>,
    pub(in crate::local_pool::accounts) totp_secret: Option<String>,
    pub(in crate::local_pool::accounts) provider_account_id: Option<String>,
    /// Untrusted IDs collected from the import document and JWT claims. They
    /// are retained only until the authenticated account check completes.
    pub(in crate::local_pool::accounts) account_id_hints: Vec<String>,
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
                    error_codes::AGENT_IDENTITY_INVALID,
                    "Agent Identity credential is invalid",
                )
            });
        }
        bearer_authorization(&self.access_token).map_err(|_| {
            ImportItemError::new(
                error_codes::ACCESS_TOKEN_REJECTED,
                "imported access token is invalid",
            )
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
                ImportItemError::new(
                    error_codes::ACCESS_TOKEN_REJECTED,
                    "imported access token is invalid",
                )
            })
    }

    pub(in crate::local_pool::accounts) fn into_stored(
        self,
        local_account_id: &str,
        issued_at_ms: u64,
        generation: u64,
    ) -> ItemResult<StoredCodexCredentials> {
        let phone = self.phone.clone();
        let password = self.password.clone();
        let totp_secret = self.totp_secret.clone();
        if self.access_token.is_empty() {
            let agent_identity = self.agent_identity.ok_or_else(|| {
                ImportItemError::new(
                    error_codes::ACCESS_TOKEN_MISSING,
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
            .map(|stored| stored.apply_stored_login_material(phone, password, totp_secret))
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
        Ok(stored.apply_stored_login_material(phone, password, totp_secret))
    }

    /// Identity from the import document and its JWTs.
    /// Refresh-token exchange stays separate: OAuth claims outrank these hints.
    fn from_import_document(
        access_token: String,
        agent_identity: Option<AgentIdentityCredential>,
        refresh_token: Option<String>,
        id_token: Option<String>,
        expires_at_ms: Option<u64>,
        document: ImportDocumentIdentity<'_>,
    ) -> Self {
        Self {
            access_token,
            agent_identity,
            refresh_token,
            id_token,
            expires_at_ms,
            email: document.email.or(document.imported_identity.email),
            phone: document.phone,
            password: document.password,
            totp_secret: document.totp_secret,
            provider_account_id: document.account_id_hints.first().cloned(),
            account_id_hints: document.account_id_hints,
            provider_user_id: document
                .imported_identity
                .provider_user_id
                .or(document.item_user_id),
            organization_id: document.organization_id,
            plan_type: document
                .imported_identity
                .plan_type
                .or_else(|| document.plan_hint.map(str::to_string)),
            subscription_active_until_ms: document
                .imported_identity
                .subscription_active_until_ms
                .or(document.subscription_active_until_hint),
            account_is_fedramp: document.item_account_is_fedramp
                || document.imported_identity.account_is_fedramp,
        }
    }
}

pub(in crate::local_pool::accounts) async fn build_import_credential_material(
    item: ParsedImportItem,
    issued_at_ms: u64,
    plan_hint: Option<&str>,
    subscription_active_until_hint: Option<u64>,
    proxy: Option<&ProxyConfig>,
    request_timeout_seconds: u64,
    endpoint: &Url,
) -> ItemResult<ImportedCredentialMaterial> {
    if !is_http_endpoint(endpoint)
        || url_has_userinfo(endpoint)
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(ImportItemError::new(
            error_codes::PROVIDER_ACCOUNT_LOOKUP_FAILED,
            "ChatGPT account lookup endpoint is invalid",
        ));
    }
    let email = item.email().map(str::to_string);
    let phone = item.phone().map(str::to_string);
    let password = item.password().map(str::to_string);
    let totp_secret = item.totp_secret().map(str::to_string);
    let item_account_id = item.account_id.clone();
    let item_user_id = item.chatgpt_user_id.clone();
    let organization_id = item.organization_id.clone();
    let item_account_is_fedramp = item.account_is_fedramp;
    let secrets = item.into_secrets();
    let original_refresh = secrets.refresh_token().map(str::to_string);
    let imported_identity = super::imported_identity(secrets.id_token(), secrets.access_token());
    let account_id_hints = account_id_hints(item_account_id, &imported_identity)?;
    let agent_identity = imported_agent_identity(&secrets)?;
    let access_token = secrets.access_token().map(str::to_string);
    let id_token = secrets.id_token().map(str::to_string);
    let expires_at_ms = imported_identity.access_expires_at_ms;
    let document = ImportDocumentIdentity {
        email,
        phone,
        password,
        totp_secret,
        item_user_id,
        organization_id,
        plan_hint,
        subscription_active_until_hint,
        item_account_is_fedramp,
        imported_identity,
        account_id_hints,
    };

    if let Some(access_token) = access_token {
        let material = ImportedCredentialMaterial::from_import_document(
            access_token,
            agent_identity,
            original_refresh,
            id_token,
            expires_at_ms,
            document,
        );
        return resolve_import_account_identity(material, endpoint, proxy, request_timeout_seconds)
            .await;
    }

    let Some(refresh_token) = original_refresh else {
        let agent_identity = agent_identity.ok_or_else(|| {
            ImportItemError::new(
                error_codes::ACCESS_TOKEN_MISSING,
                "ChatGPT account import requires an access or refresh token",
            )
        })?;
        return Ok(ImportedCredentialMaterial::from_import_document(
            String::new(),
            Some(agent_identity),
            None,
            None,
            None,
            document,
        ));
    };
    let material =
        material_from_refresh_token(refresh_token, agent_identity, issued_at_ms, proxy, document)
            .await?;
    resolve_import_account_identity(material, endpoint, proxy, request_timeout_seconds).await
}

fn imported_agent_identity(
    secrets: &ImportSecretMaterial,
) -> ItemResult<Option<AgentIdentityCredential>> {
    match (secrets.agent_private_key(), secrets.agent_runtime_id()) {
        (Some(private_key), Some(runtime_id)) => Ok(Some(
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
                    error_codes::AGENT_IDENTITY_INVALID,
                    "Agent Identity credential is invalid",
                )
            })?,
        )),
        (None, None) => Ok(None),
        _ => Err(ImportItemError::new(
            error_codes::AGENT_IDENTITY_INVALID,
            "Agent Identity credential is incomplete",
        )),
    }
}

/// OAuth claims from a refresh exchange outrank the import document and its JWTs.
async fn material_from_refresh_token(
    refresh_token: String,
    agent_identity: Option<AgentIdentityCredential>,
    issued_at_ms: u64,
    proxy: Option<&ProxyConfig>,
    document: ImportDocumentIdentity<'_>,
) -> ItemResult<ImportedCredentialMaterial> {
    let oauth = CodexOAuthClient::new_with_proxy(proxy).map_err(|_| {
        ImportItemError::new(
            error_codes::REFRESH_EXCHANGE_UNAVAILABLE,
            "refresh-token exchange is unavailable",
        )
    })?;
    let tokens = oauth
        .exchange_refresh_token(&refresh_token, issued_at_ms)
        .await
        .map_err(|failure| ImportItemError::new(&failure.code, "refresh-token exchange failed"))?;
    let oauth_claims = tokens.identity_claims().map_err(|_| {
        ImportItemError::new(
            error_codes::INVALID_IDENTITY_TOKEN,
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
    let ImportDocumentIdentity {
        email,
        phone,
        password,
        totp_secret,
        item_user_id,
        organization_id,
        plan_hint,
        subscription_active_until_hint,
        item_account_is_fedramp,
        imported_identity,
        mut account_id_hints,
    } = document;
    if let Some(account_id) = oauth_account_id {
        push_account_id_hint(&mut account_id_hints, account_id);
    }
    ensure_account_id_hints_are_consistent(&account_id_hints)?;
    Ok(ImportedCredentialMaterial {
        access_token,
        agent_identity,
        refresh_token: rotated_refresh.or(Some(refresh_token)),
        id_token,
        expires_at_ms,
        email: email.or(oauth_email).or(imported_identity.email),
        phone,
        password,
        totp_secret,
        provider_account_id: account_id_hints.first().cloned(),
        account_id_hints,
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
        account_is_fedramp: item_account_is_fedramp
            || account_is_fedramp
            || imported_identity.account_is_fedramp,
    })
}
