use super::pricing_context;
use crate::local_pool::{
    error::CommandError,
    state::DesktopState,
    store::telemetry_db::{CacheSession, LocalUsagePage},
};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;
use zenith_relay_core::protocol::{UsageQuery, UsageRange};

#[tauri::command]
pub fn get_local_usage_page(
    input: Option<UsageQuery>,
    state: State<'_, DesktopState>,
) -> Result<LocalUsagePage, CommandError> {
    let usage_query = input;
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

fn normalize_usage_query(query: UsageQuery) -> UsageQuery {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default();
    normalize_usage_query_at(query, now)
}

fn normalize_usage_query_at(mut query: UsageQuery, now: u64) -> UsageQuery {
    query.normalize_pagination();
    for query_field in [
        &mut query.model_query,
        &mut query.source_or_account_query,
        &mut query.error_category,
        &mut query.request_id_query,
        &mut query.request_id,
    ] {
        if let Some(text) = query_field {
            *text = text.trim().to_string();
            if text.is_empty() {
                *query_field = None;
            }
        }
    }
    query.from_ms = match query.range {
        Some(UsageRange::Daily) => Some(now.saturating_sub(24 * 60 * 60 * 1_000)),
        Some(UsageRange::Weekly) => Some(now.saturating_sub(7 * 24 * 60 * 60 * 1_000)),
        Some(UsageRange::Monthly) => Some(now.saturating_sub(30 * 24 * 60 * 60 * 1_000)),
        Some(UsageRange::Custom) | None => query.from_ms,
    };
    if matches!(query.range, Some(UsageRange::Daily)) {
        query.to_ms = Some(now);
    }
    query
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_usage_uses_a_rolling_24_hour_window() {
        for timestamp in [
            "2026-10-22T02:20:58+03:00",
            "2026-10-22T23:59:59+03:00",
            "2026-03-08T03:30:00-04:00",
            "2026-11-01T01:30:00-05:00",
        ] {
            let now = chrono::DateTime::parse_from_rfc3339(timestamp)
                .unwrap()
                .timestamp_millis() as u64;
            let query = normalize_usage_query_at(
                UsageQuery {
                    range: Some(UsageRange::Daily),
                    from_ms: Some(1),
                    to_ms: Some(2),
                    ..Default::default()
                },
                now,
            );
            assert_eq!(query.from_ms, Some(now - 86_400_000), "{timestamp}");
            assert_eq!(query.to_ms, Some(now), "{timestamp}");
        }
    }

    #[test]
    fn daily_usage_window_advances_and_saturates_at_the_epoch() {
        for now in [0, 1_000, 1_784_201_430_000, 1_784_205_030_000] {
            let query = normalize_usage_query_at(
                UsageQuery {
                    range: Some(UsageRange::Daily),
                    ..Default::default()
                },
                now,
            );
            assert_eq!(query.from_ms, Some(now.saturating_sub(86_400_000)));
            assert_eq!(query.to_ms, Some(now));
        }
    }
}
