use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::Rng;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use url::Url;
use zenith_relay_core::accounts::{
    decode_unverified_jwt_payload, TokenRefresh, TokenRefreshAdapter, TokenRefreshFailure,
    TokenRefreshFailureKind,
};
use zenith_relay_core::providers::chatgpt::{
    token_refresh_failure_kind, token_refresh_provider_error_code,
};
use zenith_relay_core::{normalize_error_code, ProxyConfig};

use super::{collect_limited, LimitedBodyError};

pub const CODEX_OAUTH_ISSUER: &str = "https://auth.openai.com";
pub const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const CODEX_OAUTH_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const CODEX_OAUTH_ORIGINATOR: &str = "codex_cli_rs";
pub(super) const CODEX_OAUTH_CALLBACK_PORTS: [u16; 2] = [1455, 1457];

const CALLBACK_PATH: &str = "/auth/callback";
const MAX_CALLBACK_URL_BYTES: usize = 8 * 1024;
const MAX_TOKEN_BYTES: usize = 64 * 1024;
const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;
const PENDING_TTL_MS: u64 = 15 * 60 * 1_000;

#[derive(Clone)]
pub struct CodexOAuthClient {
    http: reqwest::Client,
    authorize_endpoint: Url,
    token_endpoint: Url,
}

impl CodexOAuthClient {
    #[cfg(test)]
    pub fn new() -> Result<Self, OAuthError> {
        Self::new_with_proxy(None)
    }

    pub fn new_with_proxy(proxy: Option<&ProxyConfig>) -> Result<Self, OAuthError> {
        let issuer = Url::parse(CODEX_OAUTH_ISSUER)
            .map_err(|_| OAuthError::new(OAuthErrorCode::InvalidConfiguration, false))?;
        let authorize_endpoint = issuer
            .join("oauth/authorize")
            .map_err(|_| OAuthError::new(OAuthErrorCode::InvalidConfiguration, false))?;
        let token_endpoint = issuer
            .join("oauth/token")
            .map_err(|_| OAuthError::new(OAuthErrorCode::InvalidConfiguration, false))?;
        Self::with_endpoints_and_proxy(authorize_endpoint, token_endpoint, proxy)
    }

    #[cfg(test)]
    fn with_endpoints(authorize_endpoint: Url, token_endpoint: Url) -> Result<Self, OAuthError> {
        Self::with_endpoints_and_proxy(authorize_endpoint, token_endpoint, None)
    }

    fn with_endpoints_and_proxy(
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
            authorize_endpoint,
            token_endpoint,
        })
    }

    pub fn begin(&self, callback_port: u16, now_ms: u64) -> Result<OAuthStart, OAuthError> {
        if !CODEX_OAUTH_CALLBACK_PORTS.contains(&callback_port) {
            return Err(OAuthError::new(OAuthErrorCode::InvalidCallbackPort, false));
        }

        let redirect_uri = format!("http://localhost:{callback_port}{CALLBACK_PATH}");
        let code_verifier = random_urlsafe::<64>();
        let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
        let state = random_urlsafe::<32>();
        let pending = OAuthPendingSession {
            redirect_uri,
            state,
            code_verifier,
            created_at_ms: now_ms,
        };

        let mut authorization_url = self.authorize_endpoint.clone();
        authorization_url
            .query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", CODEX_OAUTH_CLIENT_ID)
            .append_pair("redirect_uri", &pending.redirect_uri)
            .append_pair("scope", CODEX_OAUTH_SCOPE)
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true")
            .append_pair("originator", CODEX_OAUTH_ORIGINATOR)
            .append_pair("state", &pending.state);

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
        let response = self
            .http
            .post(self.token_endpoint.clone())
            .form(&AuthorizationCodeRequest {
                grant_type: "authorization_code",
                code: callback.code(),
                redirect_uri: &pending.redirect_uri,
                client_id: CODEX_OAUTH_CLIENT_ID,
                code_verifier: &pending.code_verifier,
            })
            .send()
            .await
            .map_err(|_| OAuthError::new(OAuthErrorCode::Transport, true))?;
        let status = response.status();
        let body = collect_limited(response, MAX_TOKEN_RESPONSE_BYTES)
            .await
            .map_err(|error| match error {
                LimitedBodyError::Transport => OAuthError::new(OAuthErrorCode::Transport, true),
                LimitedBodyError::TooLarge => {
                    OAuthError::new(OAuthErrorCode::ResponseTooLarge, false)
                }
            })?;
        if !status.is_success() {
            return Err(OAuthError {
                code: OAuthErrorCode::TokenEndpointRejected,
                provider_code: token_refresh_provider_error_code(&body),
                http_status: Some(status.as_u16()),
                retryable: status.is_server_error() || status.as_u16() == 429,
            });
        }

        parse_token_response(&body, now_ms)
    }

    pub async fn exchange_refresh_token(
        &self,
        refresh_token: &str,
        now_ms: u64,
    ) -> Result<OAuthTokenSet, TokenRefreshFailure> {
        validate_token(refresh_token).map_err(|_| {
            TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "invalid_refresh_token")
        })?;
        let response = self
            .http
            .post(self.token_endpoint.clone())
            .json(&RefreshTokenRequest {
                client_id: CODEX_OAUTH_CLIENT_ID,
                grant_type: "refresh_token",
                refresh_token,
            })
            .send()
            .await
            .map_err(|_| {
                TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "transport")
            })?;
        let status = response.status();
        let body = collect_limited(response, MAX_TOKEN_RESPONSE_BYTES)
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
        if !status.is_success() {
            let code = token_refresh_provider_error_code(&body)
                .unwrap_or_else(|| "token_refresh_failed".into());
            return Err(TokenRefreshFailure::new(
                token_refresh_failure_kind(&code),
                &code,
            ));
        }

        parse_token_response(&body, now_ms).map_err(|_| {
            TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "invalid_response")
        })
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

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthPendingSession {
    redirect_uri: String,
    state: String,
    code_verifier: String,
    created_at_ms: u64,
}

