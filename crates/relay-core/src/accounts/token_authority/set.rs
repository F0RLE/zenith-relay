use std::fmt;

#[derive(Clone, Eq, PartialEq)]
pub struct TokenSet {
    pub(in crate::accounts::token_authority) access_token: String,
    pub(in crate::accounts::token_authority) refresh_token: Option<String>,
    pub(in crate::accounts::token_authority) id_token: Option<String>,
    pub(in crate::accounts::token_authority) expires_at_ms: Option<u64>,
    pub(in crate::accounts::token_authority) issued_at_ms: u64,
    pub(in crate::accounts::token_authority) generation: u64,
}

/// Refresh an access token this long before it expires.
pub const TOKEN_REFRESH_SKEW_MS: u64 = 60_000;

/// Returns whether an access token remains usable after the refresh skew.
pub fn access_token_is_usable(
    expires_at_ms: Option<u64>,
    now_ms: u64,
    refresh_skew_ms: u64,
) -> bool {
    expires_at_ms.is_none_or(|expires_at| expires_at > now_ms.saturating_add(refresh_skew_ms))
}

impl TokenSet {
    pub fn new(
        access_token: impl Into<String>,
        refresh_token: Option<String>,
        id_token: Option<String>,
        expires_at_ms: Option<u64>,
        issued_at_ms: u64,
        generation: u64,
    ) -> Result<Self, &'static str> {
        let access_token = access_token.into();
        if access_token.trim().is_empty() {
            return Err("access token must not be empty");
        }
        Ok(Self {
            access_token,
            refresh_token: crate::omit_blank(refresh_token),
            id_token: crate::omit_blank(id_token),
            expires_at_ms,
            issued_at_ms,
            generation,
        })
    }

    pub fn access_only(
        access_token: impl Into<String>,
        expires_at_ms: Option<u64>,
        issued_at_ms: u64,
    ) -> Result<Self, &'static str> {
        Self::new(access_token, None, None, expires_at_ms, issued_at_ms, 0)
    }

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

    pub fn issued_at_ms(&self) -> u64 {
        self.issued_at_ms
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_access_usable(&self, now_ms: u64, refresh_skew_ms: u64) -> bool {
        access_token_is_usable(self.expires_at_ms, now_ms, refresh_skew_ms)
    }

    pub fn refresh_eligible(&self, now_ms: u64, refresh_skew_ms: u64) -> bool {
        self.refresh_token.is_some() && !self.is_access_usable(now_ms, refresh_skew_ms)
    }
}

impl fmt::Debug for TokenSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenSet")
            .field("access_token", &"[redacted]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "[redacted]"))
            .field("expires_at_ms", &self.expires_at_ms)
            .field("issued_at_ms", &self.issued_at_ms)
            .field("generation", &self.generation)
            .finish()
    }
}

#[derive(Clone)]
pub struct TokenRefresh {
    pub(in crate::accounts::token_authority) access_token: String,
    pub(in crate::accounts::token_authority) refresh_token: Option<String>,
    pub(in crate::accounts::token_authority) id_token: Option<String>,
    pub(in crate::accounts::token_authority) expires_at_ms: Option<u64>,
}

impl TokenRefresh {
    pub fn new(
        access_token: impl Into<String>,
        refresh_token: Option<String>,
        id_token: Option<String>,
        expires_at_ms: Option<u64>,
    ) -> Result<Self, &'static str> {
        let access_token = access_token.into();
        if access_token.trim().is_empty() {
            return Err("refreshed access token must not be empty");
        }
        Ok(Self {
            access_token,
            refresh_token: crate::omit_blank(refresh_token),
            id_token: crate::omit_blank(id_token),
            expires_at_ms,
        })
    }
}

impl fmt::Debug for TokenRefresh {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenRefresh")
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
