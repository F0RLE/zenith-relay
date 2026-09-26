// Usage events and management summaries serialize the same request metadata.
// Defining their complete items through one macro keeps that wire contract in
// lockstep while allowing each type to retain its own ownership-only fields.
macro_rules! define_usage_request_contract {
    ($(#[$attribute:meta])* $visibility:vis struct $name:ident { $($fields:tt)* }) => {
        $(#[$attribute])*
        $visibility struct $name {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub routing: Option<RoutingDiagnostics>,
            pub requested_model: Option<String>,
            pub resolved_model: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub requested_reasoning_effort: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub effective_reasoning_effort: Option<String>,
            pub wire_api: WireApi,
            #[serde(default)]
            pub service_tier: DefaultServiceTier,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub applied_service_tier: Option<ObservedServiceTier>,
            pub success: bool,
            pub http_status: u16,
            pub error_category: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub upstream_error: Option<crate::usage::UpstreamErrorDetails>,
            $($fields)*
        }
    };
}

pub mod accounts;
pub mod automations;
pub mod catalog;
mod catalog_io;
mod error;
pub mod error_codes;
pub mod gateway;
pub mod model_metadata;
pub mod pricing;
pub mod protocol;
pub mod providers;
pub mod proxy;
pub mod quota;
mod runtime;
pub mod scheduler;
pub mod sources;
mod time;
pub mod tool_policy;
mod transport;
pub mod usage;

pub use catalog::{
    apply_codex_ultra_from_official_model, canonicalize_model_ids, canonicalize_reasoning_levels,
    codex_catalog_entry_is_compatible, codex_model_alias, codex_model_display_name,
    codex_model_is_picker_eligible, decode_codex_model_alias,
    deserialize_model_reasoning_allowed_levels, is_valid_model_id, is_valid_model_token,
    merge_model_display_order, normalize_codex_catalog_priorities, normalize_model_ids,
    normalize_model_reasoning_allowed_levels, normalize_native_codex_catalog_entry,
    normalize_upstream_codex_catalog_entry, reasoning_policy_key, reasoning_policy_levels,
    routed_codex_catalog_entry, source_model_declares_image_input, source_row_declares_reasoning,
    ModelRegistry, ModelRules, CODEX_CATALOG_PRIORITY_BASE, CODEX_RELAY_CATALOG_HASH,
};
pub use error::{normalize_error_code, Error, Result};
pub use pricing::{
    pricing_refresh_delay, pricing_refresh_jitter_seconds, usd_per_request_to_micro_usd,
    usd_per_token_to_micro_usd_per_million, usd_to_micro, CatalogEntry, CatalogRefreshDeadline,
    CatalogRefreshKind, ImageModelPrice, ImageRequestPrice, PriceEvidence, PriceSource,
    PricingCacheEnvelope, PricingCatalog, PricingCatalogHandle, PricingContext, PricingError,
    PricingMetadata, PricingSourceSummary, ResolvedPrice, SourcePricingMetadata, TokenPrice,
    CACHE_FORMAT, CACHE_SCHEMA_VERSION, LITELLM_SOURCE_URL, MAX_CACHE_BYTES, MAX_CACHE_RECORDS,
    MAX_CACHE_STRING_LENGTH, PRICING_REFRESH_INTERVAL_SECONDS, PRICING_REFRESH_JITTER_MAX_SECONDS,
};
pub use protocol::{
    bridged_response_id_scoped, merge_configuration_preset_settings,
    normalize_configuration_preset, prepare_responses_to_messages_scoped,
    validate_resolved_configuration_preset_members, AdapterError, AdapterRequestContext,
    AdapterResponse, AdapterResult, AdapterStreamBridge, GeminiBridgeRequest, GeminiBridgeResponse,
    GeminiStreamBridge, MessagesBridgeRequest, MessagesBridgeResponse, MessagesBridgeState,
    MessagesBridgeStore, MessagesReasoningMode, MessagesStreamBridge, NativeResponsesReplayState,
    NativeResponsesReplayStore, PreparedAdapterRequest, SourceAdapter, UpstreamProtocol,
};
pub use providers::chatgpt::{RuntimeChatGptAccount, RuntimeChatGptAuth};
pub use proxy::{normalize_proxy_url, proxy_reference_id, ProxyConfig};
pub use runtime::{
    changed_runtime_source_policy_updates, normalize_image_base_model,
    normalize_model_service_tier_overrides, DefaultServiceTier, ExecutionFence, GatewayRuntime,
    GatewayRuntimeOptions, ResponseAffinityBinding, ResponseAffinityStore, RuntimeActivitySnapshot,
    RuntimeCandidatePolicy, RuntimeLocalKey, RuntimeMixedLocalKey, RuntimeSource,
    RuntimeSourcePolicyRecord, RuntimeSourcePolicyUpdate,
};
pub use scheduler::refresh::{
    RefreshCompletion, RefreshCoordinator, RefreshIdentity, RefreshJob, RefreshJobId, RefreshKind,
    RefreshOutcome,
};
pub use scheduler::rotation::{
    AdmissionError as RotationAdmissionError, AttemptId as RotationAttemptId,
    AttemptObservation as RotationAttemptObservation, AuthState as RotationAuthState,
    CandidateAvailability as RotationCandidateAvailability,
    CandidateBlockReason as RotationCandidateBlockReason,
    CircuitSnapshot as RotationCircuitSnapshot, CircuitState as RotationCircuitState,
    CommitStage as RotationCommitStage, DispatchStartError as RotationDispatchStartError,
    ExecutionCertainty as RotationExecutionCertainty,
    ExecutionEvidence as RotationExecutionEvidence,
    ExecutionObservation as RotationExecutionObservation,
    HealthObservation as RotationHealthObservation,
    IdempotencyContract as RotationIdempotencyContract, QuotaState as RotationQuotaState,
    RateState as RotationRateState, RecoveryPolicy as RotationRecoveryPolicy,
    RequestBudget as RotationRequestBudget, RequestId as RotationRequestId,
    RetryDecision as RotationRetryDecision, RetryEvidence as RotationRetryEvidence,
    RetryStopReason as RotationRetryStopReason, RotationCandidate, RotationEngine, RotationLease,
    RotationMode, RotationOperation, RotationRequest, RotationRoute, RotationSelection,
    RotationSelectionReason, RotationSettlement, SettlementError as RotationSettlementError,
};
pub use scheduler::{
    account_candidate_health, resolve_pool_routing, ActiveModelRuntime, CandidateHealth,
    CandidateKind, CandidateQuota, CandidateQuotaState, CandidateRuntimeSnapshot, CandidateScope,
    ModelRetryRuntime, PoolMemberKind, PoolRoutingMember, PoolRoutingMode, PoolRoutingPolicy,
    PoolScheduler, RoutingDiagnostics, RuntimeCandidate, Selection, SelectionReason,
    SelectionRequest, PROMPT_AFFINITY_TTL_MS, QUOTA_STALE_AFTER_MS, RESPONSE_AFFINITY_TTL_MS,
};
pub use sources::{
    discover_source_models, discover_source_models_and_protocol_bindings,
    discover_source_models_for_protocol_bindings, discover_source_with_protocol_config,
    discover_source_with_protocol_config_with_scope, endpoint_url_protocol,
    fetch_source_provider_stats, is_loopback_url, normalize_source_protocol_bindings,
    probe_source_generation, probe_source_generation_with_scope, read_source_models,
    read_source_models_with_scope, read_source_provider_stats,
    read_source_provider_stats_with_scope, runtime_source_models_for_any_wire_api,
    runtime_source_models_for_wire_api, runtime_source_models_with_cache_write_pricing,
    runtime_source_protocol_bindings, runtime_source_supports_any_wire_api,
    runtime_source_supports_wire_api, service_protocol, source_models_for_wire_api,
    source_points_to_gateway, CacheWriteTtl, CapabilityOrigin, CapabilityStatus, LocalGatewayKey,
    ModelEndpointCapability, ProtocolFeature, ProviderSource, SourceBalanceKind, SourceConnector,
    SourceDiscovery, SourceProbeInput, SourceProbeResult, SourceProtocolBinding,
    SourceProtocolBindingKey, SourceProtocolConfig, SourceProviderStats, SourceRead,
    SourceStatsAmount, SourceStatsCurrency, SourceStatsProvider, SourceStatsStatus, WireApi,
};
pub use time::{unix_time_ms, unix_time_ms_at};
pub use tool_policy::{ToolPolicy, ToolPolicyMode, ToolPolicyOutcome, ToolPolicyUpdate};
pub use usage::{
    estimate_api_equivalent_with_catalog, estimate_api_equivalent_with_token_price,
    estimate_candidate_api_equivalent_with_catalog, normalize_model_price_overrides,
    normalize_observed_service_tier, normalize_reasoning_effort, resolve_candidate_price,
    sql_like_contains_pattern, ApiEquivalentSummary, ApiEquivalentUsage, ApiModelPriceOverride,
    ApiModelPriceSources, ErrorOrigin, ObservedServiceTier, SourceModelPriceOverrides,
    TerminalOutputKind, ToolChoiceMode, ToolUseDiagnostics, UsageCallback, UsageEvent, UsageValue,
};