impl OAuthPendingSession {
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub fn created_at_ms(&self) -> u64 {
        self.created_at_ms
    }

    pub fn expires_at_ms(&self) -> u64 {
        self.created_at_ms.saturating_add(PENDING_TTL_MS)
    }

    pub fn parse_callback(
        &self,
        callback_url: &str,
        now_ms: u64,
    ) -> Result<OAuthCallback, OAuthError> {
        if callback_url.len() > MAX_CALLBACK_URL_BYTES {
            return Err(OAuthError::new(OAuthErrorCode::InvalidCallback, false));
        }
        if now_ms.saturating_sub(self.created_at_ms) > PENDING_TTL_MS {
            return Err(OAuthError::new(OAuthErrorCode::ExpiredCallback, false));
        }
        let callback_url = Url::parse(callback_url)
            .map_err(|_| OAuthError::new(OAuthErrorCode::InvalidCallback, false))?;
        let expected = Url::parse(&self.redirect_uri)
            .map_err(|_| OAuthError::new(OAuthErrorCode::InvalidConfiguration, false))?;
        if callback_url.scheme() != expected.scheme()
            || callback_url.host_str() != expected.host_str()
            || callback_url.port_or_known_default() != expected.port_or_known_default()
            || callback_url.path() != expected.path()
            || !callback_url.username().is_empty()
            || callback_url.password().is_some()
            || callback_url.fragment().is_some()
        {
            return Err(OAuthError::new(OAuthErrorCode::InvalidCallback, false));
        }

        let mut code = None;
        let mut state = None;
        let mut provider_error = None;
        for (key, value) in callback_url.query_pairs() {
            match key.as_ref() {
                "code" => set_once(&mut code, value.into_owned())?,
                "state" => set_once(&mut state, value.into_owned())?,
                "error" => set_once(&mut provider_error, value.into_owned())?,
                _ => {}
            }
        }
        if state.as_deref() != Some(self.state.as_str()) {
            return Err(OAuthError::new(OAuthErrorCode::StateMismatch, false));
        }
        if let Some(provider_error) = provider_error {
            return Err(OAuthError {
                code: OAuthErrorCode::AuthorizationDenied,
                provider_code: normalize_error_code(&provider_error),
                http_status: None,
                retryable: false,
            });
        }
        let code = code
            .filter(|code| !code.trim().is_empty() && code.len() <= MAX_TOKEN_BYTES)
            .ok_or_else(|| OAuthError::new(OAuthErrorCode::MissingAuthorizationCode, false))?;
        Ok(OAuthCallback { code })
    }
}

impl fmt::Debug for OAuthPendingSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthPendingSession")
            .field("redirect_uri", &self.redirect_uri)
            .field("state", &"[redacted]")
            .field("code_verifier", &"[redacted]")
            .field("created_at_ms", &self.created_at_ms)
            .finish()
    }
}

