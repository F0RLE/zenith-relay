use crate::{
    ApiEquivalentSummary, DefaultServiceTier, ErrorOrigin, ObservedServiceTier, PriceSource,
    PricingMetadata, RoutingDiagnostics, ToolUseDiagnostics, WireApi,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

define_usage_request_contract! {
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub id: i64,
    pub request_id: String,
    #[serde(default = "default_attempt")]
    pub attempt: u16,
    pub candidate_kind: String,
    pub candidate_hint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_origin: Option<ErrorOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use: Option<ToolUseDiagnostics>,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_ms: Option<u64>,
    #[serde(flatten)]
    pub tokens: UsageTokenBreakdown,
    #[serde(default)]
    pub api_equivalent: ApiEquivalentSummary,
    pub created_at_ms: u64,
}
}

/// Normalized token counters for one completed request. Cache counters remain
/// part of input and reasoning remains part of output, so clients must not add
/// either component to the reported totals a second time.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTokenBreakdown {
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_input_tokens: Option<u64>,
    /// Provider-reported cache-window durations, normalized as a comma-separated list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_ttl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

fn default_attempt() -> u16 {
    1
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub requests: u64,
    pub successful_requests: u64,
    pub latency_ms: u64,
    pub ttft_ms: u64,
    pub ttft_samples: u64,
    pub generation_ms: u64,
    pub generation_samples: u64,
    pub generation_output_tokens: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cached_input_samples: u64,
    #[serde(default)]
    pub cache_write_input_tokens: u64,
    #[serde(default)]
    pub cache_write_input_samples: u64,
    pub reasoning_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub speed_output_tokens: u64,
    pub speed_duration_ms: u64,
    pub api_equivalent: ApiEquivalentSummary,
}

