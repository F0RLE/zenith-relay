use super::origin::{OriginError, PinnedOrigin};
use reqwest::{header::LOCATION, Method};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{fmt, time::Duration};
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

mod operations;
mod profile;
mod usage_query;

#[derive(Debug)]
pub enum RemoteClientError {
    Origin(OriginError),
    InvalidToken,
    Transport,
    RedirectRejected,
    HttpStatus(u16),
    PoolRoutingConflict,
    ResponseTooLarge,
    InvalidResponse,
    Protocol(String),
}

impl fmt::Display for RemoteClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Origin(error) => write!(formatter, "{error}"),
            Self::InvalidToken => formatter.write_str("remote management token is invalid"),
            Self::Transport => formatter.write_str("remote server request failed"),
            Self::RedirectRejected => formatter.write_str("remote server redirect was rejected"),
            Self::HttpStatus(status) => write!(formatter, "remote server returned HTTP {status}"),
            Self::PoolRoutingConflict => {
                formatter.write_str("pool routing changed; reload the current policy before saving")
            }
            Self::ResponseTooLarge => formatter.write_str("remote server response is too large"),
            Self::InvalidResponse => formatter.write_str("remote server response is invalid"),
            Self::Protocol(error) => write!(formatter, "remote protocol is incompatible: {error}"),
        }
    }
}

impl std::error::Error for RemoteClientError {}

impl From<OriginError> for RemoteClientError {
    fn from(error: OriginError) -> Self {
        Self::Origin(error)
    }
}

pub struct RemoteClient {
    origin: PinnedOrigin,
    token: String,
    http: reqwest::Client,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteProfileCredential {
    pub key_id: String,
    pub base_url: String,
    pub secret: String,
}

impl RemoteClient {
    pub fn new(
        base_url: &str,
        token: &str,
        allow_insecure_http: bool,
    ) -> Result<Self, RemoteClientError> {
        validate_token(token)?;
        let origin = PinnedOrigin::parse(base_url, allow_insecure_http)?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .user_agent("Zenith Relay")
            .build()
            .map_err(|_| RemoteClientError::Transport)?;
        Ok(Self {
            origin,
            token: token.to_string(),
            http,
        })
    }

    pub fn origin(&self) -> &str {
        self.origin.as_str()
    }

    async fn request<I: Serialize + ?Sized, O: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        input: Option<&I>,
        authenticated: bool,
    ) -> Result<O, RemoteClientError> {
        let url = self.origin.endpoint(path)?;
        let mut request = self.http.request(method, url);
        if authenticated {
            request = request.bearer_auth(&self.token);
        }
        if let Some(input) = input {
            request = request.json(input);
        }
        let response = request
            .send()
            .await
            .map_err(|_| RemoteClientError::Transport)?;
        if response.status().is_redirection() {
            let _ = response.headers().get(LOCATION);
            return Err(RemoteClientError::RedirectRejected);
        }
        if !response.status().is_success() {
            return Err(RemoteClientError::HttpStatus(response.status().as_u16()));
        }
        decode_success_body(response).await
    }
}

async fn decode_success_body<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, RemoteClientError> {
    let bytes = response
        .bytes()
        .await
        .map_err(|_| RemoteClientError::Transport)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(RemoteClientError::ResponseTooLarge);
    }
    serde_json::from_slice(&bytes).map_err(|_| RemoteClientError::InvalidResponse)
}

fn validate_token(token: &str) -> Result<(), RemoteClientError> {
    if token.len() < 24
        || token.len() > 8 * 1024
        || token.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(RemoteClientError::InvalidToken)
    } else {
        Ok(())
    }
}
#[cfg(test)]
mod tests;