pub struct OAuthStart {
    authorization_url: Url,
    pending: OAuthPendingSession,
}

impl OAuthStart {
    pub fn authorization_url(&self) -> &Url {
        &self.authorization_url
    }

    #[cfg(test)]
    pub fn pending(&self) -> &OAuthPendingSession {
        &self.pending
    }

    pub fn into_pending(self) -> OAuthPendingSession {
        self.pending
    }
}

impl fmt::Debug for OAuthStart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthStart")
            .field("authorization_endpoint", &self.authorization_url.path())
            .field("pending", &self.pending)
            .finish()
    }
}

pub struct OAuthCallback {
    code: String,
}

impl OAuthCallback {
    fn code(&self) -> &str {
        &self.code
    }
}

impl fmt::Debug for OAuthCallback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthCallback")
            .field("code", &"[redacted]")
            .finish()
    }
}

pub struct OAuthTokenSet {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_at_ms: Option<u64>,
}

impl OAuthTokenSet {
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    pub fn id_token(&self) -> Option<&str> {
        self.id_token.as_deref()
    }

    pub fn expires_at_ms(&self) -> Option<u64> {
        self.expires_at_ms
    }

    pub fn identity_claims(&self) -> Result<Option<OAuthIdentityClaims>, OAuthError> {
        self.id_token
            .as_deref()
            .map(parse_identity_claims)
            .transpose()
    }

    pub fn into_secret_parts(self) -> (String, Option<String>, Option<String>, Option<u64>) {
        (
            self.access_token,
            self.refresh_token,
            self.id_token,
            self.expires_at_ms,
        )
    }
}

impl fmt::Debug for OAuthTokenSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthTokenSet")
            .field("access_token", &"[redacted]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "[redacted]"))
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct OAuthIdentityClaims {
    email: Option<String>,
    plan_type: Option<String>,
    subscription_active_until_ms: Option<u64>,
    user_id: Option<String>,
    account_id: Option<String>,
    account_is_fedramp: bool,
    expires_at_ms: Option<u64>,
}

impl OAuthIdentityClaims {
    pub fn email(&self) -> Option<&str> {
        self.email.as_deref()
    }

    pub fn plan_type(&self) -> Option<&str> {
        self.plan_type.as_deref()
    }

    pub fn subscription_active_until_ms(&self) -> Option<u64> {
        self.subscription_active_until_ms
    }

    pub fn user_id(&self) -> Option<&str> {
        self.user_id.as_deref()
    }

    pub fn account_id(&self) -> Option<&str> {
        self.account_id.as_deref()
    }

    pub fn account_is_fedramp(&self) -> bool {
        self.account_is_fedramp
    }
}

impl fmt::Debug for OAuthIdentityClaims {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthIdentityClaims")
            .field("email", &self.email.as_ref().map(|_| "[redacted]"))
            .field("plan_type", &self.plan_type)
            .field(
                "subscription_active_until_ms",
                &self.subscription_active_until_ms,
            )
            .field("user_id", &self.user_id.as_ref().map(|_| "[redacted]"))
            .field(
                "account_id",
                &self.account_id.as_ref().map(|_| "[redacted]"),
            )
            .field("account_is_fedramp", &self.account_is_fedramp)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OAuthErrorCode {
    AuthorizationDenied,
    ExpiredCallback,
    InvalidCallback,
    InvalidCallbackPort,
    InvalidConfiguration,
    InvalidJwt,
    InvalidResponse,
    MissingAuthorizationCode,
    ResponseTooLarge,
    StateMismatch,
    TokenEndpointRejected,
    Transport,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OAuthError {
    pub code: OAuthErrorCode,
    pub provider_code: Option<String>,
    pub http_status: Option<u16>,
    pub retryable: bool,
}

impl OAuthError {
    fn new(code: OAuthErrorCode, retryable: bool) -> Self {
        Self {
            code,
            provider_code: None,
            http_status: None,
            retryable,
        }
    }
}

impl fmt::Display for OAuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.code {
            OAuthErrorCode::AuthorizationDenied => "OAuth authorization was denied",
            OAuthErrorCode::ExpiredCallback => "OAuth callback expired",
            OAuthErrorCode::InvalidCallback => "OAuth callback is invalid",
            OAuthErrorCode::InvalidCallbackPort => "OAuth callback port is invalid",
            OAuthErrorCode::InvalidConfiguration => "OAuth configuration is invalid",
            OAuthErrorCode::InvalidJwt => "OAuth token claims are invalid",
            OAuthErrorCode::InvalidResponse => "OAuth token response is invalid",
            OAuthErrorCode::MissingAuthorizationCode => {
                "OAuth callback is missing an authorization code"
            }
            OAuthErrorCode::ResponseTooLarge => "OAuth response is too large",
            OAuthErrorCode::StateMismatch => "OAuth callback state does not match",
            OAuthErrorCode::TokenEndpointRejected => "OAuth token endpoint rejected the request",
            OAuthErrorCode::Transport => "OAuth request failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for OAuthError {}

#[derive(Serialize)]
struct AuthorizationCodeRequest<'a> {
    grant_type: &'static str,
    code: &'a str,
    redirect_uri: &'a str,
    client_id: &'static str,
    code_verifier: &'a str,
}

#[derive(Serialize)]
struct RefreshTokenRequest<'a> {
    client_id: &'static str,
    grant_type: &'static str,
    refresh_token: &'a str,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct JwtClaims {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    exp: Option<u64>,
    #[serde(rename = "https://api.openai.com/profile", default)]
    profile: Option<ProfileClaims>,
    #[serde(rename = "https://api.openai.com/auth", default)]
    auth: Option<AuthClaims>,
}

#[derive(Deserialize)]
struct ProfileClaims {
    #[serde(default)]
    email: Option<String>,
}

#[derive(Deserialize)]
struct AuthClaims {
    #[serde(default)]
    chatgpt_plan_type: Option<String>,
    #[serde(default)]
    chatgpt_subscription_active_until: Option<Value>,
    #[serde(default)]
    chatgpt_user_id: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    chatgpt_account_id: Option<String>,
    #[serde(default)]
    chatgpt_account_is_fedramp: bool,
}

fn parse_token_response(body: &[u8], now_ms: u64) -> Result<OAuthTokenSet, OAuthError> {
    let response: TokenResponse = serde_json::from_slice(body)
        .map_err(|_| OAuthError::new(OAuthErrorCode::InvalidResponse, false))?;
    let access_token = response
        .access_token
        .ok_or_else(|| OAuthError::new(OAuthErrorCode::InvalidResponse, false))?;
    validate_token(&access_token)?;
    validate_optional_token(response.refresh_token.as_deref())?;
    validate_optional_token(response.id_token.as_deref())?;
    let expires_at_ms = response
        .expires_in
        .map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1_000)))
        .or_else(|| jwt_expiration_ms(&access_token).ok().flatten());
    Ok(OAuthTokenSet {
        access_token,
        refresh_token: nonempty(response.refresh_token),
        id_token: nonempty(response.id_token),
        expires_at_ms,
    })
}

