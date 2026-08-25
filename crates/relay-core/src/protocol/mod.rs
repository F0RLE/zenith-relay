mod adapter;
mod capabilities;
mod management;
mod sse;
mod version;

pub use adapter::{
    bridged_response_id, bridged_response_id_scoped, prepare_responses_to_messages,
    prepare_responses_to_messages_scoped, translate_messages_response, AdapterError,
    AdapterRequestContext, AdapterResponse, AdapterResult, GeminiBridgeRequest,
    GeminiBridgeResponse, MessagesBridgeRequest, MessagesBridgeResponse, MessagesBridgeState,
    MessagesBridgeStore, MessagesReasoningMode, MessagesStreamBridge, NativeResponsesReplayState,
    NativeResponsesReplayStore, PreparedAdapterRequest, SourceAdapter, UpstreamProtocol,
};
pub(crate) use adapter::{
    remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids,
    repair_custom_tool_item_ids,
};
pub use adapter::{AdapterStreamBridge, GeminiStreamBridge};
pub use capabilities::{Capabilities, Feature, CURRENT_PROTOCOL_VERSION};
pub use management::{
    account_candidate_enabled, account_operational_state, apply_model_display_order,
    apply_model_reasoning_summary, apply_model_speed_summary, model_has_api_source_route,
    model_has_native_account_route, operational_status, pool_model_summaries,
    pooled_source_runtime_available, quota_refresh_status, source_runtime_available,
    valid_generated_id, AccountOperationalInput, AccountOperationalState, AccountPresetRule,
    AccountRoutingBlockReason, AccountSummary, ApiError, ClientWireApi, ConfigurationPreset,
    ConfigurationPresetApplyInput, ConfigurationPresetApplyResult, ConfigurationPresetChange,
    ConfigurationPresetDocument, ConfigurationPresetPreview, ConfigurationPresetPreviewInput,
    ConfigurationPresetSettings, ErrorEnvelope, GatewayDiagnostic, GatewaySummary, HealthResponse,
    ModelSummary, OperationalStatus, PresetQuotaPolicy, PresetRoutingPolicy, ProfileKeyRotation,
    ProxyMode, QuotaRefreshStatus, QuotaWindowUsage, RemoteAccountLocation,
    RevealedAccountIdentity, RuntimeStateSnapshot, RuntimeTargetSummary, SourcePresetRule,
    SourceSummary, UsageBucket, UsageGroup, UsagePage, UsageQuery, UsageRange, UsageSummary,
    UsageTotals, CONFIGURATION_PRESET_FORMAT, CONFIGURATION_PRESET_SCHEMA_VERSION,
    PROFILE_KEY_ROTATION_SCHEMA_VERSION,
};
pub(crate) use sse::event_end as sse_event_end;
pub use version::{negotiate, ClientProtocolRange, NegotiatedProtocol, ProtocolError};
