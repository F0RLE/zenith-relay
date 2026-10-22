use super::ApiEquivalentSummary;
use super::{sql_count_u64, sql_optional_u64};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod estimate;

pub use estimate::{
    estimate_api_equivalent_breakdown_with_token_price, estimate_api_equivalent_with_catalog,
    estimate_api_equivalent_with_token_price, estimate_candidate_api_equivalent_with_catalog,
    normalize_model_price_overrides, resolve_candidate_price, CandidatePriceQuery,
    CatalogPriceResolver,
};

/// Itemized parts of an API-price estimate, never separate billed charges.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiEquivalentBreakdown {
    pub input: Option<ApiEquivalentSummary>,
    pub uncached_input: Option<ApiEquivalentSummary>,
    pub cache_read: Option<ApiEquivalentSummary>,
    pub cache_write: Option<ApiEquivalentSummary>,
    pub output: Option<ApiEquivalentSummary>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
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
    #[serde(
        default,
        skip_serializing_if = "crate::pricing::TokenRateSet::is_empty_ref"
    )]
    pub flex: crate::pricing::TokenRateSet,
    #[serde(
        default,
        skip_serializing_if = "crate::pricing::TokenRateSet::is_empty_ref"
    )]
    pub priority: crate::pricing::TokenRateSet,
    #[serde(
        default,
        skip_serializing_if = "crate::pricing::TokenRateSet::is_empty_ref"
    )]
    pub ultrafast: crate::pricing::TokenRateSet,
    #[serde(
        default,
        skip_serializing_if = "crate::pricing::LongContextRates::is_empty_ref"
    )]
    pub above_100k: crate::pricing::LongContextRates,
    #[serde(
        default,
        skip_serializing_if = "crate::pricing::LongContextRates::is_empty_ref"
    )]
    pub above_128k: crate::pricing::LongContextRates,
    #[serde(
        default,
        skip_serializing_if = "crate::pricing::LongContextRates::is_empty_ref"
    )]
    pub above_200k: crate::pricing::LongContextRates,
    #[serde(
        default,
        skip_serializing_if = "crate::pricing::LongContextRates::is_empty_ref"
    )]
    pub above_272k: crate::pricing::LongContextRates,
    #[serde(
        default,
        skip_serializing_if = "crate::pricing::LongContextRates::is_empty_ref"
    )]
    pub above_512k: crate::pricing::LongContextRates,
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
/// Storage-neutral price evidence used by Desktop telemetry.
pub type SourceModelPriceOverrides = BTreeMap<String, BTreeMap<String, ApiModelPriceSources>>;

/// Which published token schedule applies to a usage group.
///
/// `fast` and `priority` share one schedule. Ultrafast has its own rates.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UsagePriceClass {
    #[default]
    Standard,
    Flex,
    Priority,
    Ultrafast,
}

impl UsagePriceClass {
    pub fn from_observed(observed_tier: Option<&str>) -> Self {
        match observed_tier
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "flex" => Self::Flex,
            "priority" | "fast" => Self::Priority,
            "ultrafast" => Self::Ultrafast,
            _ => Self::Standard,
        }
    }

    pub fn from_label(price_class_label: &str) -> Self {
        match price_class_label {
            "flex" => Self::Flex,
            "priority" => Self::Priority,
            "ultrafast" => Self::Ultrafast,
            _ => Self::Standard,
        }
    }

    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Flex => "flex",
            Self::Priority => "priority",
            Self::Ultrafast => "ultrafast",
        }
    }
}

/// Prompt-size band for a single request, or for an aggregate that was grouped
/// by that band. The summed token count of a mixed group is not a band.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UsageContextBand {
    #[default]
    Base,
    Above100k,
    Above128k,
    Above200k,
    Above272k,
    Above512k,
}

impl UsageContextBand {
    pub const fn threshold(self) -> u64 {
        match self {
            Self::Base => 0,
            Self::Above100k => 100_000,
            Self::Above128k => 128_000,
            Self::Above200k => 200_000,
            Self::Above272k => 272_000,
            Self::Above512k => 512_000,
        }
    }

    pub fn from_input_tokens(input_tokens: Option<u64>) -> Self {
        [
            Self::Above512k,
            Self::Above272k,
            Self::Above200k,
            Self::Above128k,
            Self::Above100k,
        ]
        .into_iter()
        .find(|band| input_tokens.unwrap_or_default() > band.threshold())
        .unwrap_or_default()
    }

    pub fn from_label(context_band_label: &str) -> Self {
        match context_band_label {
            "above_100k" => Self::Above100k,
            "above_128k" => Self::Above128k,
            "above_200k" => Self::Above200k,
            "above_272k" => Self::Above272k,
            "above_512k" => Self::Above512k,
            _ => Self::Base,
        }
    }

    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Above100k => "above_100k",
            Self::Above128k => "above_128k",
            Self::Above200k => "above_200k",
            Self::Above272k => "above_272k",
            Self::Above512k => "above_512k",
        }
    }
}

