use crate::local_pool::{
    error::CommandError,
    state::DesktopState,
    store::telemetry_db::{LocalUsagePage, UsageLog},
};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;
use zenith_relay_core::protocol::{UsageQuery, UsageRange};

#[tauri::command]
pub fn get_local_usage(
    limit: Option<u16>,
    state: State<'_, DesktopState>,
) -> Result<Vec<UsageLog>, CommandError> {
    state
        .telemetry
        .list(limit.unwrap_or(100))
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_local_usage_page(
    input: Option<UsageQuery>,
    state: State<'_, DesktopState>,
) -> Result<LocalUsagePage, CommandError> {
    state
        .telemetry
        .usage_page(&normalize_usage_query(input.unwrap_or_default()))
        .map_err(Into::into)
}

#[tauri::command]
pub fn clear_local_usage(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    state.telemetry.clear().map_err(Into::into)
}

fn normalize_usage_query(mut query: UsageQuery) -> UsageQuery {
    query.page = query.page.max(1);
    query.page_size = if query.page_size == 0 {
        50
    } else {
        query.page_size.clamp(1, 200)
    };
    for value in [
        &mut query.model_query,
        &mut query.source_or_account_query,
        &mut query.local_key_query,
        &mut query.error_category,
        &mut query.request_id_query,
    ] {
        if let Some(text) = value {
            *text = text.trim().to_string();
            if text.is_empty() {
                *value = None;
            }
        }
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default();
    query.from_ms = match query.range {
        Some(UsageRange::Daily) => Some(now.saturating_sub(24 * 60 * 60 * 1_000)),
        Some(UsageRange::Weekly) => Some(now.saturating_sub(7 * 24 * 60 * 60 * 1_000)),
        Some(UsageRange::Monthly) => Some(now.saturating_sub(30 * 24 * 60 * 60 * 1_000)),
        Some(UsageRange::Custom) | None => query.from_ms,
    };
    query
}
