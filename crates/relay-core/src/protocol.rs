mod account_trash;
mod adapter;
pub(crate) mod completion;
pub(crate) mod interactions;
mod management;
pub(crate) mod sse;

pub use account_trash::TrashedAccountSummary;
pub use adapter::AdapterStreamBridge;
pub use adapter::{
    bridged_response_id, bridged_response_id_scoped, prepare_responses_to_messages,
    prepare_responses_to_messages_scoped, translate_messages_response, AdapterContinuationState,
    AdapterContinuationStore, AdapterError, AdapterRequestContext, AdapterResponse, AdapterResult,
    MessagesReasoningMode, NativeResponsesReplayState, NativeResponsesReplayStore,
    PreparedAdapterRequest, SourceAdapter, TranslatedResponse, TranslationRequest,
    TranslationStream, UpstreamProtocol,
};
pub(crate) use adapter::{
    candidate_incomplete_reason, ensure_compaction_trigger, gemini_incomplete,
    is_compaction_checkpoint_type, merge_usage, prepare_bridged_compaction, prompt_blocked,
    remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids,
    repair_custom_tool_item_ids, wrap_compaction_response_bytes, BridgedCompaction,
};
#[cfg(test)]
pub use management::pool_model_summaries;
pub use management::{
    account_candidate_enabled, account_operational_state, api_equivalent_projection_window,
    apply_member_model_display_order, apply_model_display_order,
    apply_model_display_order_with_catalog, apply_model_metadata, apply_model_protocol_routes,
    apply_model_reasoning_summary, apply_model_speed_summary, apply_pool_model_configuration,
    apply_pool_model_configuration_with_context, canonical_pool_model_id,
    complete_model_display_order, configured_source_model_ids, local_gateway_client_wire_apis,
    max_retry_candidates_in_range, member_model_catalog, merge_configuration_preset_settings,
    merge_model_equivalents, model_has_api_source_route, model_has_native_account_route,
    normalize_configuration_preset, operational_status, pool_candidate_count,
    pool_model_projection_with_pricing, pool_model_summaries_with_pricing,
    pool_pricing_source_summary, pool_routing_summary, pooled_source_runtime_available,
    quota_refresh_status, quota_request_timeout_in_range, source_runtime_available,
    update_model_reasoning_policy, validate_resolved_configuration_preset_members,
    AccountOperationalInput, AccountOperationalSource, AccountOperationalState, AccountPresetRule,
    AccountRefreshState, AccountRoutingBlockReason, AccountSummary, ClientWireApi,
    ConfigurationPreset, ConfigurationPresetApplyInput, ConfigurationPresetApplyResult,
    ConfigurationPresetChange, ConfigurationPresetDocument, ConfigurationPresetPreview,
    ConfigurationPresetPreviewInput, ConfigurationPresetSettings, CredentialDiagnostics,
    CredentialRecovery, CredentialStatus, DesktopCapabilities, GatewayDiagnostic, GatewaySummary,
    ModelCatalogIdentity, ModelDisplayProjection, ModelPolicyError, ModelProtocolRoute,
    ModelSummary, OperationalStatus, PoolMembershipInput, PoolModelConfigurationInput,
    PoolModelProjection, PresetQuotaPolicy, PresetRoutingPolicy, ProxyMode, QuotaRefreshStatus,
    QuotaWindowUsage, RefreshStatus, RevealedAccountIdentity, RuntimeStateSnapshot,
    SetModelContextInput, SetModelEnabledInput, SetModelOrderInput, SetModelReasoningInput,
    SetModelServiceTierInput, SourcePresetRule, SourceRefreshState, SourceSummary,
    SourceSummaryRecord, UsageBucket, UsageGroup, UsagePage, UsageQuery, UsageRange, UsageSummary,
    UsageTokenBreakdown, UsageTotals, UsageWorkload, WeeklyValueCalibration,
    CONFIGURATION_PRESET_FORMAT, CONFIGURATION_PRESET_SCHEMA_VERSION, DEFAULT_MAX_RETRY_CANDIDATES,
    DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS, MAX_MAX_RETRY_CANDIDATES,
    MAX_QUOTA_REQUEST_TIMEOUT_SECONDS, MIN_MAX_RETRY_CANDIDATES, MIN_QUOTA_REQUEST_TIMEOUT_SECONDS,
};
#[cfg(test)]
pub(crate) use sse::take_event as take_sse_event;
pub(crate) use sse::{
    event_data as sse_data, event_end as sse_event_end, lines as sse_lines,
    push_pending_frames as push_pending_sse_frames,
};
