//! Stable Relay error identifiers. Domain modules own classification and recovery.
//! Add a code here and document it in both localized Help references.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ErrorDefinition {
    pub code: &'static str,
    pub public_code: &'static str,
    /// Defaults used only when classifying an upstream failure.
    pub upstream: Option<(u16, &'static str)>,
}

macro_rules! error_codes {
    ($($name:ident => ($code:literal, $public:literal, $upstream:expr),)*) => {
        $(pub const $name: &str = $code;)*

        pub static ALL: &[ErrorDefinition] = &[
            $(ErrorDefinition { code: $code, public_code: $public, upstream: $upstream },)*
        ];

        pub fn definition(code: &str) -> Option<ErrorDefinition> {
            match code {
                $($code => Some(ErrorDefinition { code: $code, public_code: $public, upstream: $upstream }),)*
                _ => None,
            }
        }
    };
}

#[rustfmt::skip]
error_codes! {
    ADMISSION_QUEUE_FULL => ("admission_queue_full", "admission_queue_full", None),
    ADMISSION_WAIT_EXPIRED => ("admission_wait_expired", "admission_wait_expired", None),
    ACCESS_TOKEN_MISSING => ("access_token_missing", "access_token_missing", None),
    ACCESS_TOKEN_REJECTED => ("access_token_rejected", "access_token_rejected", None),
    ACCOUNT_AUTH => ("account_auth", "account_auth", None),
    ACCOUNT_CHANGED => ("account_changed", "account_changed", None),
    ACCOUNT_CHECK_FAILED => ("account_check_failed", "account_check_failed", None),
    ACCOUNT_CHECK_RESPONSE_TOO_LARGE => ("account_check_response_too_large", "account_check_response_too_large", None),
    ACCOUNT_CHECK_UNAVAILABLE => ("account_check_unavailable", "account_check_unavailable", None),
    ACCOUNT_EXPORT_FAILED => ("account_export_failed", "account_export_failed", None),
    ACCOUNT_IDENTITY_CLAIM_CONFLICT => ("account_identity_claim_conflict", "account_identity_claim_conflict", None),
    ACCOUNT_IDENTITY_MISMATCH => ("account_identity_mismatch", "account_identity_mismatch", None),
    ACCOUNT_MISSING => ("account_missing", "account_missing", None),
    ACCOUNT_NOT_FOUND => ("account_not_found", "account_not_found", None),
    ACCOUNT_PROFILE_RATE_LIMITED => ("account_profile_rate_limited", "account_profile_rate_limited", None),
    ACCOUNT_PURCHASE_COST_INVALID => ("account_purchase_cost_invalid", "account_purchase_cost_invalid", None),
    ACCOUNT_REFRESH => ("account_refresh", "account_refresh", None),
    ACCOUNT_REFRESH_FAILED => ("account_refresh_failed", "account_refresh_failed", None),
    ACCOUNT_RUNTIME_CREDENTIAL_MISSING => ("account_runtime_credential_missing", "account_runtime_credential_missing", None),
    ACCOUNT_RUNTIME_PROVIDER_ACCOUNT_ID_MISSING => ("account_runtime_provider_account_id_missing", "account_runtime_provider_account_id_missing", None),
    ACCOUNT_RUNTIME_PROXY_INVALID => ("account_runtime_proxy_invalid", "account_runtime_proxy_invalid", None),
    ACCOUNT_SECRET_INVALID => ("account_secret_invalid", "account_secret_invalid", None),
    ACCOUNT_SECRET_MISSING => ("account_secret_missing", "account_secret_missing", None),
    ACCOUNT_STORE_FAILED => ("account_store_failed", "account_store_failed", None),
    ACCOUNT_TOKEN_PERSISTENCE => ("account_token_persistence", "account_token_persistence", None),
    ADAPTER_BINDING_UNSUPPORTED => ("adapter_binding_unsupported", "adapter_binding_unsupported", None),
    ADAPTER_COMPACTION_UNSUPPORTED => ("adapter_compaction_unsupported", "adapter_compaction_unsupported", None),
    ADAPTER_CONTINUATION_MISMATCH => ("adapter_continuation_mismatch", "adapter_continuation_mismatch", None),
    ADAPTER_CONTINUATION_MISSING => ("adapter_continuation_missing", "adapter_continuation_missing", None),
    ADAPTER_INVALID_REQUEST => ("adapter_invalid_request", "adapter_invalid_request", None),
    ADAPTER_PARAMETER_UNSUPPORTED => ("adapter_parameter_unsupported", "adapter_parameter_unsupported", None),
    ADAPTER_REASONING_UNSUPPORTED => ("adapter_reasoning_unsupported", "adapter_reasoning_unsupported", None),
    ADAPTER_TOOL_UNSUPPORTED => ("adapter_tool_unsupported", "adapter_tool_unsupported", None),
    ADAPTER_UPSTREAM_RESPONSE_INVALID => ("adapter_upstream_response_invalid", "adapter_upstream_response_invalid", None),
    ADAPTER_UPSTREAM_STREAM_INVALID => ("adapter_upstream_stream_invalid", "adapter_upstream_stream_invalid", None),
    AGENT_IDENTITY_INVALID => ("agent_identity_invalid", "agent_identity_invalid", None),
    ALL_CANDIDATES_COOLING_DOWN => ("all_candidates_cooling_down", "all_candidates_cooling_down", None),
    ALL_SOURCES_COOLING_DOWN => ("all_sources_cooling_down", "all_sources_cooling_down", None),
    ALL_SOURCES_TEMPORARILY_UNAVAILABLE => ("all_sources_temporarily_unavailable", "all_sources_temporarily_unavailable", None),
    AMBIGUOUS_CREDENTIALS => ("ambiguous_credentials", "ambiguous_credentials", None),
    API_KEY_MISSING => ("api_key_missing", "api_key_missing", None),
    CALLBACK_ALREADY_RECEIVED => ("callback_already_received", "callback_already_received", None),
    CALLBACK_INVALID => ("callback_invalid", "callback_invalid", None),
    CALLBACK_PORT_UNAVAILABLE => ("callback_port_unavailable", "callback_port_unavailable", None),
    CHAT_FEATURE_NOT_SUPPORTED => ("chat_feature_not_supported", "chat_feature_not_supported", None),
    CLEANUP_INCOMPLETE => ("cleanup_incomplete", "cleanup_incomplete", None),
    CLIENT_API_NOT_ALLOWED => ("client_api_not_allowed", "client_api_not_allowed", None),
    CLIENT_CANCELLED => ("client_cancelled", "client_cancelled", None),
    CODEX_BACKGROUND_BLOCKED_ACTIVITY_SUMMARY => ("codex_background_blocked_activity_summary", "codex_background_blocked_activity_summary", None),
    CODEX_BACKGROUND_BLOCKED_TASK_TITLE => ("codex_background_blocked_task_title", "codex_background_blocked_task_title", None),
    CONFIGURATION_PRESET_INVALID => ("configuration_preset_invalid", "configuration_preset_invalid", None),
    CONFIGURATION_REFERENCE_MISSING => ("configuration_reference_missing", "configuration_reference_missing", None),
    CONFIGURATION_REVISION_STALE => ("configuration_revision_stale", "configuration_revision_stale", None),
    CONFIGURATION_RUNTIME_FAILED => ("configuration_runtime_failed", "configuration_runtime_failed", None),
    CONFIGURATION_STORE_FAILED => ("configuration_store_failed", "configuration_store_failed", None),
    CONFLICT => ("conflict", "conflict", None),
    PROXY_CHECK_TIMEOUT => ("proxy_check_timeout", "proxy_check_timeout", None),
    PROXY_CHECK_CONNECTION_FAILED => ("proxy_check_connection_failed", "proxy_check_connection_failed", None),
    PROXY_CHECK_AUTH_FAILED => ("proxy_check_auth_failed", "proxy_check_auth_failed", None),
    PROXY_CHECK_REJECTED => ("proxy_check_rejected", "proxy_check_rejected", None),
    PROXY_CHECK_INVALID_RESPONSE => ("proxy_check_invalid_response", "proxy_check_invalid_response", None),
    PROXY_CHECK_UNAVAILABLE => ("proxy_check_unavailable", "proxy_check_unavailable", None),
    COMPACTION_RESPONSE_INVALID => ("compaction_response_invalid", "compaction_response_invalid", Some((502, "upstream compaction did not finish with a valid encrypted result; request was not replayed"))),
    CREDENTIAL_LOAD_FAILED => ("credential_load_failed", "credential_load_failed", None),
    CREDENTIAL_PERSIST_FAILED => ("credential_persist_failed", "credential_persist_failed", None),
    CREDENTIAL_REFRESH_REQUIRES_REAUTH => ("credential_refresh_requires_reauth", "credential_refresh_requires_reauth", None),
    CREDENTIAL_REFRESH_RETRYABLE => ("credential_refresh_retryable", "credential_refresh_retryable", None),
    CREDENTIAL_STORE_UNAVAILABLE => ("credential_store_unavailable", "credential_store_unavailable", None),
    CREDENTIALS_MISSING => ("credentials_missing", "credentials_missing", None),
    DIAGNOSTIC_FAILED => ("diagnostic_failed", "diagnostic_failed", None),
    DIAGNOSTIC_INCOMPLETE => ("diagnostic_incomplete", "diagnostic_incomplete", None),
    DIAGNOSTIC_INVALID => ("diagnostic_invalid", "diagnostic_invalid", None),
    DIAGNOSTIC_KEY_UNAVAILABLE => ("diagnostic_key_unavailable", "diagnostic_key_unavailable", None),
    DIAGNOSTIC_MODEL_UNAVAILABLE => ("diagnostic_model_unavailable", "diagnostic_model_unavailable", None),
    DIAGNOSTIC_TOO_LARGE => ("diagnostic_too_large", "diagnostic_too_large", None),
    DIAGNOSTIC_UPSTREAM_FAILED => ("diagnostic_upstream_failed", "diagnostic_upstream_failed", None),
    DUPLICATE_ITEM => ("duplicate_item", "duplicate_item", None),
    EMPTY_INPUT => ("empty_input", "empty_input", None),
    EXPIRED => ("expired", "expired", None),
    GATEWAY_STOPPED => ("gateway_stopped", "gateway_stopped", None),
    GATEWAY_SYNC_FAILED => ("gateway_sync_failed", "gateway_sync_failed", None),
    GATEWAY_UNAVAILABLE => ("gateway_unavailable", "gateway_unavailable", None),
    IMAGE_GENERATION_NOT_ENABLED => ("image_generation_not_enabled", "image_generation_not_enabled", None),
    IMAGE_GENERATION_USER_ERROR => ("image_generation_user_error", "image_generation_user_error", None),
    IMAGE_OUTPUT_MISSING => ("image_output_missing", "image_output_missing", None),
    IMPORT_EXPIRED => ("import_expired", "import_expired", None),
    IMPORT_INPUT_CONFLICT => ("import_input_conflict", "import_input_conflict", None),
    IMPORT_INVALID => ("import_invalid", "import_invalid", None),
    IMPORT_NOT_FOUND => ("import_not_found", "import_not_found", None),
    IMPORT_SELECTION_INVALID => ("import_selection_invalid", "import_selection_invalid", None),
    IMPORT_SERIALIZE => ("import_serialize", "import_serialize", None),
    IMPORT_SESSION_INVALID => ("import_session_invalid", "import_session_invalid", None),
    INPUT_TOO_LARGE => ("input_too_large", "input_too_large", None),
    INVALID_ACCESS_TOKEN => ("invalid_access_token", "invalid_access_token", None),
    INVALID_ACCOUNT => ("invalid_account", "invalid_account", None),
    INVALID_ACCOUNT_IDENTITY => ("invalid_account_identity", "invalid_account_identity", None),
    INVALID_ACCOUNT_ID => ("invalid_account_id", "invalid_account_id", None),
    INVALID_AGENT_TASK_ID => ("invalid_agent_task_id", "invalid_agent_task_id", None),
    INVALID_API_KEY => ("invalid_api_key", "invalid_api_key", None),
    INVALID_CHATGPT_ACCOUNT_ID => ("invalid_chatgpt_account_id", "invalid_chatgpt_account_id", None),
    INVALID_CONFIGURATION => ("invalid_configuration", "invalid_configuration", None),
    INVALID_CREDENTIALS => ("invalid_credentials", "invalid_credentials", None),
    INVALID_GRANT => ("invalid_grant", "invalid_grant", None),
    INVALID_HOST => ("invalid_host", "invalid_host", None),
    INVALID_IDENTITY_TOKEN => ("invalid_identity_token", "invalid_identity_token", None),
    INVALID_IMAGE_MODEL => ("invalid_image_model", "invalid_image_model", None),
    INVALID_LABEL => ("invalid_label", "invalid_label", None),
    INVALID_LOGIN_ID => ("invalid_login_id", "invalid_login_id", None),
    INVALID_REFRESH_TOKEN => ("invalid_refresh_token", "invalid_refresh_token", None),
    INVALID_REQUEST => ("invalid_request", "invalid_request", None),
    INVALID_STREAM_ID => ("invalid_stream_id", "invalid_stream_id", None),
    INVALID_SESSION_ID => ("invalid_session_id", "invalid_session_id", None),
    INVALID_SOURCE_FILE => ("invalid_source_file", "invalid_source_file", None),
    INVALID_STATE => ("invalid_state", "invalid_state", None),
    INVALID_TASK_ID => ("invalid_task_id", "invalid_task_id", None),
    INVALID_TOKEN_SET => ("invalid_token_set", "invalid_token_set", None),
    IO => ("io", "io", None),
    ITEM_NOT_FOUND => ("item_not_found", "item_not_found", None),
    ITEM_NOT_SELECTABLE => ("item_not_selectable", "item_not_selectable", None),
    JSON_TOO_DEEP => ("json_too_deep", "json_too_deep", None),
    LISTENER_UNAVAILABLE => ("listener_unavailable", "listener_unavailable", None),
    MALFORMED_JSON => ("malformed_json", "malformed_json", None),
    MANAGEMENT_BLOCKED => ("management_blocked", "management_blocked", None),
    MANAGEMENT_UNAUTHORIZED => ("management_unauthorized", "management_unauthorized", None),
    MAX_RETRY_CANDIDATES_INVALID => ("max_retry_candidates_invalid", "max_retry_candidates_invalid", None),
    METADATA_PERSIST_FAILED => ("metadata_persist_failed", "metadata_persist_failed", None),
    MISSING_CREDENTIALS => ("missing_credentials", "missing_credentials", None),
    MODEL_ID_INVALID => ("model_id_invalid", "model_id_invalid", None),
    MODEL_NOT_FOUND => ("model_not_found", "model_not_found", None),
    MODEL_ORDER_INVALID => ("model_order_invalid", "model_order_invalid", None),
    MODEL_PRICE_INVALID => ("model_price_invalid", "model_price_invalid", None),
    MODEL_REASONING_RECOVERY_FAILED => ("model_reasoning_recovery_failed", "model_reasoning_recovery_failed", None),
    MODEL_SERVICE_TIER_UNSUPPORTED => ("model_service_tier_unsupported", "model_service_tier_unsupported", None),
    MODELS_ACCOUNT_LOCATION => ("models_account_location", "models_account_location", None),
    MODELS_CLIENT_INIT => ("models_client_init", "models_client_init", None),
    MODELS_AGENT_TASK_INVALID => ("models_agent_task_invalid", "models_agent_task_invalid", None),
    MODELS_FORBIDDEN => ("models_forbidden", "models_forbidden", None),
    MODELS_HTTP_STATUS => ("models_http_status", "models_http_status", None),
    MODELS_INVALID_ACCESS_TOKEN => ("models_invalid_access_token", "models_invalid_access_token", None),
    MODELS_INVALID_ACCOUNT_ID => ("models_invalid_account_id", "models_invalid_account_id", None),
    MODELS_INVALID_CLIENT_VERSION => ("models_invalid_client_version", "models_invalid_client_version", None),
    MODELS_INVALID_ENDPOINT => ("models_invalid_endpoint", "models_invalid_endpoint", None),
    MODELS_INVALID_RESPONSE => ("models_invalid_response", "models_invalid_response", None),
    MODELS_PREPARE => ("models_prepare", "models_prepare", None),
    MODELS_PROFILE_RESTORE => ("models_profile_restore", "models_profile_restore", None),
    MODELS_PROXY_UNAVAILABLE => ("models_proxy_unavailable", "models_proxy_unavailable", None),
    MODELS_RATE_LIMITED => ("models_rate_limited", "models_rate_limited", None),
    MODELS_REQUIRED => ("models_required", "models_required", None),
    MODELS_RESPONSE_TOO_LARGE => ("models_response_too_large", "models_response_too_large", None),
    MODELS_SECRET_STORE => ("models_secret_store", "models_secret_store", None),
    MODELS_STORAGE => ("models_storage", "models_storage", None),
    MODELS_TRANSPORT => ("models_transport", "models_transport", None),
    MODELS_UNAUTHORIZED => ("models_unauthorized", "models_unauthorized", None),
    MODELS_UPSTREAM => ("models_upstream", "models_upstream", None),
    NO_ELIGIBLE_SOURCE => ("no_eligible_source", "no_eligible_source", None),
    NOT_AGENT_IDENTITY => ("not_agent_identity", "not_agent_identity", None),
    NOT_FOUND => ("not_found", "not_found", None),
    OPERATION_FAILED => ("operation_failed", "operation_failed", None),
    PERSISTENCE_FAILED => ("persistence_failed", "persistence_failed", None),
    POOL_MEMBERS_EMPTY => ("pool_members_empty", "pool_members_empty", None),
    POOL_MEMBERS_TOO_MANY => ("pool_members_too_many", "pool_members_too_many", None),
    POOL_ROUTING_CONFLICT => ("pool_routing_conflict", "pool_routing_conflict", None),
    PORTABLE_UPDATE_UNAVAILABLE => ("portable_update_unavailable", "portable_update_unavailable", None),
    PORTABLE_UPDATE_UNSUPPORTED => ("portable_update_unsupported", "portable_update_unsupported", None),
    PORTABLE_NOT_WRITABLE => ("portable_not_writable", "portable_not_writable", None),
    PORTABLE_UPDATE_FAILED => ("portable_update_failed", "portable_update_failed", None),
    PREVIEW_INVALID => ("preview_invalid", "preview_invalid", None),
    PREVIEW_SERIALIZE => ("preview_serialize", "preview_serialize", None),
    PRICING_CATALOG_REFRESH_FAILED => ("pricing_catalog_refresh_failed", "pricing_catalog_refresh_failed", None),
    PROFILE_ATTACH_UNAVAILABLE => ("profile_attach_unavailable", "profile_attach_unavailable", None),
    PROFILE_RESTORE_BLOCKED => ("profile_restore_blocked", "profile_restore_blocked", None),
    PROFILE_ROTATION_INVALID => ("profile_rotation_invalid", "profile_rotation_invalid", None),
    PROFILE_ROTATION_MISSING => ("profile_rotation_missing", "profile_rotation_missing", None),
    PROVIDER_ACCOUNT_ID_MISSING => ("provider_account_id_missing", "provider_account_id_missing", None),
    PROVIDER_ACCOUNT_LOOKUP_FAILED => ("provider_account_lookup_failed", "provider_account_lookup_failed", None),
    PROXY_ASSIGNMENT_DUPLICATE => ("proxy_assignment_duplicate", "proxy_assignment_duplicate", None),
    PROXY_ASSIGNMENT_INVALID => ("proxy_assignment_invalid", "proxy_assignment_invalid", None),
    PROXY_INVALID => ("proxy_invalid", "proxy_invalid", None),
    PROXY_ROUTE_AMBIGUOUS => ("proxy_route_ambiguous", "proxy_route_ambiguous", None),
    PROXY_UNAVAILABLE => ("proxy_unavailable", "proxy_unavailable", None),
    QUOTA_ACCOUNT_LOCATION => ("quota_account_location", "quota_account_location", None),
    QUOTA_AUTHORIZATION_PREPARE => ("quota_authorization_prepare", "quota_authorization_prepare", None),
    QUOTA_EXHAUSTED => ("quota_exhausted", "quota_exhausted", None),
    QUOTA_FORBIDDEN => ("quota_forbidden", "quota_forbidden", None),
    QUOTA_HTTP_STATUS => ("quota_http_status", "quota_http_status", None),
    QUOTA_INVALID_PERCENTAGE => ("quota_invalid_percentage", "quota_invalid_percentage", None),
    QUOTA_INVALID_RESPONSE => ("quota_invalid_response", "quota_invalid_response", None),
    QUOTA_POLICY_INVALID => ("quota_policy_invalid", "quota_policy_invalid", None),
    QUOTA_PREPARE => ("quota_prepare", "quota_prepare", None),
    QUOTA_PROBE_FAILED => ("quota_probe_failed", "quota_probe_failed", None),
    QUOTA_PROXY_UNAVAILABLE => ("quota_proxy_unavailable", "quota_proxy_unavailable", None),
    QUOTA_QUEUE_FAILED => ("quota_queue_failed", "quota_queue_failed", None),
    QUOTA_RATE_LIMITED => ("quota_rate_limited", "quota_rate_limited", None),
    QUOTA_RESPONSE_TOO_LARGE => ("quota_response_too_large", "quota_response_too_large", None),
    QUOTA_SECRET_INVALID => ("quota_secret_invalid", "quota_secret_invalid", None),
    QUOTA_SECRET_LOAD => ("quota_secret_load", "quota_secret_load", None),
    QUOTA_SECRET_MISSING => ("quota_secret_missing", "quota_secret_missing", None),
    QUOTA_SECRET_STORE => ("quota_secret_store", "quota_secret_store", None),
    QUOTA_STORAGE => ("quota_storage", "quota_storage", None),
    QUOTA_TIMEOUT => ("quota_timeout", "quota_timeout", None),
    QUOTA_TOKEN_PREPARE => ("quota_token_prepare", "quota_token_prepare", None),
    QUOTA_TOKEN_REFRESH => ("quota_token_refresh", "quota_token_refresh", None),
    QUOTA_TRANSPORT => ("quota_transport", "quota_transport", None),
    QUOTA_UNAUTHORIZED => ("quota_unauthorized", "quota_unauthorized", None),
    QUOTA_UPSTREAM => ("quota_upstream", "quota_upstream", None),
    REASONING_LEVELS_INVALID => ("reasoning_levels_invalid", "reasoning_levels_invalid", None),
    RECOVERY_REQUIRED => ("recovery_required", "recovery_required", None),
    REFRESH_EXCHANGE_FAILED => ("refresh_exchange_failed", "refresh_exchange_failed", None),
    REFRESH_EXCHANGE_UNAVAILABLE => ("refresh_exchange_unavailable", "refresh_exchange_unavailable", None),
    REFRESH_LOCK_TIMEOUT => ("refresh_lock_timeout", "refresh_lock_timeout", None),
    REFRESH_LOCK_CONFIGURATION => ("refresh_lock_configuration", "refresh_lock_configuration", None),
    REFRESH_LOCK_UNAVAILABLE => ("refresh_lock_unavailable", "refresh_lock_unavailable", None),
    REFRESH_TOKEN_EXPIRED => ("refresh_token_expired", "refresh_token_expired", None),
    REFRESH_TOKEN_INVALIDATED => ("refresh_token_invalidated", "refresh_token_invalidated", None),
    REFRESH_TOKEN_MISSING => ("refresh_token_missing", "refresh_token_missing", None),
    REFRESH_TOKEN_REUSED => ("refresh_token_reused", "refresh_token_reused", None),
    REMOTE_MISSING => ("remote_missing", "remote_missing", None),
    REQUEST_TIMEOUT => ("request_timeout", "request_timeout", None),
    REQUEST_TOO_LARGE => ("request_too_large", "request_too_large", None),
    REQUEST_ENCODING_INVALID => ("request_encoding_invalid", "request_encoding_invalid", None),
    REQUEST_ENCODING_UNSUPPORTED => ("request_encoding_unsupported", "request_encoding_unsupported", None),
    RESET_CREDITS_FAILED => ("reset_credits_failed", "reset_credits_failed", None),
    RESPONSE_AFFINITY_MISS => ("response_affinity_miss", "response_affinity_miss", Some((400, "Responses continuation route is unavailable"))),
    RESPONSE_AFFINITY_PERSISTENCE_FAILED => ("response_affinity_persistence_failed", "response_affinity_persistence_failed", None),
    RESPONSE_CONTINUATION_UNAVAILABLE => ("response_continuation_unavailable", "response_continuation_unavailable", None),
    RESPONSE_INCOMPLETE => ("response_incomplete", "response_incomplete", None),
    RUNTIME_RELOAD_FAILED => ("runtime_reload_failed", "runtime_reload_failed", None),
    RUNTIME_UNAVAILABLE => ("runtime_unavailable", "runtime_unavailable", None),
    SECRET_INVALID => ("secret_invalid", "secret_invalid", None),
    SECRET_MISSING => ("secret_missing", "secret_missing", None),
    SECRET_SERIALIZE => ("secret_serialize", "secret_serialize", None),
    SECRET_STORE_UNAVAILABLE => ("secret_store_unavailable", "secret_store_unavailable", None),
    SESSION_COLLISION => ("session_collision", "session_collision", None),
    SESSION_NOT_FOUND => ("session_not_found", "session_not_found", None),
    SNAPSHOT_INVALID => ("snapshot_invalid", "snapshot_invalid", None),
    SNAPSHOT_IO => ("snapshot_io", "snapshot_io", None),
    SNAPSHOT_MISMATCH => ("snapshot_mismatch", "snapshot_mismatch", None),
    SNAPSHOT_MISSING => ("snapshot_missing", "snapshot_missing", None),
    SNAPSHOT_UNSAFE => ("snapshot_unsafe", "snapshot_unsafe", None),
    SOURCE_BASE_URL_INVALID => ("source_base_url_invalid", "source_base_url_invalid", None),
    SOURCE_INVALID => ("source_invalid", "source_invalid", None),
    SOURCE_MODEL_DISCOVERY_FAILED => ("source_model_discovery_failed", "source_model_discovery_failed", None),
    SOURCE_MODEL_PRICE_INVALID => ("source_model_price_invalid", "source_model_price_invalid", None),
    SOURCE_NOT_FOUND => ("source_not_found", "source_not_found", None),
    SOURCE_POOL_PROTOCOL_UNSUPPORTED => ("source_pool_protocol_unsupported", "source_pool_protocol_unsupported", None),
    SOURCE_PRICING_IDENTITY_INVALID => ("source_pricing_identity_invalid", "source_pricing_identity_invalid", None),
    SOURCE_PRIORITY_TARGET_NOT_FOUND => ("source_priority_target_not_found", "source_priority_target_not_found", None),
    SOURCE_PROTOCOL_INVALID => ("source_protocol_invalid", "source_protocol_invalid", None),
    SOURCE_RECOVERY_DELAY_INVALID => ("source_recovery_delay_invalid", "source_recovery_delay_invalid", None),
    SOURCE_RUNTIME_INVALID => ("source_runtime_invalid", "source_runtime_invalid", None),
    SOURCE_SECRET_MISSING => ("source_secret_missing", "source_secret_missing", None),
    SOURCE_SECRET_STORE_FAILED => ("source_secret_store_failed", "source_secret_store_failed", None),
    SOURCE_SELF_ROUTE => ("source_self_route", "source_self_route", None),
    SOURCE_STATS_UNAVAILABLE => ("source_stats_unavailable", "source_stats_unavailable", None),
    SOURCE_STORE_FAILED => ("source_store_failed", "source_store_failed", None),
    SOURCE_TEST_FAILED => ("source_test_failed", "source_test_failed", None),
    SOURCE_PROBE_UNAVAILABLE => ("source_probe_unavailable", "source_probe_unavailable", None),
    SOURCE_PROBE_UNSUPPORTED => ("source_probe_unsupported", "source_probe_unsupported", None),
    SOURCE_PROBE_INVALID_RESPONSE => ("source_probe_invalid_response", "source_probe_invalid_response", None),
    SOURCE_PROBE_STALE => ("source_probe_stale", "source_probe_stale", None),
    STORE_FAILED => ("store_failed", "store_failed", None),
    STREAM_ERROR => ("stream_error", "stream_error", None),
    STREAM_EVENT_TOO_LARGE => ("stream_event_too_large", "stream_event_too_large", None),
    STREAM_FIRST_OUTPUT_TIMEOUT => ("stream_first_output_timeout", "stream_first_output_timeout", None),
    STREAM_IDLE_TIMEOUT => ("stream_idle_timeout", "stream_idle_timeout", None),
    STREAM_INCOMPLETE => ("stream_incomplete", "stream_incomplete", None),
    STREAM_INVALID => ("stream_invalid", "stream_invalid", None),
    STREAM_SEMANTIC_TIMEOUT => ("stream_semantic_timeout", "stream_semantic_timeout", None),
    SUBSCRIPTION_ACCESS_TOKEN_INVALID => ("subscription_access_token_invalid", "subscription_access_token_invalid", None),
    SUBSCRIPTION_ACCOUNT_ID_INVALID => ("subscription_account_id_invalid", "subscription_account_id_invalid", None),
    SUBSCRIPTION_ACCOUNT_MISSING => ("subscription_account_missing", "subscription_account_missing", None),
    SUBSCRIPTION_CONFIGURATION => ("subscription_configuration", "subscription_configuration", None),
    SUBSCRIPTION_FORBIDDEN => ("subscription_forbidden", "subscription_forbidden", None),
    SUBSCRIPTION_HTTP_STATUS => ("subscription_http_status", "subscription_http_status", None),
    SUBSCRIPTION_INVALID_RESPONSE => ("subscription_invalid_response", "subscription_invalid_response", None),
    SUBSCRIPTION_RATE_LIMITED => ("subscription_rate_limited", "subscription_rate_limited", None),
    SUBSCRIPTION_RESPONSE_TOO_LARGE => ("subscription_response_too_large", "subscription_response_too_large", None),
    SUBSCRIPTION_TRANSPORT => ("subscription_transport", "subscription_transport", None),
    SUBSCRIPTION_UNAUTHORIZED => ("subscription_unauthorized", "subscription_unauthorized", None),
    SUBSCRIPTION_UPSTREAM => ("subscription_upstream", "subscription_upstream", None),
    SYSTEM_KEY_MISSING => ("system_key_missing", "system_key_missing", None),
    TOKEN_AUTHORITY_FAILED => ("token_authority_failed", "token_authority_failed", None),
    TOKEN_INVALIDATED => ("token_invalidated", "token_invalidated", None),
    TOO_MANY_ITEMS => ("too_many_items", "too_many_items", None),
    TOOL_USE_NOT_SUPPORTED => ("tool_use_not_supported", "tool_use_not_supported", None),
    UNKNOWN_AUTH_MODE => ("unknown_auth_mode", "unknown_auth_mode", None),
    UNSUPPORTED_BUNDLE_VERSION => ("unsupported_bundle_version", "unsupported_bundle_version", None),
    UNSUPPORTED_SCHEMA => ("unsupported_schema", "unsupported_schema", None),
    UNSUPPORTED_SNAPSHOT_VERSION => ("unsupported_snapshot_version", "unsupported_snapshot_version", None),
    UNSUPPORTED_VALUE => ("unsupported_value", "unsupported_value", None),
    UPSTREAM_ACCOUNT_DISABLED => ("upstream_account_disabled", "account_deactivated", Some((403, "upstream account is disabled"))),
    UPSTREAM_ACCOUNT_VERIFICATION_REQUIRED => ("upstream_account_verification_required", "account_verification_required", Some((403, "upstream account verification is required"))),
    UPSTREAM_BAD_GATEWAY => ("upstream_bad_gateway", "bad_gateway", Some((502, "upstream gateway failed"))),
    UPSTREAM_BODY => ("upstream_body", "upstream_body", None),
    UPSTREAM_BODY_TOO_LARGE => ("upstream_body_too_large", "upstream_body_too_large", None),
    UPSTREAM_CANCELLED => ("upstream_cancelled", "upstream_cancelled", None),
    UPSTREAM_CANDIDATE_REJECTED => ("upstream_candidate_rejected", "source_rejected", Some((503, "upstream source rejected this model request"))),
    UPSTREAM_CONFLICT => ("upstream_conflict", "conflict", Some((409, "upstream request conflicted with current state"))),
    UPSTREAM_CONTENT_POLICY => ("upstream_content_policy", "content_policy_violation", Some((400, "upstream content policy rejected the request"))),
    UPSTREAM_CONTEXT_TOO_LARGE => ("upstream_context_too_large", "context_too_large", Some((400, "request context exceeds the model limit"))),
    UPSTREAM_EDGE_CHALLENGE => ("upstream_edge_challenge", "edge_security_challenge", Some((503, "upstream edge security challenged the request"))),
    UPSTREAM_ENCRYPTED_CONTENT_INVALID => ("upstream_encrypted_content_invalid", "invalid_encrypted_content", Some((400, "encrypted reasoning context is invalid"))),
    UPSTREAM_ERROR => ("upstream_error", "upstream_error", None),
    UPSTREAM_FAILURE => ("upstream_failure", "upstream_failure", None),
    UPSTREAM_FORBIDDEN => ("upstream_forbidden", "permission_denied", Some((403, "upstream access was forbidden"))),
    UPSTREAM_GATEWAY_TIMEOUT => ("upstream_gateway_timeout", "gateway_timeout", Some((504, "upstream gateway timed out"))),
    UPSTREAM_INSTRUCTIONS_REQUIRED => ("upstream_instructions_required", "missing_required_parameter", Some((400, "upstream requires request instructions"))),
    UPSTREAM_INVALID_REQUEST => ("upstream_invalid_request", "invalid_request", Some((400, "upstream rejected the request"))),
    UPSTREAM_MODEL_CAPACITY => ("upstream_model_capacity", "model_at_capacity", Some((503, "upstream model is at capacity"))),
    UPSTREAM_MODEL_NOT_FOUND => ("upstream_model_not_found", "model_not_found", Some((404, "upstream model is unavailable"))),
    UPSTREAM_MODEL_UNAVAILABLE => ("upstream_model_unavailable", "model_not_available", Some((503, "upstream model is temporarily unavailable"))),
    UPSTREAM_MODEL_UNSUPPORTED => ("upstream_model_unsupported", "model_not_supported", Some((406, "upstream does not support this model"))),
    UPSTREAM_NOT_FOUND => ("upstream_not_found", "not_found", Some((404, "upstream resource was not found"))),
    UPSTREAM_OVERLOADED => ("upstream_overloaded", "server_is_overloaded", Some((503, "upstream service is overloaded"))),
    UPSTREAM_PAYLOAD_TOO_LARGE => ("upstream_payload_too_large", "request_too_large", Some((413, "upstream rejected the request size"))),
    UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND => ("upstream_previous_response_not_found", "previous_response_not_found", Some((400, "previous response is unavailable"))),
    UPSTREAM_QUOTA_EXHAUSTED => ("upstream_quota_exhausted", "insufficient_quota", Some((429, "upstream usage quota is exhausted"))),
    UPSTREAM_RATE_LIMITED => ("upstream_rate_limited", "rate_limit_exceeded", Some((429, "upstream rate limit was reached"))),
    UPSTREAM_REFRESH_TOKEN_REUSED => ("upstream_refresh_token_reused", "upstream_refresh_token_reused", None),
    UPSTREAM_REGION_UNSUPPORTED => ("upstream_region_unsupported", "unsupported_country_region_territory", Some((403, "upstream rejected the request region"))),
    UPSTREAM_REQUEST_TIMEOUT => ("upstream_request_timeout", "request_timeout", Some((408, "upstream request timed out"))),
    UPSTREAM_SERVER_ERROR => ("upstream_server_error", "internal_server_error", Some((500, "upstream service failed"))),
    UPSTREAM_STATUS => ("upstream_status", "upstream_status", None),
    UPSTREAM_STREAM => ("upstream_stream", "upstream_stream", None),
    UPSTREAM_TERMINAL => ("upstream_terminal", "upstream_terminal", None),
    UPSTREAM_TOOL_CALL_MISMATCH => ("upstream_tool_call_mismatch", "tool_call_not_found", Some((400, "tool output does not match an active tool call"))),
    UPSTREAM_TRANSPORT => ("upstream_transport", "upstream_transport", None),
    UPSTREAM_TRANSPORT_BODY => ("upstream_transport_body", "upstream_transport_body", None),
    UPSTREAM_TRANSPORT_CONNECT => ("upstream_transport_connect", "upstream_transport_connect", None),
    UPSTREAM_TRANSPORT_REQUEST => ("upstream_transport_request", "upstream_transport_request", None),
    UPSTREAM_TRANSPORT_TIMEOUT => ("upstream_transport_timeout", "upstream_transport_timeout", None),
    UPSTREAM_UNAUTHORIZED => ("upstream_unauthorized", "invalid_api_key", Some((401, "upstream authentication failed"))),
    UPSTREAM_UNAVAILABLE => ("upstream_unavailable", "service_unavailable", Some((503, "upstream service is unavailable"))),
    UPSTREAM_UNSUPPORTED_REQUEST => ("upstream_unsupported_request", "unsupported_request", Some((400, "upstream does not support this request"))),
    UPSTREAM_USAGE_NOT_INCLUDED => ("upstream_usage_not_included", "usage_not_included", Some((403, "upstream account plan does not include this capability"))),
    UPSTREAM_WEBSOCKET => ("upstream_websocket", "upstream_websocket", None),
    UPSTREAM_WEBSOCKET_CLOSED => ("upstream_websocket_closed", "upstream_websocket_closed", None),
    UPSTREAM_WEBSOCKET_CONNECTION_LIMIT => ("upstream_websocket_connection_limit", "websocket_connection_limit_reached", Some((429, "upstream WebSocket connection limit was reached"))),
    UPSTREAM_WEBSOCKET_UNSUPPORTED => ("upstream_websocket_unsupported", "websocket_not_supported", Some((426, "upstream does not support WebSocket requests"))),
    USAGE_PERSISTENCE_FAILED => ("usage_persistence_failed", "usage_persistence_failed", None),
    USE_SOURCE_IMPORT => ("use_source_import", "use_source_import", None),
    VAULT_FAILED => ("vault_failed", "vault_failed", None),
    WAKE_ACCOUNT_MISSING => ("wake_account_missing", "wake_account_missing", None),
    WAKE_CONFIRMATION_UNSUPPORTED => ("wake_confirmation_unsupported", "wake_confirmation_unsupported", None),
    WAKE_CREDENTIALS_UNAVAILABLE => ("wake_credentials_unavailable", "wake_credentials_unavailable", None),
    WAKE_FORBIDDEN => ("wake_forbidden", "wake_forbidden", None),
    WAKE_HTTP_STATUS => ("wake_http_status", "wake_http_status", None),
    WAKE_INVALID_ACCESS_TOKEN => ("wake_invalid_access_token", "wake_invalid_access_token", None),
    WAKE_INVALID_CONFIGURATION => ("wake_invalid_configuration", "wake_invalid_configuration", None),
    WAKE_INVALID_ENDPOINT => ("wake_invalid_endpoint", "wake_invalid_endpoint", None),
    WAKE_INVALID_PROVIDER_ACCOUNT_ID => ("wake_invalid_provider_account_id", "wake_invalid_provider_account_id", None),
    WAKE_INVALID_REQUEST => ("wake_invalid_request", "wake_invalid_request", None),
    WAKE_INVALID_RESPONSE => ("wake_invalid_response", "wake_invalid_response", None),
    WAKE_MODEL_UNAVAILABLE => ("wake_model_unavailable", "wake_model_unavailable", None),
    WAKE_PROXY_UNAVAILABLE => ("wake_proxy_unavailable", "wake_proxy_unavailable", None),
    WAKE_RATE_LIMITED => ("wake_rate_limited", "wake_rate_limited", None),
    WAKE_REQUEST_TOO_LARGE => ("wake_request_too_large", "wake_request_too_large", None),
    WAKE_RESPONSE_TOO_LARGE => ("wake_response_too_large", "wake_response_too_large", None),
    WAKE_TAGS_UNSUPPORTED => ("wake_tags_unsupported", "wake_tags_unsupported", None),
    WAKE_TASK_NOT_FOUND => ("wake_task_not_found", "wake_task_not_found", None),
    WAKE_TIMEOUT => ("wake_timeout", "wake_timeout", None),
    WAKE_TRANSPORT => ("wake_transport", "wake_transport", None),
    WAKE_UNAUTHORIZED => ("wake_unauthorized", "wake_unauthorized", None),
    WAKE_UPSTREAM => ("wake_upstream", "wake_upstream", None),
    WEBSOCKET_IDLE_TIMEOUT => ("websocket_idle_timeout", "websocket_idle_timeout", None),
}

