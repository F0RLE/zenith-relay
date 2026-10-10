use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const BASIS_POINTS_OAUTH_CLIENT_ID: &str = "app_fnr0pYvVwwFDocDumLG3H2Bp";
pub const BASIS_POINTS_OAUTH_REDIRECT_URI: &str =
    "https://bps.openai.com/basispoints/extension/360590d7-f8f9-4d88-bf75-0edfe0a4b9f3/auth/callback";

/// The client that issued a token set also owns its refresh flow.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthClientKind {
    #[default]
    Codex,
    #[serde(alias = "excel")]
    ExcelBps,
}

impl OAuthClientKind {
    pub const fn client_id(self) -> &'static str {
        match self {
            Self::Codex => CODEX_OAUTH_CLIENT_ID,
            Self::ExcelBps => BASIS_POINTS_OAUTH_CLIENT_ID,
        }
    }

    pub fn from_client_id(client_id: &str) -> Option<Self> {
        match client_id {
            CODEX_OAUTH_CLIENT_ID => Some(Self::Codex),
            BASIS_POINTS_OAUTH_CLIENT_ID => Some(Self::ExcelBps),
            _ => None,
        }
    }

    /// Issuing-client hints choose a refresh flow; they do not authenticate a JWT.
    pub fn from_token_hints(
        id_token: Option<&str>,
        access_token: Option<&str>,
    ) -> Result<Option<Self>, &'static str> {
        let mut selected = None;
        for (token, field) in [(id_token, "aud"), (access_token, "client_id")] {
            let Some(claims) =
                token.and_then(crate::accounts::decode_unverified_jwt_payload::<serde_json::Value>)
            else {
                continue;
            };
            let Some(value) = claims.get(field) else {
                continue;
            };
            for hint in std::iter::once(value)
                .chain(value.as_array().into_iter().flatten())
                .filter_map(serde_json::Value::as_str)
                .filter_map(Self::from_client_id)
            {
                if selected.is_some_and(|kind| kind != hint) {
                    return Err("OAuth token client hints conflict");
                }
                selected = Some(hint);
            }
        }
        Ok(selected)
    }

    pub fn validate_token_hints(
        self,
        id_token: Option<&str>,
        access_token: Option<&str>,
    ) -> Result<(), &'static str> {
        if Self::from_token_hints(id_token, access_token)?.is_some_and(|kind| kind != self) {
            return Err("OAuth tokens do not match the issuing client");
        }
        Ok(())
    }

    pub const fn authorize_path(self) -> &'static str {
        match self {
            Self::Codex => "/oauth/authorize",
            Self::ExcelBps => "/api/accounts/authorize",
        }
    }

    pub const fn scope(self) -> &'static str {
        match self {
            Self::Codex => {
                "openid profile email offline_access api.connectors.read api.connectors.invoke"
            }
            Self::ExcelBps => "openid offline_access email profile organization.read",
        }
    }

    pub const fn is_local_callback(self) -> bool {
        matches!(self, Self::Codex)
    }

    /// Keep legacy Codex keys stable while separating token sets from another client.
    pub fn scope_identity_key(self, key: &str) -> String {
        match self {
            Self::Codex => key.to_string(),
            Self::ExcelBps => {
                let mut digest = Sha256::new();
                digest.update(self.client_id());
                digest.update(b"\0");
                digest.update(key);
                hex::encode(digest.finalize())
            }
        }
    }
}
