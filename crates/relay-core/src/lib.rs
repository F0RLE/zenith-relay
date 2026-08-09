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
mod transport;
pub mod usage;

pub const DEFAULT_COOLDOWN_AFTER_FAILURES: u8 = 3;
pub const DEFAULT_KEEP_LAST_CANDIDATE_AVAILABLE: bool = true;

pub use catalog::{
    canonicalize_model_ids, codex_catalog_entry_is_compatible, codex_model_alias,
    codex_model_display_name, codex_model_is_picker_eligible, decode_codex_model_alias,
    deserialize_model_reasoning_allowed_levels, normalize_codex_catalog_priorities,
    normalize_model_reasoning_allowed_levels, normalize_native_codex_catalog_entry,
    normalize_upstream_codex_catalog_entry, routed_codex_catalog_entry,
    source_model_declares_image_input, ModelRegistry, ModelRules, CODEX_CATALOG_PRIORITY_BASE,
    CODEX_RELAY_CATALOG_HASH,
};
pub use error::{normalize_error_code, Error, Result};
pub use protocol::{
    bridged_response_id_scoped, prepare_responses_to_messages_scoped, AdapterError, AdapterResult,
    MessagesBridgeRequest, MessagesBridgeResponse, MessagesBridgeState, MessagesBridgeStore,
    MessagesReasoningMode, MessagesStreamBridge, NativeResponsesReplayState,
    NativeResponsesReplayStore, PreparedAdapterRequest, SourceAdapter,
};
pub use providers::chatgpt::{RuntimeChatGptAccount, RuntimeChatGptAuth};
pub use proxy::{normalize_proxy_url, proxy_reference_id, ProxyConfig};
pub use runtime::{
    normalize_image_base_model, DefaultServiceTier, GatewayRuntime, GatewayRuntimeOptions,
    ResponseAffinityBinding, ResponseAffinityStore, RuntimeCandidatePolicy, RuntimeLocalKey,
    RuntimeMixedLocalKey, RuntimeSource, RuntimeSourcePolicyUpdate,
};
pub use scheduler::{
    account_candidate_health, normalize_subscription_plan_order, ActiveModelRuntime,
    CandidateHealth, CandidateKind, CandidateQuota, CandidateRuntimeSnapshot, CandidateScope,
    PoolScheduler, RoutingDiagnostics, RoutingStrategy, RuntimeCandidate, Selection,
    SelectionReason, SelectionRequest, QUOTA_STALE_AFTER_MS, RESPONSE_AFFINITY_TTL_MS,
};
pub use sources::{
    discover_source_models, discover_source_models_and_protocol_bindings,
    discover_source_models_for_protocol_bindings, fetch_source_provider_stats,
    normalize_source_protocol_bindings, source_points_to_gateway, LocalGatewayKey, ProviderSource,
    SourceConnector, SourceDiscovery, SourceProtocolBinding, SourceProtocolBindingKey,
    SourceProviderStats, SourceStatsProvider, WireApi,
};
pub use usage::{
    api_model_price, api_pricing_revision, estimate_api_equivalent,
    estimate_api_equivalent_with_price_override, normalize_model_price_overrides,
    ApiEquivalentSummary, ApiModelPrice, ApiModelPriceOverride, TerminalOutputKind, ToolChoiceMode,
    ToolUseDiagnostics, UsageCallback, UsageEvent, UsageValue,
    MAX_MODEL_PRICE_MICRO_USD_PER_MILLION,
};
