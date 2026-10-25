use super::pricing_context;
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    state::DesktopState,
    store::telemetry_db::{CacheSession, CacheSessionScope, LocalUsagePage},
};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;
use zenith_relay_core::protocol::UsageQuery;

#[tauri::command]
pub async fn get_local_usage_page(
    input: Option<UsageQuery>,
    include_report: Option<bool>,
    state: State<'_, DesktopState>,
) -> Result<LocalUsagePage, CommandError> {
    // SQLite and pricing must not occupy the desktop event-loop thread.
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        load_local_usage_page(input, include_report.unwrap_or(true), &state)
    })
    .await
    .map_err(|_| LocalPoolError::new(ErrorCode::Io, "Local usage read was interrupted"))?
}

fn load_local_usage_page(
    usage_query: Option<UsageQuery>,
    include_report: bool,
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
    let query = normalize_usage_query(usage_query.unwrap_or_default());
    let mut page = if include_report {
        state
            .telemetry
            .usage_page_with_pricing(&query, &catalog, &context)
    } else {
        state
            .telemetry
            .usage_send_page_with_pricing(&query, &catalog, &context)
    }
    .map_err(CommandError::from)?;
    page.pricing.catalog_status = state.pricing_status();
    Ok(page)
}

#[tauri::command]
pub async fn clear_local_usage(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    let telemetry = state.telemetry.clone();
    super::run_blocking(move || telemetry.clear())
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn get_local_cache_sessions(
    input: Option<UsageQuery>,
    scope: Option<CacheSessionScope>,
    state: State<'_, DesktopState>,
) -> Result<Vec<CacheSession>, CommandError> {
    // Session history may scan SQLite. Never block the desktop event loop
    // while opening request details, even when another query owns the DB lock.
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state
            .telemetry
            .cache_sessions(
                &normalize_usage_query(input.unwrap_or_default()),
                scope.as_ref(),
            )
            .map_err(CommandError::from)
    })
    .await
    .map_err(|_| LocalPoolError::new(ErrorCode::Io, "Cache session read was interrupted"))?
}

fn normalize_usage_query(mut query: UsageQuery) -> UsageQuery {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default();
    query.normalize_at(now);
    query
}
