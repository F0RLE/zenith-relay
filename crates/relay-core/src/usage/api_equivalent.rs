use super::ApiEquivalentSummary;
use super::{sql_count_u64, sql_optional_u64};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod estimate;

pub use estimate::{
    estimate_api_equivalent_with_catalog, estimate_api_equivalent_with_token_price,
    estimate_candidate_api_equivalent_with_catalog, normalize_model_price_overrides,
    resolve_candidate_price, CandidatePriceQuery, CatalogPriceResolver,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiModelPriceOverride {
    pub input_micro_usd_per_million: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_micro_usd_per_million: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_5m_micro_usd_per_million: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h_micro_usd_per_million: Option<u64>,
    pub output_micro_usd_per_million: u64,
}

/// Price provenance for a compatible API source.
///
/// Account usage never uses this type as a catalog: account API-equivalent
/// values come from the declared-family LiteLLM snapshot. For API sources,
/// provider-discovered prices win, LiteLLM exact/canonical records are the
/// next fallbacks, and an operator's manual value is used only when neither
/// exists.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiModelPriceSources {
    pub provider: Option<ApiModelPriceOverride>,
    pub manual: Option<ApiModelPriceOverride>,
}

/// Per-source price provenance indexed by source id and normalized model id.
///
/// This remains a storage-neutral representation so desktop telemetry and the
/// user-managed server apply the same pricing policy without sharing a schema.
pub type SourceModelPriceOverrides = BTreeMap<String, BTreeMap<String, ApiModelPriceSources>>;

/// Token measurements required to estimate an API-equivalent value.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ApiEquivalentUsage {
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_5m_tokens: Option<u64>,
    pub cache_write_1h_tokens: Option<u64>,
    pub unknown_cache_write_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

macro_rules! cache_write_ttl_bucket_sums_sql {
    () => {
        "SUM(CASE WHEN cache_write_ttl = '5m' THEN cache_write_input_tokens ELSE 0 END), \
         SUM(CASE WHEN cache_write_ttl = '1h' THEN cache_write_input_tokens ELSE 0 END), \
         SUM(CASE WHEN cache_write_ttl IS NULL OR cache_write_ttl NOT IN ('5m', '1h') \
             THEN cache_write_input_tokens ELSE 0 END)"
    };
}

/// Priced cache-write buckets in a stable order: 5 minutes, 1 hour, then unknown.
pub const CACHE_WRITE_TTL_BUCKET_SUMS_SQL: &str = cache_write_ttl_bucket_sums_sql!();

/// SQL aggregate shared by the desktop log and the user-managed server.
/// Column order is part of the contract: input, cached input, total cache
/// write, then the 5-minute, 1-hour, and unknown write buckets, then output,
/// total, and the sample counts those buckets need.
pub const API_EQUIVALENT_AGGREGATE_SQL: &str = concat!(
    "SUM(input_tokens), SUM(cached_input_tokens), SUM(cache_write_input_tokens), ",
    cache_write_ttl_bucket_sums_sql!(),
    ", SUM(output_tokens), SUM(total_tokens), COUNT(input_tokens), \
     COUNT(cached_input_tokens), COUNT(cache_write_input_tokens), \
     COUNT(output_tokens), COUNT(total_tokens)"
);

/// Usage totals shared by desktop logs and the user-managed server.
/// `speed_guard` stays local: the server uses `MAX(COALESCE(output_tokens, 0), 0)`
/// and desktop logs use `COALESCE(output_tokens, 0)`.
#[macro_export]
macro_rules! usage_total_columns_sql {
    ($speed_guard:literal) => {
        concat!(
            "COUNT(*), \
    COALESCE(SUM(CASE WHEN success != 0 THEN 1 ELSE 0 END), 0), \
    COALESCE(SUM(latency_ms), 0), COALESCE(SUM(ttft_ms), 0), COUNT(ttft_ms), \
    COALESCE(SUM(CASE WHEN success != 0 AND generation_ms > 0 \
        AND MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0) - 1, 0) > 0 \
        AND MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0) - 1, 0) <= generation_ms \
        THEN generation_ms ELSE 0 END), 0), \
    COUNT(CASE WHEN success != 0 AND generation_ms > 0 \
        AND MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0) - 1, 0) > 0 \
        AND MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0) - 1, 0) <= generation_ms \
        THEN generation_ms END), \
    COALESCE(SUM(CASE WHEN success != 0 AND generation_ms > 0 \
        AND MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0) - 1, 0) > 0 \
        AND MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0) - 1, 0) <= generation_ms \
        THEN MAX(COALESCE(output_tokens, 0) - COALESCE(reasoning_tokens, 0) - 1, 0) ELSE 0 END), 0), \
    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_input_tokens), 0), \
    COUNT(cached_input_tokens), COALESCE(SUM(cache_write_input_tokens), 0), \
    COUNT(cache_write_input_tokens), COALESCE(SUM(reasoning_tokens), 0), \
    COALESCE(SUM(output_tokens), 0), \
    COALESCE(SUM(total_tokens), 0)",
            ", COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > 0 AND latency_ms > 0 AND ",
            $speed_guard,
            " THEN MAX(COALESCE(output_tokens, 0), 0) ELSE 0 END), 0), COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > 0 AND latency_ms > 0 AND ",
            $speed_guard,
            " THEN latency_ms ELSE 0 END), 0)"
        )
    };
}

