use super::agent_identity::AgentIdentityCredential;
use super::{BasisPointsCapturedHeaders, OAuthClientKind};
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
    pub oauth_client_kind: OAuthClientKind,
    pub id: String,
    pub source_id: String,
    pub chatgpt_account_id: String,
    pub chatgpt_user_id: Option<String>,
    pub responses_url: String,
    /// Legacy input, ignored. Only the issuing OAuth client selects the transport.
    pub basis_points_enabled: bool,
    pub basis_points_headers: Option<BasisPointsCapturedHeaders>,
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
            .field("oauth_client_kind", &self.oauth_client_kind)
            .field("id", &self.id)
            .field("source_id", &self.source_id)
            .field("chatgpt_account_id", &"[redacted]")
            .field(
                "chatgpt_user_id",
                &self.chatgpt_user_id.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "responses_url",
                &crate::sources::redact_url(&self.responses_url),
            )
            .field("basis_points_enabled", &self.basis_points_enabled)
            .field(
                "basis_points_headers",
                &self.basis_points_headers.as_ref().map(|_| "[redacted]"),
            )
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
