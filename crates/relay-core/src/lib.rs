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
            pub transport: crate::usage::UsageTransport,
            #[serde(default)]
            pub service_tier: DefaultServiceTier,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub applied_service_tier: Option<ObservedServiceTier>,
            pub success: bool,
            pub http_status: u16,
            pub error_category: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub upstream_error: Option<crate::usage::UpstreamErrorDetails>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub image_usage: Option<crate::usage::ImageUsage>,
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
mod ids;
mod json_fingerprint;
mod json_size;
pub mod model_metadata;
mod paths;
pub mod poison;
pub mod pricing;
pub mod protocol;
pub mod providers;
pub mod proxy;
pub mod quality;
pub mod quota;
mod record_impl;
pub mod request_timezone;
mod runtime;
pub mod scheduler;
pub mod sources;
mod time;
pub mod tool_policy;
mod transport;
pub mod usage;

pub use catalog::{
    CODEX_CATALOG_PRIORITY_BASE, CODEX_RELAY_CATALOG_HASH, MAX_MODEL_LIST_LEN, ModelContextPolicy,
    ModelIdListError, ModelRegistry, ModelRules, apply_codex_ultra_from_official_model,
    canonicalize_reasoning_levels, claude_desktop_model_alias,
    claude_desktop_reasoning_model_alias, codex_catalog_entry_is_compatible, codex_model_alias,
    codex_model_display_name, codex_model_is_picker_eligible, codex_model_is_picker_eligible_for,
    decode_codex_model_alias, deserialize_model_reasoning_allowed_levels, is_degraded_route_model,
    is_valid_model_id, is_valid_model_token, merge_model_display_order, model_id_key,
    normalize_bounded_model_ids, normalize_codex_catalog_priorities,
    normalize_model_context_policies, normalize_model_ids,
    normalize_model_reasoning_allowed_levels, normalize_native_codex_catalog_entry,
    normalize_upstream_codex_catalog_entry, publish_routed_codex_context, reasoning_level_rank,
    reasoning_policy_key, reasoning_policy_levels, routed_codex_catalog_entry,
    source_model_declares_image_input, source_row_declares_reasoning,
};
pub use error::{Error, Result, normalize_error_code};
pub use ids::{is_ascii_ref, is_ascii_token};
pub use paths::path_with_suffix;
pub use pricing::{
    CACHE_FORMAT, CACHE_SCHEMA_VERSION, CatalogEntry, CatalogRefreshDeadline, CatalogRefreshKind,
    ImageModelPrice, ImageRequestPrice, InvalidPricingIdentity, LITELLM_SOURCE_URL,
    MAX_CACHE_BYTES, MAX_CACHE_RECORDS, MAX_CACHE_STRING_LENGTH, PRICING_REFRESH_INTERVAL_SECONDS,
    PRICING_REFRESH_JITTER_MAX_SECONDS, PriceEvidence, PriceSource, PricingCacheEnvelope,
    PricingCatalog, PricingCatalogHandle, PricingContext, PricingError, PricingMetadata,
    PricingSourceSummary, ResolvedPrice, SourcePricingMetadata, TokenPrice,
    normalize_pricing_identity, pricing_refresh_delay, pricing_refresh_jitter_seconds,
    usd_per_request_to_micro_usd, usd_per_token_to_micro_usd_per_million, usd_to_micro,
};
pub use protocol::{
    AdapterContinuationState, AdapterContinuationStore, AdapterError, AdapterRequestContext,
    AdapterResponse, AdapterResult, AdapterStreamBridge, MessagesReasoningMode,
    NativeResponsesReplayState, NativeResponsesReplayStore, PreparedAdapterRequest, SourceAdapter,
    TranslatedResponse, TranslationRequest, TranslationStream, UpstreamProtocol,
    bridged_response_id_scoped, merge_configuration_preset_settings,
    normalize_configuration_preset, prepare_responses_to_messages_scoped,
    validate_resolved_configuration_preset_members,
};
pub use providers::chatgpt::{RuntimeChatGptAccount, RuntimeChatGptAuth};
pub use proxy::{ProxyConfig, normalize_proxy_url, proxy_reference_id};
pub use request_timezone::{is_valid_timezone, normalize_timezone};
pub use runtime::{
    DefaultServiceTier, ExecutionFence, GatewayRuntime, GatewayRuntimeOptions, IMAGE_API_MODEL,
    PoolAccess, PoolParticipant, RESPONSE_AFFINITY_DELETE_CANDIDATE_SQL,
    RESPONSE_AFFINITY_DELETE_EXPIRED_SQL, RESPONSE_AFFINITY_DELETE_SQL, RESPONSE_AFFINITY_FIND_SQL,
    RESPONSE_AFFINITY_UPSERT_SQL, ResponseAffinityBinding, ResponseAffinityStore,
    RuntimeActivitySnapshot, RuntimeCandidatePolicy, RuntimeContinuationStores, RuntimeLocalKey,
    RuntimeMixedLocalKey, RuntimeSource, RuntimeSourcePolicyRecord, RuntimeSourcePolicyUpdate,
    changed_runtime_source_policy_updates, normalize_image_base_model,
    normalize_model_service_tier_overrides, pool_catalog_visibility_changed,
    pool_dispatch_permission_changed,
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
    ActiveModelRuntime, AttemptOutcome, CandidateHealth, CandidateKind, CandidateQuota,
    CandidateQuotaReference, CandidateQuotaState, CandidateResourceHeadroom,
    CandidateRuntimeSnapshot, CandidateScope, ModelRetryRuntime, PROMPT_AFFINITY_TTL_MS,
    PoolMemberKind, PoolRoutingMember, PoolRoutingMode, PoolRoutingPolicy, PoolScheduler,
    QUOTA_STALE_AFTER_MS, RESPONSE_AFFINITY_TTL_MS, RoutingDiagnostics, RuntimeCandidate,
    Selection, SelectionReason, SelectionRequest, account_candidate_health, resolve_pool_routing,
};
pub use sources::{
    CacheWriteTtl, CapabilityOrigin, CapabilityStatus, LocalGatewayKey,
    MAX_SOURCE_RECOVERY_DELAY_SECONDS, ModelEndpointCapability, ModelTokenLimits, ProtocolFeature,
    ProviderSource, SourceBalanceKind, SourceCatalogEvidence, SourceCatalogRecord, SourceConnector,
    SourceDiscovery, SourceProbeInput, SourceProbeResult, SourceProtocolBinding,
    SourceProtocolBindingKey, SourceProtocolConfig, SourceProtocolResolution, SourceProviderStats,
    SourceRead, SourceStatsAmount, SourceStatsCurrency, SourceStatsProvider, SourceStatsStatus,
    SourceTransportIdentity, SourceTransportRecord, WireApi, apply_source_priorities,
    cache_write_model_ids, discover_source_models, discover_source_models_and_protocol_bindings,
    discover_source_models_for_protocol_bindings, discover_source_with_protocol_config,
    discover_source_with_protocol_config_with_scope, endpoint_url_protocol,
    fetch_source_provider_stats, is_http_endpoint, is_loopback_url,
    normalize_source_protocol_bindings, probe_source_generation,
    probe_source_generation_with_scope, read_source_models, read_source_models_with_scope,
    read_source_provider_stats, read_source_provider_stats_with_proxy,
    read_source_provider_stats_with_scope, read_source_provider_stats_with_scope_and_proxy,
    runtime_source_models_for_any_wire_api, runtime_source_models_for_wire_api,
    runtime_source_models_with_cache_write_pricing, runtime_source_protocol_bindings,
    runtime_source_supports_any_wire_api, runtime_source_supports_wire_api, service_protocol,
    source_catalog_changed, source_models_for_wire_api, source_points_to_gateway,
    source_runtime_policy_compatible, url_has_userinfo,
};
pub use time::{unix_time_ms, unix_time_ms_at, unix_time_ms_from_rfc3339};
pub use tool_policy::{ToolPolicy, ToolPolicyMode, ToolPolicyOutcome};
pub use transport::collect_limited;
pub use usage::{
    ApiEquivalentSummary, ApiEquivalentUsage, ApiModelPriceOverride, ApiModelPriceSources,
    CandidatePriceQuery, CatalogPriceResolver, ErrorOrigin, ObservedServiceTier, ObservedUsageSums,
    SourceModelPriceOverrides, TerminalOutputKind, ToolChoiceMode, ToolUseDiagnostics,
    UsageCallback, UsageEvent, UsageTransport, UsageValue, estimate_api_equivalent_with_catalog,
    estimate_api_equivalent_with_token_price, estimate_candidate_api_equivalent_with_catalog,
    normalize_model_price_overrides, normalize_observed_service_tier, normalize_reasoning_effort,
    resolve_candidate_price, sql_like_contains_pattern,
};

/// Drops optional text that is empty or only whitespace.
/// Any other value keeps its original spelling.
pub fn omit_blank<T: AsRef<str>>(optional_text: Option<T>) -> Option<T> {
    optional_text.filter(|text| !text.as_ref().trim().is_empty())
}

#[cfg(test)]
mod omit_blank_tests {
    use super::omit_blank;

    #[test]
    fn blank_text_is_absent_and_other_spelling_stays() {
        assert_eq!(omit_blank(None::<String>), None);
        assert_eq!(omit_blank(Some(" \n\t ".to_string())), None);
        assert_eq!(
            omit_blank(Some(" GPT-5 ".to_string())).as_deref(),
            Some(" GPT-5 ")
        );
        assert_eq!(omit_blank(Some("ok")), Some("ok"));
    }
}
