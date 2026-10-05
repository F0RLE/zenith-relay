use crate::local_pool::{
    accounts::{
        credentials::CredentialStore,
        proxy::{COMMON_PROXY_SECRET_REF, PROXY_POOL_SECRET_REF},
        NativeSecretBackend,
    },
    commands::profiles::restore_managed_profiles_before_reset,
    error::{CommandError, ErrorCode, LocalPoolError},
    state::DesktopState,
    store::secret_store,
};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
use zenith_relay_core::{DefaultServiceTier, ErrorOrigin, ObservedServiceTier};

mod export;
pub(crate) use export::write_account_export;
use export::{invalid_export_row, support_bundle, write_export};

const MAX_EXPORT_ROWS: usize = 500;
const MAX_EXPORT_TEXT: usize = 512;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayFolder {
    Data,
    Logs,
    ErrorLogs,
    CrashLogs,
    OperationLogs,
    ProfileBackups,
    #[serde(rename = "opencode_backups")]
    OpenCodeBackups,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageExportRow {
    pub(super) time: String,
    pub(super) success: bool,
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) requested_reasoning_effort: Option<String>,
    #[serde(default)]
    pub(super) effective_reasoning_effort: Option<String>,
    pub(super) connection: String,
    pub(super) transport: zenith_relay_core::UsageTransport,
    pub(super) latency_ms: u64,
    pub(super) ttft_ms: Option<u64>,
    pub(super) input_tokens: Option<u64>,
    pub(super) cached_input_tokens: Option<u64>,
    pub(super) cache_write_input_tokens: Option<u64>,
    #[serde(default)]
    pub(super) cache_write_ttl: Option<zenith_relay_core::CacheWriteTtl>,
    pub(super) reasoning_tokens: Option<u64>,
    pub(super) output_tokens: Option<u64>,
    pub(super) tokens: Option<u64>,
    pub(super) request_id: Option<String>,
    pub(super) http_status: Option<u16>,
    pub(super) error_category: Option<String>,
    #[serde(default)]
    pub(super) error_origin: Option<ErrorOrigin>,
    pub(super) service_tier: Option<DefaultServiceTier>,
    pub(super) applied_service_tier: Option<ObservedServiceTier>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SupportBundle {
    pub(super) generated_at: String,
    pub(super) app_version: &'static str,
    pub(super) platform: &'static str,
    pub(super) mode: SupportMode,
    pub(super) schema_version: Option<u32>,
    pub(super) gateway_running: bool,
    pub(super) source_count: usize,
    pub(super) account_count: usize,
    pub(super) key_count: usize,
    pub(super) automation_count: usize,
    pub(super) usage_count: usize,
    pub(super) warning_count: usize,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportMode {
    Local,
    Remote,
    Zenith,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SupportContext {
    pub(super) mode: SupportMode,
    pub(super) schema_version: Option<u32>,
    pub(super) gateway_running: bool,
    pub(super) source_count: usize,
    pub(super) account_count: usize,
    pub(super) key_count: usize,
    pub(super) automation_count: usize,
    pub(super) usage_count: usize,
    pub(super) warning_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportBundlePreview {
    bundle: SupportBundle,
    excluded: [&'static str; 5],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayStorageInfo {
    data_path: String,
    logs_path: String,
    error_logs_path: String,
    crash_logs_path: String,
    operation_logs_path: String,
}

#[tauri::command]
pub fn get_relay_storage_info(state: State<'_, DesktopState>) -> RelayStorageInfo {
    RelayStorageInfo {
        data_path: state.data_root().to_string_lossy().into_owned(),
        logs_path: state.logs_root().to_string_lossy().into_owned(),
        error_logs_path: state.error_logs_root().to_string_lossy().into_owned(),
        crash_logs_path: state.crash_logs_root().to_string_lossy().into_owned(),
        operation_logs_path: state.operation_logs_root().to_string_lossy().into_owned(),
    }
}

#[tauri::command]
pub fn open_relay_folder(
    folder: RelayFolder,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    let path = match folder {
        RelayFolder::Data => state.data_root(),
        RelayFolder::Logs => state.logs_root(),
        RelayFolder::ErrorLogs => state.error_logs_root(),
        RelayFolder::CrashLogs => state.crash_logs_root(),
        RelayFolder::OperationLogs => state.operation_logs_root(),
        RelayFolder::ProfileBackups => state.profile_backup_root(),
        RelayFolder::OpenCodeBackups => state.opencode_backup_root(),
    };
    fs::create_dir_all(&path).map_err(io_error)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|error| io_error(error.to_string()))
}

#[tauri::command]
pub async fn reset_local_pool_data(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    restore_managed_profiles_before_reset(&state).await?;
    state.gateway.stop().await;
    let (source_refs, account_ids, key_refs) = {
        let mut store = state.store()?;
        let refs = (
            store
                .sources()
                .iter()
                .map(|source| source.secret_ref.clone())
                .collect::<Vec<_>>(),
            store
                .accounts()
                .iter()
                .map(|account| account.account.id.clone())
                .collect::<Vec<_>>(),
            store
                .keys()
                .iter()
                .map(|key| key.secret_ref.clone())
                .collect::<Vec<_>>(),
        );
        store.reset_local_records()?;
        refs
    };
    state.telemetry.clear()?;

    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let mut failed = false;
    for secret_ref in source_refs.into_iter().chain(key_refs) {
        failed |= secret_store::delete(&secret_ref).is_err();
    }
    for account_id in account_ids {
        failed |= credentials.delete(&account_id).is_err();
    }
    failed |= secret_store::delete(COMMON_PROXY_SECRET_REF).is_err();
    failed |= secret_store::delete(PROXY_POOL_SECRET_REF).is_err();
    remove_transient_dir(state.transient_root().join("imports"), &mut failed);
    remove_transient_dir(state.transient_root().join("oauth_pending"), &mut failed);
    if failed {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "local records were reset, but some protected or transient data could not be removed",
        )
        .into());
    }
    Ok(())
}

#[tauri::command]
pub fn export_usage(
    rows: Vec<UsageExportRow>,
    app: AppHandle,
) -> Result<Option<String>, CommandError> {
    if rows.len() > MAX_EXPORT_ROWS || rows.iter().any(invalid_export_row) {
        return Err(LocalPoolError::new(ErrorCode::InvalidState, "usage export is invalid").into());
    }
    write_export("usage", &rows, &app)
}

#[tauri::command]
pub fn export_support_bundle(
    context: SupportContext,
    app: AppHandle,
) -> Result<Option<String>, CommandError> {
    let bundle = support_bundle(context);
    write_export("support", &bundle, &app)
}

#[tauri::command]
pub fn preview_support_bundle(context: SupportContext) -> SupportBundlePreview {
    SupportBundlePreview {
        bundle: support_bundle(context),
        excluded: [
            "secrets",
            "prompts",
            "responses",
            "raw_identities",
            "raw_headers",
        ],
    }
}

fn remove_transient_dir(path: impl AsRef<Path>, failed: &mut bool) {
    let path = path.as_ref();
    if path.exists() {
        *failed |= fs::remove_dir_all(path).is_err();
    }
}

fn io_error(error: impl ToString) -> CommandError {
    LocalPoolError::new(ErrorCode::Io, error.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_folder_serializes_open_code_backup_directory_name() {
        let folder: RelayFolder = serde_json::from_str("\"opencode_backups\"").unwrap();
        assert!(matches!(folder, RelayFolder::OpenCodeBackups));
    }

    #[test]
    fn relay_folder_serializes_diagnostic_directories() {
        for (value, expected) in [
            ("logs", RelayFolder::Logs),
            ("error_logs", RelayFolder::ErrorLogs),
            ("crash_logs", RelayFolder::CrashLogs),
            ("operation_logs", RelayFolder::OperationLogs),
        ] {
            let parsed: RelayFolder =
                serde_json::from_str(&format!("\"{value}\"")).expect("diagnostic folder");
            assert!(matches!(
                (parsed, expected),
                (RelayFolder::Logs, RelayFolder::Logs)
                    | (RelayFolder::ErrorLogs, RelayFolder::ErrorLogs)
                    | (RelayFolder::CrashLogs, RelayFolder::CrashLogs)
                    | (RelayFolder::OperationLogs, RelayFolder::OperationLogs)
            ));
        }
    }

    #[test]
    fn export_validation_rejects_control_text_and_oversized_fields() {
        let mut row = UsageExportRow {
            time: "2026-07-11T00:00:00Z".into(),
            success: true,
            model: Some("gpt-test".into()),
            requested_reasoning_effort: Some("max".into()),
            effective_reasoning_effort: Some("low".into()),
            connection: "account".into(),
            transport: zenith_relay_core::UsageTransport::Http,
            latency_ms: 1,
            ttft_ms: Some(1),
            input_tokens: Some(1),
            cached_input_tokens: Some(1),
            cache_write_input_tokens: Some(1),
            cache_write_ttl: Some(zenith_relay_core::CacheWriteTtl::FiveMinutes),
            reasoning_tokens: Some(1),
            output_tokens: Some(1),
            tokens: Some(2),
            request_id: Some("request-test".into()),
            http_status: Some(200),
            error_category: None,
            error_origin: None,
            service_tier: Some(DefaultServiceTier::Fast),
            applied_service_tier: Some("default".into()),
        };
        assert!(!invalid_export_row(&row));
        row.connection = "bad\nvalue".into();
        assert!(invalid_export_row(&row));
        row.connection = "x".repeat(MAX_EXPORT_TEXT + 1);
        assert!(invalid_export_row(&row));
        row.connection = "account".into();
        row.effective_reasoning_effort = Some("untrusted".into());
        assert!(invalid_export_row(&row));
    }

    #[test]
    fn support_preview_contains_only_redacted_aggregate_fields() {
        let preview = preview_support_bundle(SupportContext {
            mode: SupportMode::Local,
            schema_version: Some(4),
            gateway_running: true,
            source_count: 1,
            account_count: 2,
            key_count: 1,
            automation_count: 1,
            usage_count: 5,
            warning_count: 0,
        });
        let encoded = serde_json::to_string(&preview).unwrap();
        assert!(encoded.contains("raw_identities"));
        for secret in [
            "synthetic-access-token",
            "private prompt",
            "generated response",
        ] {
            assert!(!encoded.contains(secret));
        }
    }
}
