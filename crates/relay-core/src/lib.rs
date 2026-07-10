pub mod accounts;
pub mod automations;
pub mod catalog;
mod error;
pub mod gateway;
pub mod quota;
mod runtime;
pub mod scheduler;
pub mod sources;
pub mod usage;

pub use catalog::{ModelRegistry, ModelRules};
pub use error::{Error, Result};
pub use runtime::{
    discover_source_models, GatewayRuntime, GatewayRuntimeOptions, RuntimeAccount,
    RuntimeAccountAuth, RuntimeLocalKey, RuntimeMixedLocalKey, RuntimeSource,
};
pub use scheduler::{
    CandidateHealth, CandidateKind, CandidateQuota, CandidateScope, PoolScheduler,
    RuntimeCandidate, Selection, SelectionRequest,
};
pub use sources::{LocalGatewayKey, ProviderSource, WireApi};
pub use usage::{UsageCallback, UsageEvent};
