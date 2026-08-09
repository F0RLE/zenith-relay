use std::fmt;

#[derive(Debug)]
pub enum Error {
    Validation(String),
    UnsupportedWireApi,
    UpstreamBodyTooLarge,
    Upstream(reqwest::Error),
    InvalidUpstreamResponse(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(message) => formatter.write_str(message),
            Self::UnsupportedWireApi => {
                formatter.write_str("only the Responses wire API is supported")
            }
            Self::UpstreamBodyTooLarge => {
                formatter.write_str("upstream response body is too large")
            }
            Self::Upstream(_) => formatter.write_str("upstream request failed"),
            Self::InvalidUpstreamResponse(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Upstream(error) => Some(error),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Upstream(error)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn safe_error_code(value: &str) -> String {
    let value = value.trim();
    if valid_error_code(value) {
        value.to_string()
    } else {
        "redacted".to_string()
    }
}

pub fn normalize_error_code(value: &str) -> Option<String> {
    let value = value.trim();
    valid_error_code(value).then(|| value.to_ascii_lowercase())
}

fn valid_error_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::{normalize_error_code, safe_error_code};

    #[test]
    fn error_codes_are_bounded_and_redacted() {
        assert_eq!(safe_error_code(" code-1 "), "code-1");
        assert_eq!(normalize_error_code(" CODE-1 ").as_deref(), Some("code-1"));
        assert_eq!(safe_error_code("contains whitespace"), "redacted");
        assert_eq!(normalize_error_code("<script>"), None);
    }
}
