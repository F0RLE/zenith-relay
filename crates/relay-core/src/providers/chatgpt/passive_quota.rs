use crate::quota::{QuotaSnapshot, QuotaWindow, QuotaWindowInput, QuotaWindowKind, ResetTime};
use reqwest::header::HeaderMap;

const RESET_CYCLE_SKEW_MS: u64 = 60_000;

pub fn merge_codex_quota_headers(
    previous: &QuotaSnapshot,
    headers: &HeaderMap,
    observed_at_ms: u64,
) -> Option<QuotaSnapshot> {
    let primary = parse_window(headers, "primary", QuotaWindowKind::Primary, observed_at_ms);
    let secondary = parse_window(
        headers,
        "secondary",
        QuotaWindowKind::Secondary,
        observed_at_ms,
    );
    let mut merged = previous.clone();
    let removed_placeholder = merged
        .secondary
        .as_ref()
        .is_some_and(QuotaWindow::is_empty_provider_placeholder);
    if removed_placeholder {
        merged.secondary = None;
    }
    if primary.is_none() && secondary.is_none() {
        return removed_placeholder.then_some(merged);
    }
    let new_cycle = primary
        .as_ref()
        .is_some_and(|observed| is_new_cycle(previous.primary.as_ref(), observed))
        || secondary
            .as_ref()
            .is_some_and(|observed| is_new_cycle(previous.secondary.as_ref(), observed));

    if let Some(window) = primary {
        merged.primary = merge_window(previous.primary.as_ref(), window);
    }
    if let Some(window) = secondary {
        merged.secondary = merge_window(previous.secondary.as_ref(), window);
    }
    let observed_limit = merged
        .primary
        .iter()
        .chain(merged.secondary.iter())
        .any(|window| window.available_basis_points == Some(0));
    merged.limit_reached = observed_limit || (previous.limit_reached && !new_cycle);
    merged.updated_at_ms = Some(
        previous
            .updated_at_ms
            .unwrap_or_default()
            .max(observed_at_ms),
    );
    merged.error = None;
    Some(merged)
}

fn parse_window(
    headers: &HeaderMap,
    name: &str,
    kind: QuotaWindowKind,
    observed_at_ms: u64,
) -> Option<QuotaWindow> {
    let used = header_number(headers, &format!("x-codex-{name}-used-percent"))
        .filter(|value| (0.0..=100.0).contains(value));
    let reset_seconds = header_u64(headers, &format!("x-codex-{name}-reset-after-seconds"));
    let window_minutes = header_u64(headers, &format!("x-codex-{name}-window-minutes"))
        .and_then(|value| u32::try_from(value).ok());
    if used.is_none() && reset_seconds.is_none() && window_minutes.is_none() {
        return None;
    }
    QuotaWindow::normalize(
        QuotaWindowInput {
            kind,
            available_percent: used.map(|used| 100.0 - used),
            explicitly_full: None,
            reset: reset_seconds.map(ResetTime::RelativeSeconds),
            window_minutes,
            provider_cycle_id: None,
            observed_at_ms,
        },
        None,
    )
    .ok()
    .filter(|window| !window.is_empty_provider_placeholder())
}

fn merge_window(previous: Option<&QuotaWindow>, mut observed: QuotaWindow) -> Option<QuotaWindow> {
    let Some(previous) = previous else {
        return Some(observed);
    };
    if observed.observed_at_ms < previous.observed_at_ms {
        return Some(previous.clone());
    }
    let new_cycle = is_new_cycle(Some(previous), &observed);
    if !new_cycle {
        observed.available_basis_points = match (
            previous.available_basis_points,
            observed.available_basis_points,
        ) {
            (Some(previous), Some(observed)) => Some(previous.min(observed)),
            (previous, None) => previous,
            (None, observed) => observed,
        };
    }
    observed.reset_at_ms = observed.reset_at_ms.or(previous.reset_at_ms);
    observed.window_minutes = observed.window_minutes.or(previous.window_minutes);
    Some(observed)
}

