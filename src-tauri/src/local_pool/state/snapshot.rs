use super::DesktopState;
use crate::{
    local_pool::{
        accounts::{
            credentials::{CredentialStore, StoredCodexCredentials},
            NativeSecretBackend,
        },
        error::{ErrorCode, LocalPoolError, Result},
        models::{
            AutomationRecords, GatewaySettings, LocalAccountRecord, LocalPoolSnapshot,
            ProviderSourceRecord, RuntimeTarget,
        },
        store::secret_store,
    },
    platform,
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::atomic::Ordering,
};

pub(super) trait SecretLookup {
    fn load(&self, secret_ref: &str) -> Result<Option<String>>;
}

struct OsSecretLookup;

impl SecretLookup for OsSecretLookup {
    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        crate::local_pool::store::secret_store::load(secret_ref)
    }
}

pub(crate) struct LocalRuntimeInputs {
    pub gateway: GatewaySettings,
    pub sources: Vec<ProviderSourceRecord>,
    pub accounts: Vec<LocalAccountRecord>,
    pub automations: AutomationRecords,
    pub warnings: Vec<String>,
    pub running: bool,
    pub source_api_keys: BTreeMap<String, Option<String>>,
    pub account_credentials: HashMap<String, Option<StoredCodexCredentials>>,
}

struct SnapshotBase {
    gateway: GatewaySettings,
    sources: Vec<ProviderSourceRecord>,
    accounts: Vec<LocalAccountRecord>,
    automations: AutomationRecords,
    warnings: Vec<String>,
    running: bool,
}

impl DesktopState {
    pub async fn snapshot(&self) -> Result<LocalPoolSnapshot> {
        self.snapshot_with(&OsSecretLookup).await
    }

    pub(super) async fn snapshot_with(
        &self,
        secrets: &impl SecretLookup,
    ) -> Result<LocalPoolSnapshot> {
        let SnapshotBase {
            gateway,
            sources,
            accounts,
            automations,
            mut warnings,
            running,
        } = self.snapshot_base().await?;
        for source in &sources {
            if secrets.load(&source.secret_ref)?.is_none() {
                warnings.push(warning_code("source_secret_missing", &source.id));
            }
        }
        for account in &accounts {
            if !account_secret_available(account, secrets)? {
                warnings.push(warning_code("account_secret_missing", &account.account.id));
            }
        }
        Ok(LocalPoolSnapshot {
            schema_version: crate::local_pool::models::CURRENT_SCHEMA_VERSION,
            runtime_target: RuntimeTarget {
                kind: "local",
                connected: running,
            },
            gateway,
            platform: platform::platform_name(),
            capabilities: platform::capabilities(),
            sources,
            accounts,
            automations: automations.tasks,
            wake_history: automations.state.history().iter().cloned().collect(),
            warnings,
        })
    }

    pub(crate) async fn runtime_inputs(&self) -> Result<LocalRuntimeInputs> {
        let SnapshotBase {
            gateway,
            sources,
            accounts,
            automations,
            mut warnings,
            running,
        } = self.snapshot_base().await?;
        let source_api_keys = sources
            .iter()
            .map(|source| {
                secret_store::load(&source.secret_ref).map(|value| (source.id.clone(), value))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let credential_store = CredentialStore::from_backend(NativeSecretBackend);
        let account_credentials = accounts
            .iter()
            .map(|account| {
                credential_store
                    .load(&account.account.id)
                    .map(|credentials| (account.account.id.clone(), credentials))
                    .map_err(|error| {
                        LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error.to_string())
                    })
            })
            .collect::<std::result::Result<HashMap<_, _>, _>>()?;
        for source in &sources {
            if source_api_keys
                .get(&source.id)
                .and_then(Option::as_ref)
                .is_none()
            {
                warnings.push(warning_code("source_secret_missing", &source.id));
            }
        }
        for account in &accounts {
            if account_credentials
                .get(&account.account.id)
                .and_then(Option::as_ref)
                .is_none()
            {
                warnings.push(warning_code("account_secret_missing", &account.account.id));
            }
        }
        Ok(LocalRuntimeInputs {
            gateway,
            sources,
            accounts,
            automations,
            warnings,
            running,
            source_api_keys,
            account_credentials,
        })
    }

    async fn snapshot_base(&self) -> Result<SnapshotBase> {
        let running = self.gateway.address().await.is_some();
        let (gateway, sources, accounts, automations) = {
            let store = self.store()?;
            (
                store.gateway().clone(),
                store.sources().to_vec(),
                store.accounts().to_vec(),
                store.automations().clone(),
            )
        };
        let mut warnings = Vec::new();
        if self.failed_usage_writes.load(Ordering::Relaxed) > 0 {
            warnings.push("usage_persistence_failed".to_string());
        }
        if self.failed_affinity_writes.load(Ordering::Relaxed) > 0 {
            warnings.push("response_affinity_persistence_failed".to_string());
        }
        if gateway.enabled && !running {
            warnings.push("gateway_configured_but_not_running".to_string());
        }
        if let Some(error) = self.catalog_refresh_warning() {
            warnings.push(error);
        }
        Ok(SnapshotBase {
            gateway,
            sources,
            accounts,
            automations,
            warnings,
            running,
        })
    }
}

pub(super) fn account_secret_available(
    account: &LocalAccountRecord,
    secrets: &impl SecretLookup,
) -> Result<bool> {
    let secret_ref =
        crate::local_pool::accounts::credentials::credential_secret_ref(&account.account.id)
            .map_err(LocalPoolError::invalid_state)?;
    Ok(secrets.load(&secret_ref)?.is_some())
}

fn warning_code(code: &str, id: &str) -> String {
    let id = id.trim();
    let redacted = if id.chars().count() <= 12 {
        id.to_string()
    } else {
        format!("{}...", id.chars().take(8).collect::<String>())
    };
    format!("{code}:{redacted}")
}
