use super::*;

#[derive(Clone, Default)]
pub(super) struct UsageAggregate {
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

    pub(super) fn from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<Self> {
        let candidate_kind: String = row.get(offset)?;
        let candidate_id: String = row.get(offset + 1)?;
        let model: String = row.get(offset + 2)?;
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
            &model,
            input_tokens.map(|value| value.max(0) as u64),
            cached_input_tokens.map(|value| value.max(0) as u64),
            cache_write_input_tokens.map(|value| value.max(0) as u64),
            cache_write_ttl.as_deref(),
            output_tokens.map(|value| value.max(0) as u64),
            total_tokens.map(|value| value.max(0) as u64),
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
            price_class: match zenith_relay_core::usage::UsagePriceClass::from_observed(
                applied_service_tier,
            ) {
                zenith_relay_core::usage::UsagePriceClass::Flex => "flex",
                zenith_relay_core::usage::UsagePriceClass::Priority => "priority",
                zenith_relay_core::usage::UsagePriceClass::Standard => "standard",
            }
            .to_string(),
            context_band: match zenith_relay_core::usage::UsageContextBand::from_input_tokens(
                input_tokens,
            ) {
                zenith_relay_core::usage::UsageContextBand::Above272k => "above_272k",
                zenith_relay_core::usage::UsageContextBand::Above200k => "above_200k",
                zenith_relay_core::usage::UsageContextBand::Base => "base",
            }
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

pub(super) fn apply_aggregate_delta(
    transaction: &Transaction<'_>,
    aggregate: &UsageAggregate,
    multiplier: i64,
) -> Result<()> {
    transaction
        .execute(
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
