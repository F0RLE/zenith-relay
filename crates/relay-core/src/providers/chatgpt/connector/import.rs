//! Account import orchestration. Credential resolution and transactional storage
//! are host ports; admission, metadata preservation and probe order live here.
use super::{credentials::StoredCodexCredentials, observations::QuotaReadOutcome};
use crate::{
    ProxyConfig,
    accounts::{
        AccountAuthMode, AccountHealthState, AccountRecord, ImportAuthMode, ParsedImportItem,
        management::ManagedAccount,
    },
    error_codes,
    providers::chatgpt::OAuthClientKind,
    quota::{Subscription, SubscriptionInput},
};
use std::future::Future;

pub const MAX_ACCOUNT_LABEL_BYTES: usize = 128;
pub const IMPORT_QUOTA_BATCH_SIZE: usize = 5;

pub fn should_probe_quota(requested: bool, row_count: usize) -> bool {
    requested && row_count <= IMPORT_QUOTA_BATCH_SIZE
}

pub fn account_model_state_is_valid(account: &impl ImportRecord) -> bool {
    account.images_enabled()
        || account.discovered_models().is_some()
        || !account.configured_models().is_empty()
        || (account.account().last_error_code.is_some()
            && account.account().health != AccountHealthState::Healthy)
}

#[derive(Clone, Copy)]
pub struct AccountImportOptions<'a> {
    pub add_to_pool: bool,
    pub discover_models: bool,
    pub probe_quota: bool,
    pub configured_models: &'a [String],
}

pub struct ResolvedImport<A> {
    pub credentials: StoredCodexCredentials,
    pub old_credential: Option<StoredCodexCredentials>,
    pub existing_account: Option<A>,
    pub preserved_refresh_token: bool,
    pub subscription_active_until_ms: Option<u64>,
    pub proxy: Option<ProxyConfig>,
    pub provider_account_id: String,
    pub identity_is_registered: bool,
}

pub trait ImportRecord: ManagedAccount + Send + Sync {
    fn configured_models(&self) -> &[String];
    fn discovered_models(&self) -> Option<&[String]>;
    fn set_discovered_models(&mut self, models: Option<Vec<String>>);
    /// Preserve host-owned recovery and model restrictions, not new auth evidence.
    fn preserve_host_settings(&mut self, previous: &Self);
    fn normalize(&mut self);
}