pub fn public_code(code: &str) -> &str {
    definition(code).map_or(code, |error| error.public_code)
}

pub fn public_type(status: u16, code: &str) -> &'static str {
    if code == "insufficient_quota" {
        return "insufficient_quota";
    }
    match status {
        401 => "authentication_error",
        403 => "permission_error",
        429 => "rate_limit_error",
        500..=599 => "server_error",
        _ => "invalid_request_error",
    }
}

pub fn upstream_message(code: &str) -> &'static str {
    definition(code)
        .and_then(|error| error.upstream)
        .map_or("all eligible upstream sources failed", |(_, message)| {
            message
        })
}

pub fn upstream_status(code: &str) -> u16 {
    definition(code)
        .and_then(|error| error.upstream)
        .map_or(502, |(status, _)| status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn definitions_are_unique_valid_and_documented() {
        let mut seen = HashSet::new();
        let guides = [
            include_str!("../../../docs/help/en/README.md"),
            include_str!("../../../docs/help/ru/README.md"),
        ];
        for error in ALL {
            assert!(seen.insert(error.code), "duplicate code: {}", error.code);
            assert_eq!(
                crate::normalize_error_code(error.code).as_deref(),
                Some(error.code)
            );
            assert_eq!(definition(error.code), Some(*error));
            if let Some((status, message)) = error.upstream {
                assert!((400..600).contains(&status), "{}", error.code);
                assert!(!message.is_empty(), "{}", error.code);
            }
            for guide in guides {
                for code in [error.code, error.public_code] {
                    assert!(
                        guide
                            .lines()
                            .filter(|line| line.starts_with('|'))
                            .any(|line| {
                                let cells = line.split('|').skip(1).take(3).collect::<Vec<_>>();
                                cells.len() == 3
                                    && cells[0].contains(&format!("`{code}`"))
                                    && !cells[1].trim().is_empty()
                                    && !cells[2].trim().is_empty()
                            }),
                        "error has no cause and recovery row: {code}"
                    );
                }
            }
        }
    }

    #[test]
    fn unknown_provider_errors_remain_unknown() {
        assert_eq!(definition("future_provider_failure"), None);
        assert_eq!(
            public_code("future_provider_failure"),
            "future_provider_failure"
        );
        assert_eq!(upstream_status("future_provider_failure"), 502);
    }

    #[test]
    fn public_envelopes_distinguish_quota_rate_limit_and_authentication() {
        for (category, code, status, kind) in [
            (
                UPSTREAM_QUOTA_EXHAUSTED,
                "insufficient_quota",
                429,
                "insufficient_quota",
            ),
            (
                UPSTREAM_RATE_LIMITED,
                "rate_limit_exceeded",
                429,
                "rate_limit_error",
            ),
            (
                UPSTREAM_UNAUTHORIZED,
                "invalid_api_key",
                401,
                "authentication_error",
            ),
            (
                UPSTREAM_MODEL_CAPACITY,
                "model_at_capacity",
                503,
                "server_error",
            ),
            (
                UPSTREAM_CONTEXT_TOO_LARGE,
                "context_too_large",
                400,
                "invalid_request_error",
            ),
        ] {
            assert_eq!(public_code(category), code);
            assert_eq!(upstream_status(category), status);
            assert_eq!(public_type(status, code), kind);
        }
    }
}
