use super::profile::{
    remote_object_path, validate_profile_credential, validate_profile_key_rotation,
};
use super::usage_query::usage_path;
use super::{decode_success_body, RemoteClient, RemoteClientError, RemoteProfileCredential};
use reqwest::Method;
use zenith_relay_core::accounts::{AccountExportDocument, AccountExportRequest};
use zenith_relay_core::protocol::{
    negotiate, Capabilities, ClientProtocolRange, ConfigurationPresetApplyInput,
    ConfigurationPresetApplyResult, ConfigurationPresetDocument, ConfigurationPresetPreview,
    ConfigurationPresetPreviewInput, GatewayDiagnostic, HealthResponse, NegotiatedProtocol,
    ProfileKeyRotation, RevealedAccountIdentity, RuntimeStateSnapshot, UsagePage, UsageQuery,
};
use zenith_relay_core::{CandidateRuntimeSnapshot, SourceProviderStats};

impl RemoteClient {
    pub async fn health(&self) -> Result<HealthResponse, RemoteClientError> {
        self.request(Method::GET, "/health", Option::<&()>::None, false)
            .await
    }

    pub async fn capabilities(&self) -> Result<Capabilities, RemoteClientError> {
        self.request(Method::GET, "/capabilities", Option::<&()>::None, true)
            .await
    }

    pub async fn negotiate(
        &self,
    ) -> Result<(HealthResponse, Capabilities, NegotiatedProtocol), RemoteClientError> {
        let health = self.health().await?;
        let capabilities = self.capabilities().await?;
        let negotiated = negotiate(ClientProtocolRange::default(), &capabilities)
            .map_err(|error| RemoteClientError::Protocol(error.to_string()))?;
        Ok((health, capabilities, negotiated))
    }

    pub async fn state(&self) -> Result<RuntimeStateSnapshot, RemoteClientError> {
        self.request(Method::GET, "/state", Option::<&()>::None, true)
            .await
    }

    pub async fn configuration_preset(
        &self,
    ) -> Result<ConfigurationPresetDocument, RemoteClientError> {
        self.request(
            Method::GET,
            "/configuration/preset",
            Option::<&()>::None,
            true,
        )
        .await
    }

    pub async fn preview_configuration_preset(
        &self,
        input: &ConfigurationPresetPreviewInput,
    ) -> Result<ConfigurationPresetPreview, RemoteClientError> {
        self.request(
            Method::POST,
            "/configuration/preset/preview",
            Some(input),
            true,
        )
        .await
    }

    pub async fn apply_configuration_preset(
        &self,
        input: &ConfigurationPresetApplyInput,
    ) -> Result<ConfigurationPresetApplyResult, RemoteClientError> {
        self.request(
            Method::POST,
            "/configuration/preset/apply",
            Some(input),
            true,
        )
        .await
    }

    pub(crate) async fn profile_credential(
        &self,
    ) -> Result<RemoteProfileCredential, RemoteClientError> {
        let credential: RemoteProfileCredential = self
            .request(
                Method::GET,
                "/profile/credential",
                Option::<&()>::None,
                true,
            )
            .await?;
        validate_profile_credential(&self.origin, credential)
    }

    pub(crate) async fn prepare_profile_key_rotation(
        &self,
    ) -> Result<ProfileKeyRotation, RemoteClientError> {
        let rotation: ProfileKeyRotation = self
            .request(
                Method::POST,
                "/profile/credential/rotations",
                Option::<&()>::None,
                true,
            )
            .await?;
        validate_profile_key_rotation(&self.origin, rotation)
    }

    pub(crate) async fn commit_profile_key_rotation(
        &self,
        rotation_id: &str,
    ) -> Result<(), RemoteClientError> {
        let response = self
            .mutate(
                Method::POST,
                &remote_object_path("profile/credential/rotations", rotation_id)?,
                None,
            )
            .await?;
        response
            .is_null()
            .then_some(())
            .ok_or(RemoteClientError::InvalidResponse)
    }

