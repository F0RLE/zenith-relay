use super::*;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaEconomicsEstimateState {
    #[default]
    Collecting,
    Estimated,
    Stale,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaEconomicsConfidence {
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaEconomicsSummary {
    pub purchase_cost_micro_usd: Option<u64>,
    pub potential_micro_usd: Option<u64>,
    pub potential_low_micro_usd: Option<u64>,
    pub potential_high_micro_usd: Option<u64>,
    pub potential_requests: Option<u64>,
    pub potential_total_tokens: Option<u64>,
    pub available_now_micro_usd: Option<u64>,
    pub estimate_state: QuotaEconomicsEstimateState,
    pub confidence: Option<QuotaEconomicsConfidence>,
    pub observed_basis_points: u64,
    pub sample_count: usize,
    #[serde(default)]
    pub windows: Vec<QuotaEconomicsWindowSummary>,
    #[serde(default)]
    pub cycles: Vec<QuotaCycleRecord>,
    #[serde(default)]
    pub observations: Vec<QuotaObservationRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaEconomicsWindowSummary {
    pub kind: QuotaWindowKind,
    pub potential_micro_usd: Option<u64>,
    pub potential_low_micro_usd: Option<u64>,
    pub potential_high_micro_usd: Option<u64>,
    pub potential_requests: Option<u64>,
    pub potential_total_tokens: Option<u64>,
    pub full_window_micro_usd: Option<u64>,
    pub full_window_low_micro_usd: Option<u64>,
    pub full_window_high_micro_usd: Option<u64>,
    pub full_window_requests: Option<u64>,
    pub full_window_total_tokens: Option<u64>,
    pub estimate_state: QuotaEconomicsEstimateState,
    pub confidence: Option<QuotaEconomicsConfidence>,
    pub observed_basis_points: u64,
    pub sample_count: usize,
    #[serde(default)]
    pub service_tiers: Vec<QuotaEconomicsTierSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_benchmark: Option<QuotaPlanBenchmark>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaEconomicsTierSummary {
    pub service_tier: DefaultServiceTier,
    pub potential_micro_usd: Option<u64>,
    pub potential_requests: Option<u64>,
    pub potential_total_tokens: Option<u64>,
    pub observed_basis_points: u64,
    pub sample_count: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaPlanBenchmark {
    pub provider: String,
    pub plan: String,
    pub window_kind: QuotaWindowKind,
    pub window_minutes: u32,
    pub service_tier: DefaultServiceTier,
    pub pricing_revision: String,
    pub account_count: usize,
    pub cycle_count: usize,
    pub latest_completed_at_ms: u64,
    pub stale: bool,
    pub confidence: QuotaEconomicsConfidence,
    pub full_window_micro_usd: u64,
    pub mean_full_window_micro_usd: u64,
    pub low_full_window_micro_usd: u64,
    pub high_full_window_micro_usd: u64,
    pub potential_micro_usd: Option<u64>,
    pub weekly_equivalent_micro_usd: Option<u64>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct BenchmarkKey {
    provider: String,
    plan: String,
    window_kind: QuotaWindowKind,
    window_minutes: u32,
    service_tier: String,
    pricing_revision: String,
}

const BENCHMARK_MAX_AGE_MS: u64 = 90 * 24 * 60 * 60 * 1_000;
const BENCHMARK_STALE_AFTER_MS: u64 = 30 * 24 * 60 * 60 * 1_000;

pub fn quota_plan_benchmarks<'a>(
    accounts: impl IntoIterator<Item = (&'a str, &'a QuotaEconomicsState)>,
    now_ms: u64,
    pricing_revision: &str,
) -> Vec<QuotaPlanBenchmark> {
    let pricing_revision = pricing_revision.trim();
    let mut grouped = BTreeMap::<BenchmarkKey, BTreeMap<String, Vec<&QuotaCycleRecord>>>::new();
    for (account_id, state) in accounts {
        for history in [&state.primary, &state.secondary] {
            for cycle in &history.cycles {
                let Some(service_tier) = cycle.service_tier else {
                    continue;
                };
                let Some(plan) = cycle.plan.as_ref().filter(|value| !value.is_empty()) else {
                    continue;
                };
                let Some(window_minutes) = cycle.window_minutes.filter(|value| *value > 0) else {
                    continue;
                };
                if cycle.status != QuotaCycleStatus::Complete
                    || cycle.epoch != history.epoch
                    || cycle.unattributed_basis_points > 0
                    || cycle.consumed_basis_points < 9_900
                    || cycle.api_equivalent_micro_usd.is_none()
                    || cycle.completed_at_ms.saturating_add(BENCHMARK_MAX_AGE_MS) < now_ms
                    || cycle.pricing_revision.as_deref() != Some(pricing_revision)
                {
                    continue;
                }
                let key = BenchmarkKey {
                    provider: cycle.provider.clone(),
                    plan: plan.clone(),
                    window_kind: cycle.window_kind,
                    window_minutes,
                    service_tier: service_tier_name(service_tier).to_string(),
                    pricing_revision: pricing_revision.to_string(),
                };
                grouped
                    .entry(key)
                    .or_default()
                    .entry(account_id.to_string())
                    .or_default()
                    .push(cycle);
            }
        }
    }
    grouped
        .into_iter()
        .filter_map(|(key, mut account_cycles)| {
            let mut account_api_values = Vec::new();
            let mut cycle_count = 0_usize;
            let mut latest_completed_at_ms = 0_u64;
            for cycles in account_cycles.values_mut() {
                cycles.sort_unstable_by_key(|cycle| std::cmp::Reverse(cycle.completed_at_ms));
                cycles.truncate(3);
                cycle_count = cycle_count.saturating_add(cycles.len());
                latest_completed_at_ms = latest_completed_at_ms.max(
                    cycles
                        .iter()
                        .map(|cycle| cycle.completed_at_ms)
                        .max()
                        .unwrap_or_default(),
                );
                let mut api_values = cycles
                    .iter()
                    .filter_map(|cycle| cycle.api_equivalent_micro_usd)
                    .collect::<Vec<_>>();
                api_values.sort_unstable();
                if !api_values.is_empty() {
                    account_api_values.push(median(&api_values));
                }
            }
            if account_api_values.len() < 2 {
                return None;
            }
            account_api_values.sort_unstable();
            let full_window_micro_usd = median(&account_api_values);
            let low_full_window_micro_usd = unweighted_quantile(&account_api_values, 1, 4)?;
            let high_full_window_micro_usd = unweighted_quantile(&account_api_values, 3, 4)?;
            let dispersion = if full_window_micro_usd == 0 {
                u64::MAX
            } else {
                u64::try_from(
                    u128::from(high_full_window_micro_usd - low_full_window_micro_usd)
                        .saturating_mul(10_000)
                        / u128::from(full_window_micro_usd),
                )
                .unwrap_or(u64::MAX)
            };
            let confidence =
                if account_api_values.len() >= 10 && cycle_count >= 20 && dispersion <= 1_500 {
                    QuotaEconomicsConfidence::High
                } else if account_api_values.len() >= 5 && cycle_count >= 10 && dispersion <= 2_500
                {
                    QuotaEconomicsConfidence::Medium
                } else {
                    QuotaEconomicsConfidence::Low
                };
            let mean_full_window_micro_usd = u64::try_from(
                account_api_values
                    .iter()
                    .map(|value| u128::from(*value))
                    .sum::<u128>()
                    / account_api_values.len() as u128,
            )
            .unwrap_or(u64::MAX);
            Some(QuotaPlanBenchmark {
                provider: key.provider,
                plan: key.plan,
                window_kind: key.window_kind,
                window_minutes: key.window_minutes,
                service_tier: parse_service_tier(&key.service_tier),
                pricing_revision: key.pricing_revision,
                account_count: account_api_values.len(),
                cycle_count,
                latest_completed_at_ms,
                stale: latest_completed_at_ms.saturating_add(BENCHMARK_STALE_AFTER_MS) < now_ms,
                confidence,
                full_window_micro_usd,
                mean_full_window_micro_usd,
                low_full_window_micro_usd,
                high_full_window_micro_usd,
                potential_micro_usd: None,
                weekly_equivalent_micro_usd: (key.window_minutes >= 1_440
                    && key.window_minutes != 10_080)
                    .then(|| scale_duration(full_window_micro_usd, 10_080, key.window_minutes)),
            })
        })
        .collect()
}

pub fn attach_quota_plan_benchmarks(
    summary: &mut QuotaEconomicsSummary,
    provider: &str,
    plan: Option<&str>,
    quota: &QuotaSnapshot,
    service_tier: DefaultServiceTier,
    pricing_revision: &str,
    benchmarks: &[QuotaPlanBenchmark],
) {
    let provider = provider.trim().to_ascii_lowercase();
    let plan = plan.map(super::super::windows::normalize_subscription_plan);
    for window_summary in &mut summary.windows {
        let Some(window) = quota.window(window_summary.kind) else {
            continue;
        };
        let Some(window_minutes) = window.window_minutes else {
            continue;
        };
        let Some(available) = window.available_basis_points else {
            continue;
        };
        let Some(mut benchmark) = benchmarks
            .iter()
            .find(|benchmark| {
                benchmark.provider == provider
                    && Some(benchmark.plan.as_str()) == plan.as_deref()
                    && benchmark.window_kind == window_summary.kind
                    && benchmark.window_minutes == window_minutes
                    && benchmark.service_tier == service_tier
                    && benchmark.pricing_revision == pricing_revision.trim()
            })
            .cloned()
        else {
            continue;
        };
        benchmark.potential_micro_usd =
            Some(scale(benchmark.full_window_micro_usd, available, 10_000));
        window_summary.plan_benchmark = Some(benchmark);
    }
}

pub fn quota_economics_summary(
    state: &QuotaEconomicsState,
    quota: &QuotaSnapshot,
    active_service_tier: DefaultServiceTier,
    now_ms: u64,
    stale_after_ms: u64,
) -> QuotaEconomicsSummary {
    quota_economics_summary_internal(
        state,
        quota,
        active_service_tier,
        now_ms,
        stale_after_ms,
        true,
    )
}

/// Builds a summary only when the persisted calibration belongs to the
/// caller's current valuation formula. This keeps provider pricing policy out
/// of the generic quota module.
pub fn quota_economics_summary_for_revision(
    state: &QuotaEconomicsState,
    quota: &QuotaSnapshot,
    active_service_tier: DefaultServiceTier,
    now_ms: u64,
    stale_after_ms: u64,
    revision: &str,
) -> QuotaEconomicsSummary {
    quota_economics_summary_internal(
        state,
        quota,
        active_service_tier,
        now_ms,
        stale_after_ms,
        state.pricing_revision.as_deref() == Some(revision.trim()),
    )
}

fn quota_economics_summary_internal(
    state: &QuotaEconomicsState,
    quota: &QuotaSnapshot,
    _active_service_tier: DefaultServiceTier,
    now_ms: u64,
    stale_after_ms: u64,
    use_history: bool,
) -> QuotaEconomicsSummary {
    let empty_primary = WindowEconomicsHistory::default();
    let empty_secondary = WindowEconomicsHistory::default();
    let primary_history = if use_history {
        &state.primary
    } else {
        &empty_primary
    };
    let secondary_history = if use_history {
        &state.secondary
    } else {
        &empty_secondary
    };
    let primary = window_summary(
        QuotaWindowKind::Primary,
        primary_history,
        quota.primary.as_ref(),
        now_ms,
        stale_after_ms,
    );
    let secondary = window_summary(
        QuotaWindowKind::Secondary,
        secondary_history,
        quota.secondary.as_ref(),
        now_ms,
        stale_after_ms,
    );
    let stock = stock_estimate(
        &primary,
        &secondary,
        quota.primary.as_ref(),
        quota.secondary.as_ref(),
    );
    let available_now = limiting_estimate(&primary, &secondary);
    let selected = stock
        .source
        .unwrap_or_else(|| select_summary(&primary, &secondary));
    QuotaEconomicsSummary {
        purchase_cost_micro_usd: state.purchase_cost_micro_usd,
        potential_micro_usd: stock.potential.median,
        potential_low_micro_usd: stock.potential.low,
        potential_high_micro_usd: stock.potential.high,
        potential_requests: stock.requests,
        potential_total_tokens: stock.total_tokens,
        available_now_micro_usd: available_now.potential.median,
        estimate_state: selected.estimate_state,
        confidence: selected.confidence,
        observed_basis_points: selected.observed_basis_points,
        sample_count: selected.sample_count,
        windows: vec![primary, secondary],
        cycles: recent_cycles(state),
        observations: recent_observations(state),
    }
}

fn recent_observations(state: &QuotaEconomicsState) -> Vec<QuotaObservationRecord> {
    let mut observations = state
        .primary
        .observations
        .iter()
        .chain(&state.secondary.observations)
        .cloned()
        .collect::<Vec<_>>();
    observations.sort_unstable_by_key(|observation| std::cmp::Reverse(observation.observed_at_ms));
    observations.truncate(MAX_QUOTA_OBSERVATIONS);
    observations
}

fn recent_cycles(state: &QuotaEconomicsState) -> Vec<QuotaCycleRecord> {
    let mut cycles = state
        .primary
        .cycles
        .iter()
        .chain(&state.secondary.cycles)
        .cloned()
        .collect::<Vec<_>>();
    cycles.sort_unstable_by_key(|cycle| std::cmp::Reverse(cycle.completed_at_ms));
    cycles.truncate(MAX_CYCLE_RECORDS);
    cycles
}

fn window_summary(
    kind: QuotaWindowKind,
    history: &WindowEconomicsHistory,
    window: Option<&QuotaWindow>,
    now_ms: u64,
    stale_after_ms: u64,
) -> QuotaEconomicsWindowSummary {
    let samples = history
        .samples
        .iter()
        .copied()
        .filter(|sample| primary_calibration_rate(*sample).is_some())
        .collect::<Vec<_>>();
    let observed_basis_points = samples
        .iter()
        .map(|sample| u64::from(sample.consumed_basis_points))
        .sum();
    let sample_count = samples.len();
    if observed_basis_points < MIN_ESTIMATE_BASIS_POINTS {
        let mut summary = empty_window_summary(
            kind,
            QuotaEconomicsEstimateState::Collecting,
            observed_basis_points,
            sample_count,
        );
        if let Some(available) =
            window.and_then(|window| fresh_available(window, now_ms, stale_after_ms))
        {
            summary.service_tiers = service_tier_summaries(history, available);
        }
        return summary;
    }
    let Some(available) = window.and_then(|window| fresh_available(window, now_ms, stale_after_ms))
    else {
        return empty_window_summary(
            kind,
            QuotaEconomicsEstimateState::Stale,
            observed_basis_points,
            sample_count,
        );
    };
    // Every published figure is the same estimator applied to a directly
    // measured quantity. Requests and tokens are not projected from the money
    // figure, and the money figure is not projected from anything: a single
    // conversion step here would make the band describe the dispersion of one
    // metric while the median describes another.
    let potential = metric_estimate(&samples, available, |usage| usage.api_equivalent_micro_usd);
    let requests = metric_estimate(&samples, available, |usage| Some(usage.requests));
    let tokens = metric_estimate(&samples, available, |usage| Some(usage.total_tokens));
    let full = metric_estimate(&samples, 10_000, |usage| usage.api_equivalent_micro_usd);
    let full_requests = metric_estimate(&samples, 10_000, |usage| Some(usage.requests));
    let full_tokens = metric_estimate(&samples, 10_000, |usage| Some(usage.total_tokens));
    let has_complete_cycle = history.cycles.iter().any(|cycle| {
        cycle.epoch == history.epoch
            && cycle.status == QuotaCycleStatus::Complete
            && cycle.unattributed_basis_points == 0
            && cycle.consumed_basis_points >= 9_900
            && cycle.api_equivalent_micro_usd.is_some()
    });
    QuotaEconomicsWindowSummary {
        kind,
        potential_micro_usd: potential.median,
        potential_low_micro_usd: potential.low,
        potential_high_micro_usd: potential.high,
        potential_requests: requests.median,
        potential_total_tokens: tokens.median,
        full_window_micro_usd: full.median,
        full_window_low_micro_usd: full.low,
        full_window_high_micro_usd: full.high,
        full_window_requests: full_requests.median,
        full_window_total_tokens: full_tokens.median,
        estimate_state: QuotaEconomicsEstimateState::Estimated,
        confidence: Some(confidence(
            observed_basis_points,
            &samples,
            has_complete_cycle,
        )),
        observed_basis_points,
        sample_count,
        service_tiers: service_tier_summaries(history, available),
        plan_benchmark: None,
    }
}

fn service_tier_summaries(
    history: &WindowEconomicsHistory,
    available: u16,
) -> Vec<QuotaEconomicsTierSummary> {
    [DefaultServiceTier::Standard, DefaultServiceTier::Fast]
        .into_iter()
        .filter_map(|tier| tier_summary(history, tier, available))
        .collect()
}

fn tier_summary(
    history: &WindowEconomicsHistory,
    tier: DefaultServiceTier,
    available: u16,
) -> Option<QuotaEconomicsTierSummary> {
    // The presence filter has to be repeated here: `observed_basis_points`
    // below decides whether the tier is published at all, so counting a sample
    // this estimator cannot value would advertise a tier with an empty figure.
    let samples = history
        .samples
        .iter()
        .copied()
        .filter(|sample| {
            sample.service_tier == Some(tier) && primary_calibration_rate(*sample).is_some()
        })
        .collect::<Vec<_>>();
    let observed_basis_points = samples
        .iter()
        .map(|sample| u64::from(sample.consumed_basis_points))
        .sum::<u64>();
    (observed_basis_points >= MIN_ESTIMATE_BASIS_POINTS).then(|| QuotaEconomicsTierSummary {
        service_tier: tier,
        potential_micro_usd: metric_estimate(&samples, available, |usage| {
            usage.api_equivalent_micro_usd
        })
        .median,
        potential_requests: metric_estimate(&samples, available, |usage| Some(usage.requests))
            .median,
        potential_total_tokens: metric_estimate(&samples, available, |usage| {
            Some(usage.total_tokens)
        })
        .median,
        observed_basis_points,
        sample_count: samples.len(),
    })
}

#[derive(Clone, Copy, Default)]
struct MetricEstimate {
    median: Option<u64>,
    low: Option<u64>,
    high: Option<u64>,
}

fn metric_estimate(
    samples: &[IntervalSample],
    available_basis_points: u16,
    value: impl Fn(CapacityUsage) -> Option<u64>,
) -> MetricEstimate {
    let estimates = samples
        .iter()
        .filter_map(|sample| {
            let measured = value(sample.usage)?;
            (measured > 0).then_some((
                scale(
                    measured,
                    available_basis_points,
                    sample.consumed_basis_points,
                ),
                u64::from(sample.consumed_basis_points),
            ))
        })
        .collect::<Vec<_>>();
    if estimates.is_empty() {
        return MetricEstimate::default();
    }
    MetricEstimate {
        median: weighted_quantile(estimates.clone(), 1, 2),
        low: weighted_quantile(estimates.clone(), 1, 4),
        high: weighted_quantile(estimates, 3, 4),
    }
}

fn select_summary<'a>(
    primary: &'a QuotaEconomicsWindowSummary,
    secondary: &'a QuotaEconomicsWindowSummary,
) -> &'a QuotaEconomicsWindowSummary {
    if secondary.observed_basis_points > primary.observed_basis_points
        || (secondary.observed_basis_points == primary.observed_basis_points
            && secondary.sample_count >= primary.sample_count)
    {
        secondary
    } else {
        primary
    }
}

#[derive(Default)]
struct CombinedEstimate<'a> {
    potential: MetricEstimate,
    requests: Option<u64>,
    total_tokens: Option<u64>,
    source: Option<&'a QuotaEconomicsWindowSummary>,
}

fn stock_estimate<'a>(
    primary: &'a QuotaEconomicsWindowSummary,
    secondary: &'a QuotaEconomicsWindowSummary,
    primary_window: Option<&QuotaWindow>,
    secondary_window: Option<&QuotaWindow>,
) -> CombinedEstimate<'a> {
    [(primary, primary_window), (secondary, secondary_window)]
        .into_iter()
        .filter(|(summary, _)| window_has_estimate(summary))
        .max_by_key(|(summary, window)| {
            (
                window
                    .and_then(|window| window.window_minutes)
                    .unwrap_or_default(),
                summary.observed_basis_points,
                summary.sample_count,
            )
        })
        .map_or_else(CombinedEstimate::default, |(summary, _)| {
            estimate_from_window(summary)
        })
}

