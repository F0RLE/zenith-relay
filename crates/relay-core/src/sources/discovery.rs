use super::{
    normalize_source_protocol_bindings, ProviderSource, SourceConnector, SourceProtocolBinding,
};
use crate::transport::{collect_limited, MAX_MODEL_CATALOG_BODY_BYTES};
use crate::{Error, Result};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

pub async fn discover_source_models(source: &ProviderSource) -> Result<Vec<String>> {
    discover_source_models_for_protocol_bindings(source, &[]).await
}

/// Discovers a source catalog using each explicitly configured binding.
/// Authentication and the discovery endpoint follow the binding's upstream
/// protocol. No request body or response format is adapted during discovery.
pub async fn discover_source_models_for_protocol_bindings(
    source: &ProviderSource,
    protocol_bindings: &[SourceProtocolBinding],
) -> Result<Vec<String>> {
    discover_source_models_and_protocol_bindings(source, protocol_bindings)
        .await
        .map(|discovery| discovery.models)
}

/// The result of binding-aware model discovery.
///
/// `models` is the de-duplicated union in upstream response order. Each
/// `protocol_bindings` entry contains the models advertised for that binding
/// after its explicit allow-list is applied. A failed or empty binding is
/// omitted. A successful `/models` response is catalog evidence, not a
/// completion capability probe; operators must assign a model only to routes
/// the upstream documents or has safely verified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceDiscovery {
    pub models: Vec<String>,
    pub protocol_bindings: Vec<SourceProtocolBinding>,
}

/// Discovers models independently for every configured binding.
///
/// Providers sometimes expose different model catalogs (and even different
/// credentials) on their Responses, Chat Completions, and Messages endpoints.
/// Discovery must therefore never reuse the first successful response for the
/// remaining bindings. A configured non-empty `model_ids` list is a strict
/// allow-list for that binding; an empty list means use the catalog returned
/// under that binding's authentication. The function succeeds when at least
/// one binding has a non-empty catalog.
pub async fn discover_source_models_and_protocol_bindings(
    source: &ProviderSource,
    protocol_bindings: &[SourceProtocolBinding],
) -> Result<SourceDiscovery> {
    source.validate()?;
    let bindings = normalize_source_protocol_bindings(
        protocol_bindings.to_vec(),
        source.wire_api,
        &source.models,
    )?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    discover_protocol_bindings_with_client(
        &client,
        &SourceConnector::new(source, &bindings)?,
        &bindings,
        protocol_bindings,
    )
    .await
}

pub(crate) async fn discover_models_with_client(
    client: &reqwest::Client,
    source: &SourceConnector,
    bindings: &[SourceProtocolBinding],
) -> Result<Vec<String>> {
    // GatewayRuntime only retains normalized bindings, so it cannot tell a
    // legacy expanded model list from an explicit per-protocol allow-list.
    // Keep this compatibility helper broad; management paths use the public
    // discovery API above and retain that distinction.
    discover_protocol_bindings_with_client(client, source, bindings, &[])
        .await
        .map(|discovery| discovery.models)
}

async fn discover_protocol_bindings_with_client(
    client: &reqwest::Client,
    source: &SourceConnector,
    bindings: &[SourceProtocolBinding],
    configured_bindings: &[SourceProtocolBinding],
) -> Result<SourceDiscovery> {
    let mut last_error = None;
    let mut discovered_models = Vec::new();
    let mut discovered_model_keys = HashSet::new();
    let mut discovered_bindings = Vec::new();

    for binding in bindings {
        let (authorization_name, authorization) = source.authorization_for_binding(binding);
        let request = client
            .get(source.models_url.clone())
            .headers(source.protocol_headers_for_binding(binding))
            .header(authorization_name, authorization);
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                last_error = Some(Error::Upstream(error));
                continue;
            }
        };
        if !response.status().is_success() {
            last_error = Some(Error::InvalidUpstreamResponse(
                "upstream model discovery failed",
            ));
            continue;
        }
        let body = match collect_limited(response, MAX_MODEL_CATALOG_BODY_BYTES).await {
            Ok(body) => body,
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };
        let body: Value = match serde_json::from_slice(&body) {
            Ok(body) => body,
            Err(_) => {
                last_error = Some(Error::InvalidUpstreamResponse(
                    "upstream model response is invalid",
                ));
                continue;
            }
        };
        let Some(data) = body.get("data").and_then(Value::as_array) else {
            last_error = Some(Error::InvalidUpstreamResponse(
                "upstream model response is invalid",
            ));
            continue;
        };
        let mut seen = HashSet::new();
        let upstream_models = data
            .iter()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .filter(|model| seen.insert(model.to_ascii_lowercase()))
            .map(str::to_string)
            .collect::<Vec<_>>();

        // An explicitly supplied model list is scoped to this protocol. The
        // normalized legacy binding may contain source.models after expansion,
        // so use the original configured binding to distinguish an allow-list
        // from the legacy "discover everything" form.
        let explicit_models = configured_bindings
            .iter()
            .find(|configured| configured.key() == binding.key())
            .and_then(|configured| {
                let models = configured
                    .model_ids
                    .iter()
                    .map(|model| model.trim().to_ascii_lowercase())
                    .filter(|model| !model.is_empty())
                    .collect::<HashSet<_>>();
                (!models.is_empty()).then_some(models)
            });
        let models = upstream_models
            .into_iter()
            .filter(|model| {
                explicit_models
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(&model.to_ascii_lowercase()))
            })
            .collect::<Vec<_>>();
        if models.is_empty() {
            continue;
        }

        for model in &models {
            if discovered_model_keys.insert(model.to_ascii_lowercase()) {
                discovered_models.push(model.clone());
            }
        }
        discovered_bindings.push(SourceProtocolBinding {
            wire_api: binding.wire_api,
            adapter: binding.adapter,
            reasoning_mode: binding.reasoning_mode,
            model_ids: models,
        });
    }

    if discovered_bindings.is_empty() {
        return Err(last_error.unwrap_or(Error::InvalidUpstreamResponse(
            "source did not expose any confirmed models",
        )));
    }
    Ok(SourceDiscovery {
        models: discovered_models,
        protocol_bindings: discovered_bindings,
    })
}
