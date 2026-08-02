use super::{
    records::SourcePriceOverrides,
    sqlite::{db_error, optional_u64, to_json, Store},
};
use rusqlite::{
    params, params_from_iter, types::Value as SqlValue, Connection, TransactionBehavior,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use zenith_relay_core::{
    estimate_api_equivalent_with_price_override,
    protocol::{UsageBucket, UsageGroup, UsagePage, UsageQuery, UsageSummary, UsageTotals},
    ApiEquivalentSummary, ApiModelPriceOverride, DefaultServiceTier, UsageEvent, WireApi,
};

#[cfg(test)]
use zenith_relay_core::ToolUseDiagnostics;

const DAY_MS: u64 = 24 * 60 * 60 * 1_000;

const RAW_USAGE_RETENTION_MS: u64 = 90 * DAY_MS;

const DAILY_ROLLUP_RETENTION_MS: u64 = 400 * DAY_MS;

const MAX_RAW_USAGE_EVENTS: u64 = 100_000;

impl Store {
    pub fn record_usage(&self, event: &UsageEvent, created_at_ms: u64) -> Result<(), String> {
        self.record_usage_batch(&[(event, created_at_ms)])
    }

    pub fn record_usage_batch(&self, events: &[(&UsageEvent, u64)]) -> Result<(), String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        {
            let mut statement = transaction
                .prepare(
                    r#"INSERT INTO usage_events(
                        request_id, attempt, local_key_id, candidate_kind, candidate_hint,
                        requested_model, resolved_model, wire_api, success, http_status,
                        error_category, latency_ms, ttft_ms, generation_ms, input_tokens,
                        cached_input_tokens, cache_write_input_tokens, reasoning_tokens,
                        output_tokens, total_tokens, created_at_ms, routing_json,
                        service_tier, applied_service_tier, tool_use_json
                    ) SELECT
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                        ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
                        ?23, ?24, ?25
                    WHERE NOT EXISTS (
                        SELECT 1 FROM usage_request_tombstones WHERE request_id = ?1
                    )
                    AND (?4 != 'account' OR EXISTS (
                        SELECT 1 FROM accounts WHERE id = ?26
                    ))
                    ON CONFLICT(request_id) DO UPDATE SET
                        attempt=excluded.attempt,
                        local_key_id=excluded.local_key_id,
                        candidate_kind=excluded.candidate_kind,
                        candidate_hint=excluded.candidate_hint,
                        requested_model=excluded.requested_model,
                        resolved_model=excluded.resolved_model,
                        wire_api=excluded.wire_api,
                        success=excluded.success,
                        http_status=excluded.http_status,
                        error_category=excluded.error_category,
                        latency_ms=excluded.latency_ms,
                        ttft_ms=excluded.ttft_ms,
                        generation_ms=excluded.generation_ms,
                        input_tokens=excluded.input_tokens,
                        cached_input_tokens=excluded.cached_input_tokens,
                        cache_write_input_tokens=excluded.cache_write_input_tokens,
                        reasoning_tokens=excluded.reasoning_tokens,
                        output_tokens=excluded.output_tokens,
                        total_tokens=excluded.total_tokens,
                        created_at_ms=excluded.created_at_ms,
                        routing_json=excluded.routing_json,
                        service_tier=excluded.service_tier,
                        applied_service_tier=excluded.applied_service_tier,
                        tool_use_json=excluded.tool_use_json
                    WHERE excluded.attempt >= usage_events.attempt"#,
                )
                .map_err(db_error)?;
            for (event, created_at_ms) in events {
                let candidate_id = event
                    .account_id
                    .as_deref()
                    .or(event.candidate_id.as_deref())
                    .unwrap_or(&event.source_id);
                let candidate_kind = if event.account_id.is_some() {
                    "account"
                } else {
                    "source"
                };
                let candidate_hint =
                    hex::encode(Sha256::digest(candidate_id.as_bytes()))[..12].to_string();
                let routing_json = event.routing.as_ref().map(to_json).transpose()?;
                let tool_use_json = event
                    .tool_use
                    .has_evidence()
                    .then(|| to_json(&event.tool_use))
                    .transpose()?;
                statement
                    .execute(params![
                        event.request_id,
                        event.attempt,
                        event.local_key_id,
                        candidate_kind,
                        candidate_hint,
                        event.requested_model,
                        event.resolved_model,
                        wire_api_name(event.wire_api),
                        i64::from(event.success),
                        i64::from(event.http_status),
                        event.error_category,
                        event.latency_ms as i64,
                        event.ttft_ms.map(|value| value as i64),
                        event.generation_ms.map(|value| value as i64),
                        event.input_tokens.map(|value| value as i64),
                        event.cached_input_tokens.map(|value| value as i64),
                        event.cache_write_input_tokens.map(|value| value as i64),
                        event.reasoning_tokens.map(|value| value as i64),
                        event.output_tokens.map(|value| value as i64),
                        event.total_tokens.map(|value| value as i64),
                        *created_at_ms as i64,
                        routing_json,
                        service_tier_name(event.service_tier),
                        event.applied_service_tier.map(service_tier_name),
                        tool_use_json,
                        candidate_id,
                    ])
                    .map_err(db_error)?;
            }
        }
        transaction.commit().map_err(db_error)
    }

    pub fn usage_page(&self, query: &UsageQuery) -> Result<UsagePage, String> {
        let price_overrides = self.model_price_overrides()?;
        let source_price_overrides = self.source_price_overrides()?;
        let page = query.page.max(1);
        let page_size = if query.page_size == 0 {
            50
        } else {
            query.page_size.clamp(1, 200)
        };
        let connection = self.lock()?;
        let (where_sql, values) = usage_filter(query);
        let mut totals = usage_totals(&connection, &where_sql, &values)?;
        let mut models = usage_groups(
            &connection,
            &where_sql,
            &values,
            "COALESCE(resolved_model, requested_model, '')",
        )?;
        let mut model_equivalents = usage_model_equivalents(
            &connection,
            &where_sql,
            &values,
            &price_overrides,
            &source_price_overrides,
        )?;
        for group in &mut models {
            group.totals.api_equivalent = model_equivalents.remove(&group.key).unwrap_or_default();
            totals.api_equivalent.merge(group.totals.api_equivalent);
        }
        let pool_members = usage_groups(
            &connection,
            &where_sql,
            &values,
            "COALESCE(candidate_hint, '')",
        )?;
        let buckets = usage_buckets(
            &connection,
            &where_sql,
            &values,
            query,
            &price_overrides,
            &source_price_overrides,
        )?;
        let total = totals.requests;
        let offset = u64::from(page.saturating_sub(1)) * u64::from(page_size);
        let sql = format!(
            "SELECT id, request_id, local_key_id, candidate_kind, candidate_hint, requested_model, resolved_model, wire_api, success, http_status, error_category, latency_ms, ttft_ms, generation_ms, input_tokens, cached_input_tokens, cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens, created_at_ms, routing_json, service_tier, applied_service_tier, tool_use_json \
             FROM usage_events{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?"
        );
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let mut page_values = values;
        page_values.push(SqlValue::Integer(i64::from(page_size)));
        page_values.push(SqlValue::Integer(offset.min(i64::MAX as u64) as i64));
        let rows = statement
            .query_map(params_from_iter(page_values.iter()), |row| {
                let wire_api: String = row.get(7)?;
                Ok(UsageSummary {
                    id: row.get(0)?,
                    request_id: row.get(1)?,
                    local_key_id: row.get(2)?,
                    candidate_kind: row.get(3)?,
                    candidate_hint: row.get(4)?,
                    candidate_label: None,
                    routing: row
                        .get::<_, Option<String>>(21)?
                        .as_deref()
                        .and_then(|value| serde_json::from_str(value).ok()),
                    requested_model: row.get(5)?,
                    resolved_model: row.get(6)?,
                    wire_api: parse_wire_api(&wire_api),
                    service_tier: parse_service_tier(&row.get::<_, String>(22)?),
                    applied_service_tier: row
                        .get::<_, Option<String>>(23)?
                        .as_deref()
                        .map(parse_service_tier),
                    tool_use: row
                        .get::<_, Option<String>>(24)?
                        .as_deref()
                        .and_then(|value| serde_json::from_str(value).ok()),
                    success: row.get::<_, i64>(8)? != 0,
                    http_status: row.get::<_, i64>(9)?.clamp(0, i64::from(u16::MAX)) as u16,
                    error_category: row.get(10)?,
                    latency_ms: row.get::<_, i64>(11)?.max(0) as u64,
                    ttft_ms: optional_u64(row.get(12)?),
                    generation_ms: optional_u64(row.get(13)?),
                    input_tokens: optional_u64(row.get(14)?),
                    cached_input_tokens: optional_u64(row.get(15)?),
                    cache_write_input_tokens: optional_u64(row.get(16)?),
                    reasoning_tokens: optional_u64(row.get(17)?),
                    output_tokens: optional_u64(row.get(18)?),
                    total_tokens: optional_u64(row.get(19)?),
                    api_equivalent: ApiEquivalentSummary::default(),
                    created_at_ms: row.get::<_, i64>(20)?.max(0) as u64,
                })
            })
            .map_err(db_error)?;
        let mut events = rows.collect::<Result<Vec<_>, _>>().map_err(db_error)?;
        for event in &mut events {
            let model = event
                .resolved_model
                .as_deref()
                .or(event.requested_model.as_deref());
            event.api_equivalent = estimate_api_equivalent_with_price_override(
                model,
                event.input_tokens,
                event.cached_input_tokens,
                event.cache_write_input_tokens,
                event.output_tokens,
                event.total_tokens,
                configured_model_price(
                    &price_overrides,
                    &source_price_overrides,
                    &event.candidate_kind,
                    &event.candidate_hint,
                    model,
                ),
            );
        }
        let total_pages = if total == 0 {
            0
        } else {
            total.div_ceil(u64::from(page_size)) as u32
        };
        Ok(UsagePage {
            events,
            total,
            page,
            page_size,
            total_pages,
            totals,
            models,
            pool_members,
            buckets,
        })
    }

    pub fn api_equivalents(&self) -> Result<HashMap<String, ApiEquivalentSummary>, String> {
        let price_overrides = self.model_price_overrides()?;
        let source_price_overrides = self.source_price_overrides()?;
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT candidate_kind, candidate_id, model,
                    SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens),
                    SUM(output_tokens), SUM(total_tokens), SUM(input_samples),
                    SUM(cached_input_samples), SUM(cache_write_input_samples)
                 FROM (
                    SELECT candidate_kind, candidate_id, model,
                        input_tokens, cached_input_tokens, cache_write_input_tokens,
                        output_tokens, total_tokens, input_samples,
                        cached_input_samples, cache_write_input_samples
                    FROM usage_candidate_rollups
                    UNION ALL
                    SELECT candidate_kind, candidate_hint,
                        COALESCE(resolved_model, requested_model, ''),
                        COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_input_tokens), 0),
                        COALESCE(SUM(cache_write_input_tokens), 0), COALESCE(SUM(output_tokens), 0),
                        COALESCE(SUM(total_tokens), 0), COUNT(input_tokens),
                        COUNT(cached_input_tokens), COUNT(cache_write_input_tokens)
                    FROM usage_events GROUP BY 1, 2, 3
                 ) GROUP BY candidate_kind, candidate_id, model",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                let kind = row.get::<_, String>(0)?;
                let candidate_id = row.get::<_, String>(1)?;
                let model = row.get::<_, Option<String>>(2)?;
                let input_tokens: Option<i64> = row.get(3)?;
                let cached_input_tokens: Option<i64> = row.get(4)?;
                let cache_write_input_tokens: Option<i64> = row.get(5)?;
                let output_tokens: Option<i64> = row.get(6)?;
                let total_tokens: Option<i64> = row.get(7)?;
                let input_samples: i64 = row.get(8)?;
                let cached_samples: i64 = row.get(9)?;
                let cache_write_samples: i64 = row.get(10)?;
                Ok((candidate_id.clone(), {
                    estimate_api_equivalent_with_price_override(
                        model.as_deref(),
                        optional_u64(input_tokens),
                        (input_samples > 0 && cached_samples == input_samples)
                            .then(|| optional_u64(cached_input_tokens))
                            .flatten(),
                        (input_samples > 0 && cache_write_samples == input_samples)
                            .then(|| optional_u64(cache_write_input_tokens))
                            .flatten(),
                        optional_u64(output_tokens),
                        optional_u64(total_tokens),
                        configured_model_price(
                            &price_overrides,
                            &source_price_overrides,
                            &kind,
                            &candidate_id,
                            model.as_deref(),
                        ),
                    )
                }))
            })
            .map_err(db_error)?;
        let mut equivalents = HashMap::<String, ApiEquivalentSummary>::new();
        for row in rows {
            let (candidate_hint, estimate) = row.map_err(db_error)?;
            equivalents
                .entry(candidate_hint)
                .or_default()
                .merge(estimate);
        }
        Ok(equivalents)
    }

    pub fn key_usage_totals(&self, local_key_id: &str) -> Result<UsageTotals, String> {
        let price_overrides = self.model_price_overrides()?;
        let source_price_overrides = self.source_price_overrides()?;
        let connection = self.lock()?;
        let mut totals = connection
            .query_row(
                &format!(
                    "SELECT {ROLLUP_TOTAL_COLUMNS} FROM usage_key_rollups \
                     WHERE local_key_id = ?1 AND period_start_ms = -1"
                ),
                [local_key_id],
                |row| usage_totals_from_row(row, 0),
            )
            .map_err(db_error)?;
        merge_usage_totals(
            &mut totals,
            usage_totals(
                &connection,
                " WHERE local_key_id = ?",
                &[SqlValue::Text(local_key_id.to_string())],
            )?,
        );

        let sql = "SELECT candidate_kind, candidate_id, model,
                SUM(input_tokens), SUM(cached_input_tokens),
                SUM(cache_write_input_tokens), SUM(output_tokens), SUM(total_tokens),
                SUM(input_samples), SUM(cached_input_samples),
                SUM(cache_write_input_samples), SUM(output_samples), SUM(total_samples)
            FROM (
                SELECT candidate_kind, candidate_id, model, input_tokens, cached_input_tokens,
                    cache_write_input_tokens, output_tokens, total_tokens,
                    input_samples, cached_input_samples, cache_write_input_samples,
                    output_samples, total_samples
                FROM usage_key_rollups
                WHERE local_key_id = ?1 AND period_start_ms = -1
                UNION ALL
                SELECT candidate_kind, candidate_hint,
                    COALESCE(resolved_model, requested_model, ''),
                    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_input_tokens), 0),
                    COALESCE(SUM(cache_write_input_tokens), 0), COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(total_tokens), 0), COUNT(input_tokens),
                    COUNT(cached_input_tokens), COUNT(cache_write_input_tokens),
                    COUNT(output_tokens), COUNT(total_tokens)
                FROM usage_events
                WHERE local_key_id = ?2
                GROUP BY 1, 2, 3
            )
            GROUP BY candidate_kind, candidate_id, model";
        let mut statement = connection.prepare(sql).map_err(db_error)?;
        let rows = statement
            .query_map(params![local_key_id, local_key_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    nonnegative_u64(row.get(3)?),
                    nonnegative_u64(row.get(4)?),
                    nonnegative_u64(row.get(5)?),
                    nonnegative_u64(row.get(6)?),
                    nonnegative_u64(row.get(7)?),
                    nonnegative_u64(row.get(8)?),
                    nonnegative_u64(row.get(9)?),
                    nonnegative_u64(row.get(10)?),
                    nonnegative_u64(row.get(11)?),
                    nonnegative_u64(row.get(12)?),
                ))
            })
            .map_err(db_error)?;
        for row in rows {
            let (
                candidate_kind,
                candidate_id,
                model,
                input_tokens,
                cached_input_tokens,
                cache_write_input_tokens,
                output_tokens,
                total_tokens,
                input_samples,
                cached_input_samples,
                cache_write_input_samples,
                output_samples,
                total_samples,
            ) = row.map_err(db_error)?;
            let model = (!model.is_empty()).then_some(model.as_str());
            let input_tokens = (input_samples > 0).then_some(input_tokens);
            let cached_input_tokens = (input_samples > 0 && cached_input_samples == input_samples)
                .then_some(cached_input_tokens);
            let cache_write_input_tokens = (input_samples > 0
                && cache_write_input_samples == input_samples)
                .then_some(cache_write_input_tokens);
            let output_tokens = (output_samples > 0).then_some(output_tokens);
            let total_tokens = (total_samples > 0).then_some(total_tokens);
            totals
                .api_equivalent
                .merge(estimate_api_equivalent_with_price_override(
                    model,
                    input_tokens,
                    cached_input_tokens,
                    cache_write_input_tokens,
                    output_tokens,
                    total_tokens,
                    configured_model_price(
                        &price_overrides,
                        &source_price_overrides,
                        &candidate_kind,
                        &candidate_id,
                        model,
                    ),
                ));
        }
        Ok(totals)
    }

    pub fn prune_usage_history(&self, now_ms: u64) -> Result<usize, String> {
        self.prune_usage_history_with_limits(
            now_ms.saturating_sub(RAW_USAGE_RETENTION_MS),
            MAX_RAW_USAGE_EVENTS,
            now_ms.saturating_sub(DAILY_ROLLUP_RETENTION_MS),
        )
    }

    pub(super) fn prune_usage_history_with_limits(
        &self,
        raw_cutoff_ms: u64,
        max_raw_events: u64,
        daily_rollup_cutoff_ms: u64,
    ) -> Result<usize, String> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        transaction
            .execute(
                &format!(
                    "INSERT INTO usage_candidate_rollups(
                        candidate_kind, candidate_id, model,
                        input_tokens, input_samples, cached_input_tokens, cached_input_samples,
                        cache_write_input_tokens, cache_write_input_samples,
                        output_tokens, output_samples, total_tokens, total_samples
                     )
                     SELECT candidate_kind, candidate_hint,
                        COALESCE(resolved_model, requested_model, ''),
                        COALESCE(SUM(input_tokens), 0), COUNT(input_tokens),
                        COALESCE(SUM(cached_input_tokens), 0), COUNT(cached_input_tokens),
                        COALESCE(SUM(cache_write_input_tokens), 0), COUNT(cache_write_input_tokens),
                        COALESCE(SUM(output_tokens), 0), COUNT(output_tokens),
                        COALESCE(SUM(total_tokens), 0), COUNT(total_tokens)
                     FROM usage_events WHERE {USAGE_PRUNE_PREDICATE}
                     GROUP BY 1, 2, 3
                     ON CONFLICT(candidate_kind, candidate_id, model) DO UPDATE SET
                        input_tokens=input_tokens + excluded.input_tokens,
                        input_samples=input_samples + excluded.input_samples,
                        cached_input_tokens=cached_input_tokens + excluded.cached_input_tokens,
                        cached_input_samples=cached_input_samples + excluded.cached_input_samples,
                        cache_write_input_tokens=cache_write_input_tokens + excluded.cache_write_input_tokens,
                        cache_write_input_samples=cache_write_input_samples + excluded.cache_write_input_samples,
                        output_tokens=output_tokens + excluded.output_tokens,
                        output_samples=output_samples + excluded.output_samples,
                        total_tokens=total_tokens + excluded.total_tokens,
                        total_samples=total_samples + excluded.total_samples"
                ),
                params![
                    raw_cutoff_ms.min(i64::MAX as u64) as i64,
                    max_raw_events.min(i64::MAX as u64) as i64,
                ],
            )
            .map_err(db_error)?;
        for period_sql in ["-1", "(created_at_ms / 86400000) * 86400000"] {
            let sql = format!(
                "INSERT INTO usage_key_rollups(
                    local_key_id, period_start_ms, candidate_kind, candidate_id, model,
                    {ROLLUP_USAGE_COLUMNS}
                 )
                 SELECT local_key_id, {period_sql}, candidate_kind, candidate_hint,
                    COALESCE(resolved_model, requested_model, ''),
                    {USAGE_TOTAL_COLUMNS}, COUNT(input_tokens), COUNT(output_tokens),
                    COUNT(total_tokens)
                 FROM usage_events
                 WHERE {USAGE_PRUNE_PREDICATE}
                 GROUP BY local_key_id, 2, 3, 4, 5
                 ON CONFLICT(local_key_id, period_start_ms, candidate_kind, candidate_id, model)
                 DO UPDATE SET
                    {ROLLUP_UPDATE_COLUMNS}"
            );
            transaction
                .execute(
                    &sql,
                    params![
                        raw_cutoff_ms.min(i64::MAX as u64) as i64,
                        max_raw_events.min(i64::MAX as u64) as i64,
                    ],
                )
                .map_err(db_error)?;
        }
        transaction
            .execute(
                &format!(
                    "INSERT INTO usage_request_tombstones(request_id, archived_at_ms)
                     SELECT request_id, created_at_ms FROM usage_events
                     WHERE {USAGE_PRUNE_PREDICATE}
                     ON CONFLICT(request_id) DO UPDATE SET
                        archived_at_ms = MAX(usage_request_tombstones.archived_at_ms, excluded.archived_at_ms)"
                ),
                params![
                    raw_cutoff_ms.min(i64::MAX as u64) as i64,
                    max_raw_events.min(i64::MAX as u64) as i64,
                ],
            )
            .map_err(db_error)?;
        let deleted = transaction
            .execute(
                &format!("DELETE FROM usage_events WHERE {USAGE_PRUNE_PREDICATE}"),
                params![
                    raw_cutoff_ms.min(i64::MAX as u64) as i64,
                    max_raw_events.min(i64::MAX as u64) as i64,
                ],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "DELETE FROM usage_key_rollups WHERE period_start_ms >= 0 AND period_start_ms < ?1",
                [daily_rollup_cutoff_ms.min(i64::MAX as u64) as i64],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "DELETE FROM usage_request_tombstones WHERE archived_at_ms < ?1",
                [daily_rollup_cutoff_ms.min(i64::MAX as u64) as i64],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(deleted)
    }

    pub fn clear_usage(&self) -> Result<usize, String> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(db_error)?;
        let deleted = transaction
            .execute("DELETE FROM usage_events", [])
            .map_err(db_error)?;
        transaction
            .execute("DELETE FROM usage_key_rollups", [])
            .map_err(db_error)?;
        transaction
            .execute("DELETE FROM usage_candidate_rollups", [])
            .map_err(db_error)?;
        transaction
            .execute("DELETE FROM usage_request_tombstones", [])
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(deleted)
    }
}

