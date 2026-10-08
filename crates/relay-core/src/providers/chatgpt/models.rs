use super::{is_agent_identity_task_invalid_response, CodexIdentityEnvelope};
use crate::scheduler::refresh::http::{HttpClass, ManagementHttpScope};
use crate::{transport::collect_limited, Error, ProxyConfig};
use reqwest::{
    header::{HeaderValue, AUTHORIZATION},
    redirect::Policy,
};
use std::time::Duration;
use url::Url;

mod failure;
mod parse;
pub use failure::{ModelDiscoveryFailure, ModelDiscoveryFailureCode};

pub const CODEX_MODELS_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/models";

const MAX_ACCOUNT_ID_BYTES: usize = 512;
const MAX_MODEL_SLUG_BYTES: usize = 256;
const MAX_MODELS: usize = 4_096;
const MAX_MODELS_RESPONSE_BYTES: usize = crate::transport::MAX_MODEL_CATALOG_BODY_BYTES;

#[derive(Clone)]
pub struct CodexModelsClient {
    http: reqwest::Client,
    endpoint: Url,
    scope: ManagementHttpScope,
}

impl CodexModelsClient {
    pub fn new_with_proxy(proxy: Option<&ProxyConfig>) -> Result<Self, ModelDiscoveryFailure> {
        Self::new_with_proxy_and_timeout_and_user_agent(
            proxy,
            Duration::from_secs(10),
            "Zenith Relay",
        )
    }

    pub fn new_with_proxy_and_timeout_and_user_agent(
        proxy: Option<&ProxyConfig>,
        request_timeout: Duration,
        user_agent: &'static str,
    ) -> Result<Self, ModelDiscoveryFailure> {
        let endpoint = Url::parse(CODEX_MODELS_ENDPOINT)
            .map_err(|_| ModelDiscoveryFailure::new(ModelDiscoveryFailureCode::InvalidEndpoint))?;
        Self::with_endpoint_proxy_timeout_and_user_agent(
            endpoint,
            proxy,
            request_timeout,
            user_agent,
        )
    }

    #[cfg(test)]
    pub fn with_endpoint(endpoint: Url) -> Result<Self, ModelDiscoveryFailure> {
        Self::with_endpoint_proxy_timeout_and_user_agent(
            endpoint,
            None,
            Duration::from_secs(10),
            "Zenith Relay",
        )
    }

    fn with_endpoint_proxy_timeout_and_user_agent(
        endpoint: Url,
        proxy: Option<&ProxyConfig>,
        request_timeout: Duration,
        user_agent: &'static str,
    ) -> Result<Self, ModelDiscoveryFailure> {
        parse::validate_endpoint(&endpoint)?;
        let builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(request_timeout)
            .user_agent(user_agent);
        let http = match proxy {
            Some(proxy) => proxy.apply(builder),
            None => builder,
        }
        .build()
        .map_err(|_| ModelDiscoveryFailure::new(ModelDiscoveryFailureCode::InvalidEndpoint))?;
        Ok(Self {
            http,
            endpoint,
            scope: ManagementHttpScope::default(),
        })
    }

    pub fn with_http_scope(mut self, scope: ManagementHttpScope) -> Self {
        self.scope = scope;
        self
    }

    pub async fn discover(
        &self,
        access_token: &str,
        chatgpt_account_id: &str,
        client_version: &str,
    ) -> Result<Vec<String>, ModelDiscoveryFailure> {
        parse::validate_access_token(access_token)?;
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {access_token}")).map_err(|_| {
                ModelDiscoveryFailure::new(ModelDiscoveryFailureCode::InvalidAccessToken)
            })?;
        authorization.set_sensitive(true);
        self.discover_authorized(authorization, chatgpt_account_id, client_version)
            .await
    }

    pub async fn discover_authorized(
        &self,
        authorization: HeaderValue,
        chatgpt_account_id: &str,
        client_version: &str,
    ) -> Result<Vec<String>, ModelDiscoveryFailure> {
        parse::validate_account_id(chatgpt_account_id)?;
        parse::validate_client_version(client_version)?;
        let identity = CodexIdentityEnvelope::new(chatgpt_account_id, client_version)
            .map_err(|_| ModelDiscoveryFailure::new(ModelDiscoveryFailureCode::InvalidAccountId))?;
        let mut request_url = self.endpoint.clone();
        request_url
            .query_pairs_mut()
            .append_pair("client_version", client_version);
        let (response, permit) = self
            .scope
            .send(
                &self.http,
                identity.apply(
                    self.http
                        .get(request_url)
                        .header(AUTHORIZATION, authorization),
                ),
                HttpClass::Ordinary,
            )
            .await
            .map_err(|_| ModelDiscoveryFailure::retryable(ModelDiscoveryFailureCode::Transport))?;
        let status = response.status();
        let retry_after_ms =
            crate::transport::retry_after_ms(response.headers(), std::time::SystemTime::now());
        let models_response_body = collect_limited(response, MAX_MODELS_RESPONSE_BYTES)
            .await
            .map_err(|error| match error {
                Error::UpstreamBodyTooLarge => {
                    ModelDiscoveryFailure::new(ModelDiscoveryFailureCode::ResponseTooLarge)
                }
                _ => ModelDiscoveryFailure::retryable(ModelDiscoveryFailureCode::Transport),
            })
            .map_err(|mut failure| {
                failure.retry_after_ms = retry_after_ms;
                failure
            })?;
        drop(permit);
        if !status.is_success() {
            let (code, retryable) = if is_agent_identity_task_invalid_response(
                status.as_u16(),
                &models_response_body,
            ) {
                (ModelDiscoveryFailureCode::AgentTaskInvalid, false)
            } else {
                match status.as_u16() {
                    401 => (ModelDiscoveryFailureCode::Unauthorized, false),
                    403 => (ModelDiscoveryFailureCode::Forbidden, false),
                    429 => (ModelDiscoveryFailureCode::RateLimited, true),
                    _ if status.is_server_error() => (ModelDiscoveryFailureCode::Upstream, true),
                    _ => (ModelDiscoveryFailureCode::HttpStatus, false),
                }
            };
            return Err(ModelDiscoveryFailure {
                code,
                retryable,
                http_status: Some(status.as_u16()),
                retry_after_ms,
            });
        }

        parse::parse_models(&models_response_body)
    }
}

#[cfg(test)]
mod tests;