/// Offsets inside [`API_EQUIVALENT_AGGREGATE_SQL`].
/// Offset 2 is the combined cache-write sum. Priced reads skip it and use the
/// 5-minute, 1-hour, and unknown buckets that follow.
pub const PRICED_AGGREGATE_INPUT_TOKENS: usize = 0;
pub const PRICED_AGGREGATE_CACHED_INPUT_TOKENS: usize = 1;
pub const PRICED_AGGREGATE_CACHE_WRITE_5M_TOKENS: usize = 3;
pub const PRICED_AGGREGATE_CACHE_WRITE_1H_TOKENS: usize = 4;
pub const PRICED_AGGREGATE_UNKNOWN_CACHE_WRITE_TOKENS: usize = 5;
pub const PRICED_AGGREGATE_OUTPUT_TOKENS: usize = 6;
pub const PRICED_AGGREGATE_TOTAL_TOKENS: usize = 7;
pub const PRICED_AGGREGATE_INPUT_SAMPLES: usize = 8;
pub const PRICED_AGGREGATE_CACHED_SAMPLES: usize = 9;
pub const PRICED_AGGREGATE_CACHE_WRITE_SAMPLES: usize = 10;
pub const PRICED_AGGREGATE_OUTPUT_SAMPLES: usize = 11;
pub const PRICED_AGGREGATE_TOTAL_SAMPLES: usize = 12;

/// First token column in a candidate rollup row.
/// `candidate_kind`, `candidate_id`, and `model` come first. The columns after
/// them follow [`API_EQUIVALENT_AGGREGATE_SQL`]. Historical rollups do not
/// store the output and total sample counts at the end of that list.
pub const CANDIDATE_ROLLUP_TOKEN_OFFSET: usize = 3;

/// Converted SQL sums plus the sample counts that prove each bucket was observed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ObservedUsageSums {
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_5m_tokens: Option<u64>,
    pub cache_write_1h_tokens: Option<u64>,
    pub unknown_cache_write_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub input_samples: u64,
    pub cached_samples: u64,
    pub cache_write_samples: u64,
    pub output_samples: u64,
    pub total_samples: u64,
    pub gate_measured_buckets: bool,
}

impl ObservedUsageSums {
    /// Read one live [`API_EQUIVALENT_AGGREGATE_SQL`] group.
    /// `token(offset)` may be NULL. `sample(offset)` is a SQL count. NULL and
    /// negative token sums stay unmeasured. A negative count becomes zero.
    /// The combined cache-write column is skipped; priced reads use the
    /// 5-minute, 1-hour, and unknown buckets.
    pub fn from_priced_aggregate<E>(
        token: impl FnMut(usize) -> Result<Option<i64>, E>,
        sample: impl FnMut(usize) -> Result<i64, E>,
    ) -> Result<Self, E> {
        Self::from_aggregate(token, sample, true)
    }

    /// Read a historical candidate rollup.
    /// Those rows stop before the output and total sample counts. Leaving
    /// `gate_measured_buckets` false keeps the summed output and total instead
    /// of treating the missing counts as unmeasured.
    pub fn from_rollup_aggregate<E>(
        token: impl FnMut(usize) -> Result<Option<i64>, E>,
        sample: impl FnMut(usize) -> Result<i64, E>,
    ) -> Result<Self, E> {
        Self::from_aggregate(token, sample, false)
    }

