use super::{
    normalize_source_protocol_bindings, ProviderSource, SourceAdapter, SourceConnector,
    SourceProtocolBinding, SourceProtocolBindingKey,
};
use crate::transport::{collect_limited, MAX_MODEL_CATALOG_BODY_BYTES};
use crate::{ApiModelPriceOverride, Error, Result, UpstreamProtocol};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Duration;

mod pricing;

use pricing::detected_model_price;

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
/// `protocol_bindings` entry normally contains the models advertised for that
/// binding after its explicit allow-list is applied. A successful automatic
/// source-wide binding is preserved with an empty `model_ids`; `models` still
/// holds its current discovered catalog. A successful `/models` response is
/// catalog evidence, not a completion capability probe; operators must assign
/// a model only to routes the upstream documents or has safely verified. A
/// valid empty response is still success and is represented by a binding with
/// an empty `model_ids` list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceDiscovery {
    pub models: Vec<String>,
    pub protocol_bindings: Vec<SourceProtocolBinding>,
    /// A provider may expose its OpenAI-compatible catalog below `/v1` even
    /// when the user entered only the host root. This is set only after the
    /// root request returned 404 and the `/v1` retry succeeded.
    pub resolved_base_url: Option<String>,
    /// Complete token prices declared by the source model catalog. These are
    /// refreshed with discovery and never replace a user-configured override.
    pub detected_model_prices: BTreeMap<String, ApiModelPriceOverride>,
}

/// Discovers models independently for every configured binding.
///
/// Providers sometimes expose different model catalogs (and even different
/// credentials) on their Responses, Chat Completions, and Messages endpoints.
/// Discovery must therefore never reuse the first successful response for the
/// remaining bindings. A configured non-empty `model_ids` list is a strict
/// allow-list for that binding, except that a single legacy list equal to the
/// prior source catalog, or the native Responses remainder beside an explicit
/// Responses bridge, is recognized as an automatic source-wide route. An empty
/// single binding also uses the catalog returned under that binding's
/// authentication. The function succeeds when at least one binding returns a
/// valid catalog response, even when that catalog is empty; it fails only when
/// every binding request fails or is malformed.
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
    // A source-wide route is an automatic catalog binding, not a snapshot
    // allow-list. The native Responses route can retain that role when the
    // remaining Responses models are explicitly assigned to a bridge.
    let automatic_catalog_routes =
        automatic_catalog_routes(protocol_bindings, &bindings, &source.models);
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut discovery = discover_protocol_bindings_with_client(
        &client,
        &SourceConnector::new(source, &bindings)?,
        &bindings,
        protocol_bindings,
        &automatic_catalog_routes,
    )
    .await?;
    if bindings.len() == 1 && automatic_catalog_routes.contains(&bindings[0].key()) {
        if let Some(binding) = discovery.protocol_bindings.first_mut() {
            binding.model_ids.clear();
        }
    }
    Ok(discovery)
}

fn automatic_catalog_routes(
    configured_bindings: &[SourceProtocolBinding],
    bindings: &[SourceProtocolBinding],
    source_models: &[String],
) -> BTreeSet<SourceProtocolBindingKey> {
    let mut automatic = BTreeSet::new();
    let Some(binding) = bindings.first() else {
        return automatic;
    };
    if configured_bindings.is_empty()
        || (configured_bindings.len() == 1
            && (configured_bindings[0].model_ids.is_empty()
                || (!source_models.is_empty()
                    && normalized_model_ids(&configured_bindings[0].model_ids)
                        == normalized_model_ids(source_models))))
    {
        automatic.insert(binding.key());
        return automatic;
    }

    let source_models = normalized_model_ids(source_models);
    for binding in bindings
        .iter()
        .filter(|binding| binding.adapter == SourceAdapter::Native)
    {
        let assigned_elsewhere = bindings
            .iter()
            .filter(|candidate| {
                candidate.wire_api == binding.wire_api && candidate.key() != binding.key()
            })
            .flat_map(|candidate| normalized_model_ids(&candidate.model_ids))
            .collect::<HashSet<_>>();
        let expected = source_models
            .difference(&assigned_elsewhere)
            .cloned()
            .collect::<HashSet<_>>();
        if !expected.is_empty() && normalized_model_ids(&binding.model_ids) == expected {
            automatic.insert(binding.key());
        }
    }
    automatic
}

