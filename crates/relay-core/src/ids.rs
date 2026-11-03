/// A non-empty ASCII token no longer than `max_len` bytes.
///
/// Letters, digits, `-`, and `_` are allowed. Callers that trim or add another
/// rule, such as a required prefix, keep that rule at the call site.
pub fn is_ascii_token(token: &str, max_len: usize) -> bool {
    !token.is_empty()
        && token.len() <= max_len
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Secret-reference form of [`is_ascii_token`]. `:` separates the reference kind.
pub fn is_ascii_ref(reference: &str, max_len: usize) -> bool {
    !reference.is_empty()
        && reference.len() <= max_len
        && reference
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
}
