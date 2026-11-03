use super::client::{response_format, subscription_headers, subscription_public_headers};
use crate::quota::QuotaRefreshFailure;
use crate::scheduler::refresh::http::{
    HttpAdmissionError, HttpClass, HttpSendError, ManagementHttpScope,
};
use reqwest::{Client, RequestBuilder, header::HeaderValue};
use serde::Serialize;
use std::time::{Duration, Instant};
use url::Url;

/// Creates a credential-free client bound only to the explicitly selected proxy.
pub async fn probe_subscription_proxy(
    proxy: &crate::ProxyConfig,
    scope: ManagementHttpScope,
) -> Result<Vec<SubscriptionEndpointProbe>, QuotaRefreshFailure> {
    Ok(
        super::CodexSubscriptionClient::new(diagnostic_client(proxy)?)?
            .with_http_scope(scope)
            .probe_anonymous()
            .await,
    )
}

/// Uses one saved route and the same query for the anonymous and bearer observations.
/// The account-check body is parsed transiently to validate the canonical identity.
pub async fn probe_subscription_authorization(
    proxy: &crate::ProxyConfig,
    scope: ManagementHttpScope,
    authorization: HeaderValue,
    preferred_account_id: &str,
) -> Result<Vec<SubscriptionAuthorizationProbe>, QuotaRefreshFailure> {
    let client =
        super::CodexSubscriptionClient::new(diagnostic_client(proxy)?)?.with_http_scope(scope);
    tokio::time::timeout(
        Duration::from_secs(60),
        client.probe_authorization(authorization, preferred_account_id),
    )
    .await
    .map_err(|_| QuotaRefreshFailure::new(crate::error_codes::SUBSCRIPTION_TRANSPORT, false))?
}

fn diagnostic_client(proxy: &crate::ProxyConfig) -> Result<Client, QuotaRefreshFailure> {
    proxy
        .apply(
            Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10)),
        )
        .build()
        .map_err(|_| {
            QuotaRefreshFailure::new(crate::error_codes::SUBSCRIPTION_CONFIGURATION, false)
        })
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionAuthorizationProbe {
    pub authentication: &'static str,
    pub observation: SubscriptionEndpointProbe,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionEndpointProbe {
    pub stage: &'static str,
    pub elapsed_ms: u64,
    pub http_status: Option<u16>,
    pub response_format: Option<&'static str>,
    pub response_mitigation: Option<&'static str>,
    pub cf_ray: Option<Box<str>>,
    pub retry_after_ms: Option<u64>,
    pub error_code: Option<&'static str>,
}

impl SubscriptionEndpointProbe {
    pub fn stops_probe(&self) -> bool {
        self.error_code.is_some()
            || self.http_status == Some(429)
            || self.retry_after_ms.is_some_and(|delay| delay > 0)
            || self.response_mitigation.is_some()
    }
}

pub(super) async fn compare_authorization(
    http: &Client,
    scope: &ManagementHttpScope,
    subscriptions: &Url,
    authorization: HeaderValue,
    account_id: &str,
) -> Result<Vec<SubscriptionAuthorizationProbe>, QuotaRefreshFailure> {
    let mut observations = Vec::with_capacity(2);
    for (authentication, credential) in [("none", None), ("bearer", Some(authorization))] {
        let headers = match credential {
            Some(credential) => subscription_headers(credential, "/backend-api/subscriptions")?,
            None => subscription_public_headers("/backend-api/subscriptions")?,
        };
        let request = http
            .get(subscriptions.clone())
            .query(&[("account_id", account_id)])
            .headers(headers);
        let observation = observe_request(http, scope, request, "subscriptions").await;
        let stop = observation.stops_probe();
        observations.push(SubscriptionAuthorizationProbe {
            authentication,
            observation,
        });
        if stop {
            break;
        }
    }
    Ok(observations)
}

pub(super) async fn probe(
    http: &Client,
    scope: &ManagementHttpScope,
    accounts_endpoint: &Url,
    subscriptions_endpoint: &Url,
) -> Vec<SubscriptionEndpointProbe> {
    let mut observations = Vec::with_capacity(2);
    for (stage, endpoint, target) in [
        (
            "accounts_check",
            accounts_endpoint,
            "/backend-api/accounts/check/v4-2023-04-27",
        ),
        (
            "subscriptions",
            subscriptions_endpoint,
            "/backend-api/subscriptions",
        ),
    ] {
        let mut observation = empty_observation(stage);
        let Ok(headers) = subscription_public_headers(target) else {
            observation.error_code = Some("configuration");
            observations.push(observation);
            break;
        };
        observation = observe_request(
            http,
            scope,
            http.get(endpoint.clone()).headers(headers),
            stage,
        )
        .await;
        let stop = observation.stops_probe();
        observations.push(observation);
        if stop {
            break;
        }
    }
    observations
}

fn empty_observation(stage: &'static str) -> SubscriptionEndpointProbe {
    SubscriptionEndpointProbe {
        stage,
        elapsed_ms: 0,
        http_status: None,
        response_format: None,
        response_mitigation: None,
        cf_ray: None,
        retry_after_ms: None,
        error_code: None,
    }
}

async fn observe_request(
    http: &Client,
    scope: &ManagementHttpScope,
    request: RequestBuilder,
    stage: &'static str,
) -> SubscriptionEndpointProbe {
    let started = Instant::now();
    let mut observation = empty_observation(stage);
    let sent = tokio::time::timeout(
        Duration::from_secs(20),
        scope.send(http, request, HttpClass::Ordinary),
    )
    .await;
    match sent {
        Ok(Ok((response, _permit))) => {
            observation.http_status = Some(response.status().as_u16());
            observation.response_format = Some(response_format(response.headers()));
            observation.response_mitigation = response
                .headers()
                .get("cf-mitigated")
                .is_some_and(|value| value == "challenge")
                .then_some("cloudflare_challenge");
            observation.cf_ray = QuotaRefreshFailure::validated_cf_ray(
                response
                    .headers()
                    .get("cf-ray")
                    .and_then(|value| value.to_str().ok()),
            );
            observation.retry_after_ms =
                crate::transport::retry_after_ms(response.headers(), std::time::SystemTime::now());
            // Bodies, including login/challenge HTML, are never collected.
        }
        Ok(Err(error)) => {
            observation.error_code = Some(match error {
                HttpSendError::Admission(HttpAdmissionError::TimedOut)
                | HttpSendError::Transport { timeout: true } => "timeout",
                HttpSendError::Admission(_) => "admission_unavailable",
                HttpSendError::Stale => "configuration_changed",
                HttpSendError::Transport { timeout: false } => "transport_unavailable",
            });
        }
        Err(_) => observation.error_code = Some("timeout"),
    }
    observation.elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    observation
}
