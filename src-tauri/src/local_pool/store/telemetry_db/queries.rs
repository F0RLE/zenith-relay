use super::*;
use std::sync::atomic::Ordering;

impl TelemetryDb {
    pub fn list(&self, limit: u16) -> Result<Vec<UsageLog>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
        let mut statement = connection
            .prepare(
                "SELECT id, strftime('%Y-%m-%dT%H:%M:%SZ', created_at), request_id, attempt,
                    local_key_id, source_id, candidate_id, account_id, requested_model,
                    resolved_model, wire_api, success, http_status, error_category, latency_ms,
                    ttft_ms, generation_ms, input_tokens, cached_input_tokens,
                    cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens,
                    service_tier, applied_service_tier, routing_json, tool_use_json
                 FROM request_logs ORDER BY id DESC LIMIT ?1",
            )
            .map_err(db_error)?;
        let logs = statement
            .query_map([limit.clamp(1, 500)], usage_log_from_row)
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(logs)
    }

    #[cfg(test)]
    pub fn usage_page(&self, query: &UsageQuery) -> Result<LocalUsagePage> {
        self.usage_page_with_price_overrides(query, &BTreeMap::new(), &BTreeMap::new())
    }

    pub fn usage_page_with_price_overrides(
        &self,
        query: &UsageQuery,
        price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
        source_price_overrides: &SourcePriceOverrides,
    ) -> Result<LocalUsagePage> {
        let page = query.page.max(1);
        let page_size = if query.page_size == 0 {
            50
        } else {
            query.page_size.clamp(1, 200)
        };
        let connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
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
            price_overrides,
            source_price_overrides,
        )?;
        for group in &mut models {
            group.totals.api_equivalent = model_equivalents.remove(&group.key).unwrap_or_default();
            totals.api_equivalent.merge(group.totals.api_equivalent);
        }
        let pool_members = usage_groups(
            &connection,
            &where_sql,
            &values,
            "COALESCE(account_id, source_id, '')",
        )?;
        let buckets = usage_buckets(
            &connection,
            &where_sql,
            &values,
            query,
            price_overrides,
            source_price_overrides,
        )?;
        let total = totals.requests;
        let offset = u64::from(page.saturating_sub(1)) * u64::from(page_size);
        let sql = format!(
            "SELECT id, strftime('%Y-%m-%dT%H:%M:%SZ', created_at), request_id, attempt,
                local_key_id, source_id, candidate_id, account_id, requested_model,
                resolved_model, wire_api, success, http_status, error_category, latency_ms,
                ttft_ms, generation_ms, input_tokens, cached_input_tokens,
                cache_write_input_tokens, reasoning_tokens, output_tokens, total_tokens,
                service_tier, applied_service_tier, routing_json, tool_use_json
             FROM request_logs{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?"
        );
        let mut statement = connection.prepare(&sql).map_err(db_error)?;
        let mut page_values = values;
        page_values.push(SqlValue::Integer(i64::from(page_size)));
        page_values.push(SqlValue::Integer(offset.min(i64::MAX as u64) as i64));
        let mut events = statement
            .query_map(params_from_iter(page_values.iter()), usage_log_from_row)
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        for event in &mut events {
            let candidate_kind = if event.account_id.is_some() {
                "account"
            } else {
                "source"
            };
            let candidate_id = event.account_id.as_deref().unwrap_or(&event.source_id);
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
                    price_overrides,
                    source_price_overrides,
                    candidate_kind,
                    candidate_id,
                    model,
                ),
            );
        }
        let total_pages = if total == 0 {
            0
        } else {
            total.div_ceil(u64::from(page_size)) as u32
        };
        Ok(LocalUsagePage {
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

    #[cfg(test)]
    pub fn api_equivalents(&self) -> Result<UsageEquivalents> {
        self.api_equivalents_with_price_overrides(&BTreeMap::new(), &BTreeMap::new())
    }

    pub fn api_equivalents_with_price_overrides(
        &self,
        price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
        source_price_overrides: &SourcePriceOverrides,
    ) -> Result<UsageEquivalents> {
        let usage_revision = self.usage_revision.load(Ordering::Acquire);
        let pricing_revision = serde_json::to_string(&(
            api_pricing_revision(),
            price_overrides,
            source_price_overrides,
        ))
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("usage pricing revision serialization failed: {error}"),
            )
        })?;
        if let Some(cached) = self
            .api_equivalent_cache
            .lock()
            .map_err(lock_error)?
            .as_ref()
            .filter(|cached| {
                cached.usage_revision == usage_revision
                    && cached.pricing_revision == pricing_revision
            })
        {
            return Ok(cached.value.clone());
        }
        let connection = self
            .connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))?;
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
                    SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
                        COALESCE(account_id, source_id),
                        COALESCE(resolved_model, requested_model, ''),
                        COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_input_tokens), 0),
                        COALESCE(SUM(cache_write_input_tokens), 0), COALESCE(SUM(output_tokens), 0),
                        COALESCE(SUM(total_tokens), 0), COUNT(input_tokens),
                        COUNT(cached_input_tokens), COUNT(cache_write_input_tokens)
                    FROM request_logs GROUP BY 1, 2, 3
                 ) GROUP BY candidate_kind, candidate_id, model",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                let input_tokens: Option<i64> = row.get(3)?;
                let cached_input_tokens: Option<i64> = row.get(4)?;
                let cache_write_input_tokens: Option<i64> = row.get(5)?;
                let output_tokens: Option<i64> = row.get(6)?;
                let total_tokens: Option<i64> = row.get(7)?;
                let input_samples: i64 = row.get(8)?;
                let cached_samples: i64 = row.get(9)?;
                let cache_write_samples: i64 = row.get(10)?;
                let model = row.get::<_, Option<String>>(2)?;
                let kind = row.get::<_, String>(0)?;
                let id = row.get::<_, String>(1)?;
                Ok((
                    kind.clone(),
                    id.clone(),
                    estimate_api_equivalent_with_price_override(
                        model.as_deref(),
                        input_tokens.map(rust_u64),
                        (input_samples > 0 && cached_samples == input_samples)
                            .then(|| cached_input_tokens.map(rust_u64))
                            .flatten(),
                        (input_samples > 0 && cache_write_samples == input_samples)
                            .then(|| cache_write_input_tokens.map(rust_u64))
                            .flatten(),
                        output_tokens.map(rust_u64),
                        total_tokens.map(rust_u64),
                        configured_model_price(
                            price_overrides,
                            source_price_overrides,
                            &kind,
                            &id,
                            model.as_deref(),
                        ),
                    ),
                ))
            })
            .map_err(db_error)?;
        let mut equivalents = UsageEquivalents::default();
        for row in rows {
            let (kind, id, estimate) = row.map_err(db_error)?;
            let values = if kind == "account" {
                &mut equivalents.accounts
            } else {
                &mut equivalents.sources
            };
            values.entry(id).or_default().merge(estimate);
        }
        drop(statement);
        drop(connection);
        if self.usage_revision.load(Ordering::Acquire) == usage_revision {
            self.api_equivalent_cache
                .lock()
                .map_err(lock_error)?
                .replace(CachedUsageEquivalents {
                    usage_revision,
                    pricing_revision,
                    value: equivalents.clone(),
                });
        }
        Ok(equivalents)
    }
}
