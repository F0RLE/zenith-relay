//! Explicit connection diagnostics. No generation, capability mutation or raw wire data.

use super::{ProviderSource, SourceConnector, SourceProtocolBinding, WireApi};
use crate::scheduler::refresh::http::{
    HttpAdmissionError, HttpClass, HttpSendError, management_http_gate,
};
use crate::{Result, UpstreamProtocol};
use reqwest::Client;
use reqwest_websocket::Upgrade;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::{Instant, timeout};

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProbeFailure {
    Timeout,
    Unavailable,
}

impl ProbeFailure {
    fn http(error: HttpSendError) -> Self {
        match error {
            HttpSendError::Admission(HttpAdmissionError::TimedOut)
            | HttpSendError::Transport { timeout: true } => Self::Timeout,
            _ => Self::Unavailable,
        }
    }

    fn websocket(error: reqwest_websocket::Error) -> Self {
        match error {
            reqwest_websocket::Error::Reqwest(error) if error.is_timeout() => Self::Timeout,
            _ => Self::Unavailable,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionProbeStatus {
    Responded,
    Timeout,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProbeObservation {
    pub status: ConnectionProbeStatus,
    pub elapsed_ms: u64,
    pub http_status: Option<u16>,
    pub http_version: Option<String>,
    /// A validated upgrade, never evidence of generation or model support.
    pub websocket_upgraded: Option<bool>,
}

impl ConnectionProbeObservation {
    fn pending(websocket: bool) -> Self {
        Self {
            status: ConnectionProbeStatus::Unavailable,
            elapsed_ms: 0,
            http_status: None,
            http_version: None,
            websocket_upgraded: websocket.then_some(false),
        }
    }

    fn finish(
        &mut self,
        started: Instant,
        outcome: std::result::Result<
            std::result::Result<(), ProbeFailure>,
            tokio::time::error::Elapsed,
        >,
    ) {
        self.elapsed_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        self.status = match outcome {
            Ok(Ok(())) => ConnectionProbeStatus::Responded,
            Ok(Err(ProbeFailure::Unavailable)) => ConnectionProbeStatus::Unavailable,
            Ok(Err(ProbeFailure::Timeout)) | Err(_) => ConnectionProbeStatus::Timeout,
        };
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceConnectionDiagnostics {
    pub wire_api: WireApi,
    pub http: ConnectionProbeObservation,
    pub websocket: Option<ConnectionProbeObservation>,
}

fn probe_client(source: &ProviderSource, websocket: bool) -> Result<Client> {
    let mut builder = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(PROBE_TIMEOUT);
    if websocket {
        // Upgrade uses HTTP/1.1, independently of a successful HTTP/2 catalog read.
        builder = builder.http1_only();
    }
    Ok(match source.proxy_url.as_deref() {
        Some(proxy) => crate::ProxyConfig::parse(proxy)
            .map_err(|message| crate::Error::Validation(message.to_string()))?
            .apply(builder),
        None => builder,
    }
    .build()?)
}

/// Uses only the selected source's validated endpoints, authentication and proxy.
/// The model catalog is read only to HTTP headers; no response body is retained.
/// Responses WebSocket is upgraded and dropped without sending response.create.
pub async fn check_source_connection(
    source: &ProviderSource,
    bindings: &[SourceProtocolBinding],
) -> Result<SourceConnectionDiagnostics> {
    source.validate()?;
    let bindings = super::runtime_source_protocol_bindings(
        bindings.to_vec(),
        source.wire_api,
        &source.models,
    )?;
    let connector = SourceConnector::new(source, &bindings)?;
    let binding = bindings
        .iter()
        .find(|binding| binding.wire_api == source.wire_api)
        .or_else(|| bindings.first())
        .ok_or_else(|| crate::Error::Validation("source has no protocol binding".into()))?;
    let client = probe_client(source, false)?;
    let (authorization_name, authorization) = connector.authorization_for_binding(binding);
    let headers = connector.protocol_headers_for_binding(binding);
    let mut http = ConnectionProbeObservation::pending(false);
    let mut websocket_allowed = true;
    let started = Instant::now();
    let outcome = timeout(PROBE_TIMEOUT, async {
        let (response, _permit) = management_http_gate()
            .send(
                &client,
                client
                    .get(connector.models_endpoint_for_binding(binding).clone())
                    .headers(headers.clone())
                    .header(authorization_name.clone(), authorization.clone()),
                HttpClass::Ordinary,
            )
            .await
            .map_err(ProbeFailure::http)?;
        http.http_status = Some(response.status().as_u16());
        http.http_version = Some(format!("{:?}", response.version()));
        websocket_allowed = response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS
            && crate::transport::retry_after_ms(response.headers(), std::time::SystemTime::now())
                .is_none();
        Ok(())
    })
    .await;
    http.finish(started, outcome);
    drop(client);
    let websocket = if websocket_allowed
        && binding.adapter.upstream_protocol(binding.wire_api) == UpstreamProtocol::Responses
    {
        let client = probe_client(source, true)?;
        let endpoint = connector
            .endpoint(binding.key(), "", false)
            .ok_or_else(|| crate::Error::Validation("source endpoint is invalid".into()))?;
        let mut observation = ConnectionProbeObservation::pending(true);
        let started = Instant::now();
        let outcome = timeout(PROBE_TIMEOUT, async {
            let _permit = management_http_gate()
                .acquire(&endpoint, HttpClass::Ordinary)
                .await
                .map_err(|error| ProbeFailure::http(HttpSendError::Admission(error)))?;
            let upgrade = client
                .get(endpoint)
                .headers(headers)
                .header(authorization_name, authorization)
                .upgrade()
                .send()
                .await
                .map_err(ProbeFailure::websocket)?;
            observation.http_status = Some(upgrade.status().as_u16());
            if upgrade.status() == reqwest::StatusCode::SWITCHING_PROTOCOLS {
                let websocket = upgrade
                    .into_websocket()
                    .await
                    .map_err(ProbeFailure::websocket)?;
                observation.websocket_upgraded = Some(true);
                drop(websocket);
            }
            Ok(())
        })
        .await;
        observation.finish(started, outcome);
        Some(observation)
    } else {
        None
    };
    Ok(SourceConnectionDiagnostics {
        wire_api: binding.wire_api,
        http,
        websocket,
    })
}

#[cfg(test)]
mod tests;