/// SQL expression for [`UsagePriceClass`], shared by desktop and server aggregates.
#[macro_export]
macro_rules! usage_price_class_sql {
    () => {
        "CASE lower(trim(COALESCE(applied_service_tier, ''))) \
         WHEN 'flex' THEN 'flex' \
         WHEN 'priority' THEN 'priority' \
         WHEN 'fast' THEN 'priority' \
         WHEN 'ultrafast' THEN 'ultrafast' \
         ELSE 'standard' END"
    };
}

pub const USAGE_PRICE_CLASS_SQL: &str = usage_price_class_sql!();

/// Allows host archive statements to embed the same expression with `concat!`.
#[macro_export]
macro_rules! usage_context_band_sql {
    () => {
        "CASE \
         WHEN COALESCE(input_tokens, 0) > 512000 THEN 'above_512k' \
         WHEN COALESCE(input_tokens, 0) > 272000 THEN 'above_272k' \
         WHEN COALESCE(input_tokens, 0) > 200000 THEN 'above_200k' \
         WHEN COALESCE(input_tokens, 0) > 128000 THEN 'above_128k' \
         WHEN COALESCE(input_tokens, 0) > 100000 THEN 'above_100k' \
         ELSE 'base' END"
    };
}

/// SQL expression for [`UsageContextBand`]. NULL input stays in the base band.
pub const USAGE_CONTEXT_BAND_SQL: &str = usage_context_band_sql!();

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
    pub price_class: UsagePriceClass,
    pub context_band: UsageContextBand,
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

/// SQL aggregate used by Desktop telemetry.
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

/// Usage totals for Desktop telemetry.
#[macro_export]
macro_rules! usage_total_columns_sql {
    () => {
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
            "COALESCE(output_tokens, 0) <= latency_ms",
            " THEN MAX(COALESCE(output_tokens, 0), 0) ELSE 0 END), 0), COALESCE(SUM(CASE WHEN success != 0 AND COALESCE(output_tokens, 0) > 0 AND latency_ms > 0 AND ",
            "COALESCE(output_tokens, 0) <= latency_ms",
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
/// `candidate_kind`, `candidate_id`, `model`, `price_class`, and
/// `context_band` come first. The columns after them follow
/// [`API_EQUIVALENT_AGGREGATE_SQL`]. Historical rollups do not store the
/// output and total sample counts at the end of that list.
pub const CANDIDATE_ROLLUP_TOKEN_OFFSET: usize = 5;

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
        let measured_tokens = |samples: u64, tokens: Option<u64>| {
            if sums.gate_measured_buckets {
                (samples > 0).then_some(tokens).flatten()
            } else {
                tokens
            }
        };
        let cache_writes = sums.cache_write_samples > 0;
        let cache_write_tokens =
            |tokens: Option<u64>| cache_writes.then(|| tokens.unwrap_or_default());
        Self {
            input_tokens: measured_tokens(sums.input_samples, sums.input_tokens),
            cached_input_tokens: (sums.input_samples > 0
                && sums.cached_samples == sums.input_samples)
                .then_some(sums.cached_input_tokens)
                .flatten(),
            cache_write_5m_tokens: cache_write_tokens(sums.cache_write_5m_tokens),
            cache_write_1h_tokens: cache_write_tokens(sums.cache_write_1h_tokens),
            unknown_cache_write_tokens: cache_write_tokens(sums.unknown_cache_write_tokens),
            output_tokens: measured_tokens(sums.output_samples, sums.output_tokens),
            total_tokens: measured_tokens(sums.total_samples, sums.total_tokens),
            price_class: UsagePriceClass::Standard,
            context_band: UsageContextBand::Base,
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
        let (short_cache_write_tokens, long_cache_write_tokens, unknown_cache_write_tokens) =
            match cache_write_ttl {
                Some("5m") => (cache_write_input_tokens, Some(0), Some(0)),
                Some("1h") => (Some(0), cache_write_input_tokens, Some(0)),
                _ => (Some(0), Some(0), cache_write_input_tokens),
            };
        Self {
            input_tokens,
            cached_input_tokens,
            cache_write_5m_tokens: short_cache_write_tokens,
            cache_write_1h_tokens: long_cache_write_tokens,
            unknown_cache_write_tokens,
            output_tokens,
            total_tokens,
            price_class: UsagePriceClass::Standard,
            context_band: UsageContextBand::Base,
        }
    }

    /// Selects the schedule for one observed request.
    /// The band comes from that request's input count, never from a later sum.
    pub fn with_observed_rates(
        mut self,
        applied_service_tier: Option<&str>,
        input_tokens: Option<u64>,
    ) -> Self {
        self.price_class = UsagePriceClass::from_observed(applied_service_tier);
        self.context_band = UsageContextBand::from_input_tokens(input_tokens);
        self
    }

    /// Selects the schedule for one aggregate group that was already split.
    pub fn with_aggregate_rates(mut self, price_class: &str, context_band: &str) -> Self {
        self.price_class = UsagePriceClass::from_label(price_class);
        self.context_band = UsageContextBand::from_label(context_band);
        self
    }
}

#[cfg(test)]
mod pricing_tests;

#[cfg(test)]
mod usage_sql_tests;
