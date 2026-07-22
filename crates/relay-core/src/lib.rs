pub mod accounts;
pub mod automations;
pub mod catalog;
mod error;
pub mod gateway;
pub mod protocol;
pub mod proxy;
pub mod quota;
mod runtime;
pub mod scheduler;
pub mod sources;
pub mod usage;

pub use catalog::{ModelRegistry, ModelRules};
pub use error::{Error, Result};
pub use proxy::{normalize_proxy_url, ProxyConfig};
pub use runtime::{
    discover_source_models, normalize_image_base_model, DefaultServiceTier, GatewayRuntime,
    GatewayRuntimeOptions, ResponseAffinityBinding, ResponseAffinityStore, RuntimeAccount,
    RuntimeAccountAuth, RuntimeLocalKey, RuntimeMixedLocalKey, RuntimeSource,
};
pub use scheduler::{
    account_candidate_health, normalize_subscription_plan_order, quota_stale_after_ms_for_interval,
    CandidateHealth, CandidateKind, CandidateQuota, CandidateRuntimeSnapshot, CandidateScope,
    PoolScheduler, RoutingDiagnostics, RoutingStrategy, RuntimeCandidate, Selection,
    SelectionReason, SelectionRequest, QUOTA_STALE_AFTER_MS, RESPONSE_AFFINITY_TTL_MS,
};
pub use sources::{source_points_to_gateway, LocalGatewayKey, ProviderSource, WireApi};
pub use usage::{
    api_model_price, estimate_api_equivalent, estimate_api_equivalent_with_price_override,
    ApiEquivalentSummary, ApiModelPrice, ApiModelPriceOverride, UsageCallback, UsageEvent,
};
