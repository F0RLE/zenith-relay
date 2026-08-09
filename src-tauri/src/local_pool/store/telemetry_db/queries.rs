use super::*;

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
}
