use std::fmt;

#[derive(Clone, Eq, PartialEq)]
pub struct OAuthIdentityClaims {
    pub(super) email: Option<String>,
    pub(super) plan_type: Option<String>,
    pub(super) subscription_active_until_ms: Option<u64>,
    pub(super) user_id: Option<String>,
    pub(super) account_id: Option<String>,
    pub(super) account_is_fedramp: bool,
    pub(super) expires_at_ms: Option<u64>,
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