fn usage_filter(query: &UsageQuery) -> (String, Vec<SqlValue>) {
    let mut clauses = Vec::new();
    let mut values = Vec::new();
    if let Some(value) = query.from_ms {
        clauses.push("created_at_ms >= ?");
        values.push(SqlValue::Integer(value.min(i64::MAX as u64) as i64));
    }
    if let Some(value) = query.to_ms {
        clauses.push("created_at_ms <= ?");
        values.push(SqlValue::Integer(value.min(i64::MAX as u64) as i64));
    }
    if let Some(value) = query.model_query.as_deref() {
        clauses.push("(requested_model LIKE ? ESCAPE '\\' OR resolved_model LIKE ? ESCAPE '\\')");
        let value = SqlValue::Text(like_pattern(value));
        values.push(value.clone());
        values.push(value);
    }
    if let Some(value) = query.source_or_account_query.as_deref() {
        clauses.push("candidate_hint LIKE ? ESCAPE '\\'");
        values.push(SqlValue::Text(like_pattern(value)));
    }
    if let Some(value) = query.local_key_query.as_deref() {
        clauses.push("local_key_id LIKE ? ESCAPE '\\'");
        values.push(SqlValue::Text(like_pattern(value)));
    }
    if let Some(value) = query.wire_api {
        clauses.push("wire_api = ?");
        values.push(SqlValue::Text(wire_api_name(value).to_string()));
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
        values.push(SqlValue::Text(like_pattern(value)));
    }
    let sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    (sql, values)
}