fn limiting_estimate<'a>(
    primary: &'a QuotaEconomicsWindowSummary,
    secondary: &'a QuotaEconomicsWindowSummary,
) -> CombinedEstimate<'a> {
    match primary_is_limiting(primary, secondary) {
        Some(true) => estimate_from_window(primary),
        Some(false) => estimate_from_window(secondary),
        None => CombinedEstimate::default(),
    }
}

fn primary_is_limiting(
    primary: &QuotaEconomicsWindowSummary,
    secondary: &QuotaEconomicsWindowSummary,
) -> Option<bool> {
    [
        (primary.potential_micro_usd, secondary.potential_micro_usd),
        (
            primary.potential_total_tokens,
            secondary.potential_total_tokens,
        ),
        (primary.potential_requests, secondary.potential_requests),
    ]
    .into_iter()
    .find_map(|(primary, secondary)| {
        primary
            .zip(secondary)
            .map(|(primary, secondary)| primary <= secondary)
    })
    .or_else(|| {
        let primary_has_estimate = window_has_estimate(primary);
        let secondary_has_estimate = window_has_estimate(secondary);
        match (primary_has_estimate, secondary_has_estimate) {
            (true, false) => Some(true),
            (false, true) => Some(false),
            (true, true) => Some(select_summary(primary, secondary).kind == primary.kind),
            (false, false) => None,
        }
    })
}

