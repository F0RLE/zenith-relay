//! Read-only, bounded projection samples; never used by admission or billing.
use super::{ApiEquivalentSummary, TelemetryDb};
use std::collections::{HashMap, VecDeque};
use zenith_relay_core::quota::{QuotaWindow, QuotaWindowKind};

const MIN_BLOCK: u64 = 500;
const MAX_ACCOUNTS: usize = 512;

#[derive(Clone, Copy, PartialEq)]
struct Point {
    at: u64,
    remaining: u64,
    usage: ApiEquivalentSummary,
}

struct History {
    identity: (u64, Option<u32>, QuotaWindowKind, Option<String>, String),
    anchors: VecDeque<Point>,
    latest: Point,
}

#[derive(Default)]
pub(super) struct QuotaSamples(HashMap<String, History>);

impl QuotaSamples {
    fn observe(
        &mut self,
        account: &str,
        window: &QuotaWindow,
        usage: ApiEquivalentSummary,
        revision: &str,
    ) -> Option<(u64, ApiEquivalentSummary)> {
        let start = window.window_start_ms?;
        let remaining = u64::from(
            window
                .available_basis_points
                .filter(|value| *value <= 10_000)?,
        );
        if window.observed_at_ms < start {
            return None;
        }
        let point = Point {
            at: window.observed_at_ms,
            remaining,
            usage,
        };
        let identity = (
            start,
            window.window_minutes,
            window.kind,
            window.provider_cycle_id.clone(),
            revision.to_owned(),
        );
        if !self.0.contains_key(account) && self.0.len() >= MAX_ACCOUNTS {
            let oldest = self
                .0
                .iter()
                .min_by_key(|(_, history)| history.latest.at)
                .map(|(id, _)| id.clone())?;
            self.0.remove(&oldest);
        }
        let history = self.0.entry(account.to_owned()).or_insert_with(|| History {
            identity: identity.clone(),
            anchors: VecDeque::from([point]),
            latest: point,
        });
        if point.at < history.latest.at {
            return None;
        }
        if history.identity != identity
            || point.remaining > history.latest.remaining
            || difference(point.usage, history.latest.usage).is_none()
            || (point.at == history.latest.at && point != history.latest)
        {
            *history = History {
                identity,
                anchors: VecDeque::from([point]),
                latest: point,
            };
            return None;
        }
        if point.at > history.latest.at {
            if history
                .anchors
                .back()
                .is_some_and(|anchor| anchor.remaining.saturating_sub(remaining) >= MIN_BLOCK)
            {
                history.anchors.push_back(point);
                while history.anchors.len() > 4 {
                    history.anchors.pop_front();
                }
            }
            history.latest = point;
        }
        let first = history.anchors.front()?;
        let consumed = first.remaining.checked_sub(remaining)?;
        (consumed >= MIN_BLOCK).then_some((consumed, difference(usage, first.usage)?))
    }
}

fn difference(
    current: ApiEquivalentSummary,
    baseline: ApiEquivalentSummary,
) -> Option<ApiEquivalentSummary> {
    Some(ApiEquivalentSummary {
        micro_usd: current.micro_usd.checked_sub(baseline.micro_usd)?,
        priced_tokens: current.priced_tokens.checked_sub(baseline.priced_tokens)?,
        unpriced_tokens: current
            .unpriced_tokens
            .checked_sub(baseline.unpriced_tokens)?,
    })
}

impl TelemetryDb {
    pub(crate) fn sample_quota_window(
        &self,
        account: &str,
        window: &QuotaWindow,
        usage: ApiEquivalentSummary,
        revision: &str,
    ) -> Option<(u64, ApiEquivalentSummary)> {
        self.quota_samples
            .lock()
            .ok()?
            .observe(account, window, usage, revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(at: u64, remaining: u64) -> QuotaWindow {
        serde_json::from_value(serde_json::json!({"kind":"secondary", "windowStartMs": 1,
            "observedAtMs":at, "availableBasisPoints":remaining, "windowMinutes":10080}))
        .unwrap()
    }
    fn usage(value: u64) -> ApiEquivalentSummary {
        ApiEquivalentSummary {
            micro_usd: value,
            priced_tokens: value,
            unpriced_tokens: 0,
        }
    }
    #[test]
    fn recent_quota_samples_use_paired_deltas_and_reset_on_price_or_cycle_changes() {
        let mut samples = QuotaSamples::default();
        assert!(samples
            .observe("a", &window(10, 9000), usage(100), "v1")
            .is_none());
        assert!(samples
            .observe("a", &window(11, 8900), usage(120), "v1")
            .is_none());
        assert_eq!(
            samples.observe("a", &window(20, 8500), usage(200), "v1"),
            Some((500, usage(100)))
        );
        assert!(samples
            .observe("a", &window(21, 8000), usage(300), "v2")
            .is_none());
        assert!(samples
            .observe("a", &window(22, 9500), usage(10), "v2")
            .is_none());
        assert_eq!(
            samples.observe("a", &window(23, 9000), usage(80), "v2"),
            Some((500, usage(70)))
        );
        assert!(samples
            .observe("a", &window(22, 8000), usage(500), "v2")
            .is_none());
        assert!(samples
            .observe("a", &window(23, 9000), usage(90), "v2")
            .is_none());
        let mut next_cycle = window(30, 8500);
        next_cycle.window_start_ms = Some(25);
        assert!(samples
            .observe("a", &next_cycle, usage(200), "v2")
            .is_none());
        next_cycle.observed_at_ms = 31;
        next_cycle.available_basis_points = Some(8000);
        assert_eq!(
            samples.observe("a", &next_cycle, usage(240), "v2"),
            Some((500, usage(40)))
        );
        next_cycle.observed_at_ms = 32;
        next_cycle.provider_cycle_id = Some("synthetic-cycle".into());
        assert!(samples
            .observe("a", &next_cycle, usage(240), "v2")
            .is_none());
    }
    #[test]
    fn recent_quota_samples_drop_old_workload_and_bound_account_count() {
        let mut samples = QuotaSamples::default();
        for index in 0..=6 {
            let sample = samples.observe(
                "a",
                &window(10 + index, 10000 - index * 500),
                usage(index * index * 100),
                "v1",
            );
            if index == 6 {
                assert_eq!(sample, Some((1500, usage(2700))));
            }
        }
        for index in 0..600 {
            samples.observe(
                &format!("account-{index}"),
                &window(100 + index, 9000),
                usage(10),
                "v1",
            );
        }
        assert_eq!(samples.0.len(), MAX_ACCOUNTS);
    }
}
