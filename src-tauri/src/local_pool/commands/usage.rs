use super::pricing_context;
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    state::DesktopState,
    store::telemetry_db::{CacheSession, LocalUsagePage},
};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;
use zenith_relay_core::protocol::UsageQuery;

#[tauri::command]
pub async fn get_local_usage_page(
    input: Option<UsageQuery>,
    state: State<'_, DesktopState>,
) -> Result<LocalUsagePage, CommandError> {
    // SQLite and pricing must not occupy the desktop event-loop thread.
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || load_local_usage_page(input, &state))
        .await
        .map_err(|_| LocalPoolError::new(ErrorCode::Io, "Local usage read was interrupted"))?
}

fn load_local_usage_page(
    usage_query: Option<UsageQuery>,
    state: &DesktopState,
) -> Result<LocalUsagePage, CommandError> {
    let (gateway, sources, accounts) = {
        let store = state.store()?;
        (
            store.gateway().clone(),
            store.sources().to_vec(),
            store.accounts().to_vec(),
        )
    };
    let catalog = state.pricing_catalog();
    let reference_catalog = state.model_metadata_catalog();
    let context = pricing_context(&gateway, &sources, &accounts, &reference_catalog);
    let mut page = state
        .telemetry
        .usage_page_with_pricing(
            &normalize_usage_query(usage_query.unwrap_or_default()),
            &catalog,
            &context,
        )
        .map_err(CommandError::from)?;
    page.pricing.catalog_status = state.pricing_status();
    Ok(page)
}

#[tauri::command]
pub fn clear_local_usage(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    state.telemetry.clear().map_err(Into::into)
}

#[tauri::command]
pub fn get_local_cache_sessions(
    input: Option<UsageQuery>,
    state: State<'_, DesktopState>,
) -> Result<Vec<CacheSession>, CommandError> {
    let usage_query = input;
    state
        .telemetry
        .cache_sessions(&normalize_usage_query(usage_query.unwrap_or_default()))
        .map_err(CommandError::from)
}

fn normalize_usage_query(mut query: UsageQuery) -> UsageQuery {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default();
    query.normalize_at(now);
    query
}
