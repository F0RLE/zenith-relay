mod adapter;
mod capabilities;
mod management;
mod version;

pub(crate) use adapter::repair_call_prefixed_function_item_ids;
pub use adapter::{
    bridged_response_id, bridged_response_id_scoped, prepare_responses_to_messages,
    prepare_responses_to_messages_scoped, translate_messages_response, AdapterError,
    AdapterRequestContext, AdapterResult, MessagesBridgeRequest, MessagesBridgeResponse,
    MessagesBridgeState, MessagesBridgeStore, MessagesReasoningMode, MessagesStreamBridge,
    NativeResponsesReplayState, NativeResponsesReplayStore, PreparedAdapterRequest, SourceAdapter,
};
pub use capabilities::{Capabilities, Feature, CURRENT_PROTOCOL_VERSION};
pub use management::{
    account_operational_state, operational_status, pool_model_summaries, quota_refresh_status,
    AccountOperationalInput, AccountOperationalState, AccountPresetRule, AccountRoutingBlockReason,
    AccountSummary, ApiError, ClientAccessDocument, ClientKeyCreateInput, ClientKeyPatch,
    ClientWireApi, ConfigurationPreset, ConfigurationPresetApplyInput,
    ConfigurationPresetApplyResult, ConfigurationPresetChange, ConfigurationPresetDocument,
    ConfigurationPresetPreview, ConfigurationPresetPreviewInput, ConfigurationPresetSettings,
    ErrorEnvelope, GatewayDiagnostic, GatewaySummary, GeneratedClientKey, HealthResponse,
    KeySummary, ModelSummary, OperationalStatus, PresetQuotaPolicy, PresetRoutingPolicy,
    ProfileKeyRotation, ProxyMode, QuotaRefreshStatus, RemoteAccountLocation,
    RevealedAccountIdentity, RuntimeStateSnapshot, RuntimeTargetSummary, SourcePresetRule,
    SourceSummary, UsageBucket, UsageGroup, UsagePage, UsageQuery, UsageRange, UsageSummary,
    UsageTotals, CLIENT_ACCESS_SCHEMA_VERSION, CONFIGURATION_PRESET_FORMAT,
    CONFIGURATION_PRESET_SCHEMA_VERSION, PROFILE_KEY_ROTATION_SCHEMA_VERSION,
};
pub use version::{negotiate, ClientProtocolRange, NegotiatedProtocol, ProtocolError};
