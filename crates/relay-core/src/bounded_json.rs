//! Size-limited JSON for local configuration and fixture records.
use serde::Deserialize;

const MAX_BYTES: usize = 4 * 1024 * 1024;

fn checked_input(input: &[u8]) -> serde_json::Result<&[u8]> {
    if input.len() > MAX_BYTES {
        return Err(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "JSON record exceeds 4 MiB",
        )));
    }
    Ok(input)
}

pub fn from_slice<'de, T: Deserialize<'de>>(input: &'de [u8]) -> serde_json::Result<T> {
    // Keep serde_json's recursion limit enabled. Callers validate field semantics
    // and enforce any smaller format-specific limit before using the result.
    serde_json::from_slice(checked_input(input)?)
}

pub fn from_str<'de, T: Deserialize<'de>>(input: &'de str) -> serde_json::Result<T> {
    from_slice(input.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn size_and_nesting_limits_apply_before_acceptance() {
        let at_limit = format!("\"{}\"", "x".repeat(MAX_BYTES - 2));
        assert!(from_str::<Value>(&at_limit).is_ok());
        let too_large = format!("{at_limit} ");
        assert!(from_str::<Value>(&too_large).unwrap_err().is_io());
        assert!(
            from_slice::<Value>(too_large.as_bytes())
                .unwrap_err()
                .is_io()
        );
        let deep = format!("{}0{}", "[".repeat(150), "]".repeat(150));
        assert!(from_str::<Value>(&deep).is_err());
        assert!(from_slice::<Value>(&[0xff]).is_err());
        assert!(from_str::<Value>("{} false").is_err());
    }

    #[test]
    fn strict_fields_and_borrowed_values_keep_their_contracts() {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Record<'a> {
            value: &'a str,
        }
        let input = r#"{"value":"synthetic"}"#;
        assert_eq!(from_str::<Record<'_>>(input).unwrap().value, "synthetic");
        assert!(from_str::<Record<'_>>(r#"{"value":"synthetic","extra":1}"#).is_err());
    }
}
