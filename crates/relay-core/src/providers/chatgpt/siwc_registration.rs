use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

pub const SIWC_ISSUER: &str = "https://auth.openai.com";

/// A dynamic registration, never a Codex workspace or account-id header.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SiwcRegistration {
    pub client_id: String,
    pub host_id: String,
    pub issuer: String,
    pub subject: String,
    pub scopes: Vec<String>,
    pub earliest_refresh_at_ms: Option<u64>,
    #[serde(default)]
    pub requires_reauthentication: bool,
}

impl SiwcRegistration {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !valid_siwc_client_id(&self.client_id)
            || self.issuer != SIWC_ISSUER
            || self.host_id.is_empty()
            || self.host_id.len() > 256
            || self.subject.is_empty()
            || self.subject.len() > 256
            || self.host_id.chars().any(char::is_control)
            || self.subject.chars().any(char::is_control)
            || self.scopes.len() > 64
            || self.scopes.iter().any(|scope| {
                scope.is_empty()
                    || scope.len() > 256
                    || scope.chars().any(|c| c.is_whitespace() || c.is_control())
            })
        {
            return Err("SIWC registration is invalid");
        }
        Ok(())
    }

    pub fn plan_use_enabled(&self) -> bool {
        !self.requires_reauthentication
            && ["resource.invoke", "chatgpt.tokens.use.direct"]
                .iter()
                .all(|required| self.scopes.iter().any(|scope| scope == required))
    }

    pub fn identity_key(&self) -> String {
        let mut digest = Sha256::new();
        for part in [&self.issuer, &self.client_id, &self.subject] {
            digest.update(part.as_bytes());
            digest.update(b"\0");
        }
        hex::encode(digest.finalize())
    }
}

pub fn valid_siwc_client_id(value: &str) -> bool {
    value != super::SIWC_BOOTSTRAP_CLIENT_ID
        && super::OAuthClientKind::from_client_id(value).is_none()
        && crate::is_ascii_token(value, 256)
}

impl fmt::Debug for SiwcRegistration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SiwcRegistration")
            .field("identity", &"[redacted]")
            .field("plan_use_enabled", &self.plan_use_enabled())
            .field("requires_reauthentication", &self.requires_reauthentication)
            .finish()
    }
}
