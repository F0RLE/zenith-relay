use super::*;

#[derive(Clone, Default, PartialEq, Eq)]
pub(in crate::local_pool::store::telemetry_db) struct UsageAggregate {
    candidate_kind: String,
    candidate_id: String,
    model: String,
    price_class: String,
    context_band: String,
    input_tokens: i64,
    input_samples: i64,
    cached_input_tokens: i64,
    cached_input_samples: i64,
    cache_write_input_tokens: i64,
    cache_write_input_samples: i64,
    cache_write_5m_tokens: i64,
    cache_write_1h_tokens: i64,
    unknown_cache_write_tokens: i64,
    output_tokens: i64,
    output_samples: i64,
    total_tokens: i64,
    total_samples: i64,
}

impl UsageAggregate {
    pub(in crate::local_pool::store::telemetry_db) fn with_context_band(
        mut self,
        band: &str,
    ) -> Self {
        self.context_band = band.to_string();
        self
    }

    pub(in crate::local_pool::store::telemetry_db) fn with_price_class(
        mut self,
        class: &str,
    ) -> Self {
        self.price_class = class.to_string();
        self
    }

    pub(super) fn from_event(event: &UsageEvent) -> Self {
        let cache_write_ttl = event
            .cache_write_ttl
            .as_deref()
            .and_then(zenith_relay_core::usage::normalize_reported_cache_ttls);
        Self::from_values(
            if event.account_id.is_some() {
                "account"
            } else {
                "source"
            },
            event
                .account_id
                .as_deref()
                .unwrap_or(event.source_id.as_str()),
            event
                .resolved_model
                .as_deref()
                .or(event.requested_model.as_deref())
                .unwrap_or_default(),
            event.input_tokens,
            event.cached_input_tokens,
            event.cache_write_input_tokens,
            cache_write_ttl.as_deref(),
            event.output_tokens,
            event.total_tokens,
            event.applied_service_tier.as_deref(),
        )
    }

    pub(in crate::local_pool::store::telemetry_db) fn from_row(
        row: &rusqlite::Row<'_>,
        offset: usize,
    ) -> rusqlite::Result<Self> {
        // Immutable migrations 032/034 use the pre-035 classification. Keep
        // their subtraction keys stable when new price classes are introduced.
        let mut aggregate = Self::from_row_current(row, offset)?;
        if !matches!(
            aggregate.price_class.as_str(),
            "standard" | "flex" | "priority" | "ultrafast"
        ) {
            aggregate.price_class = "standard".into();
        }
        Ok(aggregate)
    }

