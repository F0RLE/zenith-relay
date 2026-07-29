use super::origin::{OriginError, PinnedOrigin};
use reqwest::{header::LOCATION, Method};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{fmt, time::Duration};
use zenith_relay_core::accounts::{AccountExportDocument, AccountExportRequest};
use zenith_relay_core::protocol::{
    negotiate, Capabilities, ClientKeyCreateInput, ClientKeyPatch, ClientProtocolRange,
    ConfigurationPresetApplyInput, ConfigurationPresetApplyResult, ConfigurationPresetDocument,
    ConfigurationPresetPreview, ConfigurationPresetPreviewInput, GatewayDiagnostic,
    GeneratedClientKey, HealthResponse, KeySummary, NegotiatedProtocol, ProfileKeyRotation,
    RevealedAccountIdentity, RuntimeStateSnapshot, UsagePage, UsageQuery, UsageRange,
    CLIENT_ACCESS_SCHEMA_VERSION, PROFILE_KEY_ROTATION_SCHEMA_VERSION,
};
use zenith_relay_core::WireApi;
use zenith_relay_core::{CandidateRuntimeSnapshot, SourceProviderStats};

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub enum RemoteClientError {
    Origin(OriginError),
    InvalidToken,
    Transport,
    RedirectRejected,
    HttpStatus(u16),
    ResponseTooLarge,
    InvalidResponse,
    Protocol(String),
}

impl fmt::Display for RemoteClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Origin(error) => write!(formatter, "{error}"),
            Self::InvalidToken => formatter.write_str("remote management token is invalid"),
            Self::Transport => formatter.write_str("remote server request failed"),
            Self::RedirectRejected => formatter.write_str("remote server redirect was rejected"),
            Self::HttpStatus(status) => write!(formatter, "remote server returned HTTP {status}"),
            Self::ResponseTooLarge => formatter.write_str("remote server response is too large"),
            Self::InvalidResponse => formatter.write_str("remote server response is invalid"),
            Self::Protocol(error) => write!(formatter, "remote protocol is incompatible: {error}"),
        }
    }
}

impl std::error::Error for RemoteClientError {}

impl From<OriginError> for RemoteClientError {
    fn from(error: OriginError) -> Self {
        Self::Origin(error)
    }
}

pub struct RemoteClient {
    origin: PinnedOrigin,
    token: String,
    http: reqwest::Client,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteProfileCredential {
    pub key_id: String,
    pub base_url: String,
    pub secret: String,
}

impl RemoteClient {
    pub fn new(
        base_url: &str,
        token: &str,
        allow_insecure_http: bool,
    ) -> Result<Self, RemoteClientError> {
        validate_token(token)?;
        let origin = PinnedOrigin::parse(base_url, allow_insecure_http)?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .user_agent("Zenith Relay")
            .build()
            .map_err(|_| RemoteClientError::Transport)?;
        Ok(Self {
            origin,
            token: token.to_string(),
            http,
        })
    }

    pub fn origin(&self) -> &str {
        self.origin.as_str()
    }

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
    ) -> Result<SourceProviderStats, RemoteClientError> {
        self.request(
            Method::GET,
            &format!("{}/stats", remote_object_path("sources", source_id)?),
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

    pub async fn create_client_key(
        &self,
        input: &ClientKeyCreateInput,
    ) -> Result<GeneratedClientKey, RemoteClientError> {
        let generated = self
            .request(Method::POST, "/keys", Some(input), true)
            .await?;
        validate_generated_client_key(generated)
    }

    pub async fn update_client_key(
        &self,
        key_id: &str,
        input: &ClientKeyPatch,
    ) -> Result<KeySummary, RemoteClientError> {
        self.request(
            Method::PATCH,
            &remote_object_path("keys", key_id)?,
            Some(input),
            true,
        )
        .await
    }

    pub async fn rotate_client_key(
        &self,
        key_id: &str,
    ) -> Result<GeneratedClientKey, RemoteClientError> {
        let generated = self
            .request(
                Method::POST,
                &format!("{}/rotate", remote_object_path("keys", key_id)?),
                Option::<&()>::None,
                true,
            )
            .await?;
        validate_generated_client_key(generated)
    }

    pub async fn revoke_client_key(&self, key_id: &str) -> Result<(), RemoteClientError> {
        let response = self
            .mutate(Method::DELETE, &remote_object_path("keys", key_id)?, None)
            .await?;
        if response.is_null() {
            Ok(())
        } else {
            Err(RemoteClientError::InvalidResponse)
        }
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
            return Err(RemoteClientError::HttpStatus(response.status().as_u16()));
        }
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(serde_json::Value::Null);
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| RemoteClientError::Transport)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(RemoteClientError::ResponseTooLarge);
        }
        serde_json::from_slice(&bytes).map_err(|_| RemoteClientError::InvalidResponse)
    }

    async fn request<I: Serialize + ?Sized, O: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        input: Option<&I>,
        authenticated: bool,
    ) -> Result<O, RemoteClientError> {
        let url = self.origin.endpoint(path)?;
        let mut request = self.http.request(method, url);
        if authenticated {
            request = request.bearer_auth(&self.token);
        }
        if let Some(input) = input {
            request = request.json(input);
        }
        let response = request
            .send()
            .await
            .map_err(|_| RemoteClientError::Transport)?;
        if response.status().is_redirection() {
            let _ = response.headers().get(LOCATION);
            return Err(RemoteClientError::RedirectRejected);
        }
        if !response.status().is_success() {
            return Err(RemoteClientError::HttpStatus(response.status().as_u16()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| RemoteClientError::Transport)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(RemoteClientError::ResponseTooLarge);
        }
        serde_json::from_slice(&bytes).map_err(|_| RemoteClientError::InvalidResponse)
    }
}

