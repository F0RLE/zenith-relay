use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

const MAX_SAMPLES: usize = 64;
const MIN_MEASURED_SAMPLES: usize = 20;
const WINDOW_MS: u64 = super::HEALTH_HALF_LIFE_MS;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteLatencyState {
    Unknown,
    Insufficient,
    Ready,
    Stale,
}

/// Successful physical attempts for the selected candidate and exact route.
/// Captured before admission; these quantiles do not participate in selection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteLatencyDiagnostics {
    pub state: RouteLatencyState,
    pub sample_count: u32,
    pub unknown_count: u32,
    pub minimum_samples: u32,
    pub window_ms: u64,
    pub as_of_ms: u64,
    pub last_observed_at_ms: Option<u64>,
    pub p50_ms: Option<u64>,
    pub p90_ms: Option<u64>,
    pub p95_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    observed_at_ms: u64,
    latency_ms: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct LatencyWindow {
    samples: VecDeque<Sample>,
}

impl LatencyWindow {
    pub(super) fn observe(&mut self, latency_ms: Option<u64>, now_ms: u64) {
        // Bound storage by attempts, including successful sends with no TTFT.
        // Read-time filtering also handles clock rollback without fake freshness.
        self.samples.retain(|sample| {
            sample.observed_at_ms <= now_ms && now_ms - sample.observed_at_ms <= WINDOW_MS
        });
        if self.samples.len() == MAX_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(Sample {
            observed_at_ms: now_ms,
            latency_ms,
        });
    }

    pub(super) fn snapshot(&self, now_ms: u64) -> RouteLatencyDiagnostics {
        let mut values = Vec::with_capacity(self.samples.len());
        let mut unknown_count = 0;
        let mut last_observed_at_ms: Option<u64> = None;
        for sample in &self.samples {
            if sample.observed_at_ms > now_ms {
                continue;
            }
            last_observed_at_ms = Some(last_observed_at_ms.unwrap_or(0).max(sample.observed_at_ms));
            if now_ms - sample.observed_at_ms > WINDOW_MS {
                continue;
            }
            if let Some(latency) = sample.latency_ms {
                values.push(latency);
            } else {
                unknown_count += 1;
            }
        }
        let state = if values.len() >= MIN_MEASURED_SAMPLES {
            RouteLatencyState::Ready
        } else if !values.is_empty() {
            RouteLatencyState::Insufficient
        } else if unknown_count > 0 || self.samples.is_empty() {
            RouteLatencyState::Unknown
        } else {
            RouteLatencyState::Stale
        };
        if state == RouteLatencyState::Ready {
            values.sort_unstable();
        }
        let quantile = |percent: usize| {
            (state == RouteLatencyState::Ready)
                .then(|| values[(values.len() * percent).div_ceil(100) - 1])
        };
        RouteLatencyDiagnostics {
            state,
            sample_count: values.len() as u32,
            unknown_count,
            minimum_samples: MIN_MEASURED_SAMPLES as u32,
            window_ms: WINDOW_MS,
            as_of_ms: now_ms,
            last_observed_at_ms,
            p50_ms: quantile(50),
            p90_ms: quantile(90),
            p95_ms: quantile(95),
        }
    }
}

#[cfg(test)]
mod tests;
