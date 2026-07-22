use crate::{
    app::UsageWriter,
    config::Config,
    store::{Store, Vault},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{atomic::AtomicU64, Arc, Mutex, RwLock},
};
use zenith_relay_core::{
    accounts::{AccountAuthState, AccountHealthState, TokenAuthority, TokenSet},
    protocol::Capabilities,
    quota::{QuotaSnapshot, Subscription},
    CandidateRuntimeSnapshot, GatewayRuntime, WireApi,
};

pub const SERVER_SCHEMA_VERSION: u32 = 21;
pub const MAX_SERVER_ACCOUNTS: usize = 1_024;
pub const COMMON_PROXY_SECRET_REF: &str = "proxy:common";
pub(crate) const SYSTEM_GATEWAY_KEY_ID: &str = "key_system";

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
    pub wire_api: WireApi,
    pub models: Vec<String>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub priority: i32,
    pub weight: u32,
    pub last_error_code: Option<String>,
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
    pub auth_state: AccountAuthState,
    pub health: AccountHealthState,
    pub models: Vec<String>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub priority: i32,
    pub weight: u32,
    pub subscription: Subscription,
    pub quota: QuotaSnapshot,
    pub cooldowns: BTreeMap<String, u64>,
    pub consecutive_failures: u32,
    #[serde(default)]
    pub created_at_ms: u64,
    pub last_used_at_ms: Option<u64>,
    pub last_error_code: Option<String>,
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
    pub source_ids: Option<Vec<String>>,
    pub account_ids: Option<Vec<String>>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub model_prefix: Option<String>,
    pub created_at_ms: u64,
    pub last_used_at_ms: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountCredential {
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
}

impl AccountCredential {
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
    pub(crate) failed_usage_writes: AtomicU64,
    pub(crate) usage_writer: Mutex<Option<UsageWriter>>,
    runtime: RwLock<Option<Arc<GatewayRuntime>>>,
}

impl AppState {
    pub fn new(config: Config, store: Arc<Store>, vault: Arc<Vault>) -> Result<Arc<Self>, String> {
        ensure_system_gateway_key(&store, &vault)?;
        let server_id = store.server_id()?;
        let fingerprint = identity_fingerprint(&server_id);
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
            failed_usage_writes: AtomicU64::new(0),
            usage_writer: Mutex::new(None),
            runtime: RwLock::new(None),
        }))
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

fn ensure_system_gateway_key(store: &Store, vault: &Vault) -> Result<(), String> {
    let existing = store.keys()?.into_iter().find(|key| key.system);
    let changed = existing.as_ref().is_none_or(|key| !key.enabled);
    let mut record = existing.unwrap_or_else(|| GatewayKeyRecord {
        id: SYSTEM_GATEWAY_KEY_ID.to_string(),
        label: "ChatGPT".to_string(),
        enabled: true,
        system: true,
        secret_ref: format!("key:{SYSTEM_GATEWAY_KEY_ID}"),
        source_ids: None,
        account_ids: None,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        model_prefix: None,
        created_at_ms: now_ms(),
        last_used_at_ms: None,
    });
    record.enabled = true;
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

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}
