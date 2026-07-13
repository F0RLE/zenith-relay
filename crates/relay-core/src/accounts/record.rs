use crate::quota::{QuotaSnapshot, Subscription};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountAuthMode {
    OAuth,
    ApiKey,
    ImportedToken,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReauthReason {
    InvalidGrant,
    ReusedRefreshToken,
    ExpiredRefreshToken,
    InvalidatedRefreshToken,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "reason")]
pub enum AccountAuthState {
    #[default]
    Unknown,
    Active,
    DegradedAccessOnly,
    Refreshing,
    Error,
    RequiresReauth(ReauthReason),
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountHealthState {
    #[default]
    Unknown,
    Healthy,
    Degraded,
    Unhealthy,
    Blocked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountIdentity {
    pub stable_index: String,
    pub identity_hash: String,
    pub organization_hash: Option<String>,
}

impl AccountIdentity {
    pub fn from_hashed_parts(
        source_kind: &str,
        base_url_scope: &str,
        identity_hash: &str,
        secret_fingerprint_hash: &str,
        namespace: &str,
        organization_hash: Option<&str>,
    ) -> Result<Self, &'static str> {
        let source_kind = required(source_kind)?;
        let base_url_scope = required(base_url_scope)?;
        let identity_hash = required(identity_hash)?;
        let secret_fingerprint_hash = required(secret_fingerprint_hash)?;
        let namespace = required(namespace)?;
        let organization_hash = organization_hash
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let stable_index = format!(
            "{:x}",
            Sha256::digest(
                format!(
                    "{}\0{}\0{}\0{}\0{}",
                    source_kind.to_ascii_lowercase(),
                    base_url_scope.to_ascii_lowercase(),
                    identity_hash.to_ascii_lowercase(),
                    secret_fingerprint_hash.to_ascii_lowercase(),
                    namespace.to_ascii_lowercase(),
                )
                .as_bytes(),
            )
        );
        Ok(Self {
            stable_index,
            identity_hash: identity_hash.to_ascii_lowercase(),
            organization_hash,
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRecord {
    pub id: String,
    pub label: String,
    pub identity: AccountIdentity,
    pub auth_mode: AccountAuthMode,
    pub auth_state: AccountAuthState,
    pub health: AccountHealthState,
    pub source_id: String,
    pub secret_refs: Vec<String>,
    pub subscription: Subscription,
    pub quota: QuotaSnapshot,
    pub token_generation: u64,
    pub token_updated_at_ms: Option<u64>,
    pub tags: BTreeSet<String>,
    pub enabled: bool,
    #[serde(default)]
    pub in_pool: bool,
    pub draining: bool,
    pub created_at_ms: u64,
    pub last_used_at_ms: Option<u64>,
    pub last_error_code: Option<String>,
}

impl AccountRecord {
    pub fn is_wake_eligible(&self) -> bool {
        self.enabled
            && self.in_pool
            && !self.draining
            && self.auth_state == AccountAuthState::Active
            && self.health == AccountHealthState::Healthy
    }
}

fn required(value: &str) -> Result<&str, &'static str> {
    let value = value.trim();
    if value.is_empty() {
        Err("stable identity parts must not be empty")
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_identity_uses_only_hashed_inputs() {
        let first = AccountIdentity::from_hashed_parts(
            "openai",
            "api.example.test/v1",
            "email-hash",
            "secret-hash",
            "default",
            Some("org-hash"),
        )
        .unwrap();
        let second = AccountIdentity::from_hashed_parts(
            "OPENAI",
            "API.EXAMPLE.TEST/V1",
            "EMAIL-HASH",
            "SECRET-HASH",
            "DEFAULT",
            Some("ORG-HASH"),
        )
        .unwrap();

        assert_eq!(first, second);
        assert!(!format!("{first:?}").contains('@'));
        assert_eq!(first.stable_index.len(), 64);
    }
}
