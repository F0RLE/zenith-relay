use crate::{
    config::Config,
    store::{Store, Vault},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use zenith_relay_core::{
    accounts::{AccountAuthState, AccountHealthState, TokenAuthority, TokenSet},
    protocol::Capabilities,
    quota::{QuotaSnapshot, Subscription},
    GatewayRuntime, WireApi,
};

pub const SERVER_SCHEMA_VERSION: u32 = 2;
pub const MAX_SERVER_ACCOUNTS: usize = 512;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRecord {
    pub id: String,
    pub name: String,
    pub enabled: bool,
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
    pub last_used_at_ms: Option<u64>,
    pub last_error_code: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayKeyRecord {
    pub id: String,
    pub label: String,
    pub enabled: bool,
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
    runtime: RwLock<Option<Arc<GatewayRuntime>>>,
}

impl AppState {
    pub fn new(config: Config, store: Arc<Store>, vault: Arc<Vault>) -> Result<Arc<Self>, String> {
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
}

pub fn identity_hint(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))[..12].to_string()
}

pub fn identity_fingerprint(server_id: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("zenith-relay-server\0{server_id}").as_bytes())
    )
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}
