mod export;
mod import;
mod jwt;
mod login_notes;
mod quota_state;
mod record;
mod routes;
mod token_authority;

/// Upper bound for a user-entered account purchase cost in micro-USD.
/// This is presentation metadata used only for the payback ratio.
pub const MAX_PURCHASE_COST_MICRO_USD: u64 = 1_000_000_000_000;

/// Bounds for optional account tags accepted by portable account imports.
/// Tags are user metadata, not routing credentials; bounding them keeps a
/// malformed import from expanding a prepared snapshot.
pub const MAX_ACCOUNT_TAGS: usize = 32;
pub const MAX_ACCOUNT_TAG_CHARS: usize = 64;
pub const MAX_ACCOUNT_TAG_BYTES: usize = 2_048;

pub use export::{
    AccountExportCredential, AccountExportDocument, AccountExportFormat, AccountExportRequest,
    MAX_ACCOUNT_EXPORT_BYTES, MAX_ACCOUNT_EXPORT_DESCRIPTION_CHARS, MAX_ACCOUNT_EXPORT_ITEMS,
    build_account_export, normalize_account_export_description,
};
pub use import::{
    ImportAuthMode, ImportError, ImportErrorCode, ImportFormat, ImportIssue, ImportIssueCode,
    ImportPreview, ImportPreviewRow, ImportPreviewStatus, ImportQuotaStatus, ImportSecretMaterial,
    ImportWarning, ImportWarningCode, MAX_IMPORT_BYTES, MAX_IMPORT_ITEMS, MAX_JSON_DEPTH,
    ParsedImport, ParsedImportItem, chatgpt_token_identity_key, combine_import_documents,
    parse_import,
};
pub use jwt::decode_unverified_jwt_payload;
pub use login_notes::{
    normalize_login_email, normalize_login_password, normalize_login_phone,
    normalize_login_totp_secret,
};
pub use quota_state::{AccountQuotaOutcome, AccountQuotaUpdate, reduce_account_quota};
pub use record::{
    AccountAccessState, AccountAuthMode, AccountAuthState, AccountHealthState, AccountIdentity,
    AccountRecord, AccountUsageObservation, AccountUsageState, AccountUsageUpdate,
    ProviderAccountFailure, ReauthReason, accept_discovered_models, apply_model_discovery_failure,
    automatic_quota_monitoring_eligible, clear_false_upstream_block, provider_account_failure,
    recover_model_discovery_state, reduce_account_usage, repair_model_discovery_block,
};
pub use routes::{
    AccountRouteGroup, AccountRouteIdentity, detach_account_route, select_account_route_group,
    validate_account_route_connection, validate_account_route_groups,
};
pub use token_authority::{
    PrepareStatus, PreparedToken, TOKEN_REFRESH_SKEW_MS, TokenAuthority, TokenAuthorityError,
    TokenDispatchRevision, TokenDispatchRevisionGuard, TokenPersistenceAdapter,
    TokenPersistenceFailure, TokenRefresh, TokenRefreshAdapter, TokenRefreshFailure,
    TokenRefreshFailureKind, TokenSet, access_token_is_usable,
};
