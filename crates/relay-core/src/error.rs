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