const USAGE_PRUNE_PREDICATE: &str = "created_at_ms < ?1 OR id NOT IN (
    SELECT id FROM usage_events ORDER BY created_at_ms DESC, id DESC LIMIT ?2
)";

const ROLLUP_USAGE_COLUMNS: &str = "requests, successful_requests, latency_ms,
    ttft_ms, ttft_samples, generation_ms, generation_samples,
    generation_output_tokens, input_tokens, cached_input_tokens,
    cached_input_samples, cache_write_input_tokens, cache_write_input_samples,
    reasoning_tokens, output_tokens, total_tokens, speed_output_tokens,
    speed_duration_ms, input_samples, output_samples, total_samples";

const ROLLUP_TOTAL_COLUMNS: &str = "COALESCE(SUM(requests), 0),
    COALESCE(SUM(successful_requests), 0), COALESCE(SUM(latency_ms), 0),
    COALESCE(SUM(ttft_ms), 0), COALESCE(SUM(ttft_samples), 0),
    COALESCE(SUM(generation_ms), 0), COALESCE(SUM(generation_samples), 0),
    COALESCE(SUM(generation_output_tokens), 0), COALESCE(SUM(input_tokens), 0),
    COALESCE(SUM(cached_input_tokens), 0), COALESCE(SUM(cached_input_samples), 0),
    COALESCE(SUM(cache_write_input_tokens), 0),
    COALESCE(SUM(cache_write_input_samples), 0), COALESCE(SUM(reasoning_tokens), 0),
    COALESCE(SUM(output_tokens), 0), COALESCE(SUM(total_tokens), 0),
    COALESCE(SUM(speed_output_tokens), 0), COALESCE(SUM(speed_duration_ms), 0)";