fn window_has_estimate(window: &QuotaEconomicsWindowSummary) -> bool {
    window.potential_micro_usd.is_some()
        || window.potential_total_tokens.is_some()
        || window.potential_requests.is_some()
}

fn estimate_from_window(window: &QuotaEconomicsWindowSummary) -> CombinedEstimate<'_> {
    CombinedEstimate {
        potential: MetricEstimate {
            median: window.potential_micro_usd,
            low: window.potential_low_micro_usd,
            high: window.potential_high_micro_usd,
        },
        requests: window.potential_requests,
        total_tokens: window.potential_total_tokens,
        source: Some(window),
    }
}

fn confidence(
    observed_basis_points: u64,
    samples: &[IntervalSample],
    has_complete_cycle: bool,
) -> QuotaEconomicsConfidence {
    let sample_count = samples.len();
    let dispersion_bps = rate_dispersion_bps(samples);
    if has_complete_cycle {
        QuotaEconomicsConfidence::High
    } else if observed_basis_points >= MEDIUM_CONFIDENCE_OBSERVED_BASIS_POINTS
        && sample_count >= 3
        && dispersion_bps.is_some_and(|value| value <= MEDIUM_CONFIDENCE_IQR_BPS)
    {
        QuotaEconomicsConfidence::Medium
    } else {
        QuotaEconomicsConfidence::Low
    }
}

