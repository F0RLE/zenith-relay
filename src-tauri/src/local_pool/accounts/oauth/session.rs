use super::error::{OAuthError, OAuthErrorCode};
use super::exchange::OAuthCallback;
use super::parse::set_once;
use super::{MAX_CALLBACK_URL_BYTES, MAX_TOKEN_BYTES, PENDING_TTL_MS};
use serde::{Deserialize, Serialize};
use std::fmt;
use url::Url;
use zenith_relay_core::normalize_error_code;
use zenith_relay_core::url_has_userinfo;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthPendingSession {
    pub(super) redirect_uri: String,
    pub(super) state: String,
    pub(super) code_verifier: String,
    pub(super) created_at_ms: u64,
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
            || url_has_userinfo(&callback_url)
            || callback_url.fragment().is_some()
        {
            return Err(OAuthError::new(OAuthErrorCode::InvalidCallback, false));
        }

        let mut code = None;
        let mut callback_state = None;
        let mut provider_error = None;
        for (key, query_value) in callback_url.query_pairs() {
            match key.as_ref() {
                "code" => set_once(&mut code, query_value.into_owned())?,
                "state" => set_once(&mut callback_state, query_value.into_owned())?,
                "error" => set_once(&mut provider_error, query_value.into_owned())?,
                _ => {}
            }
        }
        if callback_state.as_deref() != Some(self.state.as_str()) {
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
