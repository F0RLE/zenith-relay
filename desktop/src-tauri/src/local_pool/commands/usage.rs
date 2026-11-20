use super::pricing_context;
use crate::local_pool::{
    background::weekly_capacity::subscription_ledgers,
    error::{CommandError, ErrorCode, LocalPoolError},
    state::DesktopState,
    store::telemetry_db::{
        CacheSession, CacheSessionScope, LocalUsagePage, WeeklyCapacityQuery, WeeklyCapacityReport,
    },
};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;
use zenith_relay_core::protocol::UsageQuery;

#[tauri::command]
pub async fn get_local_weekly_capacity(
    input: Option<WeeklyCapacityQuery>,
    state: State<'_, DesktopState>,
) -> Result<WeeklyCapacityReport, CommandError> {
    let state = state.inner().clone();
    super::run_blocking(move || {
        let now = zenith_relay_core::unix_time_ms();
        let query = input.unwrap_or_default();
        if query.to_ms.is_some_and(|to| to < now) {
            return state.telemetry.weekly_capacity_report(query, now, &[]);
        }
        let accounts = state.store()?.accounts().to_vec();
        let facts = match state.cached_account_credential_facts(&accounts) {
            Ok(facts) => facts,
            // A locked credential store prevents current attribution, not a history read.
            Err(error) if error.code == ErrorCode::SecretStoreUnavailable => {
                return state.telemetry.weekly_capacity_report(query, now, &[]);
            }
            Err(error) => return Err(error),
        };
        let revision = zenith_relay_core::pricing::PricingContext::default()
            .revision_key(&state.pricing_catalog());
        let ledgers = subscription_ledgers(&accounts, &facts, now);
        let observations = ledgers
            .iter()
            .map(|ledger| ledger.observation(&revision, now))
            .collect::<Vec<_>>();
        state
            .telemetry
            .weekly_capacity_report(query, now, &observations)
    })
    .await
    .map_err(Into::into)
}

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