fn rate_dispersion_bps(samples: &[IntervalSample]) -> Option<u64> {
    let rates = samples
        .iter()
        .filter_map(|sample| {
            Some((
                primary_calibration_rate(*sample)?,
                u64::from(sample.consumed_basis_points),
            ))
        })
        .collect::<Vec<_>>();
    if rates.len() < 2 {
        return None;
    }
    let middle = weighted_quantile(rates.clone(), 1, 2)?;
    if middle == 0 {
        return None;
    }
    let low = weighted_quantile(rates.clone(), 1, 4)?;
    let high = weighted_quantile(rates, 3, 4)?;
    Some(
        u64::try_from(u128::from(high.saturating_sub(low)) * 10_000 / u128::from(middle))
            .unwrap_or(u64::MAX),
    )
}

fn median(values: &[u64]) -> u64 {
    if values.len().is_multiple_of(2) {
        values[values.len() / 2 - 1].saturating_add(values[values.len() / 2]) / 2
    } else {
        values[values.len() / 2]
    }
}

fn unweighted_quantile(values: &[u64], numerator: usize, denominator: usize) -> Option<u64> {
    if values.is_empty() || denominator == 0 || numerator > denominator {
        return None;
    }
    let index = values
        .len()
        .saturating_mul(numerator)
        .div_ceil(denominator)
        .saturating_sub(1);
    values.get(index).copied()
}