pub trait ImportHost: Sync {
    type Account: ImportRecord;
    type Error: Send;
    fn error(code: &'static str, message: &'static str) -> Self::Error;
    fn now_ms(&self) -> u64;
    fn resolve(
        &self,
        item: ParsedImportItem,
        issued_at_ms: u64,
    ) -> impl Future<Output = Result<ResolvedImport<Self::Account>, Self::Error>> + Send;
    fn discover(
        &self,
        resolved: &ResolvedImport<Self::Account>,
        issued_at_ms: u64,
    ) -> impl Future<Output = Result<Vec<String>, Self::Error>> + Send;
    fn new_record(
        &self,
        credentials: &StoredCodexCredentials,
        auth: AccountAuthMode,
        models: Vec<String>,
        priority: i32,
        issued_at_ms: u64,
    ) -> Result<Self::Account, Self::Error>;
    fn probe_quota(
        &self,
        account: &mut Self::Account,
        resolved: &ResolvedImport<Self::Account>,
    ) -> impl Future<Output = QuotaReadOutcome> + Send;
    /// Fence and compare the original credential snapshot under the account
    /// lock. Persist secrets/record, publish, and conditionally roll back on error.
    fn commit(
        &self,
        account: &Self::Account,
        resolved: &ResolvedImport<Self::Account>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

pub fn auth_mode(mode: ImportAuthMode) -> Result<AccountAuthMode, (&'static str, &'static str)> {
    match mode {
        ImportAuthMode::OAuth => Ok(AccountAuthMode::OAuth),
        ImportAuthMode::AgentIdentity | ImportAuthMode::ImportedToken => {
            Ok(AccountAuthMode::ImportedToken)
        }
        ImportAuthMode::ApiKey => Err((
            error_codes::USE_SOURCE_IMPORT,
            "API keys must be imported as compatible API sources",
        )),
        ImportAuthMode::Unknown => Err((
            error_codes::UNKNOWN_AUTH_MODE,
            "imported account authentication mode is unknown",
        )),
    }
}

pub fn valid_label(label: &str) -> bool {
    let label = label.trim();
    !label.is_empty()
        && label.len() <= MAX_ACCOUNT_LABEL_BYTES
        && !label.chars().any(char::is_control)
}

pub fn preserve_account_metadata(account: &mut AccountRecord, previous: &AccountRecord) {
    account.label = previous.label.clone();
    account.tags = previous.tags.clone();
    account.enabled = previous.enabled;
    account.in_pool = previous.in_pool;
    account.draining = previous.draining;
    account.created_at_ms = previous.created_at_ms;
    account.last_used_at_ms = previous.last_used_at_ms;
    account.health = previous.health;
    account.quota = previous.quota.clone();
    account.subscription = previous.subscription.clone();
    account.last_error_code = previous.last_error_code.clone();
}

pub fn merge_existing_account<A: ImportRecord>(account: &mut A, existing: Option<&A>) {
    if let Some(previous) = existing {
        preserve_account_metadata(account.account_mut(), previous.account());
        account.set_images_enabled(previous.images_enabled() && previous.account().in_pool);
        account.preserve_host_settings(previous);
    }
}

pub async fn import_account<H: ImportHost>(
    host: &H,
    item: ParsedImportItem,
    mode: ImportAuthMode,
    options: AccountImportOptions<'_>,
) -> Result<(H::Account, QuotaReadOutcome), H::Error> {
    if item.secrets().api_key().is_some() {
        return Err(H::error(
            error_codes::USE_SOURCE_IMPORT,
            "API keys must be imported as compatible API sources",
        ));
    }
    let issued_at_ms = host.now_ms();
    let label = item.label.clone();
    let tags = item.tags.clone();
    let priority = item.priority;
    let resolved = host.resolve(item, issued_at_ms).await?;
    let is_siwc = resolved.credentials.siwc_registration().is_some();
    let discovered = if options.discover_models && resolved.identity_is_registered && !is_siwc {
        Some(host.discover(&resolved, issued_at_ms).await?)
    } else {
        None
    };
    let existing = resolved.existing_account.as_ref();
    let models = if let Some(existing) = existing {
        existing.configured_models().to_vec()
    } else if !options.configured_models.is_empty() {
        options.configured_models.to_vec()
    } else {
        discovered.clone().unwrap_or_default()
    };
    let mut auth = auth_mode(mode).map_err(|(code, message)| H::error(code, message))?;
    if resolved.preserved_refresh_token
        && let Some(existing) = existing
    {
        auth = existing.account().auth_mode;
    }
    let priority = existing
        .map(|account| account.route_settings().priority)
        .or(priority)
        .unwrap_or_default();
    let mut account =
        host.new_record(&resolved.credentials, auth, models, priority, issued_at_ms)?;
    account.set_discovered_models(discovered.or_else(|| {
        existing.and_then(|account| account.discovered_models().map(<[String]>::to_vec))
    }));
    merge_existing_account(&mut account, existing);
    if resolved.credentials.oauth_client_kind() == OAuthClientKind::Codex {
        let images_enabled = account.images_enabled();
        account.account_mut().in_pool &= images_enabled;
    } else {
        account.set_images_enabled(false);
        account.account_mut().in_pool |= options.add_to_pool && !is_siwc;
    }
    if let Some(active_until_ms) = resolved.subscription_active_until_ms {
        account.account_mut().subscription = Subscription::normalize(SubscriptionInput {
            plan_type: account.account().subscription.plan_type.clone(),
            active_until_ms: Some(active_until_ms),
            forbidden: false,
            observed_at_ms: issued_at_ms,
        });
    }
    if existing.is_none() {
        if !label.trim().is_empty() {
            account.account_mut().label = label;
        }
        account.account_mut().tags = tags;
    }
    if !valid_label(&account.account().label) {
        return Err(H::error(
            error_codes::INVALID_LABEL,
            "imported account label is invalid",
        ));
    }
    account.normalize();
    let quota = if options.probe_quota && resolved.identity_is_registered && !is_siwc {
        host.probe_quota(&mut account, &resolved).await
    } else {
        QuotaReadOutcome::Skipped
    };
    host.commit(&account, &resolved).await?;
    Ok((account, quota))
}
