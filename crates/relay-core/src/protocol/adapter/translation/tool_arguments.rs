use std::borrow::Cow;

/// JSON has one numeric domain: an exact integral decimal or exponent value can
/// use integer syntax. Typed Responses tools may require that syntax. Work on
/// literal digits, not f64, to preserve precision, strings, and negative zero.
/// An unfinished stream number is withheld until its delimiter arrives.
pub(super) fn normalize(arguments: &str, complete: bool) -> Cow<'_, str> {
    let bytes = arguments.as_bytes();
    let mut output = None::<String>;
    let mut copied = 0;
    let mut end = arguments.len();
    let mut index = 0;
    let mut quoted = false;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if quoted => index = (index + 2).min(bytes.len()),
            b'"' => {
                quoted = !quoted;
                index += 1;
            }
            b'-' | b'0'..=b'9' if !quoted => {
                let start = index;
                index += 1;
                while index < bytes.len()
                    && matches!(bytes[index], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    index += 1;
                }
                if index == bytes.len() && !complete {
                    end = start;
                    break;
                }
                if let Some(integer) = integer_literal(&arguments[start..index]) {
                    let output =
                        output.get_or_insert_with(|| String::with_capacity(arguments.len()));
                    output.push_str(&arguments[copied..start]);
                    output.push_str(&integer);
                    copied = index;
                }
            }
            _ => index += 1,
        }
    }
    match output {
        Some(mut output) => {
            output.push_str(&arguments[copied..end]);
            Cow::Owned(output)
        }
        None => Cow::Borrowed(&arguments[..end]),
    }
}

fn integer_literal(literal: &str) -> Option<String> {
    if !literal.contains(['.', 'e', 'E']) {
        return None;
    }
    let negative = literal.starts_with('-');
    let unsigned = literal.strip_prefix('-').unwrap_or(literal);
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i64>().ok()?),
        None => (unsigned, 0),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let mut digits = format!("{whole}{fraction}");
    digits = digits.trim_start_matches('0').to_owned();
    if digits.is_empty() {
        return (!negative).then(|| "0".to_owned());
    }
    let scale = exponent.checked_sub(i64::try_from(fraction.len()).ok()?)?;
    if scale < 0 {
        let removed = usize::try_from(scale.checked_neg()?).ok()?;
        let kept = digits.len().checked_sub(removed)?;
        if !digits[kept..].bytes().all(|byte| byte == b'0') {
            return None;
        }
        digits.truncate(kept);
    } else {
        let added = usize::try_from(scale).ok()?;
        if digits.len().checked_add(added)? > 20 {
            return None;
        }
        digits.extend(std::iter::repeat_n('0', added));
    }
    if negative {
        format!("-{digits}")
            .parse::<i64>()
            .ok()
            .map(|value| value.to_string())
    } else {
        digits.parse::<u64>().ok().map(|value| value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integral_literals_keep_exact_values_and_integer_bounds() {
        for (literal, expected) in [
            ("1000.0", "1000"),
            ("12.500e2", "1250"),
            ("1200e-2", "12"),
            ("-12.0", "-12"),
            ("9007199254740993.0", "9007199254740993"),
            ("18446744073709551615.0", "18446744073709551615"),
            ("-9223372036854775808.0", "-9223372036854775808"),
        ] {
            assert_eq!(integer_literal(literal).as_deref(), Some(expected));
        }
        for unchanged in [
            "1.5",
            "1e-3",
            "-0.0",
            "-0e3",
            "18446744073709551616.0",
            "-9223372036854775809.0",
            "1e1000000",
            "9007199254740993.1",
        ] {
            assert_eq!(integer_literal(unchanged), None, "{unchanged}");
        }
    }

    #[test]
    fn strings_fractional_values_and_whitespace_are_preserved() {
        let input = r#" { "v": [1.0, 1.5, -0.0, {"s":"数字 1.0 \"2e3\"", "x":2e3}] } "#;
        let expected = r#" { "v": [1, 1.5, -0.0, {"s":"数字 1.0 \"2e3\"", "x":2000}] } "#;
        assert_eq!(normalize(input, true), expected);
    }

    #[test]
    fn every_stream_prefix_extends_previously_emitted_arguments() {
        let input = r#"{"count":1000.0,"nested":[12.500e2,1.5],"s":"1.0 \"2e3\""}"#;
        let expected = r#"{"count":1000,"nested":[1250,1.5],"s":"1.0 \"2e3\""}"#;
        let mut previous = String::new();
        for end in 0..=input.len() {
            let prefix = normalize(&input[..end], false);
            assert!(prefix.starts_with(&previous), "{end}");
            assert!(expected.starts_with(prefix.as_ref()), "{end}");
            previous = prefix.into_owned();
        }
        assert_eq!(normalize(input, true), expected);
    }

    #[test]
    fn unchanged_arguments_and_buffered_prefixes_borrow_the_input() {
        for (input, complete, expected) in [
            (
                r#"{"s":"数字 1.0","count":12}"#,
                true,
                r#"{"s":"数字 1.0","count":12}"#,
            ),
            (
                r#"{"fraction":1.5,"zero":-0.0}"#,
                true,
                r#"{"fraction":1.5,"zero":-0.0}"#,
            ),
            (r#"{"count":1000."#, false, r#"{"count":"#),
            ("", false, ""),
        ] {
            let normalized = normalize(input, complete);
            assert!(matches!(normalized, Cow::Borrowed(_)));
            assert_eq!(normalized, expected);
            assert_eq!(normalized.as_ptr(), input.as_ptr());
        }
    }
}
