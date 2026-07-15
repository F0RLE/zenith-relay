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
    discover_source_models, GatewayRuntime, GatewayRuntimeOptions, RuntimeAccount,
    RuntimeAccountAuth, RuntimeLocalKey, RuntimeMixedLocalKey, RuntimeSource,
};
pub use scheduler::{
    CandidateHealth, CandidateKind, CandidateQuota, CandidateScope, PoolScheduler,
    RoutingDiagnostics, RoutingStrategy, RuntimeCandidate, Selection, SelectionReason,
    SelectionRequest,
};
pub use sources::{source_points_to_gateway, LocalGatewayKey, ProviderSource, WireApi};
pub use usage::{
    api_model_price, estimate_api_equivalent, ApiEquivalentSummary, ApiModelPrice, UsageCallback,
    UsageEvent,
};