fn normalized_model_ids(models: &[String]) -> HashSet<String> {
    crate::catalog::normalize_model_ids(models.to_vec())
        .into_iter()
        .map(|model| model.to_ascii_lowercase())
        .collect()
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
    discover_protocol_bindings_with_client(client, source, bindings, &[], &BTreeSet::new())
        .await
        .map(|discovery| discovery.models)
}

async fn discover_protocol_bindings_with_client(
    client: &reqwest::Client,
    source: &SourceConnector,
    bindings: &[SourceProtocolBinding],
    configured_bindings: &[SourceProtocolBinding],
    automatic_catalog_routes: &BTreeSet<SourceProtocolBindingKey>,
) -> Result<SourceDiscovery> {
    let mut last_error = None;
    let mut connector = source.clone();
    let mut resolved_base_url = None;
    let mut discovered_models = Vec::new();
    let mut discovered_model_keys = HashSet::new();
    let mut discovered_bindings = Vec::new();
    let mut detected_model_prices = BTreeMap::new();
    let mut conflicting_model_prices = HashSet::new();
    let mut successful_responses = 0usize;

    for binding in bindings {
        let (authorization_name, authorization) = source.authorization_for_binding(binding);
        let request = client
            .get(connector.models_url.clone())
            .headers(connector.protocol_headers_for_binding(binding))
            .header(authorization_name.clone(), authorization.clone());
        let mut response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                last_error = Some(Error::Upstream(error));
                continue;
            }
        };
        if response.status() == reqwest::StatusCode::NOT_FOUND
            && resolved_base_url.is_none()
            && root_v1_fallback_allowed(&connector, binding)
        {
            if let Some(v1_connector) = connector.with_appended_v1(bindings) {
                let retry = client
                    .get(v1_connector.models_url.clone())
                    .headers(v1_connector.protocol_headers_for_binding(binding))
                    .header(authorization_name, authorization);
                if let Ok(candidate) = retry.send().await {
                    if candidate.status().is_success() {
                        resolved_base_url = Some(
                            v1_connector
                                .base_url
                                .as_str()
                                .trim_end_matches('/')
                                .to_string(),
                        );
                        connector = v1_connector;
                        response = candidate;
                    }
                }
            }
        }
        if !response.status().is_success() {
            last_error = Some(Error::UpstreamStatus(response.status().as_u16()));
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
        let upstream_models =
            match parse_upstream_models(binding.adapter.upstream_protocol(binding.wire_api), &body)
            {
                Some(models) => models,
                None => {
                    last_error = Some(Error::InvalidUpstreamResponse(
                        "upstream model response is invalid",
                    ));
                    continue;
                }
            };
        successful_responses += 1;

        // An explicitly supplied model list is scoped to this protocol unless
        // the route is a source-wide catalog fallback. The normalized legacy
        // binding may contain source.models after expansion, so use the
        // original configured binding to distinguish an allow-list from the
        // legacy "discover everything" form.
        let explicit_models = (!automatic_catalog_routes.contains(&binding.key()))
            .then(|| {
                configured_bindings
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
                    })
            })
            .flatten();
        let automatic_route_exclusions =
            automatic_catalog_routes.contains(&binding.key()).then(|| {
                bindings
                    .iter()
                    .filter(|candidate| {
                        candidate.wire_api == binding.wire_api && candidate.key() != binding.key()
                    })
                    .flat_map(|candidate| normalized_model_ids(&candidate.model_ids))
                    .collect::<HashSet<_>>()
            });
        let models = upstream_models
            .into_iter()
            .filter(|(model, _)| {
                if automatic_route_exclusions
                    .as_ref()
                    .is_some_and(|excluded| excluded.contains(&model.to_ascii_lowercase()))
                {
                    return false;
                }
                explicit_models
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(&model.to_ascii_lowercase()))
            })
            .collect::<Vec<_>>();
        for (model, price) in &models {
            let model_key = model.to_ascii_lowercase();
            if discovered_model_keys.insert(model_key.clone()) {
                discovered_models.push(model.clone());
            }
            let Some(price) = price else {
                continue;
            };
            if conflicting_model_prices.contains(&model_key) {
                continue;
            }
            if let Some(existing) = detected_model_prices.get(&model_key) {
                if existing != price {
                    detected_model_prices.remove(&model_key);
                    conflicting_model_prices.insert(model_key);
                }
            } else {
                detected_model_prices.insert(model_key, *price);
            }
        }
        discovered_bindings.push(SourceProtocolBinding {
            wire_api: binding.wire_api,
            adapter: binding.adapter,
            reasoning_mode: binding.reasoning_mode,
            cache_write_ttl: binding.cache_write_ttl,
            model_ids: models.into_iter().map(|(model, _)| model).collect(),
        });
    }

    if successful_responses == 0 {
        return Err(last_error.unwrap_or(Error::InvalidUpstreamResponse(
            "source did not return a valid model catalog",
        )));
    }
    Ok(SourceDiscovery {
        models: discovered_models,
        protocol_bindings: discovered_bindings,
        detected_model_prices,
        resolved_base_url,
    })
}

