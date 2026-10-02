use super::DesktopState;
use crate::{
    local_pool::{
        accounts::{
            credentials::{CredentialStore, StoredCodexCredentials},
            proxy::ProxyRoute,
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
use zenith_relay_core::error_codes;
use zenith_relay_core::{
    protocol::{AccountRefreshState, RefreshStatus, SourceRefreshState},
    scheduler::refresh::RefreshKind,
};

pub(super) trait SecretLookup {
    fn load(&self, secret_ref: &str) -> Result<Option<String>>;

    fn contains(&self, secret_ref: &str) -> Result<bool> {
        Ok(self.load(secret_ref)?.is_some())
    }
}

struct OsSecretLookup;

impl SecretLookup for OsSecretLookup {
    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        crate::local_pool::store::secret_store::load(secret_ref)
    }

    fn contains(&self, secret_ref: &str) -> Result<bool> {
        crate::local_pool::store::secret_store::contains(secret_ref)
    }
}

pub(super) struct CredentialCache {
    generation: u64,
    values: HashMap<String, Option<StoredCodexCredentials>>,
}

impl Default for CredentialCache {
    fn default() -> Self {
        Self {
            generation: 0,
            values: HashMap::new(),
        }
    }
}

pub(crate) struct LocalRuntimeInputs {
    pub gateway: GatewaySettings,
    pub sources: Vec<ProviderSourceRecord>,
    pub accounts: Vec<LocalAccountRecord>,
    pub source_api_keys: BTreeMap<String, Option<String>>,
    pub account_credentials: HashMap<String, Option<StoredCodexCredentials>>,
}

pub(crate) struct SourceRefreshSnapshot {
    pub revision: u64,
    pub stats: Option<zenith_relay_core::SourceProviderStats>,
    pub state: SourceRefreshState,
}

/// Fields the desktop snapshot needs from an account secret. Token material
/// stays in the credential cache and is not copied into each UI refresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AccountCredentialFacts {
    pub has_oauth: bool,
    pub agent_identity: bool,
    pub has_provider_account_id: bool,
    pub has_account_proxy: bool,
    pub account_proxy_valid: bool,
    pub bypass_common_proxy: bool,
}

impl AccountCredentialFacts {
    pub(crate) fn from_stored(credentials: &StoredCodexCredentials) -> Self {
        let proxy_url = credentials.proxy_url();
        Self {
            has_oauth: credentials.has_oauth(),
            agent_identity: credentials.is_agent_identity(),
            has_provider_account_id: credentials.provider_account_id().is_some(),
            has_account_proxy: proxy_url.is_some(),
            account_proxy_valid: proxy_url
                .is_some_and(|value| zenith_relay_core::ProxyConfig::parse(value).is_ok()),
            bypass_common_proxy: credentials.bypass_common_proxy(),
        }
    }

    pub(crate) fn basis_points_available(self) -> bool {
        self.has_oauth && !self.agent_identity
    }

    pub(crate) fn proxy_route(self) -> ProxyRoute {
        ProxyRoute {
            has_account_proxy: self.has_account_proxy,
            account_proxy_valid: self.account_proxy_valid,
            bypass_common_proxy: self.bypass_common_proxy,
        }
    }
}

/// UI projection inputs. Source keys are presence checks, not decrypted copies.
pub(crate) struct SnapshotInputs {
    pub gateway: GatewaySettings,
    pub sources: Vec<ProviderSourceRecord>,
    pub accounts: Vec<LocalAccountRecord>,
    pub automations: AutomationRecords,
    pub warnings: Vec<String>,
    pub running: bool,
    pub source_secret_available: BTreeMap<String, bool>,
    pub source_refresh: BTreeMap<String, SourceRefreshSnapshot>,
    pub account_refresh: BTreeMap<String, AccountRefreshState>,
    pub account_facts: HashMap<String, Option<AccountCredentialFacts>>,
}

struct SnapshotBase {
    gateway: GatewaySettings,
    sources: Vec<ProviderSourceRecord>,
    source_refresh: BTreeMap<String, SourceRefreshSnapshot>,
    account_refresh: BTreeMap<String, AccountRefreshState>,
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
            source_refresh: _,
            account_refresh: _,
            accounts,
            automations,
            mut warnings,
            running,
        } = self.snapshot_base().await?;
        for source in &sources {
            if !secrets.contains(&source.secret_ref)? {
                warnings.push(warning_code(error_codes::SOURCE_SECRET_MISSING, &source.id));
            }
        }
        for account in &accounts {
            if !account_secret_available(account, secrets)? {
                warnings.push(warning_code(
                    error_codes::ACCOUNT_SECRET_MISSING,
                    &account.account.id,
                ));
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
            ..
        } = self.snapshot_base().await?;
        let source_api_keys = sources
            .iter()
            .map(|source| {
                secret_store::load(&source.secret_ref).map(|value| (source.id.clone(), value))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let account_credentials = self.cached_account_credentials(&accounts)?;
        Ok(LocalRuntimeInputs {
            gateway,
            sources,
            accounts,
            source_api_keys,
            account_credentials,
        })
    }

    pub(crate) async fn snapshot_inputs(&self) -> Result<SnapshotInputs> {
        let SnapshotBase {
            gateway,
            sources,
            source_refresh,
            account_refresh,
            accounts,
            automations,
            mut warnings,
            running,
        } = self.snapshot_base().await?;
        let source_secret_available = sources
            .iter()
            .map(|source| {
                secret_store::contains(&source.secret_ref)
                    .map(|present| (source.id.clone(), present))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let account_facts = self.cached_account_credential_facts(&accounts)?;
        for source in &sources {
            if !source_secret_available
                .get(&source.id)
                .copied()
                .unwrap_or(false)
            {
                warnings.push(warning_code(error_codes::SOURCE_SECRET_MISSING, &source.id));
            }
        }
        for account in &accounts {
            if account_facts
                .get(&account.account.id)
                .copied()
                .flatten()
                .is_none()
            {
                warnings.push(warning_code(
                    error_codes::ACCOUNT_SECRET_MISSING,
                    &account.account.id,
                ));
            }
        }
        Ok(SnapshotInputs {
            gateway,
            sources,
            accounts,
            automations,
            warnings,
            running,
            source_secret_available,
            source_refresh,
            account_refresh,
            account_facts,
        })
    }

    fn cached_account_credentials(
        &self,
        accounts: &[LocalAccountRecord],
    ) -> Result<HashMap<String, Option<StoredCodexCredentials>>> {
        self.project_cached_credentials(accounts, Clone::clone)
    }

    fn cached_account_credential_facts(
        &self,
        accounts: &[LocalAccountRecord],
    ) -> Result<HashMap<String, Option<AccountCredentialFacts>>> {
        self.project_cached_credentials(accounts, |credentials| {
            credentials
                .as_ref()
                .map(AccountCredentialFacts::from_stored)
        })
    }

    fn project_cached_credentials<T>(
        &self,
        accounts: &[LocalAccountRecord],
        project: impl Fn(&Option<StoredCodexCredentials>) -> T,
    ) -> Result<HashMap<String, T>> {
        let generation = secret_store::generation();
        let mut cache = self.credential_cache.lock().map_err(|_| {
            LocalPoolError::new(ErrorCode::Io, "credential cache lock is unavailable")
        })?;
        if cache.generation != generation {
            cache.values.clear();
            cache.generation = generation;
        }
        let credential_store = CredentialStore::from_backend(NativeSecretBackend);
        let mut loaded = HashMap::with_capacity(accounts.len());
        for account in accounts {
            let id = &account.account.id;
            if let Some(credentials) = cache.values.get(id) {
                loaded.insert(id.clone(), project(credentials));
                continue;
            }
            let credentials = credential_store.load(id).map_err(|error| {
                LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error.to_string())
            })?;
            let projected = project(&credentials);
            cache.values.insert(id.clone(), credentials);
            loaded.insert(id.clone(), projected);
        }
        let live = accounts
            .iter()
            .map(|account| account.account.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        cache.values.retain(|id, _| live.contains(id.as_str()));
        if secret_store::generation() != generation {
            cache.generation = 0;
            cache.values.clear();
        }
        Ok(loaded)
    }

    async fn snapshot_base(&self) -> Result<SnapshotBase> {
        let running = self.gateway.address().await.is_some();
        let (gateway, sources, source_refresh, accounts, account_refresh, automations) = {
            let store = self.store()?;
            let source_refresh = store
                .sources()
                .iter()
                .map(|source| {
                    let (_, fence) = store.source_refresh_scope(&source.id)?;
                    let stats = crate::local_pool::refresh::sources::cached_stats(
                        self,
                        &fence,
                        &source.base_url,
                    );
                    Ok((
                        source.id.clone(),
                        SourceRefreshSnapshot {
                            revision: fence.revision(),
                            stats,
                            state: SourceRefreshState {
                                models: RefreshStatus::from_evidence(
                                    self.refresh
                                        .freshness(&fence.identity(), RefreshKind::Models),
                                    !source.models.is_empty(),
                                ),
                                balance: RefreshStatus::from_evidence(
                                    self.refresh
                                        .freshness(&fence.identity(), RefreshKind::Balance),
                                    false,
                                ),
                            },
                        },
                    ))
                })
                .collect::<Result<_>>()?;
            let account_refresh = store
                .accounts()
                .iter()
                .map(|record| {
                    let (_, fence) = store.account_refresh_scope(&record.account.id)?;
                    Ok((
                        record.account.id.clone(),
                        AccountRefreshState {
                            models: RefreshStatus::from_evidence(
                                self.refresh
                                    .freshness(&fence.identity(), RefreshKind::Models),
                                !record.effective_models().is_empty(),
                            ),
                            quota: RefreshStatus::from_evidence(
                                self.refresh
                                    .freshness(&fence.identity(), RefreshKind::Quota),
                                record.account.quota.updated_at_ms.is_some(),
                            ),
                        },
                    ))
                })
                .collect::<Result<_>>()?;
            (
                store.gateway().clone(),
                store.sources().to_vec(),
                source_refresh,
                store.accounts().to_vec(),
                account_refresh,
                store.automations().clone(),
            )
        };
        let mut warnings = Vec::new();
        if self.failed_usage_writes.load(Ordering::Relaxed) > 0 {
            warnings.push(error_codes::USAGE_PERSISTENCE_FAILED.to_string());
        }
        if self.failed_affinity_writes.load(Ordering::Relaxed) > 0 {
            warnings.push(error_codes::RESPONSE_AFFINITY_PERSISTENCE_FAILED.to_string());
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
            source_refresh,
            accounts,
            account_refresh,
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
    Ok(secrets.contains(&secret_ref)?)
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