fn parse_identity_claims(jwt: &str) -> Result<OAuthIdentityClaims, OAuthError> {
    let claims: JwtClaims = decode_jwt_payload(jwt)?;
    let email = nonempty(claims.email).or_else(|| claims.profile.and_then(|profile| profile.email));
    let auth = claims.auth;
    Ok(OAuthIdentityClaims {
        email,
        plan_type: auth
            .as_ref()
            .and_then(|auth| nonempty(auth.chatgpt_plan_type.clone())),
        subscription_active_until_ms: auth
            .as_ref()
            .and_then(|auth| auth.chatgpt_subscription_active_until.as_ref())
            .and_then(zenith_relay_core::providers::chatgpt::parse_subscription_timestamp_ms),
        user_id: auth.as_ref().and_then(|auth| {
            nonempty(auth.chatgpt_user_id.clone()).or_else(|| nonempty(auth.user_id.clone()))
        }),
        account_id: auth
            .as_ref()
            .and_then(|auth| nonempty(auth.chatgpt_account_id.clone())),
        account_is_fedramp: auth
            .as_ref()
            .is_some_and(|auth| auth.chatgpt_account_is_fedramp),
        expires_at_ms: claims.exp.map(|seconds| seconds.saturating_mul(1_000)),
    })
}

fn jwt_expiration_ms(jwt: &str) -> Result<Option<u64>, OAuthError> {
    let claims: JwtClaims = decode_jwt_payload(jwt)?;
    Ok(claims.exp.map(|seconds| seconds.saturating_mul(1_000)))
}

