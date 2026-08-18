use crate::local_pool::{
    error::CommandError,
    state::DesktopState,
    store::telemetry_db::{LocalUsagePage, UsageLog},
};
use chrono::{DateTime, Days, Local, Utc};
use std::collections::BTreeMap;
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
    let (price_overrides, source_price_overrides) = {
        let store = state.store()?;
        (
            store.gateway().model_price_overrides.clone(),
            store
                .sources()
                .iter()
                .map(|source| {
                    let mut prices = BTreeMap::new();
                    for model in source
                        .model_price_overrides
                        .keys()
                        .chain(source.detected_model_prices.keys())
                    {
                        prices.entry(model.clone()).or_insert_with(|| {
                            zenith_relay_core::ApiModelPriceSources {
                                provider: source.detected_model_prices.get(model).copied(),
                                manual: source.model_price_overrides.get(model).copied(),
                            }
                        });
                    }
                    (source.id.clone(), prices)
                })
                .collect::<BTreeMap<_, _>>(),
        )
    };
    state
        .telemetry
        .usage_page_with_price_overrides(
            &normalize_usage_query(input.unwrap_or_default()),
            &price_overrides,
            &source_price_overrides,
        )
        .map_err(Into::into)
}

#[tauri::command]
pub fn clear_local_usage(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    state.telemetry.clear().map_err(Into::into)
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
            }
        }
    }
    query.from_ms = match query.range {
        Some(UsageRange::Daily) => local_calendar_day_bounds(now).map(|bounds| bounds.0),
        Some(UsageRange::Weekly) => Some(now.saturating_sub(7 * 24 * 60 * 60 * 1_000)),
        Some(UsageRange::Monthly) => Some(now.saturating_sub(30 * 24 * 60 * 60 * 1_000)),
        Some(UsageRange::Custom) | None => query.from_ms,
    };
    if matches!(query.range, Some(UsageRange::Daily)) {
        query.to_ms = local_calendar_day_bounds(now).map(|bounds| bounds.1.saturating_sub(1));
    }
    query
}

fn local_calendar_day_bounds(now_ms: u64) -> Option<(u64, u64)> {
    let now =
        DateTime::<Utc>::from_timestamp_millis(i64::try_from(now_ms).ok()?)?.with_timezone(&Local);
    let start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)?
        .and_local_timezone(Local)
        .earliest()?;
    let end = now
        .date_naive()
        .checked_add_days(Days::new(1))?
        .and_hms_opt(0, 0, 0)?
        .and_local_timezone(Local)
        .earliest()?;
    Some((
        u64::try_from(start.timestamp_millis()).ok()?,
        u64::try_from(end.timestamp_millis()).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn daily_usage_uses_local_calendar_day() {
        let now = 1_784_201_430_000;
        let query = normalize_usage_query_at(
            UsageQuery {
                range: Some(UsageRange::Daily),
                ..Default::default()
            },
            now,
        );
        let from = query.from_ms.unwrap();
        let to = query.to_ms.unwrap();

        assert!(from <= now && now <= to);
        assert!((23 * 60 * 60 * 1_000 - 1..=25 * 60 * 60 * 1_000 - 1).contains(&(to - from)));
        let start = DateTime::<Utc>::from_timestamp_millis(from as i64)
            .unwrap()
            .with_timezone(&Local);
        assert_eq!((start.hour(), start.minute(), start.second()), (0, 0, 0));
    }
}
