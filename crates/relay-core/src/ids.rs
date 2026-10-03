/// A non-empty ASCII token no longer than `max_len` bytes.
///
/// Letters, digits, `-`, and `_` are allowed. Callers that trim or add another
/// rule, such as a required prefix, keep that rule at the call site.
pub fn is_ascii_token(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Secret-reference form of [`is_ascii_token`]. `:` separates the reference kind.
pub fn is_ascii_ref(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
}
