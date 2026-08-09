use std::time::{SystemTime, UNIX_EPOCH};

/// Returns Unix time in milliseconds, clamped to the public `u64` boundary.
pub fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
