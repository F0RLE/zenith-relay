pub mod accounts;
pub mod automations;
pub mod catalog;
mod error;
pub mod gateway;
pub mod protocol;
pub mod providers;
pub mod proxy;
pub mod quota;
mod runtime;
pub mod scheduler;
pub mod sources;
pub mod usage;

pub use catalog::{ModelRegistry, ModelRules};
pub use error::{Error, Result};
pub use providers::chatgpt::{RuntimeChatGptAccount, RuntimeChatGptAuth};
pub use proxy::{normalize_proxy_url, proxy_reference_id, ProxyConfig};
pub use runtime::{
    discover_source_models, normalize_image_base_model, DefaultServiceTier, GatewayRuntime,
    GatewayRuntimeOptions, ResponseAffinityBinding, ResponseAffinityStore, RuntimeLocalKey,
    RuntimeMixedLocalKey, RuntimeSource,
};
pub use scheduler::{
    account_candidate_health, normalize_subscription_plan_order, CandidateHealth, CandidateKind,
    CandidateQuota, CandidateRuntimeSnapshot, CandidateScope, PoolScheduler, RoutingDiagnostics,
    RoutingStrategy, RuntimeCandidate, Selection, SelectionReason, SelectionRequest,
    QUOTA_STALE_AFTER_MS, RESPONSE_AFFINITY_TTL_MS,
};
pub use sources::{
    fetch_source_provider_stats, source_points_to_gateway, LocalGatewayKey, ProviderSource,
    SourceProviderStats, SourceStatsProvider, WireApi,
};
pub use usage::{
    api_model_price, api_pricing_revision, estimate_api_equivalent,
    estimate_api_equivalent_with_price_override, normalize_model_price_overrides,
    ApiEquivalentSummary, ApiModelPrice, ApiModelPriceOverride, UsageCallback, UsageEvent,
    UsageValue, MAX_MODEL_PRICE_MICRO_USD_PER_MILLION,
};
