use super::origin::{OriginError, PinnedOrigin};
use reqwest::{header::LOCATION, Method};
use serde::{de::DeserializeOwned, Serialize};
use std::{fmt, time::Duration};
use zenith_relay_core::protocol::{
    negotiate, Capabilities, ClientProtocolRange, HealthResponse, NegotiatedProtocol,
    RuntimeStateSnapshot, UsagePage,
};

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub enum RemoteClientError {
    Origin(OriginError),
    InvalidToken,
    Transport,
    RedirectRejected,
    HttpStatus(u16),
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

    pub async fn health(&self) -> Result<HealthResponse, RemoteClientError> {
        self.request(Method::GET, "/health", Option::<&()>::None, false)
            .await
    }

    pub async fn capabilities(&self) -> Result<Capabilities, RemoteClientError> {
        self.request(Method::GET, "/capabilities", Option::<&()>::None, true)
            .await
    }

    pub async fn negotiate(
        &self,
    ) -> Result<(HealthResponse, Capabilities, NegotiatedProtocol), RemoteClientError> {
        let health = self.health().await?;
        let capabilities = self.capabilities().await?;
        let negotiated = negotiate(ClientProtocolRange::default(), &capabilities)
            .map_err(|error| RemoteClientError::Protocol(error.to_string()))?;
        Ok((health, capabilities, negotiated))
    }

    pub async fn state(&self) -> Result<RuntimeStateSnapshot, RemoteClientError> {
        self.request(Method::GET, "/state", Option::<&()>::None, true)
            .await
    }

    pub async fn usage(&self, page: u32, page_size: u32) -> Result<UsagePage, RemoteClientError> {
        let path = format!(
            "/usage?page={}&pageSize={}",
            page.max(1),
            page_size.clamp(1, 200)
        );
        self.request(Method::GET, &path, Option::<&()>::None, true)
            .await
    }

    pub async fn mutate(
        &self,
        method: Method,
        path: &str,
        input: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, RemoteClientError> {
        let url = self.origin.endpoint(path)?;
        let mut request = self.http.request(method, url).bearer_auth(&self.token);
        if let Some(input) = input {
            request = request.json(input);
        }
        let response = request
            .send()
            .await
            .map_err(|_| RemoteClientError::Transport)?;
        if response.status().is_redirection() {
            return Err(RemoteClientError::RedirectRejected);
        }
        if !response.status().is_success() {
            return Err(RemoteClientError::HttpStatus(response.status().as_u16()));
        }
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(serde_json::Value::Null);
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| RemoteClientError::Transport)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(RemoteClientError::ResponseTooLarge);
        }
        serde_json::from_slice(&bytes).map_err(|_| RemoteClientError::InvalidResponse)
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
        let bytes = response
            .bytes()
            .await
            .map_err(|_| RemoteClientError::Transport)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(RemoteClientError::ResponseTooLarge);
        }
        serde_json::from_slice(&bytes).map_err(|_| RemoteClientError::InvalidResponse)
    }
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
mod tests {
    use super::*;
    use axum::{http::header::AUTHORIZATION, response::Redirect, routing::get, Router};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn redirect_is_not_followed_and_token_never_reaches_other_origin() {
        let received = Arc::new(AtomicUsize::new(0));
        let observed = received.clone();
        let target = spawn(Router::new().route(
            "/state",
            get(move |headers: axum::http::HeaderMap| {
                let observed = observed.clone();
                async move {
                    if headers.get(AUTHORIZATION).is_some() {
                        observed.fetch_add(1, Ordering::SeqCst);
                    }
                    "{}"
                }
            }),
        ))
        .await;
        let redirect_target = format!("{target}/state");
        let source = spawn(Router::new().route(
            "/state",
            get(move || {
                let redirect_target = redirect_target.clone();
                async move { Redirect::temporary(&redirect_target) }
            }),
        ))
        .await;
        let client = RemoteClient::new(&source, "synthetic-management-token-value", false).unwrap();
        assert!(matches!(
            client.state().await,
            Err(RemoteClientError::RedirectRejected)
        ));
        assert_eq!(received.load(Ordering::SeqCst), 0);
    }

    async fn spawn(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}")
    }
}