fn service_tier_name(tier: DefaultServiceTier) -> &'static str {
    match tier {
        DefaultServiceTier::Standard => "standard",
        DefaultServiceTier::Fast => "fast",
    }
}

fn parse_service_tier(value: &str) -> DefaultServiceTier {
    if value == "fast" {
        DefaultServiceTier::Fast
    } else {
        DefaultServiceTier::Standard
    }
}

fn scale_duration(value: u64, target_minutes: u32, source_minutes: u32) -> u64 {
    u64::try_from(
        u128::from(value).saturating_mul(u128::from(target_minutes))
            / u128::from(source_minutes.max(1)),
    )
    .unwrap_or(u64::MAX)
}

fn fresh_available(window: &QuotaWindow, now_ms: u64, stale_after_ms: u64) -> Option<u16> {
    (window.observed_at_ms.saturating_add(stale_after_ms) >= now_ms)
        .then_some(window.available_basis_points)
        .flatten()
}

fn empty_window_summary(
    kind: QuotaWindowKind,
    estimate_state: QuotaEconomicsEstimateState,
    observed_basis_points: u64,
    sample_count: usize,
) -> QuotaEconomicsWindowSummary {
    QuotaEconomicsWindowSummary {
        kind,
        potential_micro_usd: None,
        potential_low_micro_usd: None,
        potential_high_micro_usd: None,
        potential_requests: None,
        potential_total_tokens: None,
        full_window_micro_usd: None,
        full_window_low_micro_usd: None,
        full_window_high_micro_usd: None,
        full_window_requests: None,
        full_window_total_tokens: None,
        estimate_state,
        confidence: None,
        observed_basis_points,
        sample_count,
        service_tiers: Vec::new(),
        plan_benchmark: None,
    }
}
