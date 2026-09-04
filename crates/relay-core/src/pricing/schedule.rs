use sha2::Digest;
use std::time::Duration;

/// The normal interval between background LiteLLM catalog checks.
pub const PRICING_REFRESH_INTERVAL_SECONDS: u64 = 24 * 60 * 60;

/// Maximum deterministic spread added between catalog checks.
///
/// The spread is applied only to the normal daily check. Startup validation and
/// retry deadlines remain exact, so a failed refresh is not delayed beyond its
/// configured backoff.
pub const PRICING_REFRESH_JITTER_MAX_SECONDS: u64 = 60;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogRefreshKind {
    Startup,
    Retry,
    Scheduled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CatalogRefreshDeadline {
    pub at_ms: u64,
    pub kind: CatalogRefreshKind,
}

/// Returns a stable per-instance offset in the inclusive `0..=60` range.
///
/// A cryptographic digest keeps the result independent of Rust's randomized
/// hashers, so the same local state path or server id produces the same
/// schedule after every restart.
pub fn pricing_refresh_jitter_seconds(instance_id: &str) -> u64 {
    let digest = sha2::Sha256::digest(instance_id.as_bytes());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(bytes) % (PRICING_REFRESH_JITTER_MAX_SECONDS + 1)
}

/// Returns the wait until the loader's next catalog refresh deadline.
///
/// Only the normal daily refresh gets a deterministic spread. Startup checks
/// run asynchronously but immediately, and retries preserve the exact deadline
/// selected by the loader's backoff state.
pub fn pricing_refresh_delay(
    instance_id: &str,
    deadline: CatalogRefreshDeadline,
    now_ms: u64,
) -> Duration {
    let jitter_ms = match deadline.kind {
        CatalogRefreshKind::Scheduled => {
            pricing_refresh_jitter_seconds(instance_id).saturating_mul(1_000)
        }
        CatalogRefreshKind::Startup | CatalogRefreshKind::Retry => 0,
    };
    let wake_at_ms = deadline.at_ms.saturating_add(jitter_ms);
    Duration::from_millis(wake_at_ms.saturating_sub(now_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_is_deterministic_and_bounded() {
        let first = pricing_refresh_jitter_seconds("desktop-state-a");
        assert_eq!(first, pricing_refresh_jitter_seconds("desktop-state-a"));
        assert!(first <= PRICING_REFRESH_JITTER_MAX_SECONDS);
    }

    #[test]
    fn different_instance_ids_can_get_different_offsets() {
        let offsets = [
            "desktop-state-a",
            "desktop-state-b",
            "server-id-a",
            "server-id-b",
        ]
        .map(pricing_refresh_jitter_seconds);
        assert!(offsets.windows(2).any(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn startup_and_retry_deadlines_do_not_receive_jitter() {
        let now_ms = 1_000;
        for kind in [CatalogRefreshKind::Startup, CatalogRefreshKind::Retry] {
            assert_eq!(
                pricing_refresh_delay(
                    "instance",
                    CatalogRefreshDeadline {
                        at_ms: now_ms + 5_000,
                        kind,
                    },
                    now_ms,
                ),
                Duration::from_secs(5)
            );
        }
    }

    #[test]
    fn scheduled_deadline_receives_only_the_bounded_instance_jitter() {
        let now_ms = 1_000;
        let scheduled_at_ms = now_ms + PRICING_REFRESH_INTERVAL_SECONDS * 1_000;
        let delay = pricing_refresh_delay(
            "instance",
            CatalogRefreshDeadline {
                at_ms: scheduled_at_ms,
                kind: CatalogRefreshKind::Scheduled,
            },
            now_ms,
        );
        assert!(delay >= Duration::from_secs(PRICING_REFRESH_INTERVAL_SECONDS));
        assert!(
            delay
                <= Duration::from_secs(
                    PRICING_REFRESH_INTERVAL_SECONDS + PRICING_REFRESH_JITTER_MAX_SECONDS
                )
        );
    }

    #[test]
    fn overdue_deadline_returns_zero_delay() {
        assert_eq!(
            pricing_refresh_delay(
                "instance",
                CatalogRefreshDeadline {
                    at_ms: 1,
                    kind: CatalogRefreshKind::Retry,
                },
                2,
            ),
            Duration::ZERO
        );
    }
}