const ROLLUP_UPDATE_COLUMNS: &str = "requests = usage_key_rollups.requests + excluded.requests,
    successful_requests = usage_key_rollups.successful_requests + excluded.successful_requests,
    latency_ms = usage_key_rollups.latency_ms + excluded.latency_ms,
    ttft_ms = usage_key_rollups.ttft_ms + excluded.ttft_ms,
    ttft_samples = usage_key_rollups.ttft_samples + excluded.ttft_samples,
    generation_ms = usage_key_rollups.generation_ms + excluded.generation_ms,
    generation_samples = usage_key_rollups.generation_samples + excluded.generation_samples,
    generation_output_tokens = usage_key_rollups.generation_output_tokens + excluded.generation_output_tokens,
    input_tokens = usage_key_rollups.input_tokens + excluded.input_tokens,
    cached_input_tokens = usage_key_rollups.cached_input_tokens + excluded.cached_input_tokens,
    cached_input_samples = usage_key_rollups.cached_input_samples + excluded.cached_input_samples,
    cache_write_input_tokens = usage_key_rollups.cache_write_input_tokens + excluded.cache_write_input_tokens,
    cache_write_input_samples = usage_key_rollups.cache_write_input_samples + excluded.cache_write_input_samples,
    reasoning_tokens = usage_key_rollups.reasoning_tokens + excluded.reasoning_tokens,
    output_tokens = usage_key_rollups.output_tokens + excluded.output_tokens,
    total_tokens = usage_key_rollups.total_tokens + excluded.total_tokens,
    speed_output_tokens = usage_key_rollups.speed_output_tokens + excluded.speed_output_tokens,
    speed_duration_ms = usage_key_rollups.speed_duration_ms + excluded.speed_duration_ms,
    input_samples = usage_key_rollups.input_samples + excluded.input_samples,
    output_samples = usage_key_rollups.output_samples + excluded.output_samples,
    total_samples = usage_key_rollups.total_samples + excluded.total_samples";

const USAGE_TOTAL_COLUMNS: &str = "COUNT(*), \
    COALESCE(SUM(CASE WHEN success != 0 THEN 1 ELSE 0 END), 0), \
    COALESCE(SUM(latency_ms), 0), COALESCE(SUM(ttft_ms), 0), COUNT(ttft_ms), \
    COALESCE(SUM(CASE WHEN success != 0 THEN generation_ms ELSE 0 END), 0), \
    COUNT(CASE WHEN success != 0 THEN generation_ms END), \
    COALESCE(SUM(CASE WHEN success != 0 AND generation_ms IS NOT NULL \
        THEN MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0), 0) ELSE 0 END), 0), \
    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_input_tokens), 0), \
    COUNT(cached_input_tokens), COALESCE(SUM(cache_write_input_tokens), 0), \
    COUNT(cache_write_input_tokens), COALESCE(SUM(reasoning_tokens), 0), \
    COALESCE(SUM(output_tokens), 0), \
    COALESCE(SUM(total_tokens), 0), \
    COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > COALESCE(reasoning_tokens, 0) \
        THEN output_tokens - COALESCE(reasoning_tokens, 0) ELSE 0 END), 0), \
    COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > COALESCE(reasoning_tokens, 0) AND latency_ms > 0 \
        THEN latency_ms ELSE 0 END), 0)";

fn usage_totals(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
) -> Result<UsageTotals, String> {
    let sql = format!("SELECT {USAGE_TOTAL_COLUMNS} FROM usage_events{where_sql}");
    connection
        .query_row(&sql, params_from_iter(values.iter()), |row| {
            usage_totals_from_row(row, 0)
        })
        .map_err(db_error)
}