    fn from_aggregate<E>(
        mut token: impl FnMut(usize) -> Result<Option<i64>, E>,
        mut sample: impl FnMut(usize) -> Result<i64, E>,
        gate_measured_buckets: bool,
    ) -> Result<Self, E> {
        let output_samples = if gate_measured_buckets {
            sql_count_u64(sample(PRICED_AGGREGATE_OUTPUT_SAMPLES)?)
        } else {
            0
        };
        let total_samples = if gate_measured_buckets {
            sql_count_u64(sample(PRICED_AGGREGATE_TOTAL_SAMPLES)?)
        } else {
            0
        };
        Ok(Self {
            input_tokens: sql_optional_u64(token(PRICED_AGGREGATE_INPUT_TOKENS)?),
            cached_input_tokens: sql_optional_u64(token(PRICED_AGGREGATE_CACHED_INPUT_TOKENS)?),
            cache_write_5m_tokens: sql_optional_u64(token(PRICED_AGGREGATE_CACHE_WRITE_5M_TOKENS)?),
            cache_write_1h_tokens: sql_optional_u64(token(PRICED_AGGREGATE_CACHE_WRITE_1H_TOKENS)?),
            unknown_cache_write_tokens: sql_optional_u64(token(
                PRICED_AGGREGATE_UNKNOWN_CACHE_WRITE_TOKENS,
            )?),
            output_tokens: sql_optional_u64(token(PRICED_AGGREGATE_OUTPUT_TOKENS)?),
            total_tokens: sql_optional_u64(token(PRICED_AGGREGATE_TOTAL_TOKENS)?),
            input_samples: sql_count_u64(sample(PRICED_AGGREGATE_INPUT_SAMPLES)?),
            cached_samples: sql_count_u64(sample(PRICED_AGGREGATE_CACHED_SAMPLES)?),
            cache_write_samples: sql_count_u64(sample(PRICED_AGGREGATE_CACHE_WRITE_SAMPLES)?),
            output_samples,
            total_samples,
            gate_measured_buckets,
        })
    }
}

impl ApiEquivalentUsage {
    /// Apply the sample gates shared by desktop and server usage reads.
    /// Cached input counts only when every input sample reported it. A cache
    /// write with a missing split becomes zero. Live aggregates set
    /// `gate_measured_buckets` because they counted input, output, and total.
    /// Historical rollups never stored output or total sample counts, so they
    /// leave that flag false and keep those summed values.
    pub fn from_observed_sums(sums: ObservedUsageSums) -> Self {
        let measured = |samples: u64, tokens: Option<u64>| {
            if sums.gate_measured_buckets {
                (samples > 0).then_some(tokens).flatten()
            } else {
                tokens
            }
        };
        let cache_writes = sums.cache_write_samples > 0;
        let write = |tokens: Option<u64>| cache_writes.then(|| tokens.unwrap_or_default());
        Self {
            input_tokens: measured(sums.input_samples, sums.input_tokens),
            cached_input_tokens: (sums.input_samples > 0
                && sums.cached_samples == sums.input_samples)
                .then_some(sums.cached_input_tokens)
                .flatten(),
            cache_write_5m_tokens: write(sums.cache_write_5m_tokens),
            cache_write_1h_tokens: write(sums.cache_write_1h_tokens),
            unknown_cache_write_tokens: write(sums.unknown_cache_write_tokens),
            output_tokens: measured(sums.output_samples, sums.output_tokens),
            total_tokens: measured(sums.total_samples, sums.total_tokens),
        }
    }

    /// Split one reported cache write into the priced windows.
    /// `5m` and `1h` are exact. Any other value, including no value, stays unknown.
    pub fn from_reported_tokens(
        input_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
        cache_write_input_tokens: Option<u64>,
        cache_write_ttl: Option<&str>,
        output_tokens: Option<u64>,
        total_tokens: Option<u64>,
    ) -> Self {
        let (cache_write_5m_tokens, cache_write_1h_tokens, unknown_cache_write_tokens) =
            match cache_write_ttl {
                Some("5m") => (cache_write_input_tokens, Some(0), Some(0)),
                Some("1h") => (Some(0), cache_write_input_tokens, Some(0)),
                _ => (Some(0), Some(0), cache_write_input_tokens),
            };
        Self {
            input_tokens,
            cached_input_tokens,
            cache_write_5m_tokens,
            cache_write_1h_tokens,
            unknown_cache_write_tokens,
            output_tokens,
            total_tokens,
        }
    }
}

#[cfg(test)]
mod pricing_tests;

#[cfg(test)]
mod usage_sql_tests;