    pub(crate) async fn abort_profile_key_rotation(
        &self,
        rotation_id: &str,
    ) -> Result<(), RemoteClientError> {
        let response = self
            .mutate(
                Method::DELETE,
                &remote_object_path("profile/credential/rotations", rotation_id)?,
                None,
            )
            .await?;
        response
            .is_null()
            .then_some(())
            .ok_or(RemoteClientError::InvalidResponse)
    }

    pub async fn runtime_order(&self) -> Result<Vec<CandidateRuntimeSnapshot>, RemoteClientError> {
        self.request(Method::GET, "/routing/runtime", Option::<&()>::None, true)
            .await
    }

    pub async fn usage(&self, query: &UsageQuery) -> Result<UsagePage, RemoteClientError> {
        let path = usage_path(query);
        self.request(Method::GET, &path, Option::<&()>::None, true)
            .await
    }

    pub async fn source_stats(
        &self,
        source_id: &str,
        force: bool,
    ) -> Result<SourceProviderStats, RemoteClientError> {
        self.request(
            Method::GET,
            &format!(
                "{}/stats{}",
                remote_object_path("sources", source_id)?,
                if force { "?force=true" } else { "" }
            ),
            Option::<&()>::None,
            true,
        )
        .await
    }

    pub async fn diagnose(&self, stream: bool) -> Result<GatewayDiagnostic, RemoteClientError> {
        self.request(
            Method::POST,
            "/diagnostics",
            Some(&serde_json::json!({ "stream": stream })),
            true,
        )
        .await
    }

    pub async fn export_accounts(
        &self,
        input: &AccountExportRequest,
    ) -> Result<AccountExportDocument, RemoteClientError> {
        input
            .validate()
            .map_err(|error| RemoteClientError::Protocol(error.to_string()))?;
        let document: AccountExportDocument = self
            .request(Method::POST, "/accounts/export", Some(input), true)
            .await?;
        document
            .validate()
            .map_err(|_| RemoteClientError::InvalidResponse)?;
        Ok(document)
    }

    pub async fn reveal_account_identity(
        &self,
        account_id: &str,
    ) -> Result<RevealedAccountIdentity, RemoteClientError> {
        let identity: RevealedAccountIdentity = self
            .request(
                Method::POST,
                &format!("/accounts/{account_id}/identity/reveal"),
                Option::<&()>::None,
                true,
            )
            .await?;
        if identity.account_id != account_id
            || identity.identity.is_empty()
            || identity.identity.len() > 512
            || identity
                .identity
                .bytes()
                .any(|byte| byte.is_ascii_control())
        {
            return Err(RemoteClientError::InvalidResponse);
        }
        Ok(identity)
    }

    pub async fn mutate(
        &self,
        method: Method,
        path: &str,
        input: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, RemoteClientError> {
        let url = self.origin.endpoint(path)?;
        let mut request = self.http.request(method, url).bearer_auth(&self.token);
        if let Some(input) = input {
            request = request.json(input);
        }
        let response = request
            .send()
            .await
            .map_err(|_| RemoteClientError::Transport)?;
        if response.status().is_redirection() {
            return Err(RemoteClientError::RedirectRejected);
        }
        if !response.status().is_success() {
            if path == "/routing/settings" && matches!(response.status().as_u16(), 400 | 409) {
                return Err(pool_routing_error(response).await);
            }
            return Err(RemoteClientError::HttpStatus(response.status().as_u16()));
        }
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(serde_json::Value::Null);
        }
        decode_success_body(response).await
    }
}

async fn pool_routing_error(mut response: reqwest::Response) -> RemoteClientError {
    let fallback = RemoteClientError::HttpStatus(response.status().as_u16());
    let mut bytes = Vec::new();
    // Older servers return 400 for this conflict. Inspect only a bounded code
    // envelope; never propagate the server's message or raw body to diagnostics.
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => return fallback,
        };
        if bytes.len() + chunk.len() > 4096 {
            return fallback;
        }
        bytes.extend_from_slice(&chunk);
    }
    let body: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => return fallback,
    };
    if body
        .pointer("/error/code")
        .and_then(serde_json::Value::as_str)
        == Some(zenith_relay_core::error_codes::POOL_ROUTING_CONFLICT)
    {
        RemoteClientError::PoolRoutingConflict
    } else {
        fallback
    }
}