fn usage_groups(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
    key_sql: &str,
) -> Result<Vec<UsageGroup>, String> {
    let sql = format!(
        "SELECT {key_sql}, {USAGE_TOTAL_COLUMNS} FROM usage_events{where_sql} \
         GROUP BY 1 ORDER BY COUNT(*) DESC, 1"
    );
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            Ok(UsageGroup {
                key: row.get(0)?,
                label: None,
                totals: usage_totals_from_row(row, 1)?,
            })
        })
        .map_err(db_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
}

fn usage_model_equivalents(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
    price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
    source_price_overrides: &SourcePriceOverrides,
) -> Result<HashMap<String, ApiEquivalentSummary>, String> {
    let sql = format!(
        "SELECT candidate_kind, candidate_hint, COALESCE(resolved_model, requested_model, ''),
            SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens),
            SUM(output_tokens), SUM(total_tokens), COUNT(input_tokens),
            COUNT(cached_input_tokens), COUNT(cache_write_input_tokens),
            COUNT(output_tokens), COUNT(total_tokens)
         FROM usage_events{where_sql} GROUP BY 1, 2, 3"
    );
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            let kind = row.get::<_, String>(0)?;
            let candidate_id = row.get::<_, String>(1)?;
            let model = row.get::<_, String>(2)?;
            let input_tokens = optional_u64(row.get(3)?);
            let cached_input_tokens = optional_u64(row.get(4)?);
            let cache_write_input_tokens = optional_u64(row.get(5)?);
            let output_tokens = optional_u64(row.get(6)?);
            let total_tokens = optional_u64(row.get(7)?);
            let input_samples = nonnegative_u64(row.get(8)?);
            let cached_samples = nonnegative_u64(row.get(9)?);
            let cache_write_samples = nonnegative_u64(row.get(10)?);
            let output_samples = nonnegative_u64(row.get(11)?);
            let total_samples = nonnegative_u64(row.get(12)?);
            Ok((
                model.clone(),
                estimate_api_equivalent_with_price_override(
                    (!model.is_empty()).then_some(model.as_str()),
                    (input_samples > 0).then_some(input_tokens).flatten(),
                    (input_samples > 0 && cached_samples == input_samples)
                        .then_some(cached_input_tokens)
                        .flatten(),
                    (input_samples > 0 && cache_write_samples == input_samples)
                        .then_some(cache_write_input_tokens)
                        .flatten(),
                    (output_samples > 0).then_some(output_tokens).flatten(),
                    (total_samples > 0).then_some(total_tokens).flatten(),
                    configured_model_price(
                        price_overrides,
                        source_price_overrides,
                        &kind,
                        &candidate_id,
                        (!model.is_empty()).then_some(model.as_str()),
                    ),
                ),
            ))
        })
        .map_err(db_error)?;
    let mut equivalents = HashMap::<String, ApiEquivalentSummary>::new();
    for row in rows {
        let (model, estimate) = row.map_err(db_error)?;
        equivalents.entry(model).or_default().merge(estimate);
    }
    Ok(equivalents)
}

fn usage_buckets(
    connection: &Connection,
    where_sql: &str,
    values: &[SqlValue],
    query: &UsageQuery,
    price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
    source_price_overrides: &SourcePriceOverrides,
) -> Result<Vec<UsageBucket>, String> {
    let Some(bucket_ms) = query.bucket_ms else {
        return Ok(Vec::new());
    };
    let start_ms = query.from_ms.unwrap_or_default();
    let start = SqlValue::Integer(start_ms.min(i64::MAX as u64) as i64);
    let bucket = SqlValue::Integer(bucket_ms.min(i64::MAX as u64) as i64);
    let bucket_sql = "? + ((created_at_ms - ?) / ?) * ?";
    let sql = format!(
        "SELECT {bucket_sql}, {USAGE_TOTAL_COLUMNS} \
         FROM usage_events{where_sql} GROUP BY 1 ORDER BY 1"
    );
    let mut parameters = vec![start.clone(), start, bucket.clone(), bucket];
    parameters.extend_from_slice(values);
    let mut buckets = {
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let rows = statement
            .query_map(params_from_iter(parameters.iter()), |row| {
                Ok(UsageBucket {
                    start_ms: nonnegative_u64(row.get(0)?),
                    totals: usage_totals_from_row(row, 1)?,
                })
            })
            .map_err(db_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_error)?
    };
    let price_sql = format!(
        "SELECT {bucket_sql}, candidate_kind, candidate_hint, \
            COALESCE(resolved_model, requested_model), \
            SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens), \
            SUM(output_tokens), SUM(total_tokens), COUNT(input_tokens), \
            COUNT(cached_input_tokens), COUNT(cache_write_input_tokens), \
            COUNT(output_tokens), COUNT(total_tokens) \
         FROM usage_events{where_sql} GROUP BY 1, 2, 3, 4"
    );
    let mut statement = connection.prepare(&price_sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            let kind = row.get::<_, String>(1)?;
            let candidate_id = row.get::<_, String>(2)?;
            let model = row.get::<_, Option<String>>(3)?;
            let input_tokens: Option<i64> = row.get(4)?;
            let cached_input_tokens: Option<i64> = row.get(5)?;
            let cache_write_input_tokens: Option<i64> = row.get(6)?;
            let output_tokens: Option<i64> = row.get(7)?;
            let total_tokens: Option<i64> = row.get(8)?;
            let input_samples: i64 = row.get(9)?;
            let cached_samples: i64 = row.get(10)?;
            let cache_write_samples: i64 = row.get(11)?;
            let output_samples: i64 = row.get(12)?;
            let total_samples: i64 = row.get(13)?;
            let start_ms = nonnegative_u64(row.get(0)?);
            let input_tokens = (input_samples > 0)
                .then(|| optional_u64(input_tokens))
                .flatten();
            let cached_input_tokens = (input_samples > 0 && cached_samples == input_samples)
                .then(|| optional_u64(cached_input_tokens))
                .flatten();
            let cache_write_input_tokens = (input_samples > 0
                && cache_write_samples == input_samples)
                .then(|| optional_u64(cache_write_input_tokens))
                .flatten();
            Ok((
                start_ms,
                estimate_api_equivalent_with_price_override(
                    model.as_deref(),
                    input_tokens,
                    cached_input_tokens,
                    cache_write_input_tokens,
                    (output_samples > 0)
                        .then(|| optional_u64(output_tokens))
                        .flatten(),
                    (total_samples > 0)
                        .then(|| optional_u64(total_tokens))
                        .flatten(),
                    configured_model_price(
                        price_overrides,
                        source_price_overrides,
                        &kind,
                        &candidate_id,
                        model.as_deref(),
                    ),
                ),
            ))
        })
        .map_err(db_error)?;
    let mut equivalents = HashMap::<u64, ApiEquivalentSummary>::new();
    for row in rows {
        let (start_ms, estimate) = row.map_err(db_error)?;
        equivalents.entry(start_ms).or_default().merge(estimate);
    }
    for bucket in &mut buckets {
        bucket.totals.api_equivalent = equivalents.remove(&bucket.start_ms).unwrap_or_default();
    }
    Ok(buckets)
}

