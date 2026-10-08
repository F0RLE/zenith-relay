use crate::{
    config::Config,
    store::{Store, Vault},
    usage_writer::UsageWriter,
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{atomic::AtomicU64, Arc, Mutex, RwLock},
};
use zenith_relay_core::model_id_key;
use zenith_relay_core::{
    accounts::TokenAuthority,
    model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogLoader},
    pricing::{
        CatalogStatus, PriceEvidence, PricingCatalog, PricingCatalogLoader, PricingContext,
        SourcePricingMetadata,
    },
    protocol::Capabilities,
    CandidateRuntimeSnapshot, GatewayRuntime,
};

pub use zenith_relay_core::unix_time_ms as now_ms;

pub const SERVER_SCHEMA_VERSION: u32 = 41;
pub const MAX_SERVER_ACCOUNTS: usize = 1_024;
pub const COMMON_PROXY_SECRET_REF: &str = "proxy:common";
pub(crate) const SYSTEM_GATEWAY_KEY_ID: &str = "key_system";
pub(crate) const PROFILE_KEY_ROTATION_PREFIX: &str = "key_profile_rotation_";

mod records;
mod startup;

pub use records::{
    AccountCredential, GatewayKeyRecord, ServerAccountRecord, ServerProxyRecord, SourceRecord,
};
pub use startup::{ensure_proxy_record, identity_fingerprint, identity_hint, proxy_id};
use startup::{ensure_system_gateway_key, migrate_legacy_proxies, retire_user_gateway_keys};
pub(crate) use startup::{generate_pool_key, is_internal_gateway_key};

pub struct AppState {
    pub config: Config,
    pub store: Arc<Store>,
    pub vault: Arc<Vault>,
    pub token_authority: Arc<TokenAuthority>,
    pub capabilities: Capabilities,
    pub started_at_ms: u64,
    pub wake_lock: tokio::sync::Mutex<()>,
    pub configuration_lock: tokio::sync::Mutex<()>,
    /// Serializes runtime construction/publication with account incarnation
    /// switches. A stale build must finish before an import/delete commits.
    pub(crate) runtime_build_lock: tokio::sync::Mutex<()>,
    /// Serializes credential-reference switches with vault token/task writes.
    /// Separate from configuration_lock: runtime rebuild may await the token
    /// authority while holding configuration_lock.
    pub(crate) account_credential_lock: tokio::sync::Mutex<()>,
    pub quota_reset_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    pub(crate) refresh: Arc<
        zenith_relay_core::scheduler::refresh::service::RefreshService<
            crate::jobs::RefreshReadResult,
        >,
    >,
    pub(crate) failed_usage_writes: AtomicU64,
    pub(crate) usage_writer: Mutex<Option<UsageWriter>>,
    pricing: Arc<PricingCatalogLoader>,
    model_metadata: Arc<ModelMetadataCatalogLoader>,
    runtime: RwLock<Option<Arc<GatewayRuntime>>>,
}

impl AppState {
    pub fn new(config: Config, store: Arc<Store>, vault: Arc<Vault>) -> Result<Arc<Self>, String> {
        migrate_legacy_proxies(&store, &vault)?;
        retire_user_gateway_keys(&store, &vault)?;
        ensure_system_gateway_key(&store, &vault)?;
        let server_id = store.server_id()?;
        let fingerprint = identity_fingerprint(&server_id);
        let pricing = Arc::new(
            PricingCatalogLoader::open(config.data_dir.join("litellm-prices.json"))
                .map_err(|error| error.to_string())?,
        );
        let model_metadata = Arc::new(
            ModelMetadataCatalogLoader::open(config.data_dir.join("models-dev.json"))
                .map_err(|error| error.to_string())?,
        );
        Ok(Arc::new(Self {
            config,
            store,
            vault,
            token_authority: Arc::new(
                TokenAuthority::new(MAX_SERVER_ACCOUNTS).map_err(|error| error.to_string())?,
            ),
            capabilities: Capabilities::personal_server(server_id, fingerprint),
            started_at_ms: now_ms(),
            wake_lock: tokio::sync::Mutex::new(()),
            configuration_lock: tokio::sync::Mutex::new(()),
            runtime_build_lock: tokio::sync::Mutex::new(()),
            account_credential_lock: tokio::sync::Mutex::new(()),
            quota_reset_locks: Mutex::new(HashMap::new()),
            refresh:
                zenith_relay_core::scheduler::refresh::service::RefreshService::with_cache_policy(
                    Default::default(),
                    crate::jobs::cache_observation,
                )
                .map_err(str::to_string)?,
            failed_usage_writes: AtomicU64::new(0),
            usage_writer: Mutex::new(None),
            pricing,
            model_metadata,
            runtime: RwLock::new(None),
        }))
    }

