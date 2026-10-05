use rusqlite::types::Value as SqlValue;
use zenith_relay_core::{
    protocol::{UsageQuery, UsageTotals},
    sql_like_contains_pattern, UsageEvent, WireApi,
};

/// The scalar fields that contribute to a totals row for one request.
///
/// Keeping this projection next to the SQL aggregate definition makes the
/// incremental totals cache use the same accounting rules as a full scan.
#[derive(Clone, Copy, Debug, Default)]
pub(in crate::local_pool::store::telemetry_db) struct UsageTotalsSample {
    pub(in crate::local_pool::store::telemetry_db) success: bool,
    pub(in crate::local_pool::store::telemetry_db) latency_ms: u64,
    pub(in crate::local_pool::store::telemetry_db) ttft_ms: Option<u64>,
    pub(in crate::local_pool::store::telemetry_db) generation_ms: Option<u64>,
    pub(in crate::local_pool::store::telemetry_db) input_tokens: Option<u64>,
    pub(in crate::local_pool::store::telemetry_db) cached_input_tokens: Option<u64>,
    pub(in crate::local_pool::store::telemetry_db) cache_write_input_tokens: Option<u64>,
    pub(in crate::local_pool::store::telemetry_db) reasoning_tokens: Option<u64>,
    pub(in crate::local_pool::store::telemetry_db) output_tokens: Option<u64>,
    pub(in crate::local_pool::store::telemetry_db) total_tokens: Option<u64>,
}

pub(in crate::local_pool::store::telemetry_db) fn usage_totals_from_event(
    event: &UsageEvent,
) -> UsageTotals {
    usage_totals_from_sample(UsageTotalsSample {
        success: event.success,
        latency_ms: event.latency_ms,
        ttft_ms: event.ttft_ms,
        generation_ms: event.generation_ms,
        input_tokens: event.input_tokens,
        cached_input_tokens: event.cached_input_tokens,
        cache_write_input_tokens: event.cache_write_input_tokens,
        reasoning_tokens: event.reasoning_tokens,
        output_tokens: event.output_tokens,
        total_tokens: event.total_tokens,
    })
}

pub(in crate::local_pool::store::telemetry_db) fn usage_totals_from_sample(
    sample: UsageTotalsSample,
) -> UsageTotals {
    let mut totals = UsageTotals {
        requests: 1,
        successful_requests: u64::from(sample.success),
        latency_ms: sample.latency_ms,
        input_tokens: sample.input_tokens.unwrap_or_default(),
        cached_input_tokens: sample.cached_input_tokens.unwrap_or_default(),
        reasoning_tokens: sample.reasoning_tokens.unwrap_or_default(),
        output_tokens: sample.output_tokens.unwrap_or_default(),
        total_tokens: sample.total_tokens.unwrap_or_default(),
        ..UsageTotals::default()
    };
    if let Some(ttft_ms) = sample.ttft_ms {
        totals.ttft_ms = ttft_ms;
        totals.ttft_samples = 1;
    }
    if let Some(cache_write_input_tokens) = sample.cache_write_input_tokens {
        totals.cache_write_input_tokens = cache_write_input_tokens;
        totals.cache_write_input_samples = 1;
    }
    if let Some(generation_ms) = sample.generation_ms {
        let generation_output_tokens = sample
            .output_tokens
            .unwrap_or_default()
            .saturating_sub(sample.reasoning_tokens.unwrap_or_default())
            .saturating_sub(1);
        if sample.success
            && generation_ms > 0
            && generation_output_tokens > 0
            // One token per millisecond is the upper bound for a reliable
            // generation-throughput sample. Faster values usually indicate
            // buffered output released at the end of the request.
            && generation_output_tokens <= generation_ms
        {
            totals.generation_ms = generation_ms;
            totals.generation_samples = 1;
            totals.generation_output_tokens = generation_output_tokens;
        }
    }
    if sample.success
        && sample.output_tokens.is_some_and(|tokens| tokens > 0)
        && sample.latency_ms > 0
        && sample.output_tokens.unwrap_or_default() <= sample.latency_ms
    {
        totals.speed_output_tokens = sample.output_tokens.unwrap_or_default();
        totals.speed_duration_ms = sample.latency_ms;
    }
    totals
}

