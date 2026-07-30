use super::agent_identity::AgentIdentityCredential;
use crate::accounts::{TokenAuthority, TokenPersistenceAdapter, TokenRefreshAdapter};
use crate::quota::QuotaSnapshot;
use crate::{CandidateHealth, CandidateQuota, ProxyConfig};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;

/// Runtime configuration for a ChatGPT/Codex account.
///
/// The pool and scheduler only consume the normalized candidate fields. The
/// ChatGPT account id and Responses endpoint stay inside this provider adapter
/// so another provider can supply a different runtime shape later.
#[derive(Clone)]
pub struct RuntimeChatGptAccount {
    pub id: String,
    pub source_id: String,
    pub chatgpt_account_id: String,
    pub responses_url: String,
    pub models: Vec<String>,
    pub enabled: bool,
    pub draining: bool,
    pub priority: i32,
    pub weight: u32,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub health: CandidateHealth,
    pub quota: CandidateQuota,
    pub quota_updated_at_ms: Option<u64>,
    pub quota_snapshot: QuotaSnapshot,
    pub subscription_plan_type: Option<String>,
    pub subscription_expires_at_ms: Option<u64>,
    pub last_used_at_ms: Option<u64>,
    pub cooldowns: BTreeMap<String, u64>,
    pub consecutive_failures: u32,
    pub proxy: Option<ProxyConfig>,
}

impl fmt::Debug for RuntimeChatGptAccount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeChatGptAccount")
            .field("id", &self.id)
            .field("source_id", &self.source_id)
            .field("chatgpt_account_id", &"[redacted]")
            .field("responses_url", &redacted_url(&self.responses_url))
            .field("models", &self.models)
            .field("enabled", &self.enabled)
            .field("draining", &self.draining)
            .field("priority", &self.priority)
            .field("weight", &self.weight)
            .field("allowed_models", &self.allowed_models)
            .field("excluded_models", &self.excluded_models)
            .field("health", &self.health)
            .field("quota", &self.quota)
            .field("quota_updated_at_ms", &self.quota_updated_at_ms)
            .field(
                "quota_reset_at_ms",
                &self.quota_snapshot.limiting_reset_at_ms(),
            )
            .field("subscription_plan_type", &self.subscription_plan_type)
            .field(
                "subscription_expires_at_ms",
                &self.subscription_expires_at_ms,
            )
            .field("last_used_at_ms", &self.last_used_at_ms)
            .field("cooldowns", &self.cooldowns)
            .field("consecutive_failures", &self.consecutive_failures)
            .field("proxy_configured", &self.proxy.is_some())
            .finish()
    }
}

pub struct RuntimeChatGptAuth {
    pub token_authority: Arc<TokenAuthority>,
    pub refresh_adapter: Arc<dyn TokenRefreshAdapter>,
    pub persistence_adapter: Arc<dyn TokenPersistenceAdapter>,
    pub refresh_skew_ms: u64,
    pub agent_identities: HashMap<String, AgentIdentityCredential>,
}

fn redacted_url(value: &str) -> String {
    let Ok(mut url) = url::Url::parse(value) else {
        return "[invalid]".to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}
