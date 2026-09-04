use crate::files::atomic_write;
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
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use zenith_relay_core::{
    accounts::AccountExportDocument, DefaultServiceTier, ErrorOrigin, ObservedServiceTier,
};

const MAX_EXPORT_ROWS: usize = 500;
const MAX_EXPORT_TEXT: usize = 512;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayFolder {
    Data,
    ProfileBackups,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageExportRow {
    time: String,
    success: bool,
    model: Option<String>,
    #[serde(default)]
    requested_reasoning_effort: Option<String>,
    #[serde(default)]
    effective_reasoning_effort: Option<String>,
    connection: String,
    latency_ms: u64,
    ttft_ms: Option<u64>,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
    #[serde(default)]
    cache_write_ttl: Option<zenith_relay_core::CacheWriteTtl>,
    reasoning_tokens: Option<u64>,
    output_tokens: Option<u64>,
    tokens: Option<u64>,
    request_id: Option<String>,
    http_status: Option<u16>,
    error_category: Option<String>,
    #[serde(default)]
    error_origin: Option<ErrorOrigin>,
    service_tier: Option<DefaultServiceTier>,
    applied_service_tier: Option<ObservedServiceTier>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SupportBundle {
    generated_at: String,
    app_version: &'static str,
    platform: &'static str,
    mode: SupportMode,
    schema_version: Option<u32>,
    gateway_running: bool,
    source_count: usize,
    account_count: usize,
    key_count: usize,
    automation_count: usize,
    usage_count: usize,
    warning_count: usize,
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
    mode: SupportMode,
    schema_version: Option<u32>,
    gateway_running: bool,
    source_count: usize,
    account_count: usize,
    key_count: usize,
    automation_count: usize,
    usage_count: usize,
    warning_count: usize,
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
}

#[tauri::command]
pub fn get_relay_storage_info(state: State<'_, DesktopState>) -> RelayStorageInfo {
    RelayStorageInfo {
        data_path: state.data_root().to_string_lossy().into_owned(),
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
        RelayFolder::ProfileBackups => state.profile_backup_root(),
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

fn support_bundle(context: SupportContext) -> SupportBundle {
    SupportBundle {
        generated_at: chrono::Utc::now().to_rfc3339(),
        app_version: env!("CARGO_PKG_VERSION"),
        platform: crate::platform::platform_name(),
        mode: context.mode,
        schema_version: context.schema_version,
        gateway_running: context.gateway_running,
        source_count: context.source_count,
        account_count: context.account_count,
        key_count: context.key_count,
        automation_count: context.automation_count,
        usage_count: context.usage_count,
        warning_count: context.warning_count,
    }
}

fn write_export(
    prefix: &str,
    value: &impl Serialize,
    app: &AppHandle,
) -> Result<Option<String>, CommandError> {
    let filename = format!(
        "{prefix}-{}.json",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    );
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .set_file_name(filename)
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = path.into_path().map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "selected export path is invalid")
    })?;
    let content = serde_json::to_string_pretty(value).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            format!("failed to serialize export: {error}"),
        )
    })?;
    atomic_write(&path, &format!("{content}\n")).map_err(io_error)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

pub(crate) fn write_account_export(
    document: &AccountExportDocument,
    app: &AppHandle,
) -> Result<Option<String>, CommandError> {
    document.validate().map_err(LocalPoolError::invalid_state)?;
    let filename = format!(
        "{}-{}-{}.json",
        if document.account_count == 1 {
            "account"
        } else {
            "accounts"
        },
        document.format.slug(),
        chrono::Utc::now().format("%Y%m%d-%H%M%S-%f")
    );
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .set_file_name(filename)
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = path.into_path().map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "selected export path is invalid")
    })?;
    atomic_write(&path, &document.content).map_err(io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(io_error)?;
    }
    Ok(Some(path.to_string_lossy().into_owned()))
}

fn invalid_export_row(row: &UsageExportRow) -> bool {
    [&row.time, &row.connection]
        .into_iter()
        .any(|value| invalid_text(value))
        || [
            row.model.as_deref(),
            row.request_id.as_deref(),
            row.error_category.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(invalid_text)
        || [
            row.requested_reasoning_effort.as_deref(),
            row.effective_reasoning_effort.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|value| zenith_relay_core::normalize_reasoning_effort(value).is_none())
}

fn invalid_text(value: &str) -> bool {
    value.len() > MAX_EXPORT_TEXT || value.chars().any(char::is_control)
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
    fn export_validation_rejects_control_text_and_oversized_fields() {
        let mut row = UsageExportRow {
            time: "2026-07-11T00:00:00Z".into(),
            success: true,
            model: Some("gpt-test".into()),
            requested_reasoning_effort: Some("max".into()),
            effective_reasoning_effort: Some("low".into()),
            connection: "account".into(),
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