fn root_v1_fallback_allowed(connector: &SourceConnector, binding: &SourceProtocolBinding) -> bool {
    connector.base_url.path() == "/"
        && matches!(
            binding.adapter.upstream_protocol(binding.wire_api),
            UpstreamProtocol::Responses | UpstreamProtocol::ChatCompletions
        )
}

fn parse_upstream_models(
    protocol: UpstreamProtocol,
    body: &Value,
) -> Option<Vec<(String, Option<ApiModelPriceOverride>)>> {
    let models = match protocol {
        UpstreamProtocol::GeminiGenerateContent => body.get("models")?.as_array()?,
        UpstreamProtocol::Responses
        | UpstreamProtocol::ChatCompletions
        | UpstreamProtocol::Messages => body.get("data")?.as_array()?,
    };
    let mut seen = HashSet::new();
    Some(
        models
            .iter()
            .filter_map(|model| {
                let id = match protocol {
                    UpstreamProtocol::GeminiGenerateContent => {
                        let name = model.get("name")?.as_str()?.strip_prefix("models/")?;
                        let supported = model
                            .get("supportedGenerationMethods")
                            .and_then(Value::as_array)?
                            .iter()
                            .any(|method| method.as_str() == Some("generateContent"));
                        supported.then_some(name)
                    }
                    UpstreamProtocol::Responses
                    | UpstreamProtocol::ChatCompletions
                    | UpstreamProtocol::Messages => model.get("id")?.as_str(),
                }?;
                seen.insert(id.to_ascii_lowercase())
                    .then(|| (id.to_string(), detected_model_price(model)))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WireApi;
    use axum::{routing::get, Json, Router};
    use tokio::net::TcpListener;

    #[test]
    fn native_gemini_catalog_requires_generate_content_capability() {
        let models = parse_upstream_models(
            UpstreamProtocol::GeminiGenerateContent,
            &serde_json::json!({"models": [
                {"name": "models/gemini-usable", "supportedGenerationMethods": ["generateContent"]},
                {"name": "models/gemini-unsupported", "supportedGenerationMethods": ["countTokens"]}
            ]}),
        )
        .unwrap();
        assert_eq!(models[0].0, "gemini-usable");
        assert_eq!(models.len(), 1);
    }

    #[tokio::test]
    async fn valid_empty_catalog_is_successful() {
        let app = Router::new().route(
            "/v1/models",
            get(|| async { Json(serde_json::json!({"data": []})) }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let source = ProviderSource {
            id: "source-empty".into(),
            name: "Empty source".into(),
            base_url: format!("http://{address}/v1"),
            api_key: "secret".into(),
            wire_api: WireApi::Responses,
            models: Vec::new(),
        };

        let discovery = discover_source_models_and_protocol_bindings(&source, &[])
            .await
            .unwrap();
        assert!(discovery.models.is_empty());
        assert_eq!(discovery.protocol_bindings.len(), 1);
        assert!(discovery.protocol_bindings[0].model_ids.is_empty());
        server.abort();
    }
}
