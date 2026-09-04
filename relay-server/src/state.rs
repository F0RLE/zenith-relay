use crate::{
    config::Config,
    store::{Store, Vault},
    usage_writer::UsageWriter,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::Rng;
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{atomic::AtomicU64, Arc, Mutex, RwLock},
};
use zenith_relay_core::{
    accounts::{AccountAuthState, AccountHealthState, TokenAuthority, TokenSet},
    pricing::{
        CatalogStatus, PriceEvidence, PricingCatalog, PricingCatalogLoader, PricingContext,
        SourcePricingMetadata,
    },
    protocol::Capabilities,
    providers::chatgpt::AgentIdentityCredential,
    quota::{QuotaSnapshot, Subscription},
    runtime_source_models_for_wire_api, runtime_source_supports_any_wire_api,
    runtime_source_supports_wire_api, ApiModelPriceOverride, CandidateRuntimeSnapshot,
    GatewayRuntime, RuntimeCandidatePolicy, RuntimeSourcePolicyRecord, RuntimeSourcePolicyUpdate,
    SourceProtocolBinding, WireApi,
};

pub use zenith_relay_core::unix_time_ms as now_ms;

pub const SERVER_SCHEMA_VERSION: u32 = 35;
pub const MAX_SERVER_ACCOUNTS: usize = 1_024;
pub const COMMON_PROXY_SECRET_REF: &str = "proxy:common";
pub(crate) const SYSTEM_GATEWAY_KEY_ID: &str = "key_system";
pub(crate) const PROFILE_KEY_ROTATION_PREFIX: &str = "key_profile_rotation_";

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProxyRecord {
    pub id: String,
    pub endpoint: String,
    pub secret_ref: String,
    pub created_at_ms: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRecord {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub in_pool: bool,
    pub draining: bool,
    pub base_url: String,
    pub secret_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub official_provider_family: Option<String>,
    pub wire_api: WireApi,
    #[serde(default)]
    pub protocol_bindings: Vec<SourceProtocolBinding>,
    pub models: Vec<String>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub priority: i32,
    pub weight: u32,
    #[serde(default)]
    pub recovery_delay_seconds: u64,
    #[serde(default)]
    pub model_price_overrides: BTreeMap<String, ApiModelPriceOverride>,
    #[serde(default)]
    pub detected_model_prices: BTreeMap<String, ApiModelPriceOverride>,
    pub last_error_code: Option<String>,
}

impl SourceRecord {
    pub fn models_for_wire_api(&self, wire_api: WireApi) -> Result<Vec<String>, String> {
        runtime_source_models_for_wire_api(
            &self.protocol_bindings,
            self.wire_api,
            &self.models,
            wire_api,
        )
        .map_err(|error| error.to_string())
    }

    pub fn supports_wire_api(&self, wire_api: WireApi) -> Result<bool, String> {
        runtime_source_supports_wire_api(
            &self.protocol_bindings,
            self.wire_api,
            &self.models,
            wire_api,
        )
        .map_err(|error| error.to_string())
    }

    pub fn supports_any_wire_api(&self) -> Result<bool, String> {
        runtime_source_supports_any_wire_api(&self.protocol_bindings, self.wire_api, &self.models)
            .map_err(|error| error.to_string())
    }
}

impl RuntimeSourcePolicyRecord for SourceRecord {
    fn runtime_source_policy_update(&self) -> RuntimeSourcePolicyUpdate {
        RuntimeSourcePolicyUpdate {
            source_id: self.id.clone(),
            policy: RuntimeCandidatePolicy {
                enabled: self.enabled,
                draining: self.draining,
                priority: self.priority,
                weight: self.weight,
                allowed_models: self.allowed_models.clone(),
                excluded_models: self.excluded_models.clone(),
            },
            recovery_delay_seconds: self.recovery_delay_seconds,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerAccountRecord {
    pub id: String,
    pub label: String,
    pub identity_hint: String,
    pub enabled: bool,
    #[serde(default)]
    pub in_pool: bool,
    pub draining: bool,
    pub source_id: String,
    pub secret_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_family: Option<String>,
    pub auth_state: AccountAuthState,
    pub health: AccountHealthState,
    pub models: Vec<String>,
    /// Last successful upstream discovery. The imported/configured `models`
    /// list is the stable baseline and is never replaced by a refresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered_models: Option<Vec<String>>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub priority: i32,
    pub weight: u32,
    pub subscription: Subscription,
    pub quota: QuotaSnapshot,
    #[serde(default)]
    pub purchase_cost_micro_usd: Option<u64>,
    pub cooldowns: BTreeMap<String, u64>,
    pub consecutive_failures: u32,
    #[serde(default)]
    pub created_at_ms: u64,
    pub last_used_at_ms: Option<u64>,
    pub last_error_code: Option<String>,
    #[serde(default)]
    pub proxy_id: Option<String>,
    #[serde(default)]
    pub bypass_common_proxy: bool,
}

impl ServerAccountRecord {
    pub fn effective_models(&self) -> &[String] {
        self.discovered_models.as_deref().unwrap_or(&self.models)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayKeyRecord {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    #[serde(default)]
    pub system: bool,
    pub secret_ref: String,
    pub created_at_ms: u64,
    pub last_used_at_ms: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountCredential {
    #[serde(default)]
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub expires_at_ms: Option<u64>,
    pub issued_at_ms: u64,
    pub generation: u64,
    pub chatgpt_account_id: String,
    pub responses_url: String,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub agent_private_key: Option<String>,
    #[serde(default)]
    pub agent_runtime_id: Option<String>,
    #[serde(default)]
    pub agent_task_id: Option<String>,
}

impl AccountCredential {
    pub fn agent_identity(&self) -> Result<Option<AgentIdentityCredential>, String> {
        match (
            self.agent_private_key.as_ref(),
            self.agent_runtime_id.as_ref(),
            self.agent_task_id.as_ref(),
        ) {
            (None, None, None) => Ok(None),
            (Some(private_key), Some(runtime_id), task_id) => match task_id {
                Some(task_id) => AgentIdentityCredential::new(
                    private_key.clone(),
                    runtime_id.clone(),
                    task_id.clone(),
                ),
                None => {
                    AgentIdentityCredential::unregistered(private_key.clone(), runtime_id.clone())
                }
            }
            .map(Some)
            .map_err(|error| error.to_string()),
            _ => Err("stored Agent Identity credential is incomplete".to_string()),
        }
    }

    pub fn is_agent_identity(&self) -> bool {
        self.agent_private_key.is_some()
            || self.agent_runtime_id.is_some()
            || self.agent_task_id.is_some()
    }

    pub fn has_oauth(&self) -> bool {
        !self.access_token.trim().is_empty()
    }

    pub fn authorization(&self, now_ms: u64) -> Result<HeaderValue, String> {
        if let Some(agent) = self.agent_identity()? {
            return agent
                .authorization(now_ms)
                .map_err(|error| error.to_string());
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", self.access_token))
            .map_err(|_| "stored account access token is invalid".to_string())?;
        authorization.set_sensitive(true);
        Ok(authorization)
    }

    pub fn tokens(&self) -> Result<TokenSet, String> {
        TokenSet::new(
            self.access_token.clone(),
            self.refresh_token.clone(),
            self.id_token.clone(),
            self.expires_at_ms,
            self.issued_at_ms,
            self.generation,
        )
        .map_err(str::to_string)
    }
}

pub struct AppState {
    pub config: Config,
    pub store: Arc<Store>,
    pub vault: Arc<Vault>,
    pub token_authority: Arc<TokenAuthority>,
    pub capabilities: Capabilities,
    pub started_at_ms: u64,
    pub wake_lock: tokio::sync::Mutex<()>,
    pub configuration_lock: tokio::sync::Mutex<()>,
    pub quota_reset_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    pub(crate) failed_usage_writes: AtomicU64,
    pub(crate) usage_writer: Mutex<Option<UsageWriter>>,
    pricing: Arc<PricingCatalogLoader>,
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
            quota_reset_locks: Mutex::new(HashMap::new()),
            failed_usage_writes: AtomicU64::new(0),
            usage_writer: Mutex::new(None),
            pricing,
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
            .map(|(model, price)| (model.to_ascii_lowercase(), price.into()))
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
            };
            let key = identity_hint(&source.id);
            source_metadata.insert(key.clone(), metadata.clone());
            source_metadata.insert(source.id.clone(), metadata);
            let mut evidence = BTreeMap::new();
            for (model, price) in source.detected_model_prices {
                evidence
                    .entry(model.to_ascii_lowercase())
                    .or_insert_with(PriceEvidence::default)
                    .provider = Some(price.into());
            }
            for (model, price) in source.model_price_overrides {
                evidence
                    .entry(model.to_ascii_lowercase())
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
        let mut locks = self
            .quota_reset_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        *self
            .runtime
            .write()
            .map_err(|_| "runtime lock poisoned".to_string())? = runtime;
        Ok(())
    }

    pub fn runtime_order(&self) -> Result<Vec<CandidateRuntimeSnapshot>, String> {
        Ok(self
            .runtime()?
            .map(|runtime| runtime.candidate_runtime_order())
            .unwrap_or_default())
    }
}

pub fn ensure_proxy_record(
    store: &Store,
    vault: &Vault,
    value: &str,
) -> Result<ServerProxyRecord, String> {
    let value = zenith_relay_core::normalize_proxy_url(value)?;
    let id = proxy_id(&value);
    if let Some(record) = store.proxy(&id)? {
        match vault.load(&record.secret_ref)? {
            Some(stored) if stored != value => {
                return Err("stored proxy reference is inconsistent".to_string())
            }
            Some(_) => {}
            None => vault.save(&record.secret_ref, &value)?,
        }
        return Ok(record);
    }
    let url = url::Url::parse(&value).map_err(|_| "stored proxy URL is invalid".to_string())?;
    let host = url
        .host_str()
        .ok_or_else(|| "stored proxy URL is invalid".to_string())?;
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let record = ServerProxyRecord {
        id: id.clone(),
        endpoint: format!(
            "{}://{}:{}",
            url.scheme(),
            host,
            url.port_or_known_default().unwrap_or_default()
        ),
        secret_ref: format!("proxy:{id}"),
        created_at_ms: now_ms(),
    };
    vault.save(&record.secret_ref, &value)?;
    if let Err(error) = store.save_proxy(&record) {
        let _ = vault.delete(&record.secret_ref);
        return Err(error);
    }
    Ok(record)
}

fn migrate_legacy_proxies(store: &Store, vault: &Vault) -> Result<(), String> {
    if store.common_proxy_id()?.is_none() && store.common_proxy_configured()? {
        if let Some(value) = vault.load(COMMON_PROXY_SECRET_REF)? {
            let proxy = ensure_proxy_record(store, vault, &value)?;
            store.set_common_proxy_id(Some(&proxy.id))?;
        }
    }
    for mut record in store.accounts()? {
        let Some(secret) = vault.load(&record.secret_ref)? else {
            continue;
        };
        let mut credential: AccountCredential = serde_json::from_str(&secret)
            .map_err(|_| "stored account credential is invalid".to_string())?;
        if record.proxy_id.is_none() {
            if let Some(value) = credential.proxy_url.as_deref() {
                record.proxy_id = Some(ensure_proxy_record(store, vault, value)?.id);
                store.save_account(&record)?;
            }
        }
        if record.proxy_id.is_some() && credential.proxy_url.take().is_some() {
            vault.save(
                &record.secret_ref,
                &serde_json::to_string(&credential)
                    .map_err(|_| "stored account credential is invalid".to_string())?,
            )?;
        }
    }
    Ok(())
}

fn retire_user_gateway_keys(store: &Store, vault: &Vault) -> Result<(), String> {
    let keys = store.keys()?;
    let retained_secret_refs = keys
        .iter()
        .filter(|key| key.id == SYSTEM_GATEWAY_KEY_ID || is_internal_gateway_key(key))
        .map(|key| key.secret_ref.clone())
        .collect::<HashSet<_>>();
    let retired = keys
        .into_iter()
        .filter(|key| {
            key.id != SYSTEM_GATEWAY_KEY_ID
                && !is_internal_gateway_key(key)
                && !retained_secret_refs.contains(&key.secret_ref)
        })
        .collect::<Vec<_>>();
    if retired.is_empty() {
        return Ok(());
    }

    let mut secret_refs = HashSet::new();
    let mut secrets = Vec::new();
    for key in &retired {
        if !secret_refs.insert(key.secret_ref.clone()) {
            continue;
        }
        secrets.push((key.secret_ref.clone(), vault.load(&key.secret_ref)?));
    }
    let ids = retired.iter().map(|key| key.id.clone()).collect::<Vec<_>>();
    let mut attempted_refs = Vec::new();
    for (secret_ref, _) in &secrets {
        attempted_refs.push(secret_ref.clone());
        if let Err(error) = vault.delete(secret_ref) {
            return Err(rollback_retired_gateway_keys(
                vault,
                &secrets,
                &attempted_refs,
                format!("legacy gateway credential cleanup failed: {error}"),
            ));
        }
    }
    if let Err(error) = store.delete_keys(&ids) {
        return Err(rollback_retired_gateway_keys(
            vault,
            &secrets,
            &attempted_refs,
            format!("legacy gateway credential records could not be removed: {error}"),
        ));
    }
    Ok(())
}

fn rollback_retired_gateway_keys(
    vault: &Vault,
    secrets: &[(String, Option<String>)],
    attempted_refs: &[String],
    cause: String,
) -> String {
    let mut failures = Vec::new();
    for (secret_ref, secret) in secrets {
        if !attempted_refs.iter().any(|value| value == secret_ref) {
            continue;
        }
        if let Some(secret) = secret {
            if let Err(error) = vault.save(secret_ref, secret) {
                failures.push(format!("secret restore failed: {error}"));
            }
        }
    }
    if failures.is_empty() {
        cause
    } else {
        format!("{cause}; cleanup rollback failed: {}", failures.join("; "))
    }
}

pub(crate) fn is_internal_gateway_key(key: &GatewayKeyRecord) -> bool {
    key.system
        && (key.id == SYSTEM_GATEWAY_KEY_ID || key.id.starts_with(PROFILE_KEY_ROTATION_PREFIX))
}

fn ensure_system_gateway_key(store: &Store, vault: &Vault) -> Result<(), String> {
    let existing = store
        .keys()?
        .into_iter()
        .find(|key| key.id == SYSTEM_GATEWAY_KEY_ID);
    let changed = existing
        .as_ref()
        .is_none_or(|key| !key.enabled || !key.system);
    let mut record = existing.unwrap_or_else(|| GatewayKeyRecord {
        id: SYSTEM_GATEWAY_KEY_ID.to_string(),
        label: "ChatGPT".to_string(),
        enabled: true,
        system: true,
        secret_ref: format!("key:{SYSTEM_GATEWAY_KEY_ID}"),
        created_at_ms: now_ms(),
        last_used_at_ms: None,
    });
    record.enabled = true;
    record.system = true;
    let created_secret = vault.load(&record.secret_ref)?.is_none();
    if created_secret {
        vault.save(&record.secret_ref, &generate_pool_key())?;
    }
    if changed {
        if let Err(error) = store.save_key(&record) {
            if created_secret {
                let _ = vault.delete(&record.secret_ref);
            }
            return Err(error);
        }
    }
    Ok(())
}

pub(crate) fn generate_pool_key() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("zrs_{}", URL_SAFE_NO_PAD.encode(bytes))
}

pub fn identity_hint(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))[..12].to_string()
}

pub fn identity_fingerprint(server_id: &str) -> String {
    hex::encode(Sha256::digest(
        format!("zenith-relay-server\0{server_id}").as_bytes(),
    ))
}

pub fn proxy_id(value: &str) -> String {
    zenith_relay_core::proxy_reference_id(value).expect("stored proxy URL was normalized")
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
