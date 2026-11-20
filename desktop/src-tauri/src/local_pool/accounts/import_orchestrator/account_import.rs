use super::{
    ImportItemError, ImportRowContext, ItemResult, build_import_credential_material,
    credential_item_error, model_item_error, persist_imported_account, proxy_item_error,
};
use crate::local_pool::{
    accounts::{
        NativeSecretBackend,
        credentials::{CredentialStore, StoredCodexCredentials},
        proxy::ensure_account_proxy,
        quota_refresh::AccountQuotaOutcome,
        records,
    },
    models::{GatewaySettings, LocalAccountRecord},
    runtime::current_time_ms,
    state::DesktopState,
};
use zenith_relay_core::{
    accounts::{AccountAuthMode, ParsedImportItem},
    error_codes,
    providers::chatgpt::{
        AccountModelsClient,
        connector::import::{self as shared, ImportHost, ImportRecord, ResolvedImport},
    },
};

mod quota_probe;
mod reconcile;
use quota_probe::probe_import_quota;
pub(super) use quota_probe::{account_import_proxy, import_proxy};
use reconcile::reconcile_imported_credentials;
pub(super) use shared::AccountImportOptions;

pub(super) async fn import_account_item(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    import_item: ParsedImportItem,
    context: &ImportRowContext,
    options: AccountImportOptions<'_>,
    account_check_endpoint: &url::Url,
) -> ItemResult<(LocalAccountRecord, AccountQuotaOutcome)> {
    let settings = state
        .store()
        .map_err(|_| {
            ImportItemError::new(
                error_codes::ACCOUNT_STORE_FAILED,
                "account store is unavailable",
            )
        })?
        .gateway()
        .clone();
    let item_hash = crate::diagnostics::hash_identifier(&import_item.item_id);
    crate::diagnostics::breadcrumb(
        "account-import",
        "item_processing_started",
        &[("item", item_hash.clone())],
    );
    let result = shared::import_account(
        &DesktopImport {
            state,
            credential_store,
            context,
            settings,
            account_check_endpoint,
        },
        import_item,
        context.auth_mode,
        options,
    )
    .await?;
    crate::diagnostics::breadcrumb(
        "account-import",
        "item_processing_completed",
        &[("item", item_hash)],
    );
    Ok(result)
}

struct DesktopImport<'a> {
    state: &'a DesktopState,
    credential_store: &'a CredentialStore<NativeSecretBackend>,
    context: &'a ImportRowContext,
    settings: GatewaySettings,
    account_check_endpoint: &'a url::Url,
}

impl ImportRecord for LocalAccountRecord {
    fn configured_models(&self) -> &[String] {
        &self.models
    }
    fn discovered_models(&self) -> Option<&[String]> {
        self.discovered_models.as_deref()
    }
    fn set_discovered_models(&mut self, models: Option<Vec<String>>) {
        self.discovered_models = models;
    }
    fn preserve_host_settings(&mut self, previous: &Self) {
        self.recovery = previous.recovery.clone();
        self.allowed_models = previous.allowed_models.clone();
        self.excluded_models = previous.excluded_models.clone();
        self.priority = previous.priority;
        self.weight = previous.weight;
    }
    fn normalize(&mut self) {
        LocalAccountRecord::normalize(self);
    }
}

impl ImportHost for DesktopImport<'_> {
    type Account = LocalAccountRecord;
    type Error = ImportItemError;
    fn error(code: &'static str, message: &'static str) -> Self::Error {
        ImportItemError::new(code, message)
    }
    fn now_ms(&self) -> u64 {
        current_time_ms()
    }

    async fn resolve(
        &self,
        item: ParsedImportItem,
        issued_at_ms: u64,
    ) -> ItemResult<ResolvedImport<Self::Account>> {
        let proxy = import_proxy(self.state, self.credential_store, &self.settings, &item)?;
        ensure_account_proxy(&self.settings, proxy.as_ref()).map_err(proxy_item_error)?;
        let bundle_proxy_url = item.bundle_proxy_url().map(str::to_owned);
        let material = build_import_credential_material(
            item,
            issued_at_ms,
            self.context.plan.as_deref(),
            self.context.subscription_active_until_ms,
            proxy.as_ref(),
            self.settings.quota_request_timeout_seconds,
            self.account_check_endpoint,
        )
        .await?;
        reconcile_imported_credentials(
            self.state,
            self.credential_store,
            &self.settings,
            issued_at_ms,
            material,
            bundle_proxy_url,
        )
    }

    async fn discover(
        &self,
        resolved: &ResolvedImport<Self::Account>,
        issued_at_ms: u64,
    ) -> ItemResult<Vec<String>> {
        let credentials = &resolved.credentials;
        let client = AccountModelsClient::new_with_proxy(resolved.proxy.as_ref())
            .map_err(model_item_error)?
            .with_oauth_client_kind(credentials.oauth_client_kind());
        let client_version =
            zenith_relay_core::providers::chatgpt::configured_codex_client_version();
        let discovery = client
            .discover_authorized_with_diagnostics(
                credentials
                    .authorization(issued_at_ms)
                    .map_err(credential_item_error)?,
                &resolved.provider_account_id,
                &client_version,
                crate::diagnostics::is_debug_enabled(),
            )
            .await
            .map_err(|error| {
                crate::local_pool::accounts::model_probe::record_failure(
                    credentials.local_account_id(),
                    credentials.oauth_client_kind(),
                    resolved.proxy.is_some(),
                    &error,
                );
                model_item_error(error)
            })?;
        if let Some(catalog) = &discovery.basis_points_diagnostics {
            crate::local_pool::accounts::model_probe::record_catalog(
                credentials.local_account_id(),
                resolved.proxy.is_some(),
                catalog,
            );
        }
        Ok(discovery.models)
    }

    fn new_record(
        &self,
        credentials: &StoredCodexCredentials,
        auth: AccountAuthMode,
        models: Vec<String>,
        priority: i32,
        issued_at_ms: u64,
    ) -> ItemResult<Self::Account> {
        records::new_account_record(credentials, auth, models, priority, issued_at_ms).map_err(
            |_| {
                ImportItemError::new(
                    error_codes::INVALID_ACCOUNT,
                    "imported account record is invalid",
                )
            },
        )
    }

    async fn probe_quota(
        &self,
        account: &mut Self::Account,
        resolved: &ResolvedImport<Self::Account>,
    ) -> AccountQuotaOutcome {
        probe_import_quota(
            account,
            &resolved.credentials,
            resolved.proxy.as_ref(),
            self.settings.quota_request_timeout_seconds,
        )
        .await
    }

    async fn commit(
        &self,
        account: &Self::Account,
        resolved: &ResolvedImport<Self::Account>,
    ) -> ItemResult<()> {
        persist_imported_account(
            self.state,
            self.credential_store,
            &resolved.credentials,
            resolved.old_credential.as_ref(),
            account.clone(),
        )
        .await
    }
}