    pub(in crate::local_pool::store::telemetry_db) fn from_row_current(
        row: &rusqlite::Row<'_>,
        offset: usize,
    ) -> rusqlite::Result<Self> {
        let candidate_kind: String = row.get(offset)?;
        let candidate_id: String = row.get(offset + 1)?;
        let model_id: String = row.get(offset + 2)?;
        let input_tokens: Option<i64> = row.get(offset + 3)?;
        let cached_input_tokens: Option<i64> = row.get(offset + 4)?;
        let cache_write_input_tokens: Option<i64> = row.get(offset + 5)?;
        let cache_write_ttl: Option<String> = row.get(offset + 6)?;
        let output_tokens: Option<i64> = row.get(offset + 7)?;
        let total_tokens: Option<i64> = row.get(offset + 8)?;
        let applied_service_tier: Option<String> = row.get(offset + 9)?;
        Ok(Self::from_values(
            &candidate_kind,
            &candidate_id,
            &model_id,
            input_tokens.map(|input_token_count| input_token_count.max(0) as u64),
            cached_input_tokens
                .map(|cached_input_token_count| cached_input_token_count.max(0) as u64),
            cache_write_input_tokens
                .map(|cache_write_input_token_count| cache_write_input_token_count.max(0) as u64),
            cache_write_ttl.as_deref(),
            output_tokens.map(|output_token_count| output_token_count.max(0) as u64),
            total_tokens.map(|total_token_count| total_token_count.max(0) as u64),
            applied_service_tier.as_deref(),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn from_values(
        candidate_kind: &str,
        candidate_id: &str,
        model: &str,
        input_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
        cache_write_input_tokens: Option<u64>,
        cache_write_ttl: Option<&str>,
        output_tokens: Option<u64>,
        total_tokens: Option<u64>,
        applied_service_tier: Option<&str>,
    ) -> Self {
        let mut aggregate = Self {
            candidate_kind: candidate_kind.to_string(),
            candidate_id: candidate_id.to_string(),
            model: model.to_string(),
            price_class: zenith_relay_core::usage::UsagePriceClass::observed_label(
                applied_service_tier,
            ),
            context_band: zenith_relay_core::usage::UsageContextBand::from_input_tokens(
                input_tokens,
            )
            .as_label()
            .to_string(),
            input_tokens: input_tokens.map(sql_u64).unwrap_or_default(),
            input_samples: i64::from(input_tokens.is_some()),
            cached_input_tokens: cached_input_tokens.map(sql_u64).unwrap_or_default(),
            cached_input_samples: i64::from(cached_input_tokens.is_some()),
            cache_write_input_tokens: cache_write_input_tokens.map(sql_u64).unwrap_or_default(),
            cache_write_input_samples: i64::from(cache_write_input_tokens.is_some()),
            output_tokens: output_tokens.map(sql_u64).unwrap_or_default(),
            output_samples: i64::from(output_tokens.is_some()),
            total_tokens: total_tokens.map(sql_u64).unwrap_or_default(),
            total_samples: i64::from(total_tokens.is_some()),
            ..Self::default()
        };
        let cache_write_tokens = cache_write_input_tokens.map(sql_u64).unwrap_or_default();
        match cache_write_ttl {
            Some("5m") => aggregate.cache_write_5m_tokens = cache_write_tokens,
            Some("1h") => aggregate.cache_write_1h_tokens = cache_write_tokens,
            _ => aggregate.unknown_cache_write_tokens = cache_write_tokens,
        }
        aggregate
    }
}

pub(in crate::local_pool::store::telemetry_db) fn apply_aggregate_delta(
    transaction: &Transaction<'_>,
    aggregate: &UsageAggregate,
    multiplier: i64,
) -> Result<()> {
    transaction
        .prepare_cached(
            "INSERT INTO usage_candidate_rollups(
                candidate_kind, candidate_id, model,
                price_class, context_band,
                input_tokens, input_samples, cached_input_tokens, cached_input_samples,
                cache_write_input_tokens, cache_write_input_samples,
                cache_write_5m_tokens, cache_write_1h_tokens, unknown_cache_write_tokens,
                output_tokens, output_samples, total_tokens, total_samples
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
             ON CONFLICT(candidate_kind, candidate_id, model, price_class, context_band) DO UPDATE SET
                input_tokens = input_tokens + excluded.input_tokens,
                input_samples = input_samples + excluded.input_samples,
                cached_input_tokens = cached_input_tokens + excluded.cached_input_tokens,
                cached_input_samples = cached_input_samples + excluded.cached_input_samples,
                cache_write_input_tokens = cache_write_input_tokens + excluded.cache_write_input_tokens,
                cache_write_input_samples = cache_write_input_samples + excluded.cache_write_input_samples,
                cache_write_5m_tokens = cache_write_5m_tokens + excluded.cache_write_5m_tokens,
                cache_write_1h_tokens = cache_write_1h_tokens + excluded.cache_write_1h_tokens,
                unknown_cache_write_tokens = unknown_cache_write_tokens + excluded.unknown_cache_write_tokens,
                output_tokens = output_tokens + excluded.output_tokens,
                output_samples = output_samples + excluded.output_samples,
                total_tokens = total_tokens + excluded.total_tokens,
                total_samples = total_samples + excluded.total_samples",
        )
        .map_err(db_error)?
        .execute(
            params![
                &aggregate.candidate_kind,
                &aggregate.candidate_id,
                &aggregate.model,
                &aggregate.price_class,
                &aggregate.context_band,
                aggregate.input_tokens * multiplier,
                aggregate.input_samples * multiplier,
                aggregate.cached_input_tokens * multiplier,
                aggregate.cached_input_samples * multiplier,
                aggregate.cache_write_input_tokens * multiplier,
                aggregate.cache_write_input_samples * multiplier,
                aggregate.cache_write_5m_tokens * multiplier,
                aggregate.cache_write_1h_tokens * multiplier,
                aggregate.unknown_cache_write_tokens * multiplier,
                aggregate.output_tokens * multiplier,
                aggregate.output_samples * multiplier,
                aggregate.total_tokens * multiplier,
                aggregate.total_samples * multiplier,
            ],
        )
        .map_err(db_error)?;
    Ok(())
}

#[cfg(test)]
mod legacy_decode_tests {
    use super::*;

    #[test]
    fn legacy_row_decode_keeps_immutable_migration_price_keys() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let (legacy, current) = connection
            .query_row(
                "SELECT 'source', 'source', 'model', 150001, NULL, NULL, NULL,
                    10, 150011, 'premium_v2'",
                [],
                |row| {
                    Ok((
                        UsageAggregate::from_row(row, 0)?,
                        UsageAggregate::from_row_current(row, 0)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(legacy.price_class, "standard");
        assert_eq!(current.price_class, "premium_v2");
        assert_eq!(legacy.context_band, current.context_band);
        assert_eq!(legacy.input_tokens, current.input_tokens);
        assert_eq!(legacy.total_tokens, current.total_tokens);
    }
}