fn is_new_cycle(previous: Option<&QuotaWindow>, observed: &QuotaWindow) -> bool {
    previous
        .and_then(|window| window.reset_at_ms)
        .is_some_and(|previous_reset| {
            previous_reset <= observed.observed_at_ms
                || observed
                    .reset_at_ms
                    .is_some_and(|reset| reset > previous_reset.saturating_add(RESET_CYCLE_SKEW_MS))
        })
}

fn header_number(headers: &HeaderMap, name: &str) -> Option<f64> {
    headers
        .get(name)?
        .to_str()
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn partial_headers_merge_without_increasing_a_live_cycle() {
        let mut first = HeaderMap::new();
        first.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("40"),
        );
        first.insert(
            "x-codex-primary-reset-after-seconds",
            HeaderValue::from_static("600"),
        );
        let first = merge_codex_quota_headers(&QuotaSnapshot::default(), &first, 1_000).unwrap();

        let mut later = HeaderMap::new();
        later.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("20"),
        );
        later.insert(
            "x-codex-secondary-used-percent",
            HeaderValue::from_static("70"),
        );
        let merged = merge_codex_quota_headers(&first, &later, 2_000).unwrap();

        assert_eq!(merged.primary.unwrap().available_basis_points, Some(6_000));
        assert_eq!(
            merged.secondary.unwrap().available_basis_points,
            Some(3_000)
        );
    }

    #[test]
    fn a_new_reset_cycle_may_restore_available_quota() {
        let mut exhausted = HeaderMap::new();
        exhausted.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("100"),
        );
        exhausted.insert(
            "x-codex-primary-reset-after-seconds",
            HeaderValue::from_static("1"),
        );
        let exhausted =
            merge_codex_quota_headers(&QuotaSnapshot::default(), &exhausted, 1_000).unwrap();
        assert!(exhausted.limit_reached);

        let mut restored = HeaderMap::new();
        restored.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("5"),
        );
        restored.insert(
            "x-codex-primary-reset-after-seconds",
            HeaderValue::from_static("600"),
        );
        let restored = merge_codex_quota_headers(&exhausted, &restored, 3_000).unwrap();
        assert_eq!(
            restored.primary.unwrap().available_basis_points,
            Some(9_500)
        );
        assert!(!restored.limit_reached);
    }

    #[test]
    fn partial_headers_do_not_clear_an_explicit_limit_without_a_known_new_cycle() {
        let previous = QuotaSnapshot {
            limit_reached: true,
            updated_at_ms: Some(1_000),
            ..QuotaSnapshot::default()
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-primary-reset-after-seconds",
            HeaderValue::from_static("600"),
        );

        let merged = merge_codex_quota_headers(&previous, &headers, 2_000).unwrap();
        assert!(merged.limit_reached);
    }

    #[test]
    fn empty_secondary_placeholder_is_ignored_and_removed_from_saved_quota() {
        let placeholder = QuotaWindow {
            kind: QuotaWindowKind::Secondary,
            provider_cycle_id: None,
            window_start_ms: None,
            available_basis_points: Some(10_000),
            explicitly_full: None,
            reset_at_ms: Some(1_000),
            window_minutes: Some(0),
            observed_at_ms: 1_000,
            full_transition_fingerprint: None,
            exhaustion_transition_fingerprint: None,
        };
        let previous = QuotaSnapshot {
            secondary: Some(placeholder),
            ..QuotaSnapshot::default()
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-secondary-used-percent",
            HeaderValue::from_static("0"),
        );
        headers.insert(
            "x-codex-secondary-reset-after-seconds",
            HeaderValue::from_static("0"),
        );
        headers.insert(
            "x-codex-secondary-window-minutes",
            HeaderValue::from_static("0"),
        );

        let merged = merge_codex_quota_headers(&previous, &headers, 2_000).unwrap();
        assert!(merged.secondary.is_none());
    }
}
