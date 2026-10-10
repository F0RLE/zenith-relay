use super::super::error::{CredentialError, CredentialErrorCode};
use super::super::wire::{CredentialWire, CREDENTIAL_VERSION, MAX_SECRET_JSON_BYTES};
use super::StoredCodexCredentials;
use zenith_relay_core::providers::chatgpt::{AgentIdentityCredential, OAuthClientKind};

impl StoredCodexCredentials {
    pub(in crate::local_pool::accounts::credentials) fn to_secret_json(
        &self,
    ) -> Result<String, CredentialError> {
        if self.oauth_client_kind() == OAuthClientKind::ExcelBps
            && (self.agent_identity().is_some() || !self.has_oauth())
        {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "Excel OAuth requires its own access token and cannot use Agent Identity",
            ));
        }
        let wire = CredentialWire::from(self);
        let secret_json = serde_json::to_string(&wire).map_err(|_| {
            CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "failed to encode stored ChatGPT credentials",
            )
        })?;
        if secret_json.len() > MAX_SECRET_JSON_BYTES {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "stored ChatGPT credentials exceed the size limit",
            ));
        }
        Ok(secret_json)
    }

    pub(in crate::local_pool::accounts::credentials) fn from_secret_json(
        secret_json: &str,
    ) -> Result<Self, CredentialError> {
        if secret_json.is_empty() || secret_json.len() > MAX_SECRET_JSON_BYTES {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "stored ChatGPT credentials are invalid",
            ));
        }
        let wire: CredentialWire = serde_json::from_str(secret_json).map_err(|_| {
            CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "stored ChatGPT credentials are invalid",
            )
        })?;
        if wire.version != CREDENTIAL_VERSION {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidVersion,
                "stored ChatGPT credential version is unsupported",
            ));
        }
        if wire.oauth_client_kind == OAuthClientKind::ExcelBps
            && (wire.agent_identity.is_some() || wire.access_token.is_empty())
        {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "Excel OAuth requires its own access token and cannot use Agent Identity",
            ));
        }
        let agent_identity = wire
            .agent_identity
            .map(|agent| match agent.task_id {
                Some(task_id) => {
                    AgentIdentityCredential::new(agent.private_key, agent.runtime_id, task_id)
                }
                None => AgentIdentityCredential::unregistered(agent.private_key, agent.runtime_id),
            })
            .transpose()
            .map_err(|_| {
                CredentialError::new(
                    CredentialErrorCode::InvalidSecret,
                    "stored Agent Identity credential is invalid",
                )
            })?;
        let credentials = if wire.access_token.is_empty() {
            Self::new_agent_identity(
                &wire.local_account_id,
                agent_identity.ok_or_else(|| {
                    CredentialError::new(
                        CredentialErrorCode::InvalidSecret,
                        "stored ChatGPT credential has no authorization method",
                    )
                })?,
                wire.issued_at_ms,
                wire.generation,
                wire.email,
                wire.provider_account_id,
                wire.provider_user_id,
                wire.organization_id,
                wire.plan_type,
                wire.account_is_fedramp,
            )?
        } else {
            let credentials = Self::new(
                &wire.local_account_id,
                wire.access_token,
                wire.refresh_token,
                wire.id_token,
                wire.expires_at_ms,
                wire.issued_at_ms,
                wire.generation,
                wire.email,
                wire.provider_account_id,
                wire.provider_user_id,
                wire.organization_id,
                wire.plan_type,
                wire.account_is_fedramp,
            )?;
            match agent_identity {
                Some(agent_identity) => credentials.with_agent_identity(agent_identity),
                None => credentials,
            }
        };
        let basis_points_headers = wire.basis_points_headers;
        credentials
            .with_oauth_client_kind(wire.oauth_client_kind)
            .with_proxy_route(wire.proxy_url, wire.bypass_common_proxy)
            .map(|credentials| {
                credentials.apply_stored_login_material(wire.phone, wire.password, wire.totp_secret)
            })
            .and_then(|credentials| credentials.with_basis_points_headers(basis_points_headers))
    }
}
