use std::time::{SystemTime, UNIX_EPOCH};

/// Parses an RFC3339 timestamp into Unix milliseconds.
/// Blank, invalid, and pre-epoch values are absent rather than zero.
pub fn unix_time_ms_from_rfc3339(timestamp_text: &str) -> Option<u64> {
    let parsed = chrono::DateTime::parse_from_rfc3339(timestamp_text.trim()).ok()?;
    u64::try_from(parsed.timestamp_millis()).ok()
}

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

    #[test]
    fn rfc3339_timestamp_uses_unix_milliseconds() {
        assert_eq!(
            unix_time_ms_from_rfc3339(" 1970-01-01T00:00:00.001Z "),
            Some(1)
        );
        assert_eq!(unix_time_ms_from_rfc3339("not-a-date"), None);
        assert_eq!(unix_time_ms_from_rfc3339("1969-12-31T23:59:59Z"), None);
    }
}
