use super::fetch::parse_upstream_models;
use super::{
    ModelEndpointCapability, SourceConnector, SourceProtocolBinding, SourceReadHints,
    catalog_capabilities,
};
use crate::scheduler::refresh::http::{HttpClass, ManagementHttpScope};
use crate::transport::{MAX_MODEL_CATALOG_BODY_BYTES, collect_limited};
use crate::{ApiModelPriceOverride, Error, Result, UpstreamProtocol};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

const MAX_CATALOG_PAGES: usize = 128;
const MAX_CURSOR_BYTES: usize = 8 * 1024;
pub(super) const CATALOG_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct BindingCatalog {
    pub(super) models: Vec<(String, Option<ApiModelPriceOverride>)>,
    pub(super) capabilities: Vec<ModelEndpointCapability>,
    pub(super) resolved_connector: Option<SourceConnector>,
}

pub(super) async fn read_binding_catalog(
    client: &reqwest::Client,
    source: &SourceConnector,
    binding: &SourceProtocolBinding,
    bindings: &[SourceProtocolBinding],
    hints: &SourceReadHints,
    scope: &ManagementHttpScope,
) -> Result<BindingCatalog> {
    tokio::time::timeout(
        CATALOG_TIMEOUT,
        read_pages(client, source, binding, bindings, hints, scope),
    )
    .await
    .map_err(|_| Error::ManagementHttpUnavailable)?
}

async fn read_pages(
    client: &reqwest::Client,
    source: &SourceConnector,
    binding: &SourceProtocolBinding,
    bindings: &[SourceProtocolBinding],
    hints: &SourceReadHints,
    scope: &ManagementHttpScope,
) -> Result<BindingCatalog> {
    let protocol = binding.adapter.upstream_protocol(binding.wire_api);
    let (authorization_name, authorization) = source.authorization_for_binding(binding);
    let mut connector = source.clone();
    let mut endpoint = connector.models_endpoint_for_binding(binding).clone();
    let mut resolved = false;
    let mut cursors = HashSet::new();
    let mut body_bytes = 0;
    let mut models = Vec::new();
    let mut capabilities = Vec::new();
    for page in 0..MAX_CATALOG_PAGES {
        let request = client
            .get(endpoint.clone())
            .headers(connector.protocol_headers_for_binding(binding))
            .header(authorization_name.clone(), authorization.clone());
        let (mut response, first_permit) = scope
            .send(client, request, HttpClass::Ordinary)
            .await
            .map_err(|_| Error::ManagementHttpUnavailable)?;
        let mut permit = Some(first_permit);
        hints.observe(response.headers());
        if page == 0
            && response.status() == reqwest::StatusCode::NOT_FOUND
            && root_v1_fallback_allowed(&connector, protocol)
        {
            drop(permit.take());
            if let Some(v1_connector) = connector.with_appended_v1(bindings) {
                let retry = client
                    .get(v1_connector.models_endpoint_for_binding(binding).clone())
                    .headers(v1_connector.protocol_headers_for_binding(binding))
                    .header(authorization_name.clone(), authorization.clone());
                if let Ok((candidate, candidate_permit)) =
                    scope.send(client, retry, HttpClass::Ordinary).await
                {
                    hints.observe(candidate.headers());
                    if candidate.status().is_success() {
                        connector = v1_connector;
                        response = candidate;
                        permit = Some(candidate_permit);
                        resolved = true;
                    }
                }
            }
        }
        if !response.status().is_success() {
            return Err(Error::UpstreamStatus(response.status().as_u16()));
        }
        let bytes = collect_limited(response, MAX_MODEL_CATALOG_BODY_BYTES - body_bytes).await?;
        body_bytes += bytes.len();
        drop(permit);
        let payload: Value = serde_json::from_slice(&bytes)
            .map_err(|_| Error::InvalidUpstreamResponse("upstream model response is invalid"))?;
        let page_models = parse_upstream_models(protocol, &payload).ok_or(
            Error::InvalidUpstreamResponse("upstream model response is invalid"),
        )?;
        let next = next_cursor(protocol, &payload)?;
        models.extend(page_models);
        capabilities.extend(catalog_capabilities(&payload, crate::unix_time_ms()));
        let Some((parameter, cursor)) = next else {
            return Ok(BindingCatalog {
                models,
                capabilities,
                resolved_connector: resolved.then_some(connector),
            });
        };
        if cursor.len() > MAX_CURSOR_BYTES || !cursors.insert(cursor.to_owned()) {
            return Err(Error::InvalidUpstreamResponse(
                "upstream model pagination is invalid",
            ));
        }
        if body_bytes == MAX_MODEL_CATALOG_BODY_BYTES {
            return Err(Error::UpstreamBodyTooLarge);
        }
        // Provider cursors are opaque query values, never URLs. Rebuild from
        // the same configured endpoint to retain its origin and other parameters.
        endpoint = connector.models_endpoint_for_binding(binding).clone();
        let parameters = endpoint
            .query_pairs()
            .filter(|(key, _)| key.as_ref() != parameter)
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        endpoint.set_query(None);
        endpoint
            .query_pairs_mut()
            .extend_pairs(parameters)
            .append_pair(parameter, cursor);
    }
    Err(Error::InvalidUpstreamResponse(
        "upstream model pagination exceeds its page limit",
    ))
}