fn validate_profile_credential(
    origin: &PinnedOrigin,
    credential: RemoteProfileCredential,
) -> Result<RemoteProfileCredential, RemoteClientError> {
    validate_profile_credential_fields(
        origin,
        &credential.key_id,
        &credential.base_url,
        &credential.secret,
    )?;
    Ok(credential)
}

fn validate_profile_key_rotation(
    origin: &PinnedOrigin,
    rotation: ProfileKeyRotation,
) -> Result<ProfileKeyRotation, RemoteClientError> {
    if rotation.schema_version != PROFILE_KEY_ROTATION_SCHEMA_VERSION
        || remote_object_path("profile/credential/rotations", &rotation.rotation_id).is_err()
    {
        return Err(RemoteClientError::InvalidResponse);
    }
    validate_profile_credential_fields(
        origin,
        &rotation.key_id,
        &rotation.base_url,
        &rotation.secret,
    )?;
    Ok(rotation)
}

fn validate_profile_credential_fields(
    origin: &PinnedOrigin,
    key_id: &str,
    base_url: &str,
    secret: &str,
) -> Result<(), RemoteClientError> {
    let expected_base_url = origin.endpoint("/v1")?;
    let actual_base_url =
        url::Url::parse(base_url).map_err(|_| RemoteClientError::InvalidResponse)?;
    if actual_base_url != expected_base_url
        || key_id.is_empty()
        || key_id.len() > 128
        || !key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || !secret.starts_with("zrs_")
        || secret.len() < 24
        || secret.len() > 256
        || secret.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(RemoteClientError::InvalidResponse);
    }
    Ok(())
}

fn validate_generated_client_key(
    generated: GeneratedClientKey,
) -> Result<GeneratedClientKey, RemoteClientError> {
    if generated.schema_version != CLIENT_ACCESS_SCHEMA_VERSION
        || !generated.secret.starts_with("zrs_")
        || generated.secret.len() < 24
        || generated.secret.len() > 256
        || generated.secret.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(RemoteClientError::InvalidResponse);
    }
    Ok(generated)
}

fn remote_object_path(collection: &str, id: &str) -> Result<String, RemoteClientError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(RemoteClientError::InvalidResponse);
    }
    Ok(format!("/{collection}/{id}"))
}

fn usage_path(query: &UsageQuery) -> String {
    let mut parameters = url::form_urlencoded::Serializer::new(String::new());
    parameters.append_pair("page", &query.page.max(1).to_string());
    parameters.append_pair(
        "pageSize",
        &if query.page_size == 0 {
            50
        } else {
            query.page_size.clamp(1, 200)
        }
        .to_string(),
    );
    if let Some(value) = query.range {
        parameters.append_pair("range", usage_range_name(value));
    }
    append_number(&mut parameters, "fromMs", query.from_ms);
    append_number(&mut parameters, "toMs", query.to_ms);
    append_number(&mut parameters, "bucketMs", query.bucket_ms);
    append_text(&mut parameters, "modelQuery", query.model_query.as_deref());
    append_text(
        &mut parameters,
        "sourceOrAccountQuery",
        query.source_or_account_query.as_deref(),
    );
    append_text(
        &mut parameters,
        "localKeyQuery",
        query.local_key_query.as_deref(),
    );
    if let Some(value) = query.wire_api {
        parameters.append_pair("wireApi", wire_api_name(value));
    }
    if let Some(value) = query.success {
        parameters.append_pair("success", if value { "true" } else { "false" });
    }
    append_text(
        &mut parameters,
        "errorCategory",
        query.error_category.as_deref(),
    );
    append_text(
        &mut parameters,
        "requestIdQuery",
        query.request_id_query.as_deref(),
    );
    format!("/usage?{}", parameters.finish())
}

fn append_text(
    parameters: &mut url::form_urlencoded::Serializer<'_, String>,
    name: &str,
    value: Option<&str>,
) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        parameters.append_pair(name, value);
    }
}

fn append_number(
    parameters: &mut url::form_urlencoded::Serializer<'_, String>,
    name: &str,
    value: Option<u64>,
) {
    if let Some(value) = value {
        parameters.append_pair(name, &value.to_string());
    }
}

