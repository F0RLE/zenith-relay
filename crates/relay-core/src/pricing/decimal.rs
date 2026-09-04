use serde_json::Value;

use super::PricingError;

/// Converts a non-negative USD amount into microUSD at the requested unit.
///
/// The parser intentionally works on decimal text and integer arithmetic. A
/// provider can send either a JSON number or a string, including scientific
/// notation. Values are rounded to the nearest micro-unit, with halves away
/// from zero (negative values are rejected before rounding).
pub fn usd_to_micro(value: &Value, unit_scale: u128) -> Result<u64, PricingError> {
    if decimal_digits(unit_scale).is_none() {
        // Prices are scaled in base ten. Reject arbitrary caller-provided
        // scales instead of silently applying a binary or mixed scale.
        return Err(PricingError::InvalidAmount);
    }
    let text = value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_number().map(ToString::to_string))
        .ok_or(PricingError::InvalidAmount)?;
    parse_decimal_to_scaled(&text, unit_scale)
}

/// Converts USD/token into microUSD per million tokens.
pub fn usd_per_token_to_micro_usd_per_million(value: &Value) -> Result<u64, PricingError> {
    parse_decimal_value(value, 1_000_000_000_000)
}

/// Converts USD/request into microUSD per request.
pub fn usd_per_request_to_micro_usd(value: &Value) -> Result<u64, PricingError> {
    parse_decimal_value(value, 1_000_000)
}

fn parse_decimal_value(value: &Value, scale: u128) -> Result<u64, PricingError> {
    let text = value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_number().map(ToString::to_string))
        .ok_or(PricingError::InvalidAmount)?;
    parse_decimal_to_scaled(&text, scale)
}

fn parse_decimal_to_scaled(text: &str, scale: u128) -> Result<u64, PricingError> {
    let text = text.trim();
    if text.is_empty() || text.starts_with('-') || text.starts_with('+') {
        return Err(PricingError::InvalidAmount);
    }

    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (
            mantissa,
            exponent
                .parse::<i32>()
                .map_err(|_| PricingError::InvalidAmount)?,
        ),
        None => (text, 0),
    };
    if mantissa.matches('.').count() > 1 {
        return Err(PricingError::InvalidAmount);
    }
    let (whole, fractional) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (whole.is_empty() && fractional.is_empty())
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(PricingError::InvalidAmount);
    }

    let mut digits = String::with_capacity(whole.len() + fractional.len());
    digits.push_str(whole);
    digits.push_str(fractional);
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Err(PricingError::InvalidAmount);
    }
    // Bound untrusted provider input before parsing or exponent arithmetic.
    if digits.len() > 38 || fractional.len() > 38 {
        return Err(PricingError::InvalidAmount);
    }
    let digits = digits
        .parse::<u128>()
        .map_err(|_| PricingError::InvalidAmount)?;
    let scale_digits = decimal_digits(scale).ok_or(PricingError::InvalidAmount)?;
    let fractional_digits =
        i64::try_from(fractional.len()).map_err(|_| PricingError::InvalidAmount)?;
    let shift = i64::from(exponent)
        .checked_add(scale_digits)
        .and_then(|value| value.checked_sub(fractional_digits))
        .ok_or(PricingError::Overflow)?;

    let result = if shift >= 0 {
        let power = u32::try_from(shift).map_err(|_| PricingError::Overflow)?;
        if power > 38 {
            return Err(PricingError::Overflow);
        }
        digits
            .checked_mul(10_u128.checked_pow(power).ok_or(PricingError::Overflow)?)
            .ok_or(PricingError::Overflow)?
    } else {
        let power = shift.unsigned_abs();
        if power > 38 {
            return Ok(0);
        }
        let divisor = 10_u128
            .checked_pow(u32::try_from(power).map_err(|_| PricingError::Overflow)?)
            .ok_or(PricingError::Overflow)?;
        let quotient = digits / divisor;
        let remainder = digits % divisor;
        quotient
            .checked_add(u128::from(remainder >= divisor.div_ceil(2)))
            .ok_or(PricingError::Overflow)?
    };

    let result = u64::try_from(result).map_err(|_| PricingError::Overflow)?;
    (result > 0)
        .then_some(result)
        .ok_or(PricingError::InvalidAmount)
}

fn decimal_digits(value: u128) -> Option<i64> {
    if value == 0 {
        return None;
    }
    let mut value = value;
    let mut digits = 0_i64;
    while value.is_multiple_of(10) {
        value /= 10;
        digits += 1;
    }
    (value == 1).then_some(digits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_decimal_and_scientific_token_prices_without_float_math() {
        assert_eq!(
            usd_per_token_to_micro_usd_per_million(&json!("0.0000025")),
            Ok(2_500_000)
        );
        assert_eq!(
            usd_per_token_to_micro_usd_per_million(&json!(2.5e-6)),
            Ok(2_500_000)
        );
    }

    #[test]
    fn rounds_half_up_and_rejects_invalid_values() {
        assert_eq!(usd_per_request_to_micro_usd(&json!("0.0000005")), Ok(1));
        assert_eq!(
            usd_per_token_to_micro_usd_per_million(&json!(-1)),
            Err(PricingError::InvalidAmount)
        );
        assert_eq!(
            usd_per_token_to_micro_usd_per_million(&json!("not-a-price")),
            Err(PricingError::InvalidAmount)
        );
    }

    #[test]
    fn rejects_overflow_and_bounds_untrusted_input() {
        assert_eq!(
            usd_per_token_to_micro_usd_per_million(&json!("1e100")),
            Err(PricingError::Overflow)
        );
        assert_eq!(
            usd_per_token_to_micro_usd_per_million(&json!(format!("0.{}", "1".repeat(39)))),
            Err(PricingError::InvalidAmount)
        );
    }
}