    pub(crate) fn pricing_loader(&self) -> Arc<PricingCatalogLoader> {
        self.pricing.clone()
    }

    pub(crate) fn pricing_catalog(&self) -> Arc<PricingCatalog> {
        self.pricing.snapshot()
    }

    pub(crate) fn pricing_status(&self) -> CatalogStatus {
        self.pricing.status()
    }

    pub(crate) fn model_metadata_loader(&self) -> Arc<ModelMetadataCatalogLoader> {
        self.model_metadata.clone()
    }

    pub(crate) fn model_metadata_catalog(&self) -> Arc<ModelMetadataCatalog> {
        self.model_metadata.snapshot()
    }

    /// Build a redacted pricing identity map for usage and snapshot reads.
    /// Usage storage keeps only identity hints, so this context deliberately
    /// contains no credentials or provider response data.
    pub(crate) fn pricing_context(&self) -> Result<PricingContext, String> {
        let sources = self.store.sources()?;
        let accounts = self.store.accounts()?;
        let global_manual_prices = self
            .store
            .model_price_overrides()?
            .into_iter()
            .map(|(model, price)| (model_id_key(&model), price.into()))
            .collect();
        let mut account_provider_families = BTreeMap::new();
        for account in accounts {
            let family = account
                .provider_family
                .unwrap_or_else(|| "openai".to_string());
            // Usage rows use the redacted identity hint, while snapshot model
            // projections address the same candidate by its durable id. Keep
            // both aliases in-memory so neither path loses the family.
            account_provider_families.insert(identity_hint(&account.id), family.clone());
            account_provider_families.insert(account.id, family);
        }
        let mut source_metadata = BTreeMap::new();
        let mut source_evidence = BTreeMap::new();
        for source in sources {
            let metadata = SourcePricingMetadata {
                pricing_provider: source.pricing_provider.clone(),
                official_provider_family: source.official_provider_family.clone(),
                cache_write_models: source.models_with_cache_write_pricing(),
            };
            let key = identity_hint(&source.id);
            source_metadata.insert(key.clone(), metadata.clone());
            source_metadata.insert(source.id.clone(), metadata);
            let mut evidence = BTreeMap::new();
            for (model, price) in source.detected_model_prices {
                evidence
                    .entry(model_id_key(&model))
                    .or_insert_with(PriceEvidence::default)
                    .provider = Some(price.into());
            }
            for (model, price) in source.model_price_overrides {
                evidence
                    .entry(model_id_key(&model))
                    .or_insert_with(PriceEvidence::default)
                    .manual = Some(price.into());
            }
            source_evidence.insert(key, evidence.clone());
            source_evidence.insert(source.id, evidence);
        }
        Ok(PricingContext {
            account_provider_families,
            source_metadata,
            source_evidence,
            global_manual_prices,
        })
    }

    pub(crate) fn quota_reset_lock(&self, account_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = zenith_relay_core::poison::mutex(&self.quota_reset_locks);
        locks
            .entry(account_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    pub fn runtime(&self) -> Result<Option<Arc<GatewayRuntime>>, String> {
        self.runtime
            .read()
            .map(|runtime| runtime.clone())
            .map_err(|_| "runtime lock poisoned".to_string())
    }

    pub fn replace_runtime(&self, runtime: Option<Arc<GatewayRuntime>>) -> Result<(), String> {
        let mut active = self
            .runtime
            .write()
            .map_err(|_| "runtime lock poisoned".to_string())?;
        if let Some(previous_runtime) = active.as_ref() {
            if runtime.as_ref().is_none_or(|replacement_runtime| {
                !Arc::ptr_eq(previous_runtime, replacement_runtime)
            }) {
                previous_runtime.retire_for_replacement();
            }
        }
        *active = runtime;
        Ok(())
    }

    pub fn runtime_order(&self) -> Result<Vec<CandidateRuntimeSnapshot>, String> {
        Ok(self
            .runtime()?
            .map(|runtime| runtime.candidate_runtime_order_for_key(SYSTEM_GATEWAY_KEY_ID))
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_credential_keeps_oauth_as_agent_identity_fallback() {
        let credential = AccountCredential {
            access_token: "oauth-access".into(),
            refresh_token: Some("oauth-refresh".into()),
            id_token: None,
            expires_at_ms: None,
            issued_at_ms: 1,
            generation: 2,
            chatgpt_account_id: "provider-account".into(),
            responses_url: "https://chatgpt.com/backend-api/codex/responses".into(),
            proxy_url: None,
            agent_private_key: Some(
                "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g".into(),
            ),
            agent_runtime_id: Some("runtime-test".into()),
            agent_task_id: Some("task-test".into()),
        };

        assert!(credential.has_oauth());
        assert_eq!(credential.tokens().unwrap().access_token(), "oauth-access");
        assert!(credential
            .authorization(1_785_000_000_000)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("AgentAssertion "));
    }
}
