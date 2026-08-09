use super::*;

pub(super) fn rate_is_outside_epoch(samples: &[IntervalSample], candidate: IntervalSample) -> bool {
    if samples.len() < DRIFT_BASELINE_SAMPLES {
        return false;
    }
    weighted_calibration_rate(samples)
        .zip(primary_calibration_rate(candidate))
        .is_some_and(|(baseline, candidate)| materially_different(baseline, candidate))
}

pub(super) fn drift_confirms_new_epoch(
    baseline_samples: &[IntervalSample],
    drift_samples: &[IntervalSample],
) -> bool {
    weighted_calibration_rate(baseline_samples)
        .zip(weighted_calibration_rate(drift_samples))
        .is_some_and(|(baseline, drift)| materially_different(baseline, drift))
}

/// Cost of a full window, extrapolated from one measured interval.
///
/// This is the calibration unit: measured reference dollars per unit of
/// measured quota movement. Drift detection compares accounts and epochs in
/// this unit, so it must never be derived through a second conversion.
pub(super) fn primary_calibration_rate(sample: IntervalSample) -> Option<u64> {
    sample
        .usage
        .api_equivalent_micro_usd
        .map(|value| scale(value, 10_000, sample.consumed_basis_points))
}

pub(super) fn weighted_calibration_rate(samples: &[IntervalSample]) -> Option<u64> {
    weighted_quantile(
        samples
            .iter()
            .filter_map(|sample| {
                Some((
                    primary_calibration_rate(*sample)?,
                    u64::from(sample.consumed_basis_points),
                ))
            })
            .collect(),
        1,
        2,
    )
}

pub(super) fn materially_different(baseline: u64, candidate: u64) -> bool {
    if baseline == 0 {
        return candidate > 0;
    }
    u128::from(baseline.abs_diff(candidate)).saturating_mul(10_000)
        > u128::from(baseline).saturating_mul(u128::from(DRIFT_THRESHOLD_BASIS_POINTS))
}

pub(super) fn greatest_common_divisor(mut left: u16, mut right: u16) -> u16 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

pub(super) fn scale(measured: u64, available_basis_points: u16, consumed_basis_points: u16) -> u64 {
    u64::try_from(
        u128::from(measured) * u128::from(available_basis_points)
            / u128::from(consumed_basis_points),
    )
    .unwrap_or(u64::MAX)
}

pub(super) fn weighted_quantile(
    mut values: Vec<(u64, u64)>,
    numerator: u64,
    denominator: u64,
) -> Option<u64> {
    if values.is_empty() || denominator == 0 || numerator > denominator {
        return None;
    }
    values.sort_unstable_by_key(|(value, _)| *value);
    let total_weight = values.iter().map(|(_, weight)| *weight).sum::<u64>();
    if total_weight == 0 {
        return None;
    }
    let target = u128::from(total_weight)
        .saturating_mul(u128::from(numerator))
        .div_ceil(u128::from(denominator));
    let mut cumulative = 0_u128;
    values.into_iter().find_map(|(value, weight)| {
        cumulative = cumulative.saturating_add(u128::from(weight));
        (cumulative >= target).then_some(value)
    })
}
