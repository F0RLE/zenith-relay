use super::super::{collect_limited, LimitedBodyError};
use super::error::{OAuthError, OAuthErrorCode};
use super::exchange::{OAuthCallback, OAuthStart, OAuthTokenSet};
use super::parse::{
    parse_token_response, validate_token, AuthorizationCodeRequest, RefreshTokenRequest,
};
use super::session::OAuthPendingSession;
use super::{
    OAuthClientKind, BASIS_POINTS_OAUTH_REDIRECT_URI, CALLBACK_PATH, CODEX_OAUTH_CALLBACK_PORTS,
    CODEX_OAUTH_ISSUER, CODEX_OAUTH_ORIGINATOR, MAX_TOKEN_RESPONSE_BYTES,
};
use crate::local_pool::random_urlsafe;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::redirect::Policy;
use sha2::{Digest, Sha256};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use url::Url;
use zenith_relay_core::accounts::{
    TokenRefresh, TokenRefreshAdapter, TokenRefreshFailure, TokenRefreshFailureKind,
};
use zenith_relay_core::error_codes;
use zenith_relay_core::providers::chatgpt::{
    token_refresh_failure_kind, token_refresh_provider_error_code,
};
use zenith_relay_core::scheduler::refresh::http::{management_http_gate, HttpClass};
use zenith_relay_core::ProxyConfig;

#[derive(Clone)]
pub struct CodexOAuthClient {
    http: reqwest::Client,
    kind: OAuthClientKind,
    authorize_endpoint: Url,
    token_endpoint: Url,
}

impl CodexOAuthClient {
    #[cfg(test)]
    pub fn new() -> Result<Self, OAuthError> {
        Self::new_with_proxy(None)
    }

    pub fn new_with_proxy(proxy: Option<&ProxyConfig>) -> Result<Self, OAuthError> {
        Self::new_with_proxy_for_kind(OAuthClientKind::Codex, proxy)
    }

    pub fn new_with_proxy_for_kind(
        kind: OAuthClientKind,
        proxy: Option<&ProxyConfig>,
    ) -> Result<Self, OAuthError> {
        let issuer = Url::parse(CODEX_OAUTH_ISSUER)
            .map_err(|_| OAuthError::new(OAuthErrorCode::InvalidConfiguration, false))?;
        let authorize_endpoint = issuer
            .join(kind.authorize_path())
            .map_err(|_| OAuthError::new(OAuthErrorCode::InvalidConfiguration, false))?;
        let token_endpoint = issuer
            .join("oauth/token")
            .map_err(|_| OAuthError::new(OAuthErrorCode::InvalidConfiguration, false))?;
        Self::with_endpoints_and_proxy_for_kind(kind, authorize_endpoint, token_endpoint, proxy)
    }

    #[cfg(test)]
    pub(super) fn with_endpoints(
        authorize_endpoint: Url,
        token_endpoint: Url,
    ) -> Result<Self, OAuthError> {
        Self::with_endpoints_and_proxy_for_kind(
            OAuthClientKind::Codex,
            authorize_endpoint,
            token_endpoint,
            None,
        )
    }

