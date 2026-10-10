use super::super::UsageLog;
use zenith_relay_core::{
    protocol::UsageTotals, ApiEquivalentSummary, ApiEquivalentUsage, DefaultServiceTier,
    UsageTransport, WireApi,
};

pub(super) fn usage_pricing_usage_from_row(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<ApiEquivalentUsage> {
    let price_class: String = row.get(offset - 2)?;
    let context_band: String = row.get(offset - 1)?;
    Ok(ApiEquivalentUsage::from_observed_sums(
        zenith_relay_core::ObservedUsageSums::from_priced_aggregate(
            |column| row.get(offset + column),
            |column| row.get(offset + column),
        )?,
    )
    .with_aggregate_rates(&price_class, &context_band))
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
    let latency_ms: i64 = row.get(15)?;
    let ttft_ms: Option<i64> = row.get(16)?;
    let generation_ms: Option<i64> = row.get(17)?;
    let input_tokens: Option<i64> = row.get(18)?;
    let cached_input_tokens: Option<i64> = row.get(19)?;
    let cache_write_input_tokens: Option<i64> = row.get(20)?;
    let reasoning_tokens: Option<i64> = row.get(21)?;
    let output_tokens: Option<i64> = row.get(22)?;
    let total_tokens: Option<i64> = row.get(23)?;
    let transport = row
        .get::<_, String>(11)?
        .parse::<UsageTransport>()
        .unwrap_or_default()
        .as_str()
        .to_string();
    let service_tier: String = row.get(24)?;
    let applied_service_tier: Option<String> = row.get(25)?;
    let routing_json: Option<String> = row.get(26)?;
    let tool_use_json: Option<String> = row.get(27)?;
    let error_origin: Option<String> = row.get(28)?;
    let requested_reasoning_effort: Option<String> = row.get(29)?;
    let effective_reasoning_effort: Option<String> = row.get(30)?;
    let cache_write_ttl: Option<String> = row.get(31)?;
    let client_context_id: Option<String> = row.get(32)?;
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
            .and_then(|routing_json| serde_json::from_str(routing_json).ok()),
        requested_model: row.get(8)?,
        resolved_model: row.get(9)?,
        requested_reasoning_effort: requested_reasoning_effort
            .as_deref()
            .and_then(zenith_relay_core::normalize_reasoning_effort),
        effective_reasoning_effort: effective_reasoning_effort
            .as_deref()
            .and_then(zenith_relay_core::normalize_reasoning_effort),
        wire_api: normalize_wire_api(row.get(10)?),
        transport,
        service_tier: DefaultServiceTier::from_storage_value(&service_tier),
        applied_service_tier: applied_service_tier
            .as_deref()
            .and_then(zenith_relay_core::normalize_observed_service_tier),
        success: row.get(12)?,
        http_status: row.get(13)?,
        error_category: row.get(14)?,
        error_origin: error_origin
            .as_deref()
            .and_then(|error_origin_text| error_origin_text.parse().ok()),
        upstream_error: row
            .get::<_, Option<String>>(33)?
            .as_deref()
            .and_then(|error_json| serde_json::from_str(error_json).ok()),
        tool_use: tool_use_json
            .as_deref()
            .and_then(|tool_use_json| serde_json::from_str(tool_use_json).ok()),
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

fn normalize_wire_api(stored_wire_api: String) -> String {
    WireApi::from_storage_value(&stored_wire_api)
        .map(|wire_api| wire_api.as_str().to_string())
        .unwrap_or(stored_wire_api)
}

pub(in crate::local_pool::store::telemetry_db) use zenith_relay_core::usage::sql_count_u64 as rust_u64;
pub(in crate::local_pool::store::telemetry_db) use zenith_relay_core::usage::sql_u64;

/// `NULL` and a corrupt negative sum are both "not measured". A normal zero
/// stays zero. Counters that cannot be absent still use `rust_u64`.
pub(in crate::local_pool::store::telemetry_db) use zenith_relay_core::usage::sql_optional_u64 as optional_u64;
