use super::error::OAuthError;
use super::identity::OAuthIdentityClaims;
use super::parse::parse_identity_claims;
use super::session::OAuthPendingSession;
use std::fmt;
use url::Url;

pub struct OAuthStart {
    pub(super) authorization_url: Url,
    pub(super) pending: OAuthPendingSession,
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
    pub(super) code: String,
}

impl OAuthCallback {
    pub(super) fn code(&self) -> &str {
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
    pub(super) access_token: String,
    pub(super) refresh_token: Option<String>,
    pub(super) id_token: Option<String>,
    pub(super) expires_at_ms: Option<u64>,
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
