use super::super::UsageLog;
use zenith_relay_core::{
    protocol::UsageTotals, ApiEquivalentSummary, ApiEquivalentUsage, DefaultServiceTier, WireApi,
};

pub(super) fn usage_pricing_usage_from_row(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<ApiEquivalentUsage> {
    Ok(ApiEquivalentUsage::from_observed_sums(
        zenith_relay_core::ObservedUsageSums::from_priced_aggregate(
            |column| row.get(offset + column),
            |column| row.get(offset + column),
        )?,
    ))
}

pub(super) fn usage_totals_from_row(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<UsageTotals> {
    UsageTotals::from_sql_counts(|column| row.get(offset + column))
}

pub(in crate::local_pool::store::telemetry_db) fn usage_log_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<UsageLog> {
    let latency_ms: i64 = row.get(14)?;
    let ttft_ms: Option<i64> = row.get(15)?;
    let generation_ms: Option<i64> = row.get(16)?;
    let input_tokens: Option<i64> = row.get(17)?;
    let cached_input_tokens: Option<i64> = row.get(18)?;
    let cache_write_input_tokens: Option<i64> = row.get(19)?;
    let reasoning_tokens: Option<i64> = row.get(20)?;
    let output_tokens: Option<i64> = row.get(21)?;
    let total_tokens: Option<i64> = row.get(22)?;
    let service_tier: String = row.get(23)?;
    let applied_service_tier: Option<String> = row.get(24)?;
    let routing_json: Option<String> = row.get(25)?;
    let tool_use_json: Option<String> = row.get(26)?;
    let error_origin: Option<String> = row.get(27)?;
    let requested_reasoning_effort: Option<String> = row.get(28)?;
    let effective_reasoning_effort: Option<String> = row.get(29)?;
    let cache_write_ttl: Option<String> = row.get(30)?;
    let client_context_id: Option<String> = row.get(31)?;
    Ok(UsageLog {
        id: row.get(0)?,
        created_at: row.get(1)?,
        request_id: row.get(2)?,
        attempt: row.get(3)?,
        source_id: row.get(5)?,
        candidate_id: row.get(6)?,
        account_id: row.get(7)?,
        client_context_id,
        routing: routing_json
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok()),
        requested_model: row.get(8)?,
        resolved_model: row.get(9)?,
        requested_reasoning_effort: requested_reasoning_effort
            .as_deref()
            .and_then(zenith_relay_core::normalize_reasoning_effort),
        effective_reasoning_effort: effective_reasoning_effort
            .as_deref()
            .and_then(zenith_relay_core::normalize_reasoning_effort),
        wire_api: normalize_wire_api(row.get(10)?),
        service_tier: DefaultServiceTier::from_storage_value(&service_tier),
        applied_service_tier: applied_service_tier
            .as_deref()
            .and_then(zenith_relay_core::normalize_observed_service_tier),
        success: row.get(11)?,
        http_status: row.get(12)?,
        error_category: row.get(13)?,
        error_origin: error_origin.as_deref().and_then(|value| value.parse().ok()),
        upstream_error: row
            .get::<_, Option<String>>(32)?
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok()),
        tool_use: tool_use_json
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok()),
        latency_ms: rust_u64(latency_ms),
        ttft_ms: optional_u64(ttft_ms),
        generation_ms: optional_u64(generation_ms),
        input_tokens: optional_u64(input_tokens),
        cached_input_tokens: optional_u64(cached_input_tokens),
        cache_write_input_tokens: optional_u64(cache_write_input_tokens),
        cache_write_ttl: cache_write_ttl
            .as_deref()
            .and_then(zenith_relay_core::usage::normalize_reported_cache_ttls),
        reasoning_tokens: optional_u64(reasoning_tokens),
        output_tokens: optional_u64(output_tokens),
        total_tokens: optional_u64(total_tokens),
        api_equivalent: ApiEquivalentSummary::default(),
    })
}

fn normalize_wire_api(value: String) -> String {
    WireApi::from_storage_value(&value)
        .map(|wire_api| wire_api.as_str().to_string())
        .unwrap_or(value)
}

pub(in crate::local_pool::store::telemetry_db) fn sql_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

pub(in crate::local_pool::store::telemetry_db) fn rust_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

/// `NULL` and a corrupt negative sum are both "not measured". A normal zero
/// stays zero. Counters that cannot be absent still use `rust_u64`.
pub(in crate::local_pool::store::telemetry_db) fn optional_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}
