use super::{OAuthError, OAuthErrorCode};
use serde_json::Value;

/// SIWC returns Unix seconds or an RFC3339 date, never an inferred millisecond unit.
pub(super) fn refresh_deadline_ms(value: Option<&Value>) -> Result<Option<u64>, OAuthError> {
    let invalid = || OAuthError::new(OAuthErrorCode::InvalidResponse, false);
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let milliseconds = match value {
        Value::Number(number) => number
            .as_u64()
            .and_then(|seconds| seconds.checked_mul(1_000)),
        Value::String(date) => chrono::DateTime::parse_from_rfc3339(date)
            .ok()
            .and_then(|date| u64::try_from(date.timestamp_millis()).ok()),
        _ => None,
    };
    milliseconds.map(Some).ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn siwc_refresh_deadline_accepts_seconds_and_dates_with_offsets() {
        for value in [
            json!(1_793_491_200_u64),
            json!("2026-11-01T00:00:00Z"),
            json!("2026-11-01T03:00:00+03:00"),
        ] {
            assert_eq!(
                refresh_deadline_ms(Some(&value)).unwrap(),
                Some(1_793_491_200_000)
            );
        }
        assert_eq!(
            refresh_deadline_ms(Some(&json!("2026-11-01T00:00:00.125Z"))).unwrap(),
            Some(1_793_491_200_125)
        );
    }

    #[test]
    fn siwc_refresh_deadline_preserves_absence_and_epoch() {
        assert_eq!(refresh_deadline_ms(None).unwrap(), None);
        assert_eq!(refresh_deadline_ms(Some(&Value::Null)).unwrap(), None);
        assert_eq!(refresh_deadline_ms(Some(&json!(0))).unwrap(), Some(0));
    }

    #[test]
    fn siwc_refresh_deadline_rejects_malformed_or_overflowing_values() {
        for value in [
            json!(-1),
            json!(1.5),
            json!(u64::MAX),
            json!(true),
            json!([]),
            json!({}),
            json!("1793491200"),
            json!("2026-11-01"),
            json!("1969-12-31T23:59:59Z"),
            json!("private-invalid-date"),
        ] {
            let error = refresh_deadline_ms(Some(&value)).unwrap_err();
            assert_eq!(error.code, OAuthErrorCode::InvalidResponse);
            assert!(!format!("{error:?}").contains("private-invalid-date"));
        }
    }
}
