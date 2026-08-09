use std::time::{SystemTime, UNIX_EPOCH};

/// Converts a system timestamp to Unix milliseconds, clamped to the public
/// `u64` boundary.
pub fn unix_time_ms_at(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

/// Returns Unix time in milliseconds, clamped to the public `u64` boundary.
pub fn unix_time_ms() -> u64 {
    unix_time_ms_at(SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn conversion_preserves_epoch_milliseconds_and_clamps_pre_epoch_time() {
        assert_eq!(unix_time_ms_at(UNIX_EPOCH), 0);
        assert_eq!(unix_time_ms_at(UNIX_EPOCH + Duration::from_millis(42)), 42);
        assert_eq!(unix_time_ms_at(UNIX_EPOCH - Duration::from_millis(1)), 0);
    }
}
