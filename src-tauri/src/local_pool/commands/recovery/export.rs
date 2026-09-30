use super::{SupportBundle, SupportContext, UsageExportRow, MAX_EXPORT_TEXT};
use crate::files::atomic_write;
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError};
use serde::Serialize;
#[cfg(unix)]
use std::fs;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;
use zenith_relay_core::accounts::AccountExportDocument;

pub(super) fn support_bundle(context: SupportContext) -> SupportBundle {
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

pub(super) fn write_export(
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
    atomic_write(&path, &format!("{content}\n")).map_err(super::io_error)?;
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
    atomic_write(&path, &document.content).map_err(super::io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(super::io_error)?;
    }
    Ok(Some(path.to_string_lossy().into_owned()))
}

pub(super) fn invalid_export_row(row: &UsageExportRow) -> bool {
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

pub(super) fn invalid_text(value: &str) -> bool {
    value.len() > MAX_EXPORT_TEXT || value.chars().any(char::is_control)
}
