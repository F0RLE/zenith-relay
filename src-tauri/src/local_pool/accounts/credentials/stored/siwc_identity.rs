use super::{CredentialError, CredentialErrorCode, StoredCodexCredentials, validate_token};
use crate::local_pool::accounts::oauth::{OAuthClientKind, siwc::SiwcRegistration};

impl StoredCodexCredentials {
    pub fn is_siwc_identity_only(&self) -> bool {
        self.oauth_client_kind == OAuthClientKind::Siwc
            && self
                .siwc
                .as_ref()
                .is_some_and(|registration| self.valid_siwc_identity_only(registration))
    }

    pub fn new_siwc_identity(
        local_account_id: &str,
        registration: SiwcRegistration,
        id_token: String,
        email: Option<String>,
        issued_at_ms: u64,
        generation: u64,
    ) -> Result<Self, CredentialError> {
        validate_token(&id_token)?;
        let mut credentials = Self::identity_record(
            local_account_id,
            issued_at_ms,
            generation,
            email,
            Some(registration.identity_key()),
            Some(registration.subject.clone()),
            None,
            None,
            false,
        )?;
        credentials.oauth_client_kind = OAuthClientKind::Siwc;
        credentials.id_token = Some(id_token);
        if !credentials.valid_siwc_identity_only(&registration) {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "SIWC identity-only connection cannot contain an inference or renewable grant",
            ));
        }
        credentials.with_siwc_registration(Some(registration))
    }

    pub(super) fn valid_siwc_identity_only(&self, registration: &SiwcRegistration) -> bool {
        self.access_token.is_empty()
            && self.refresh_token.is_none()
            && self.expires_at_ms.is_none()
            && self
                .id_token
                .as_deref()
                .is_some_and(|token| validate_token(token).is_ok())
            && !registration.scopes.iter().any(|scope| {
                matches!(
                    scope.as_str(),
                    "offline_access" | "chatgpt.tokens.use.direct"
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn identity() -> StoredCodexCredentials {
        StoredCodexCredentials::new_siwc_identity(
            "siwc_identity",
            SiwcRegistration {
                client_id: "oaiapp_synthetic".into(),
                host_id: "urn:uuid:synthetic-host".into(),
                issuer: "https://auth.openai.com".into(),
                subject: "synthetic-subject".into(),
                scopes: vec!["openid".into(), "profile".into(), "email".into()],
                earliest_refresh_at_ms: None,
                requires_reauthentication: false,
            },
            "synthetic-id-token".into(),
            Some("test@example.invalid".into()),
            1_000,
            7,
        )
        .unwrap()
    }

    #[test]
    fn siwc_identity_survives_storage_without_inference_or_renewal() {
        let saved = identity();
        let restored =
            StoredCodexCredentials::from_secret_json(&saved.to_secret_json().unwrap()).unwrap();
        assert!(restored.is_siwc_identity_only());
        assert!(!restored.has_oauth());
        assert!(restored.refresh_token().is_none());
        assert_eq!(restored.id_token(), Some("synthetic-id-token"));
        assert_eq!(restored.generation(), 7);
        assert!(restored.authorization(1_001).is_err());
        assert!(restored.to_token_set().is_err());
        assert!(!restored.is_access_usable(1_001, 0));
        assert!(
            crate::local_pool::accounts::records::validate_pool_account(&restored, false).is_err()
        );
        assert!(!format!("{restored:?}").contains("synthetic-id-token"));
    }

    #[test]
    fn siwc_identity_storage_rejects_forged_grant_or_identity_metadata() {
        let original: Value = serde_json::from_str(&identity().to_secret_json().unwrap()).unwrap();
        for (field, value) in [
            ("refreshToken", json!("unexpected-refresh")),
            ("idToken", Value::Null),
            ("expiresAtMs", json!(1_000)),
            ("providerAccountId", json!("foreign-account")),
            ("providerUserId", json!("foreign-subject")),
            ("oauthClientKind", json!("codex")),
        ] {
            let mut malformed = original.clone();
            malformed[field] = value;
            assert!(
                StoredCodexCredentials::from_secret_json(&malformed.to_string()).is_err(),
                "{field}"
            );
        }
        for scope in ["offline_access", "chatgpt.tokens.use.direct"] {
            let mut malformed = original.clone();
            malformed["siwc"]["scopes"] = json!(["openid", scope]);
            assert!(StoredCodexCredentials::from_secret_json(&malformed.to_string()).is_err());
        }
    }
}
