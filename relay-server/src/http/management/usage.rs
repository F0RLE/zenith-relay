use super::{store_error, validation_error, ManagementError};
use crate::state::{identity_hint, now_ms, AppState};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use std::collections::HashMap;
use std::sync::Arc;
use zenith_relay_core::protocol::{RuntimeStateSnapshot, UsagePage, UsageQuery, UsageRange};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/usage", get(usage).delete(clear_usage))
}

pub async fn usage(
    State(state): State<Arc<AppState>>,
    Query(mut query): Query<UsageQuery>,
) -> Result<Json<UsagePage>, ManagementError> {
    normalize_usage_query(&mut query)?;
    let snapshot = state.snapshot().map_err(store_error)?;
    normalize_account_query(&mut query, &snapshot);
    let catalog = state.pricing_catalog();
    let pricing = state.pricing_context().map_err(store_error)?;
    let mut page = state
        .store
        .usage_page_with_pricing(&query, &catalog, &pricing)
        .map_err(store_error)?;
    page.pricing.catalog_status = state.pricing_status();
    let labels = snapshot
        .accounts
        .into_iter()
        .map(|account| (identity_hint(&account.id), account.label))
        .chain(
            snapshot
                .sources
                .into_iter()
                .map(|source| (identity_hint(&source.id), source.name)),
        )
        .collect::<HashMap<_, _>>();
    for event in &mut page.events {
        event.candidate_label = labels.get(&event.candidate_hint).cloned();
    }
    for group in &mut page.pool_members {
        group.label = labels.get(&group.key).cloned();
    }
    Ok(Json(page))
}

fn normalize_account_query(query: &mut UsageQuery, snapshot: &RuntimeStateSnapshot) {
    let Some(value) = query.source_or_account_query.as_deref() else {
        return;
    };
    let Some(hint) = account_query_hint(
        value,
        snapshot.accounts.iter().map(|account| account.id.as_str()),
    ) else {
        return;
    };
    query.source_or_account_query = Some(hint);
}

fn account_query_hint<'a>(
    value: &str,
    account_ids: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    account_ids
        .into_iter()
        .find(|account_id| *account_id == value)
        .map(identity_hint)
}

pub async fn clear_usage(
    State(state): State<Arc<AppState>>,
) -> Result<StatusCode, ManagementError> {
    state.store.clear_usage().map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}

fn normalize_usage_query(query: &mut UsageQuery) -> Result<(), ManagementError> {
    query.normalize_pagination();
    for value in [
        &mut query.model_query,
        &mut query.source_or_account_query,
        &mut query.error_category,
        &mut query.request_id_query,
    ] {
        if let Some(text) = value {
            *text = text.trim().to_string();
            if text.is_empty() {
                *value = None;
            } else if text.len() > 256 || text.chars().any(char::is_control) {
                return Err(validation_error("usage filter is invalid"));
            }
        }
    }
    let now = now_ms();
    query.from_ms = match query.range {
        Some(UsageRange::Daily) => Some(utc_calendar_day_bounds(now).0),
        Some(UsageRange::Weekly) => Some(now.saturating_sub(7 * 24 * 60 * 60 * 1_000)),
        Some(UsageRange::Monthly) => Some(now.saturating_sub(30 * 24 * 60 * 60 * 1_000)),
        Some(UsageRange::Custom) => query.from_ms,
        None => query.from_ms,
    };
    if matches!(query.range, Some(UsageRange::Daily)) {
        query.to_ms = Some(utc_calendar_day_bounds(now).1.saturating_sub(1));
    }
    if matches!(query.range, Some(UsageRange::Custom))
        && (query.from_ms.is_none() || query.to_ms.is_none())
    {
        return Err(validation_error(
            "custom usage range requires fromMs and toMs",
        ));
    }
    if query
        .from_ms
        .zip(query.to_ms)
        .is_some_and(|(from, to)| from > to)
    {
        return Err(validation_error("usage range is invalid"));
    }
    Ok(())
}

fn utc_calendar_day_bounds(now_ms: u64) -> (u64, u64) {
    const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
    let start = now_ms / DAY_MS * DAY_MS;
    (start, start.saturating_add(DAY_MS))
}

#[cfg(test)]
mod tests {
    use super::{account_query_hint, identity_hint, utc_calendar_day_bounds};

    #[test]
    fn daily_usage_uses_utc_calendar_day() {
        const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
        let now = 20 * DAY_MS + 12_345;

        assert_eq!(utc_calendar_day_bounds(now), (20 * DAY_MS, 21 * DAY_MS));
    }

    #[test]
    fn account_id_filter_uses_the_server_usage_hint() {
        let expected = identity_hint("account-2");
        assert_eq!(
            account_query_hint("account-2", ["account-1", "account-2"]),
            Some(expected)
        );
        assert_eq!(account_query_hint("unknown", ["account-1"]), None);
    }
}