fn usage_totals_from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<UsageTotals> {
    Ok(UsageTotals {
        requests: nonnegative_u64(row.get(offset)?),
        successful_requests: nonnegative_u64(row.get(offset + 1)?),
        latency_ms: nonnegative_u64(row.get(offset + 2)?),
        ttft_ms: nonnegative_u64(row.get(offset + 3)?),
        ttft_samples: nonnegative_u64(row.get(offset + 4)?),
        generation_ms: nonnegative_u64(row.get(offset + 5)?),
        generation_samples: nonnegative_u64(row.get(offset + 6)?),
        generation_output_tokens: nonnegative_u64(row.get(offset + 7)?),
        input_tokens: nonnegative_u64(row.get(offset + 8)?),
        cached_input_tokens: nonnegative_u64(row.get(offset + 9)?),
        cached_input_samples: nonnegative_u64(row.get(offset + 10)?),
        cache_write_input_tokens: nonnegative_u64(row.get(offset + 11)?),
        cache_write_input_samples: nonnegative_u64(row.get(offset + 12)?),
        reasoning_tokens: nonnegative_u64(row.get(offset + 13)?),
        output_tokens: nonnegative_u64(row.get(offset + 14)?),
        total_tokens: nonnegative_u64(row.get(offset + 15)?),
        speed_output_tokens: nonnegative_u64(row.get(offset + 16)?),
        speed_duration_ms: nonnegative_u64(row.get(offset + 17)?),
        api_equivalent: ApiEquivalentSummary::default(),
    })
}

fn merge_usage_totals(target: &mut UsageTotals, value: UsageTotals) {
    target.requests = target.requests.saturating_add(value.requests);
    target.successful_requests = target
        .successful_requests
        .saturating_add(value.successful_requests);
    target.latency_ms = target.latency_ms.saturating_add(value.latency_ms);
    target.ttft_ms = target.ttft_ms.saturating_add(value.ttft_ms);
    target.ttft_samples = target.ttft_samples.saturating_add(value.ttft_samples);
    target.generation_ms = target.generation_ms.saturating_add(value.generation_ms);
    target.generation_samples = target
        .generation_samples
        .saturating_add(value.generation_samples);
    target.generation_output_tokens = target
        .generation_output_tokens
        .saturating_add(value.generation_output_tokens);
    target.input_tokens = target.input_tokens.saturating_add(value.input_tokens);
    target.cached_input_tokens = target
        .cached_input_tokens
        .saturating_add(value.cached_input_tokens);
    target.cached_input_samples = target
        .cached_input_samples
        .saturating_add(value.cached_input_samples);
    target.cache_write_input_tokens = target
        .cache_write_input_tokens
        .saturating_add(value.cache_write_input_tokens);
    target.cache_write_input_samples = target
        .cache_write_input_samples
        .saturating_add(value.cache_write_input_samples);
    target.reasoning_tokens = target
        .reasoning_tokens
        .saturating_add(value.reasoning_tokens);
    target.output_tokens = target.output_tokens.saturating_add(value.output_tokens);
    target.total_tokens = target.total_tokens.saturating_add(value.total_tokens);
    target.speed_output_tokens = target
        .speed_output_tokens
        .saturating_add(value.speed_output_tokens);
    target.speed_duration_ms = target
        .speed_duration_ms
        .saturating_add(value.speed_duration_ms);
    target.api_equivalent.merge(value.api_equivalent);
}

fn service_tier_name(value: DefaultServiceTier) -> &'static str {
    match value {
        DefaultServiceTier::Standard => "standard",
        DefaultServiceTier::Fast => "fast",
    }
}

fn parse_service_tier(value: &str) -> DefaultServiceTier {
    if value.eq_ignore_ascii_case("fast") || value.eq_ignore_ascii_case("priority") {
        DefaultServiceTier::Fast
    } else {
        DefaultServiceTier::Standard
    }
}

fn nonnegative_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn like_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('%');
    for character in value.chars() {
        if matches!(character, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    escaped
}

fn wire_api_name(value: WireApi) -> &'static str {
    match value {
        WireApi::Responses => "responses",
        WireApi::ChatCompletions => "chat_completions",
        WireApi::Messages => "messages",
    }
}

fn parse_wire_api(value: &str) -> WireApi {
    match value {
        "chat_completions" => WireApi::ChatCompletions,
        "messages" => WireApi::Messages,
        _ => WireApi::Responses,
    }
}