fn decode_jwt_payload<T: for<'de> Deserialize<'de>>(jwt: &str) -> Result<T, OAuthError> {
    decode_unverified_jwt_payload(jwt)
        .ok_or_else(|| OAuthError::new(OAuthErrorCode::InvalidJwt, false))
}

fn validate_token(token: &str) -> Result<(), OAuthError> {
    if token.is_empty()
        || token.len() > MAX_TOKEN_BYTES
        || token.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(OAuthError::new(OAuthErrorCode::InvalidResponse, false))
    } else {
        Ok(())
    }
}

fn validate_optional_token(token: Option<&str>) -> Result<(), OAuthError> {
    match token {
        Some(token) => validate_token(token),
        None => Ok(()),
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn set_once(slot: &mut Option<String>, value: String) -> Result<(), OAuthError> {
    if slot.replace(value).is_some() {
        Err(OAuthError::new(OAuthErrorCode::InvalidCallback, false))
    } else {
        Ok(())
    }
}

fn random_urlsafe<const N: usize>() -> String {
    let mut bytes = [0_u8; N];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn authorization_url_uses_s256_and_callback_state_is_strict() {
        let client = CodexOAuthClient::new().unwrap();
        let start = client.begin(1455, 10_000).unwrap();
        let query: HashMap<_, _> = start
            .authorization_url()
            .query_pairs()
            .into_owned()
            .collect();

        assert_eq!(
            start.authorization_url().as_str().split('?').next(),
            Some("https://auth.openai.com/oauth/authorize")
        );
        assert_eq!(query.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(
            query.get("client_id").map(String::as_str),
            Some(CODEX_OAUTH_CLIENT_ID)
        );
        assert_eq!(
            query.get("scope").map(String::as_str),
            Some(CODEX_OAUTH_SCOPE)
        );
        assert_eq!(
            query.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(
            query.get("id_token_add_organizations").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            query.get("codex_cli_simplified_flow").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            query.get("originator").map(String::as_str),
            Some(CODEX_OAUTH_ORIGINATOR)
        );
        assert!(client.begin(1457, 10_000).is_ok());
        assert_eq!(
            client.begin(1456, 10_000).err().unwrap().code,
            OAuthErrorCode::InvalidCallbackPort
        );
        let expected_challenge =
            URL_SAFE_NO_PAD.encode(Sha256::digest(start.pending.code_verifier.as_bytes()));
        assert_eq!(
            query.get("code_challenge").map(String::as_str),
            Some(expected_challenge.as_str())
        );

        let callback = format!(
            "{}?code=authorization-secret&state={}",
            start.pending.redirect_uri, start.pending.state
        );
        assert!(start.pending().parse_callback(&callback, 10_001).is_ok());
        let error = start
            .pending()
            .parse_callback(
                "http://localhost:1455/auth/callback?code=authorization-secret&state=wrong-secret",
                10_001,
            )
            .unwrap_err();
        assert_eq!(error.code, OAuthErrorCode::StateMismatch);
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("authorization-secret"));
        assert!(!rendered.contains("wrong-secret"));
        assert_eq!(
            start
                .pending()
                .parse_callback(&callback, start.pending().expires_at_ms() + 1)
                .unwrap_err()
                .code,
            OAuthErrorCode::ExpiredCallback
        );
    }

    #[tokio::test]
    async fn code_exchange_uses_form_fields_and_extracts_bounded_claims() {
        let (base_url, server) = spawn(Router::new().route("/oauth/token", post(exchange))).await;
        let client = test_client(&base_url);
        let start = client.begin(1455, 1_000).unwrap();
        let callback = start
            .pending
            .parse_callback(
                &format!(
                    "{}?code=authorization-code&state={}",
                    start.pending.redirect_uri, start.pending.state
                ),
                1_001,
            )
            .unwrap();
        let tokens = client
            .exchange_code(&start.pending, callback, 2_000)
            .await
            .unwrap();

        assert_eq!(tokens.access_token(), "access-token");
        assert_eq!(tokens.refresh_token(), Some("refresh-token"));
        assert_eq!(tokens.expires_at_ms(), Some(3_602_000));
        let claims = tokens.identity_claims().unwrap().unwrap();
        assert_eq!(claims.email(), Some("user@example.test"));
        assert_eq!(claims.plan_type(), Some("pro"));
        assert_eq!(
            claims.subscription_active_until_ms(),
            Some(1_788_998_400_000)
        );
        assert_eq!(claims.account_id(), Some("account-123"));
        assert_eq!(claims.user_id(), Some("user-123"));
        let rendered = format!("{tokens:?} {claims:?}");
        assert!(!rendered.contains("access-token"));
        assert!(!rendered.contains("refresh-token"));
        assert!(!rendered.contains("user@example.test"));
        assert!(!rendered.contains("account-123"));
        server.abort();
    }

    #[tokio::test]
    async fn refresh_uses_json_and_classifies_reauth_without_secret_leaks() {
        let (base_url, server) = spawn(
            Router::new()
                .route("/oauth/token", post(refresh_without_rotation))
                .route("/oauth/fail", post(refresh_failure)),
        )
        .await;
        let client = test_client(&base_url);
        let refreshed = client
            .exchange_refresh_token("refresh-secret", 5_000)
            .await
            .unwrap();
        assert_eq!(refreshed.access_token(), "new-access-token");
        assert_eq!(refreshed.refresh_token(), None);
        assert_eq!(refreshed.id_token(), None);

        let failing = CodexOAuthClient::with_endpoints(
            base_url.join("oauth/authorize").unwrap(),
            base_url.join("oauth/fail").unwrap(),
        )
        .unwrap();
        let failure = failing
            .exchange_refresh_token("refresh-secret", 5_000)
            .await
            .unwrap_err();
        assert_eq!(failure.kind, TokenRefreshFailureKind::ReusedRefreshToken);
        assert_eq!(failure.code, "refresh_token_reused");
        let rendered = format!("{failure:?}");
        assert!(!rendered.contains("refresh-secret"));
        assert!(!rendered.contains("provider-body-secret"));
        server.abort();
    }

    #[test]
    fn oversized_jwt_payload_is_rejected() {
        let oversized = json!({ "padding": "x".repeat(16 * 1024 + 1) });
        let token = jwt(oversized);
        let error = parse_identity_claims(&token).unwrap_err();
        assert_eq!(error.code, OAuthErrorCode::InvalidJwt);
        assert!(!format!("{error:?}").contains(&"x".repeat(128)));
    }

    async fn exchange(headers: HeaderMap, body: Bytes) -> impl IntoResponse {
        assert!(headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/x-www-form-urlencoded")));
        let fields: HashMap<_, _> = url::form_urlencoded::parse(&body).into_owned().collect();
        assert_eq!(
            fields.get("grant_type").map(String::as_str),
            Some("authorization_code")
        );
        assert_eq!(
            fields.get("code").map(String::as_str),
            Some("authorization-code")
        );
        assert_eq!(
            fields.get("redirect_uri").map(String::as_str),
            Some("http://localhost:1455/auth/callback")
        );
        assert_eq!(
            fields.get("client_id").map(String::as_str),
            Some(CODEX_OAUTH_CLIENT_ID)
        );
        assert!(fields
            .get("code_verifier")
            .is_some_and(|value| value.len() >= 43));
        Json(json!({
            "access_token": "access-token",
            "refresh_token": "refresh-token",
            "id_token": jwt(json!({
                "email": "user@example.test",
                "exp": 4_000,
                "https://api.openai.com/auth": {
                    "chatgpt_plan_type": "pro",
                    "chatgpt_subscription_active_until": "2026-09-10T00:00:00Z",
                    "chatgpt_user_id": "user-123",
                    "chatgpt_account_id": "account-123"
                }
            })),
            "expires_in": 3_600
        }))
    }

    async fn refresh_without_rotation(headers: HeaderMap, body: Bytes) -> impl IntoResponse {
        assert_eq!(
            headers
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["client_id"], CODEX_OAUTH_CLIENT_ID);
        assert_eq!(body["grant_type"], "refresh_token");
        assert_eq!(body["refresh_token"], "refresh-secret");
        Json(json!({ "access_token": "new-access-token", "expires_in": 60 }))
    }

    async fn refresh_failure() -> impl IntoResponse {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "code": "refresh_token_reused",
                    "message": "provider-body-secret"
                }
            })),
        )
    }

    fn test_client(base_url: &Url) -> CodexOAuthClient {
        CodexOAuthClient::with_endpoints(
            base_url.join("oauth/authorize").unwrap(),
            base_url.join("oauth/token").unwrap(),
        )
        .unwrap()
    }

    fn jwt(payload: Value) -> String {
        format!(
            "{}.{}.signature",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
        )
    }

    async fn spawn(router: Router) -> (Url, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (Url::parse(&format!("http://{address}/")).unwrap(), server)
    }
}
