use super::Capabilities;
use crate::{
    accounts::AccountAuthState,
    api_model_price,
    automations::{WakeHistory, WakeTask},
    quota::{QuotaSnapshot, Subscription},
    ApiEquivalentSummary, ModelRules, WireApi,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub server_id: String,
    pub started_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTargetSummary {
    pub kind: String,
    pub connected: bool,
    pub origin: Option<String>,
    pub server_id: Option<String>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySummary {
    pub running: bool,
    pub base_url: String,
    pub candidate_count: usize,
    pub visible_model_ids: Vec<String>,
    #[serde(default)]
    pub models: Vec<ModelSummary>,
    #[serde(default)]
    pub common_proxy_configured: bool,
    #[serde(default)]
    pub common_proxy_available: bool,
    #[serde(default)]
    pub account_proxy_required: bool,
    #[serde(default)]
    pub quota_refresh_interval_seconds: u64,
    #[serde(default)]
    pub quota_request_timeout_seconds: u64,
    #[serde(default = "legacy_use_free_accounts")]
    pub use_free_accounts: bool,
}

fn legacy_use_free_accounts() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSummary {
    pub id: String,
    pub enabled: bool,
    pub member_count: usize,
    pub catalog_rank: Option<u32>,
    pub input_micro_usd_per_million: Option<u64>,
    pub output_micro_usd_per_million: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    #[default]
    Direct,
    Common,
    Account,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountRoutingExclusion {
    FreePlanPolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSummary {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub in_pool: bool,
    pub draining: bool,
    pub base_url: String,
    pub wire_api: WireApi,
    pub models: Vec<String>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub priority: i32,
    pub weight: u32,
    #[serde(default)]
    pub api_equivalent: ApiEquivalentSummary,
    pub secret_available: bool,
    pub last_error_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub id: String,
    pub label: String,
    pub identity_hint: String,
    pub enabled: bool,
    #[serde(default)]
    pub in_pool: bool,
    pub draining: bool,
    pub auth_state: AccountAuthState,
    pub health: String,
    pub models: Vec<String>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub priority: i32,
    pub weight: u32,
    #[serde(default)]
    pub api_equivalent: ApiEquivalentSummary,
    pub subscription: Subscription,
    pub quota: QuotaSnapshot,
    pub secret_available: bool,
    #[serde(default)]
    pub proxy_mode: ProxyMode,
    #[serde(default)]
    pub proxy_available: bool,
    #[serde(default)]
    pub routing_exclusion: Option<AccountRoutingExclusion>,
    pub last_error_code: Option<String>,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevealedAccountIdentity {
    pub account_id: String,
    pub identity: String,
}

impl fmt::Debug for RevealedAccountIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RevealedAccountIdentity")
            .field("account_id", &self.account_id)
            .field("identity", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeySummary {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub source_ids: Option<Vec<String>>,
    pub account_ids: Option<Vec<String>>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub model_prefix: Option<String>,
    pub created_at_ms: u64,
    pub last_used_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStateSnapshot {
    pub schema_version: u32,
    pub runtime_target: RuntimeTargetSummary,
    pub gateway: GatewaySummary,
    pub platform: String,
    pub capabilities: Capabilities,
    pub sources: Vec<SourceSummary>,
    pub accounts: Vec<AccountSummary>,
    pub keys: Vec<KeySummary>,
    pub automations: Vec<WakeTask>,
    pub wake_history: Vec<WakeHistory>,
    pub warnings: Vec<String>,
}

pub fn pool_model_summaries(
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
    hidden_models: &[String],
) -> Vec<ModelSummary> {
    let mut models = BTreeMap::<String, (String, BTreeSet<String>)>::new();
    for source in sources.iter().filter(|source| {
        source.enabled && source.in_pool && !source.draining && source.secret_available
    }) {
        add_member_models(
            &mut models,
            &format!("source:{}", source.id),
            &source.models,
            &source.allowed_models,
            &source.excluded_models,
        );
    }
    for account in accounts.iter().filter(|account| {
        account.enabled
            && account.in_pool
            && !account.draining
            && account.secret_available
            && account.proxy_available
            && account.routing_exclusion.is_none()
    }) {
        add_member_models(
            &mut models,
            &format!("account:{}", account.id),
            &account.models,
            &account.allowed_models,
            &account.excluded_models,
        );
    }

    let mut summaries = models
        .into_values()
        .map(|(id, members)| {
            let price = api_model_price(&id);
            ModelSummary {
                enabled: !hidden_models
                    .iter()
                    .any(|hidden| hidden.eq_ignore_ascii_case(&id)),
                id,
                member_count: members.len(),
                catalog_rank: price.map(|price| price.catalog_rank),
                input_micro_usd_per_million: price.map(|price| price.input_micro_usd_per_million),
                output_micro_usd_per_million: price.map(|price| price.output_micro_usd_per_million),
            }
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        left.catalog_rank
            .unwrap_or(u32::MAX)
            .cmp(&right.catalog_rank.unwrap_or(u32::MAX))
            .then_with(|| {
                right
                    .output_micro_usd_per_million
                    .unwrap_or_default()
                    .cmp(&left.output_micro_usd_per_million.unwrap_or_default())
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    summaries
}

fn add_member_models(
    models: &mut BTreeMap<String, (String, BTreeSet<String>)>,
    member_id: &str,
    member_models: &[String],
    allowed_models: &[String],
    excluded_models: &[String],
) {
    let rules = ModelRules {
        allowed: allowed_models.iter().cloned().collect(),
        excluded: excluded_models.iter().cloned().collect(),
    };
    for model in member_models.iter().filter(|model| rules.allows(model)) {
        let key = model.to_ascii_lowercase();
        let entry = models
            .entry(key)
            .or_insert_with(|| (model.clone(), BTreeSet::new()));
        entry.1.insert(member_id.to_string());
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub id: i64,
    pub request_id: String,
    pub local_key_id: String,
    pub candidate_kind: String,
    pub candidate_hint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_label: Option<String>,
    pub requested_model: Option<String>,
    pub resolved_model: Option<String>,
    pub wire_api: WireApi,
    pub success: bool,
    pub http_status: u16,
    pub error_category: Option<String>,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub created_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePage {
    pub events: Vec<UsageSummary>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub total_pages: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageRange {
    Daily,
    Weekly,
    Monthly,
    Custom,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageQuery {
    #[serde(default)]
    pub page: u32,
    #[serde(default)]
    pub page_size: u32,
    pub range: Option<UsageRange>,
    pub from_ms: Option<u64>,
    pub to_ms: Option<u64>,
    pub model_query: Option<String>,
    pub source_or_account_query: Option<String>,
    pub local_key_query: Option<String>,
    pub wire_api: Option<WireApi>,
    pub success: Option<bool>,
    pub error_category: Option<String>,
    pub request_id_query: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayDiagnostic {
    pub stream: bool,
    pub model: String,
    pub latency_ms: u64,
    pub bytes_received: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub stage: String,
    pub retryable: bool,
    pub request_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ErrorEnvelope {
    pub error: ApiError,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_summaries_apply_member_rules_hidden_state_and_catalog_order() {
        let source = SourceSummary {
            id: "source_1".into(),
            name: "Synthetic".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            base_url: "https://example.test/v1".into(),
            wire_api: WireApi::Responses,
            models: vec![
                "gpt-old".into(),
                "gpt-5.4-mini".into(),
                "gpt-5.4".into(),
                "gpt-future-codex".into(),
            ],
            allowed_models: Vec::new(),
            excluded_models: vec!["gpt-old".into()],
            priority: 0,
            weight: 1,
            api_equivalent: ApiEquivalentSummary::default(),
            secret_available: true,
            last_error_code: None,
        };

        let models = pool_model_summaries(&[source], &[], &["GPT-5.4-MINI".into()]);

        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["gpt-5.4", "gpt-5.4-mini", "gpt-future-codex"]
        );
        assert!(models[0].enabled);
        assert!(!models[1].enabled);
        assert!(models[2].enabled);
        assert_eq!(models[0].member_count, 1);
        assert!(models[0].output_micro_usd_per_million.is_some());
        assert!(models[2].catalog_rank.is_none());
        assert!(models[2].output_micro_usd_per_million.is_none());
    }

    #[test]
    fn usage_summary_accepts_servers_without_reasoning_telemetry() {
        let summary: UsageSummary = serde_json::from_str(
            r#"{"id":1,"requestId":"req","localKeyId":"key","candidateKind":"source","candidateHint":"abc","requestedModel":null,"resolvedModel":null,"wireApi":"responses","success":true,"httpStatus":200,"errorCategory":null,"latencyMs":1,"inputTokens":2,"cachedInputTokens":null,"outputTokens":3,"totalTokens":5,"createdAtMs":1}"#,
        )
        .unwrap();

        assert_eq!(summary.reasoning_tokens, None);
        assert_eq!(summary.ttft_ms, None);
    }

    #[test]
    fn legacy_runtime_fields_keep_remote_servers_compatible() {
        let gateway: GatewaySummary = serde_json::from_str(
            r#"{"running":true,"baseUrl":"https://relay.test/v1","candidateCount":1,"visibleModelIds":["gpt-test"]}"#,
        )
        .unwrap();
        assert!(gateway.use_free_accounts);

        let account = AccountSummary {
            id: "account_1".into(),
            label: "Account".into(),
            identity_hint: "masked".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            auth_state: AccountAuthState::Active,
            health: "healthy".into(),
            models: vec!["gpt-test".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            api_equivalent: ApiEquivalentSummary::default(),
            subscription: Subscription::default(),
            quota: QuotaSnapshot::default(),
            secret_available: true,
            proxy_mode: ProxyMode::Direct,
            proxy_available: true,
            routing_exclusion: None,
            last_error_code: None,
        };
        let mut value = serde_json::to_value(account).unwrap();
        value.as_object_mut().unwrap().remove("routingExclusion");
        let restored: AccountSummary = serde_json::from_value(value).unwrap();
        assert_eq!(restored.routing_exclusion, None);
    }
}