    fn with_endpoints_and_proxy_for_kind(
        kind: OAuthClientKind,
        authorize_endpoint: Url,
        token_endpoint: Url,
        proxy: Option<&ProxyConfig>,
    ) -> Result<Self, OAuthError> {
        let builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(20))
            .user_agent("Zenith Relay");
        let http = match proxy {
            Some(proxy) => proxy.apply(builder),
            None => builder,
        }
        .build()
        .map_err(|_| OAuthError::new(OAuthErrorCode::InvalidConfiguration, false))?;
        Ok(Self {
            http,
            kind,
            authorize_endpoint,
            token_endpoint,
        })
    }

    pub fn kind(&self) -> OAuthClientKind {
        self.kind
    }

    pub fn for_kind(&self, kind: OAuthClientKind) -> Self {
        let mut client = self.clone();
        client.kind = kind;
        client.authorize_endpoint.set_path(kind.authorize_path());
        client
    }

    pub fn begin(&self, callback_port: u16, now_ms: u64) -> Result<OAuthStart, OAuthError> {
        if self.kind.is_local_callback() && !CODEX_OAUTH_CALLBACK_PORTS.contains(&callback_port) {
            return Err(OAuthError::new(OAuthErrorCode::InvalidCallbackPort, false));
        }

        let redirect_uri = match self.kind {
            OAuthClientKind::Codex => format!("http://localhost:{callback_port}{CALLBACK_PATH}"),
            OAuthClientKind::ExcelBps => BASIS_POINTS_OAUTH_REDIRECT_URI.to_string(),
        };
        let code_verifier = random_urlsafe(64);
        let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
        let oauth_state = match self.kind {
            OAuthClientKind::Codex => random_urlsafe(32),
            OAuthClientKind::ExcelBps => format!("bps.{}.PC", random_urlsafe(32)),
        };
        let pending = OAuthPendingSession {
            client_kind: self.kind,
            redirect_uri,
            state: oauth_state,
            code_verifier,
            created_at_ms: now_ms,
        };

        let mut authorization_url = self.authorize_endpoint.clone();
        authorization_url
            .query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", self.kind.client_id())
            .append_pair("redirect_uri", &pending.redirect_uri)
            .append_pair("scope", self.kind.scope())
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &pending.state);
        if self.kind == OAuthClientKind::Codex {
            authorization_url
                .query_pairs_mut()
                .append_pair("id_token_add_organizations", "true")
                .append_pair("codex_cli_simplified_flow", "true")
                .append_pair("originator", CODEX_OAUTH_ORIGINATOR);
        } else {
            authorization_url
                .query_pairs_mut()
                .append_pair("audience", "https://api.openai.com/v1")
                .append_pair("platform", "PC");
        }

        Ok(OAuthStart {
            authorization_url,
            pending,
        })
    }

    pub async fn exchange_code(
        &self,
        pending: &OAuthPendingSession,
        callback: OAuthCallback,
        now_ms: u64,
    ) -> Result<OAuthTokenSet, OAuthError> {
        if pending.client_kind() != self.kind {
            return Err(OAuthError::new(OAuthErrorCode::InvalidConfiguration, false));
        }
        let (response, permit) = management_http_gate()
            .send(
                &self.http,
                self.http
                    .post(self.token_endpoint())
                    .form(&AuthorizationCodeRequest {
                        grant_type: "authorization_code",
                        code: callback.code(),
                        redirect_uri: &pending.redirect_uri,
                        client_id: self.kind.client_id(),
                        code_verifier: &pending.code_verifier,
                    }),
                HttpClass::Auth,
            )
            .await
            .map_err(|_| OAuthError::new(OAuthErrorCode::Transport, true))?;
        let status = response.status();
        let token_response_body = collect_limited(response, MAX_TOKEN_RESPONSE_BYTES)
            .await
            .map_err(|error| match error {
                LimitedBodyError::Transport => OAuthError::new(OAuthErrorCode::Transport, true),
                LimitedBodyError::TooLarge => {
                    OAuthError::new(OAuthErrorCode::ResponseTooLarge, false)
                }
            })?;
        drop(permit);
        if !status.is_success() {
            return Err(OAuthError {
                code: OAuthErrorCode::TokenEndpointRejected,
                provider_code: token_refresh_provider_error_code(&token_response_body),
                http_status: Some(status.as_u16()),
                retryable: status.is_server_error() || status.as_u16() == 429,
            });
        }

        self.parse_tokens(&token_response_body, now_ms)
    }

    pub async fn exchange_refresh_token(
        &self,
        refresh_token: &str,
        now_ms: u64,
    ) -> Result<OAuthTokenSet, TokenRefreshFailure> {
        validate_token(refresh_token).map_err(|_| {
            TokenRefreshFailure::new(
                TokenRefreshFailureKind::Transient,
                error_codes::INVALID_REFRESH_TOKEN,
            )
        })?;
        let request = self.http.post(self.token_endpoint());
        let payload = RefreshTokenRequest {
            client_id: self.kind.client_id(),
            grant_type: "refresh_token",
            refresh_token,
        };
        let request = match self.kind {
            OAuthClientKind::Codex => request.json(&payload),
            OAuthClientKind::ExcelBps => request.form(&payload),
        };
        let (response, permit) = management_http_gate()
            .send(&self.http, request, HttpClass::Auth)
            .await
            .map_err(|_| {
                TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "transport")
            })?;
        let status = response.status();
        let token_response_body = collect_limited(response, MAX_TOKEN_RESPONSE_BYTES)
            .await
            .map_err(|error| match error {
                LimitedBodyError::Transport => {
                    TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "transport")
                }
                LimitedBodyError::TooLarge => TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "response_too_large",
                ),
            })?;
        drop(permit);
        if !status.is_success() {
            let code = token_refresh_provider_error_code(&token_response_body)
                .unwrap_or_else(|| "token_refresh_failed".into());
            return Err(TokenRefreshFailure::new(
                token_refresh_failure_kind(&code),
                &code,
            ));
        }

        self.parse_tokens(&token_response_body, now_ms)
            .map_err(|_| {
                TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "invalid_response")
            })
    }

    fn parse_tokens(
        &self,
        token_response_body: &[u8],
        now_ms: u64,
    ) -> Result<OAuthTokenSet, OAuthError> {
        let tokens = parse_token_response(token_response_body, now_ms)?;
        self.kind
            .validate_token_hints(tokens.id_token(), Some(tokens.access_token()))
            .map_err(|_| OAuthError::new(OAuthErrorCode::InvalidResponse, false))?;
        Ok(tokens)
    }

    fn token_endpoint(&self) -> Url {
        let mut endpoint = self.token_endpoint.clone();
        if self.kind == OAuthClientKind::ExcelBps {
            endpoint.query_pairs_mut().append_pair("unified", "true");
        }
        endpoint
    }
}

impl TokenRefreshAdapter for CodexOAuthClient {
    fn refresh<'a>(
        &'a self,
        _account_id: &'a str,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<TokenRefresh, TokenRefreshFailure>> + Send + 'a>> {
        Box::pin(async move {
            let tokens = self.exchange_refresh_token(refresh_token, now_ms).await?;
            TokenRefresh::new(
                tokens.access_token,
                tokens.refresh_token,
                tokens.id_token,
                tokens.expires_at_ms,
            )
            .map_err(|_| {
                TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "invalid_response")
            })
        })
    }
}