fn configured_model_price(
    overrides: &BTreeMap<String, ApiModelPriceOverride>,
    source_overrides: &SourcePriceOverrides,
    candidate_kind: &str,
    candidate_id: &str,
    model: Option<&str>,
) -> Option<ApiModelPriceOverride> {
    let model = model?.to_ascii_lowercase();
    (candidate_kind == "source")
        .then(|| source_overrides.get(candidate_id)?.get(&model).copied())
        .flatten()
        .or_else(|| overrides.get(&model).copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::identity_hint;
    use crate::state::ServerAccountRecord;
    use crate::state::SourceRecord;
    use crate::store::test_support::test_root;
    use std::fs;
    use zenith_relay_core::accounts::AccountAuthState;
    use zenith_relay_core::accounts::AccountHealthState;
    use zenith_relay_core::quota::QuotaSnapshot;
    use zenith_relay_core::quota::Subscription;
    use zenith_relay_core::ResponseAffinityBinding;
    use zenith_relay_core::ResponseAffinityStore;
    use zenith_relay_core::RoutingDiagnostics;
    use zenith_relay_core::SelectionReason;
    use zenith_relay_core::{TerminalOutputKind, ToolChoiceMode};

    #[test]
    fn usage_keeps_one_terminal_row_per_request() {
        let root = test_root("usage-terminal-row");
        let store = Store::open(root.join("relay.sqlite")).unwrap();
        let mut event = UsageEvent {
            request_id: "req_fallback".into(),
            attempt: 1,
            local_key_id: "key".into(),
            source_id: "source_1".into(),
            candidate_id: Some("source_1".into()),
            account_id: None,
            routing: None,
            requested_model: Some("gpt-test".into()),
            resolved_model: Some("gpt-test".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Fast,
            applied_service_tier: None,
            success: false,
            http_status: 503,
            error_category: Some("upstream_unavailable".into()),
            tool_use: ToolUseDiagnostics {
                client_tool_count: 2,
                forwarded_tool_count: 2,
                tool_choice: ToolChoiceMode::Auto,
                tool_call_count: 1,
                text_output: false,
                terminal_output: TerminalOutputKind::ToolCall,
            },
            cooldown_scope: Some("*".into()),
            retry_at_ms: Some(60_000),
            consecutive_failures: Some(1),
            latency_ms: 1,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            reasoning_tokens: None,
            output_tokens: None,
            total_tokens: None,
            quota_snapshot: None,
        };
        store.record_usage(&event, 1_000).unwrap();
        event.attempt = 2;
        event.source_id = "source_2".into();
        event.candidate_id = Some("source_2".into());
        event.success = true;
        event.http_status = 200;
        event.error_category = None;
        event.cooldown_scope = None;
        event.retry_at_ms = None;
        event.consecutive_failures = Some(0);
        event.applied_service_tier = Some(DefaultServiceTier::Standard);
        event.total_tokens = Some(10);
        store.record_usage(&event, 2_000).unwrap();

        event.request_id = "req_failed".into();
        event.attempt = 1;
        event.success = false;
        event.http_status = 503;
        event.error_category = Some("upstream_unavailable".into());
        event.total_tokens = None;
        store.record_usage(&event, 3_000).unwrap();
        event.attempt = 2;
        event.http_status = 429;
        event.error_category = Some("upstream_rate_limited".into());
        store.record_usage(&event, 4_000).unwrap();

        let page = store.usage_page(&UsageQuery::default()).unwrap();
        assert_eq!(page.total, 2);
        let fallback = page
            .events
            .iter()
            .find(|event| event.request_id == "req_fallback")
            .unwrap();
        assert!(fallback.success);
        assert_eq!(fallback.http_status, 200);
        assert_eq!(fallback.service_tier, DefaultServiceTier::Fast);
        assert_eq!(
            fallback.applied_service_tier,
            Some(DefaultServiceTier::Standard)
        );
        assert_eq!(
            fallback
                .tool_use
                .as_ref()
                .map(|tool_use| tool_use.tool_call_count),
            Some(1)
        );
        let failed = page
            .events
            .iter()
            .find(|event| event.request_id == "req_failed")
            .unwrap();
        assert!(!failed.success);
        assert_eq!(failed.http_status, 429);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleting_account_removes_telemetry_and_rejects_late_usage() {
        let root = test_root("account-delete");
        let store = Store::open(root.join("relay.sqlite")).unwrap();
        let account_id = "account-delete";
        store
            .save_account(&ServerAccountRecord {
                id: account_id.into(),
                label: "Delete me".into(),
                identity_hint: "deleted".into(),
                enabled: true,
                in_pool: true,
                draining: false,
                source_id: "codex".into(),
                secret_ref: "account:delete".into(),
                auth_state: AccountAuthState::Active,
                health: AccountHealthState::Healthy,
                models: vec!["gpt-test".into()],
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                priority: 0,
                weight: 1,
                subscription: Subscription::default(),
                quota: QuotaSnapshot::default(),
                economics: Default::default(),
                cooldowns: BTreeMap::new(),
                consecutive_failures: 0,
                created_at_ms: 1,
                last_used_at_ms: None,
                last_error_code: None,
                proxy_id: None,
                bypass_common_proxy: false,
            })
            .unwrap();
        let mut event = UsageEvent {
            request_id: "request-before-delete".into(),
            attempt: 1,
            local_key_id: "key".into(),
            source_id: "codex".into(),
            candidate_id: Some(account_id.into()),
            account_id: Some(account_id.into()),
            routing: None,
            requested_model: Some("gpt-5.4".into()),
            resolved_model: Some("gpt-5.4".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Fast,
            applied_service_tier: Some(DefaultServiceTier::Standard),
            success: true,
            http_status: 200,
            error_category: None,
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 1,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: Some(2),
            cached_input_tokens: Some(1),
            cache_write_input_tokens: Some(1),
            reasoning_tokens: None,
            output_tokens: Some(3),
            total_tokens: Some(5),
            quota_snapshot: None,
        };
        store.record_usage(&event, 10).unwrap();
        let usage = store.usage_page(&UsageQuery::default()).unwrap();
        // The account event carries a measured value, and the totals are that
        // same value merged once. Asserting the relation instead of a catalog
        // price keeps the test stable when prices move.
        assert!(usage.events[0].api_equivalent.micro_usd > 0);
        assert_eq!(usage.totals.api_equivalent, usage.events[0].api_equivalent);
        assert!(usage.events[0].tool_use.is_none());
        let candidate_hint = hex::encode(Sha256::digest(account_id.as_bytes()))[..12].to_string();
        store
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO usage_candidate_rollups(candidate_kind, candidate_id, model)
                 VALUES ('account', ?1, 'gpt-test')",
                [&candidate_hint],
            )
            .unwrap();
        store
            .upsert(&ResponseAffinityBinding {
                key: "response-delete".into(),
                candidate_id: account_id.into(),
                expires_at_ms: 1_000,
            })
            .unwrap();

        assert!(store.delete_account(account_id).unwrap().is_some());
        assert_eq!(store.usage_page(&UsageQuery::default()).unwrap().total, 0);
        assert!(!store
            .api_equivalents()
            .unwrap()
            .contains_key(&candidate_hint));
        assert!(store.find("response-delete", 1).unwrap().is_none());

        event.request_id = "request-after-delete".into();
        store.record_usage(&event, 20).unwrap();
        assert_eq!(store.usage_page(&UsageQuery::default()).unwrap().total, 0);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retention_archives_key_totals_and_rejects_late_duplicate_rows() {
        let root = test_root("usage-retention");
        let path = root.join("relay.sqlite");
        let store = Store::open(path.clone()).unwrap();
        let mut event = UsageEvent {
            request_id: "req_old".into(),
            attempt: 1,
            local_key_id: "key_alpha".into(),
            source_id: "source_alpha".into(),
            candidate_id: Some("source_alpha".into()),
            account_id: None,
            routing: None,
            requested_model: Some("gpt-5.4".into()),
            resolved_model: Some("gpt-5.4".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Fast,
            applied_service_tier: Some(DefaultServiceTier::Fast),
            success: true,
            http_status: 200,
            error_category: None,
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 100,
            ttft_ms: Some(10),
            generation_ms: Some(90),
            input_tokens: Some(1_000_000),
            cached_input_tokens: Some(400_000),
            cache_write_input_tokens: None,
            reasoning_tokens: Some(0),
            output_tokens: Some(100_000),
            total_tokens: Some(1_100_000),
            quota_snapshot: None,
        };
        store.record_usage(&event, DAY_MS).unwrap();
        event.request_id = "req_current".into();
        event.input_tokens = Some(10);
        event.cached_input_tokens = Some(0);
        event.output_tokens = Some(0);
        event.total_tokens = Some(10);
        store.record_usage(&event, 100 * DAY_MS).unwrap();

        assert_eq!(
            store
                .prune_usage_history_with_limits(50 * DAY_MS, 100, 0)
                .unwrap(),
            1
        );
        assert_eq!(store.usage_page(&UsageQuery::default()).unwrap().total, 1);
        let totals = store.key_usage_totals("key_alpha").unwrap();
        assert_eq!(totals.requests, 2);
        assert_eq!(totals.input_tokens, 1_000_010);
        assert_eq!(totals.total_tokens, 1_100_010);
        assert_eq!(totals.api_equivalent.micro_usd, 3_100_025);
        let candidate_hint = hex::encode(Sha256::digest(b"source_alpha"))[..12].to_string();
        assert_eq!(
            store.api_equivalents().unwrap().get(&candidate_hint),
            Some(&totals.api_equivalent)
        );

        event.request_id = "req_newest".into();
        store.record_usage(&event, 102 * DAY_MS).unwrap();
        assert_eq!(store.prune_usage_history_with_limits(0, 1, 0).unwrap(), 1);
        let totals = store.key_usage_totals("key_alpha").unwrap();
        assert_eq!(totals.requests, 3);
        assert_eq!(totals.api_equivalent.micro_usd, 3_100_050);
        let archived_daily_requests = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT COALESCE(SUM(requests), 0) FROM usage_key_rollups \
                 WHERE local_key_id = 'key_alpha' AND period_start_ms >= 0",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(archived_daily_requests, 2);

        event.request_id = "req_old".into();
        event.input_tokens = Some(9_000_000);
        store.record_usage(&event, 101 * DAY_MS).unwrap();
        assert_eq!(store.usage_page(&UsageQuery::default()).unwrap().total, 1);
        assert_eq!(store.key_usage_totals("key_alpha").unwrap().requests, 3);
        drop(store);

        let reopened = Store::open(path).unwrap();
        assert_eq!(reopened.key_usage_totals("key_alpha").unwrap(), totals);
        assert_eq!(reopened.clear_usage().unwrap(), 1);
        assert_eq!(
            reopened.key_usage_totals("key_alpha").unwrap(),
            UsageTotals::default()
        );
        assert!(reopened.api_equivalents().unwrap().is_empty());
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_prices_revalue_raw_and_archived_usage_per_provider() {
        let root = test_root("source-prices");
        let store = Store::open(root.join("relay.sqlite")).unwrap();
        let price = |input, output| ApiModelPriceOverride {
            input_micro_usd_per_million: input,
            cached_input_micro_usd_per_million: Some(input / 10),
            cache_write_5m_micro_usd_per_million: None,
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: output,
        };
        for (id, model_price) in [
            ("source_cheap", price(1_000_000, 2_000_000)),
            ("source_expensive", price(2_000_000, 4_000_000)),
        ] {
            store
                .save_source(&SourceRecord {
                    id: id.into(),
                    name: id.into(),
                    enabled: true,
                    in_pool: true,
                    draining: false,
                    base_url: "https://example.test/v1".into(),
                    secret_ref: format!("source:{id}"),
                    wire_api: WireApi::Responses,
                    models: vec!["private-model".into()],
                    allowed_models: Vec::new(),
                    excluded_models: Vec::new(),
                    priority: 0,
                    weight: 1,
                    recovery_delay_seconds: 0,
                    model_price_overrides: BTreeMap::from([("private-model".into(), model_price)]),
                    last_error_code: None,
                })
                .unwrap();
        }
        let mut event = UsageEvent {
            request_id: "request-cheap".into(),
            attempt: 1,
            local_key_id: "key".into(),
            source_id: "source_cheap".into(),
            candidate_id: Some("source_cheap".into()),
            account_id: None,
            routing: None,
            requested_model: Some("private-model".into()),
            resolved_model: Some("private-model".into()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: true,
            http_status: 200,
            error_category: None,
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 100,
            ttft_ms: Some(10),
            generation_ms: Some(90),
            input_tokens: Some(1_000_000),
            cached_input_tokens: Some(0),
            cache_write_input_tokens: Some(0),
            reasoning_tokens: Some(0),
            output_tokens: Some(100_000),
            total_tokens: Some(1_100_000),
            quota_snapshot: None,
        };
        store.record_usage(&event, 1).unwrap();
        event.request_id = "request-expensive".into();
        event.source_id = "source_expensive".into();
        event.candidate_id = Some("source_expensive".into());
        store.record_usage(&event, 1).unwrap();

        let page = store.usage_page(&UsageQuery::default()).unwrap();
        assert_eq!(page.totals.api_equivalent.micro_usd, 3_600_000);
        let event_value = |request_id: &str| {
            page.events
                .iter()
                .find(|event| event.request_id == request_id)
                .unwrap()
                .api_equivalent
        };
        assert_eq!(event_value("request-cheap").micro_usd, 1_200_000);
        assert_eq!(event_value("request-expensive").micro_usd, 2_400_000);
        let equivalents = store.api_equivalents().unwrap();
        assert_eq!(
            equivalents[&identity_hint("source_cheap")].micro_usd,
            1_200_000
        );
        assert_eq!(
            equivalents[&identity_hint("source_expensive")].micro_usd,
            2_400_000
        );

        assert_eq!(store.prune_usage_history_with_limits(2, 100, 0).unwrap(), 2);
        assert_eq!(
            store
                .key_usage_totals("key")
                .unwrap()
                .api_equivalent
                .micro_usd,
            3_600_000
        );
        assert_eq!(
            store
                .api_equivalents()
                .unwrap()
                .values()
                .map(|value| value.micro_usd)
                .sum::<u64>(),
            3_600_000
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_filters_paginate_escape_wildcards_and_clear() {
        use zenith_relay_core::protocol::UsageRange;

        let root = test_root("usage-query");
        let store = Store::open(root.join("relay.sqlite")).unwrap();
        for (index, success, model, error) in [
            (1, true, "gpt-5.4", None),
            (2, false, "gpt%literal", Some("quota_exhausted")),
            (3, true, "gpt-test", None),
        ] {
            store
                .record_usage(
                    &UsageEvent {
                        request_id: format!("req_{index}"),
                        attempt: 1,
                        local_key_id: "key_alpha".to_string(),
                        source_id: "source_alpha".to_string(),
                        candidate_id: Some("source_alpha".to_string()),
                        account_id: None,
                        routing: Some(RoutingDiagnostics {
                            reason: SelectionReason::QuotaHeadroom,
                            eligible_candidates: 3,
                            quota_remaining_basis_points: Some(5_400),
                            in_flight_before: 0,
                            dispatches_before: index - 1,
                        }),
                        requested_model: Some(model.to_string()),
                        resolved_model: Some(model.to_string()),
                        wire_api: WireApi::Responses,
                        service_tier: DefaultServiceTier::Standard,
                        applied_service_tier: None,
                        success,
                        http_status: if success { 200 } else { 429 },
                        error_category: error.map(str::to_string),
                        tool_use: ToolUseDiagnostics::default(),
                        cooldown_scope: None,
                        retry_at_ms: None,
                        consecutive_failures: None,
                        latency_ms: 10,
                        ttft_ms: Some(4),
                        generation_ms: Some(6),
                        input_tokens: Some(1),
                        cached_input_tokens: Some(u64::from(index != 2)),
                        cache_write_input_tokens: (index == 2).then_some(1),
                        reasoning_tokens: Some(1),
                        output_tokens: Some(1),
                        total_tokens: Some(2),
                        quota_snapshot: None,
                    },
                    2_000 + index,
                )
                .unwrap();
        }

        let page = store
            .usage_page(&UsageQuery {
                page: 1,
                page_size: 1,
                range: Some(UsageRange::Custom),
                from_ms: Some(2_000),
                to_ms: Some(3_000),
                bucket_ms: Some(1_000),
                model_query: Some("%".to_string()),
                success: Some(false),
                error_category: Some("quota_exhausted".to_string()),
                request_id_query: Some("req_2".to_string()),
                ..UsageQuery::default()
            })
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.total_pages, 1);
        assert_eq!(page.totals.requests, 1);
        assert_eq!(page.totals.total_tokens, 2);
        assert_eq!(page.totals.speed_output_tokens, 0);
        assert_eq!(page.models.len(), 1);
        assert_eq!(page.pool_members.len(), 1);
        assert_eq!(page.buckets.len(), 1);
        assert_eq!(page.buckets[0].start_ms, 2_000);
        assert_eq!(page.buckets[0].totals.total_tokens, 2);
        assert_eq!(
            page.buckets[0].totals.api_equivalent,
            page.totals.api_equivalent
        );
        assert_eq!(page.events[0].request_id, "req_2");
        assert_eq!(page.events[0].ttft_ms, Some(4));
        assert_eq!(page.events[0].cache_write_input_tokens, Some(1));
        assert_eq!(page.events[0].api_equivalent, page.totals.api_equivalent);
        assert_eq!(page.totals.cache_write_input_tokens, 1);
        assert_eq!(page.totals.cache_write_input_samples, 1);
        assert_eq!(
            page.events[0]
                .routing
                .as_ref()
                .map(|routing| routing.reason),
            Some(SelectionReason::QuotaHeadroom)
        );
        let hint = hex::encode(Sha256::digest(b"source_alpha"))[..12].to_string();
        assert_eq!(
            store.api_equivalents().unwrap().get(&hint),
            Some(&ApiEquivalentSummary {
                micro_usd: 15,
                priced_tokens: 2,
                unpriced_tokens: 4,
            })
        );
        assert_eq!(store.clear_usage().unwrap(), 3);
        assert_eq!(store.usage_page(&UsageQuery::default()).unwrap().total, 0);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