fn next_cursor(
    protocol: UpstreamProtocol,
    payload: &Value,
) -> Result<Option<(&'static str, &str)>> {
    let invalid = || Error::InvalidUpstreamResponse("upstream model pagination is invalid");
    let more = match payload.get("has_more") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(more)) => Some(*more),
        _ => return Err(invalid()),
    };
    let page_token = match payload.get("nextPageToken") {
        None | Some(Value::Null) => None,
        Some(Value::String(token)) => (!token.is_empty()).then_some(token.as_str()),
        _ => return Err(invalid()),
    };
    match protocol {
        UpstreamProtocol::Messages if page_token.is_none() => {
            if more == Some(false) {
                return Ok(None);
            }
            let last_id = match payload.get("last_id") {
                None | Some(Value::Null) => None,
                Some(Value::String(id)) => (!id.is_empty()).then_some(id.as_str()),
                _ => return Err(invalid()),
            };
            if more == Some(true) && last_id.is_none() {
                return Err(invalid());
            }
            let nonempty = payload
                .get("data")
                .or_else(|| payload.get("models"))
                .and_then(Value::as_array)
                .is_some_and(|rows| !rows.is_empty());
            Ok(last_id
                .filter(|_| more == Some(true) || nonempty)
                .map(|id| ("after_id", id)))
        }
        UpstreamProtocol::GeminiGenerateContent | UpstreamProtocol::GeminiInteractions
            if more != Some(true) =>
        {
            Ok(page_token.map(|token| ("pageToken", token)))
        }
        _ if more != Some(true) && page_token.is_none() => Ok(None),
        _ => Err(Error::InvalidUpstreamResponse(
            "upstream model pagination is unsupported",
        )),
    }
}

fn root_v1_fallback_allowed(connector: &SourceConnector, protocol: UpstreamProtocol) -> bool {
    connector.base_url.path() == "/"
        && matches!(
            protocol,
            UpstreamProtocol::Responses | UpstreamProtocol::ChatCompletions
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_cursor_preserves_the_sdk_optional_has_more_contract() {
        for more in [
            None,
            Some(Value::Null),
            Some(json!(true)),
            Some(json!(false)),
        ] {
            let expected =
                (more.as_ref() != Some(&json!(false))).then_some(("after_id", "opaque & next"));
            for field in ["data", "models"] {
                let mut payload = json!({"last_id": "opaque & next"});
                payload[field] = json!([{"id": "synthetic"}]);
                if let Some(more) = &more {
                    payload["has_more"] = more.clone();
                }
                assert_eq!(
                    next_cursor(UpstreamProtocol::Messages, &payload).unwrap(),
                    expected
                );
            }
        }
    }
}