pub(in crate::local_pool::store::telemetry_db) fn apply_usage_totals_delta(
    target: &mut UsageTotals,
    delta: UsageTotals,
    add: bool,
) {
    macro_rules! adjust {
        ($field:ident) => {
            target.$field = if add {
                target.$field.saturating_add(delta.$field)
            } else {
                target.$field.saturating_sub(delta.$field)
            };
        };
    }
    adjust!(requests);
    adjust!(successful_requests);
    adjust!(latency_ms);
    adjust!(ttft_ms);
    adjust!(ttft_samples);
    adjust!(generation_ms);
    adjust!(generation_samples);
    adjust!(generation_output_tokens);
    adjust!(input_tokens);
    adjust!(cached_input_tokens);
    adjust!(cached_input_samples);
    adjust!(cache_write_input_tokens);
    adjust!(cache_write_input_samples);
    adjust!(reasoning_tokens);
    adjust!(output_tokens);
    adjust!(total_tokens);
    adjust!(speed_output_tokens);
    adjust!(speed_duration_ms);
}

pub(in crate::local_pool::store::telemetry_db) fn is_unfiltered_all_time(
    query: &UsageQuery,
) -> bool {
    query.range.is_none()
        && query.from_ms.is_none()
        && query.to_ms.is_none()
        && query.bucket_ms.is_none()
        && query.model_query.is_none()
        && query.source_or_account_query.is_none()
        && query.wire_api.is_none()
        && query.transport.is_none()
        && query.success.is_none()
        && query.error_category.is_none()
        && query.request_id_query.is_none()
}

pub(in crate::local_pool::store::telemetry_db) fn usage_filter(
    query: &UsageQuery,
) -> (String, Vec<SqlValue>) {
    let mut clauses = Vec::new();
    let mut values = Vec::new();
    if let Some(value) = query.from_ms {
        clauses.push("created_at >= datetime(? / 1000, 'unixepoch')");
        values.push(SqlValue::Integer(super::sql_u64(value)));
    }
    if let Some(value) = query.to_ms {
        clauses.push("created_at <= datetime(? / 1000, 'unixepoch')");
        values.push(SqlValue::Integer(super::sql_u64(value)));
    }
    if let Some(value) = query.model_query.as_deref() {
        clauses.push("(requested_model LIKE ? ESCAPE '\\' OR resolved_model LIKE ? ESCAPE '\\')");
        let value = SqlValue::Text(sql_like_contains_pattern(value));
        values.push(value.clone());
        values.push(value);
    }
    if let Some(value) = query.source_or_account_query.as_deref() {
        clauses.push("(source_id LIKE ? ESCAPE '\\' OR account_id LIKE ? ESCAPE '\\')");
        let value = SqlValue::Text(sql_like_contains_pattern(value));
        values.push(value.clone());
        values.push(value);
    }
    if let Some(value) = query.wire_api {
        match value {
            WireApi::ChatCompletions => {
                clauses.push("wire_api IN (?, ?)");
                values.push(SqlValue::Text("chat_completions".to_string()));
                values.push(SqlValue::Text("chatcompletions".to_string()));
            }
            _ => {
                clauses.push("wire_api = ?");
                values.push(SqlValue::Text(value.as_str().to_string()));
            }
        }
    }
    if let Some(value) = query.transport {
        clauses.push("transport = ?");
        values.push(SqlValue::Text(value.as_str().to_string()));
    }
    if let Some(value) = query.success {
        clauses.push("success = ?");
        values.push(SqlValue::Integer(i64::from(value)));
    }
    if let Some(value) = query.error_category.as_deref() {
        clauses.push("error_category = ?");
        values.push(SqlValue::Text(value.to_string()));
    }
    if let Some(value) = query.request_id_query.as_deref() {
        clauses.push("request_id LIKE ? ESCAPE '\\'");
        values.push(SqlValue::Text(sql_like_contains_pattern(value)));
    }
    let sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    (sql, values)
}