impl UsageTotals {
    /// Decode one shared usage-total aggregate.
    /// The column order matches `usage_total_columns_sql`: requests,
    /// successes, latency, time to first token and its sample count,
    /// generation time, generation samples, generation output, input, cached
    /// input and its samples, cache writes and their samples, reasoning,
    /// output, total, speed output, and speed duration. A negative SQL count
    /// becomes zero.
    pub fn from_sql_counts<E>(mut count: impl FnMut(usize) -> Result<i64, E>) -> Result<Self, E> {
        fn nonnegative(value: i64) -> u64 {
            u64::try_from(value).unwrap_or_default()
        }
        Ok(Self {
            requests: nonnegative(count(0)?),
            successful_requests: nonnegative(count(1)?),
            latency_ms: nonnegative(count(2)?),
            ttft_ms: nonnegative(count(3)?),
            ttft_samples: nonnegative(count(4)?),
            generation_ms: nonnegative(count(5)?),
            generation_samples: nonnegative(count(6)?),
            generation_output_tokens: nonnegative(count(7)?),
            input_tokens: nonnegative(count(8)?),
            cached_input_tokens: nonnegative(count(9)?),
            cached_input_samples: nonnegative(count(10)?),
            cache_write_input_tokens: nonnegative(count(11)?),
            cache_write_input_samples: nonnegative(count(12)?),
            reasoning_tokens: nonnegative(count(13)?),
            output_tokens: nonnegative(count(14)?),
            total_tokens: nonnegative(count(15)?),
            speed_output_tokens: nonnegative(count(16)?),
            speed_duration_ms: nonnegative(count(17)?),
            api_equivalent: ApiEquivalentSummary::default(),
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageGroup {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub totals: UsageTotals,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBucket {
    pub start_ms: u64,
    pub totals: UsageTotals,
}

/// Merges model estimates after each store has decoded its own SQL rows.
pub fn merge_model_equivalents(
    rows: impl IntoIterator<Item = (String, ApiEquivalentSummary, PriceSource)>,
) -> (HashMap<String, ApiEquivalentSummary>, Vec<PriceSource>) {
    let mut equivalents = HashMap::<String, ApiEquivalentSummary>::new();
    let mut sources = Vec::new();
    for (model, estimate, source) in rows {
        equivalents.entry(model).or_default().merge(estimate);
        sources.push(source);
    }
    (equivalents, sources)
}

/// Writes merged estimates onto matching buckets. A bucket without a row keeps the default.
pub fn assign_bucket_equivalents(
    buckets: &mut [UsageBucket],
    rows: impl IntoIterator<Item = (u64, ApiEquivalentSummary)>,
) {
    let mut equivalents = HashMap::<u64, ApiEquivalentSummary>::new();
    for (start_ms, estimate) in rows {
        equivalents.entry(start_ms).or_default().merge(estimate);
    }
    for bucket in buckets {
        bucket.totals.api_equivalent = equivalents.remove(&bucket.start_ms).unwrap_or_default();
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePage {
    pub events: Vec<UsageSummary>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub total_pages: u32,
    #[serde(default)]
    pub totals: UsageTotals,
    #[serde(default)]
    pub models: Vec<UsageGroup>,
    #[serde(default)]
    pub pool_members: Vec<UsageGroup>,
    #[serde(default)]
    pub buckets: Vec<UsageBucket>,
    #[serde(default)]
    pub pricing: PricingMetadata,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageRange {
    Daily,
    Weekly,
    Monthly,
    Custom,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageQuery {
    #[serde(default)]
    pub page: u32,
    #[serde(default)]
    pub page_size: u32,
    pub range: Option<UsageRange>,
    pub from_ms: Option<u64>,
    pub to_ms: Option<u64>,
    pub bucket_ms: Option<u64>,
    pub model_query: Option<String>,
    pub source_or_account_query: Option<String>,
    pub wire_api: Option<WireApi>,
    pub success: Option<bool>,
    pub error_category: Option<String>,
    pub request_id_query: Option<String>,
    /// Include request event rows in the response. `None` preserves the
    /// legacy behavior for API clients that do not send the projection hint.
    #[serde(default)]
    pub include_events: Option<bool>,
    /// Include model aggregates in the response. `None` preserves the legacy
    /// behavior for API clients that do not send the projection hint.
    #[serde(default)]
    pub include_models: Option<bool>,
    /// Include pool-member aggregates in the response. `None` preserves the
    /// legacy behavior for API clients that do not send the projection hint.
    #[serde(default)]
    pub include_pool_members: Option<bool>,
}

impl UsageQuery {
    /// Page bounds shared by local queries, the management API, and remote
    /// usage requests. An absent size uses 50 rows; a caller cannot ask for
    /// more than 200.
    pub fn normalized_page(&self) -> (u32, u32) {
        let page = self.page.max(1);
        let page_size = if self.page_size == 0 {
            50
        } else {
            self.page_size.clamp(1, 200)
        };
        (page, page_size)
    }

    pub fn page_count(total: u64, page_size: u32) -> u32 {
        if total == 0 {
            0
        } else {
            total.div_ceil(u64::from(page_size)) as u32
        }
    }

    pub fn normalize_pagination(&mut self) {
        let (page, page_size) = self.normalized_page();
        self.page = page;
        self.page_size = page_size;
        self.bucket_ms = self.bucket_ms.filter(|value| *value >= 60_000);
    }

    pub fn includes_models(&self) -> bool {
        self.include_models != Some(false)
    }

    pub fn includes_events(&self) -> bool {
        self.include_events != Some(false)
    }

    pub fn includes_pool_members(&self) -> bool {
        self.include_pool_members != Some(false)
    }
}

#[cfg(test)]
mod tests {
    use super::{assign_bucket_equivalents, merge_model_equivalents, UsageBucket, UsageTotals};
    use crate::{ApiEquivalentSummary, PriceSource};

    #[test]
    fn sql_counts_follow_the_shared_usage_total_column_order() {
        let totals =
            UsageTotals::from_sql_counts(|index| Ok::<i64, ()>(i64::try_from(index).unwrap() + 1))
                .unwrap();
        assert_eq!(totals.requests, 1);
        assert_eq!(totals.successful_requests, 2);
        assert_eq!(totals.latency_ms, 3);
        assert_eq!(totals.ttft_ms, 4);
        assert_eq!(totals.ttft_samples, 5);
        assert_eq!(totals.generation_ms, 6);
        assert_eq!(totals.generation_samples, 7);
        assert_eq!(totals.generation_output_tokens, 8);
        assert_eq!(totals.input_tokens, 9);
        assert_eq!(totals.cached_input_tokens, 10);
        assert_eq!(totals.cached_input_samples, 11);
        assert_eq!(totals.cache_write_input_tokens, 12);
        assert_eq!(totals.cache_write_input_samples, 13);
        assert_eq!(totals.reasoning_tokens, 14);
        assert_eq!(totals.output_tokens, 15);
        assert_eq!(totals.total_tokens, 16);
        assert_eq!(totals.speed_output_tokens, 17);
        assert_eq!(totals.speed_duration_ms, 18);
        assert_eq!(
            UsageTotals::from_sql_counts(|index| Ok::<i64, ()>(if index == 3 { -1 } else { 0 }))
                .unwrap()
                .ttft_ms,
            0
        );
    }

    #[test]
    fn equivalent_rows_merge_by_model_and_bucket() {
        let (models, sources) = merge_model_equivalents([
            ("gpt".to_string(), summary(2, 3), PriceSource::Provider),
            ("gpt".to_string(), summary(4, 0), PriceSource::Manual),
            ("opus".to_string(), summary(1, 1), PriceSource::Unpriced),
        ]);
        assert_eq!(models["gpt"], summary(6, 3));
        assert_eq!(models["opus"], summary(1, 1));
        assert_eq!(
            sources,
            vec![
                PriceSource::Provider,
                PriceSource::Manual,
                PriceSource::Unpriced
            ]
        );

        let mut buckets = vec![
            UsageBucket {
                start_ms: 10,
                totals: UsageTotals::default(),
            },
            UsageBucket {
                start_ms: 20,
                totals: UsageTotals::default(),
            },
        ];
        assign_bucket_equivalents(&mut buckets, [(10, summary(5, 1)), (10, summary(1, 2))]);
        assert_eq!(buckets[0].totals.api_equivalent, summary(6, 3));
        assert_eq!(
            buckets[1].totals.api_equivalent,
            ApiEquivalentSummary::default()
        );
    }

    fn summary(micro_usd: u64, priced_tokens: u64) -> ApiEquivalentSummary {
        ApiEquivalentSummary {
            micro_usd,
            priced_tokens,
            unpriced_tokens: 0,
        }
    }
}