fn usage_range_name(value: UsageRange) -> &'static str {
    match value {
        UsageRange::Daily => "daily",
        UsageRange::Weekly => "weekly",
        UsageRange::Monthly => "monthly",
        UsageRange::Custom => "custom",
    }
}

fn wire_api_name(value: WireApi) -> &'static str {
    match value {
        WireApi::Responses => "responses",
        WireApi::ChatCompletions => "chat_completions",
        WireApi::Messages => "messages",
    }
}

fn validate_token(token: &str) -> Result<(), RemoteClientError> {
    if token.len() < 24
        || token.len() > 8 * 1024
        || token.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(RemoteClientError::InvalidToken)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        http::{header::AUTHORIZATION, Uri},
        response::Redirect,
        routing::{get, post},
        Json, Router,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use tokio::sync::Notify;

    #[tokio::test]
    async fn redirect_is_not_followed_and_token_never_reaches_other_origin() {
        let received = Arc::new(AtomicUsize::new(0));
        let observed = received.clone();
        let target = spawn(Router::new().route(
            "/state",
            get(move |headers: axum::http::HeaderMap| {
                let observed = observed.clone();
                async move {
                    if headers.get(AUTHORIZATION).is_some() {
                        observed.fetch_add(1, Ordering::SeqCst);
                    }
                    "{}"
                }
            }),
        ))
        .await;
        let redirect_target = format!("{target}/state");
        let source = spawn(Router::new().route(
            "/state",
            get(move || {
                let redirect_target = redirect_target.clone();
                async move { Redirect::temporary(&redirect_target) }
            }),
        ))
        .await;
        let client = RemoteClient::new(&source, "synthetic-management-token-value", false).unwrap();
        assert!(matches!(
            client.state().await,
            Err(RemoteClientError::RedirectRejected)
        ));
        assert_eq!(received.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn usage_query_values_are_encoded_and_cannot_add_parameters() {
        let observed = Arc::new(Mutex::new(String::new()));
        let request_uri = observed.clone();
        let server = spawn(Router::new().route(
            "/usage",
            get(move |uri: Uri| {
                let request_uri = request_uri.clone();
                async move {
                    *request_uri.lock().unwrap() = uri.to_string();
                    Json(serde_json::json!({
                        "events": [], "total": 0, "page": 1, "pageSize": 25, "totalPages": 0
                    }))
                }
            }),
        ))
        .await;
        let client = RemoteClient::new(&server, "synthetic-management-token-value", false).unwrap();
        client
            .usage(&UsageQuery {
                page: 1,
                page_size: 25,
                bucket_ms: Some(60_000),
                model_query: Some("gpt test&success=false".to_string()),
                success: Some(true),
                ..UsageQuery::default()
            })
            .await
            .unwrap();
        let uri = observed.lock().unwrap().clone();
        assert!(uri.contains("modelQuery=gpt+test%26success%3Dfalse"));
        assert!(uri.contains("success=true"));
        assert!(uri.contains("bucketMs=60000"));
        assert!(!uri.contains("modelQuery=gpt+test&success=false"));
    }

    #[tokio::test]
    async fn in_flight_request_finishes_after_external_client_owner_is_dropped() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let handler_entered = entered.clone();
        let handler_release = release.clone();
        let server = spawn(Router::new().route(
            "/usage",
            get(move || {
                let entered = handler_entered.clone();
                let release = handler_release.clone();
                async move {
                    entered.notify_one();
                    release.notified().await;
                    Json(serde_json::json!({
                        "events": [], "total": 0, "page": 1, "pageSize": 25, "totalPages": 0
                    }))
                }
            }),
        ))
        .await;
        let owner = Arc::new(
            RemoteClient::new(&server, "synthetic-management-token-value", false).unwrap(),
        );
        let request_client = owner.clone();
        let request =
            tokio::spawn(async move { request_client.usage(&UsageQuery::default()).await });

        entered.notified().await;
        drop(owner);
        release.notify_one();

        assert_eq!(request.await.unwrap().unwrap().total, 0);
    }

    #[tokio::test]
    async fn identity_reveal_rejects_a_mismatched_account() {
        let server = spawn(Router::new().route(
            "/accounts/{id}/identity/reveal",
            post(|| async {
                Json(serde_json::json!({
                    "accountId": "different-account",
                    "identity": "private@example.test"
                }))
            }),
        ))
        .await;
        let client = RemoteClient::new(&server, "synthetic-management-token-value", false).unwrap();
        assert!(matches!(
            client.reveal_account_identity("account-1").await,
            Err(RemoteClientError::InvalidResponse)
        ));
    }

    #[test]
    fn profile_credential_cannot_redirect_codex_to_another_origin() {
        let origin = PinnedOrigin::parse("https://relay.example.test", false).unwrap();
        let credential = RemoteProfileCredential {
            key_id: "key_system".to_string(),
            base_url: "https://other.example.test/v1".to_string(),
            secret: format!("zrs_{}", "a".repeat(40)),
        };
        assert!(matches!(
            validate_profile_credential(&origin, credential),
            Err(RemoteClientError::InvalidResponse)
        ));
    }

    async fn spawn(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}")
    }
}
