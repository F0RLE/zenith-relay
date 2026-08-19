use super::super::auth::{invalid_host, unauthorized, valid_local_host};
use super::super::errors::api_error;
use super::super::now_ms;
use crate::catalog::{
    canonicalize_model_ids, normalize_codex_catalog_priorities,
    normalize_native_codex_catalog_entry, normalize_upstream_codex_catalog_entry,
};
use crate::protocol::ClientWireApi;
use crate::providers::chatgpt::{valid_codex_client_version, CODEX_MODELS_CLIENT_VERSION};
use crate::runtime::AuthenticatedKey;
use crate::{
    codex_catalog_entry_is_compatible, codex_model_is_picker_eligible, is_valid_model_id,
    routed_codex_catalog_entry, GatewayRuntime, WireApi,
};
use axum::body::Body;
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, Response, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

const MAX_CODEX_MODELS_BODY_BYTES: usize = 512 * 1024;

pub(in crate::gateway) async fn models(
    State(runtime): State<Arc<GatewayRuntime>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    let Some(key) = runtime.authenticate(headers.get(AUTHORIZATION)) else {
        return unauthorized();
    };
    let client_version = uri.query().and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == "client_version")
            .map(|(_, value)| value.into_owned())
    });
    let protocols = match client_version.as_deref() {
        // Codex always executes selected models through /v1/responses.  Do
        // not publish a model there merely because it is available through a
        // different native endpoint: doing so creates a picker entry that can
        // never complete its first request.
        Some(_) => allowed_codex_model_protocols(&runtime, &key),
        // The generic OpenAI-compatible list is shared by Responses and Chat
        // Completions clients.  Anthropic Messages has its own native
        // discovery contract and must not be presented as an OpenAI model.
        None => allowed_openai_model_protocols(&runtime, &key),
    };
    let models = runtime.visible_models(&key, &protocols, now_ms());
    if let Some(client_version) = client_version.as_deref() {
        if !valid_codex_client_version(client_version) {
            return api_error(
                StatusCode::BAD_REQUEST,
                "client_version is invalid",
                "invalid_request",
            );
        }
        if let Some(catalog) =
            codex_models_response(runtime.as_ref(), &key, &protocols, &models, client_version).await
        {
            return Json(catalog).into_response();
        }
    }
    Json(json!({
        "object": "list",
        "data": models.into_iter().map(|id| json!({
            "id": id,
            "object": "model",
            "owned_by": "zenith-relay",
        })).collect::<Vec<_>>()
    }))
    .into_response()
}

async fn codex_models_response(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    allowed_protocols: &[WireApi],
    visible_models: &[String],
    client_version: &str,
) -> Option<Value> {
    let now_ms = now_ms();
    let source_metadata = runtime
        .codex_source_model_metadata(key, allowed_protocols, now_ms)
        .await;
    let routes = runtime.codex_models_routes(key, now_ms).await;
    let candidate_ids = routes
        .iter()
        .map(|(candidate_id, _)| candidate_id.clone())
        .collect::<Vec<_>>();
    let client_versions = if client_version == CODEX_MODELS_CLIENT_VERSION {
        vec![client_version]
    } else {
        vec![client_version, CODEX_MODELS_CLIENT_VERSION]
    };
    let mut live_manifests = Vec::new();
    let mut live_candidate_ids = HashSet::new();
    for (candidate_id, mut url) in routes {
        let mut candidate_manifest = None;
        for client_version in &client_versions {
            url.query_pairs_mut()
                .clear()
                .append_pair("client_version", client_version);
            let request = runtime
                .request_client(&candidate_id, false)
                .get(url.clone())
                .timeout(Duration::from_secs(10));
            let Ok(response) = runtime
                .send_authorized_request(&candidate_id, request, Some(client_version))
                .await
            else {
                continue;
            };
            if !response.status().is_success() {
                continue;
            }
            let Ok(body) =
                crate::transport::collect_limited(response, MAX_CODEX_MODELS_BODY_BYTES).await
            else {
                continue;
            };
            let Ok(upstream) = serde_json::from_slice::<Value>(&body) else {
                continue;
            };
            if upstream_codex_models(&upstream).is_none() {
                continue;
            }
            runtime.clear_candidate_capability_blocks(&candidate_id);
            runtime.remember_codex_model_manifest(&candidate_id, upstream.clone(), now_ms);
            candidate_manifest = Some(upstream);
            break;
        }
        if let Some(manifest) = candidate_manifest {
            live_candidate_ids.insert(candidate_id);
            live_manifests.push(manifest);
        }
    }
    // A successful manifest supersedes its own cache. If a different account
    // route is only temporarily unreachable, retain that account's last
    // confirmed native metadata after the live rows so it cannot disappear
    // from the Codex picker during a transient discovery failure.
    let stale = runtime.stale_codex_model_manifests(
        candidate_ids
            .iter()
            .filter(|candidate_id| !live_candidate_ids.contains(candidate_id.as_str()))
            .map(String::as_str),
    );
    build_codex_models_response_from_manifests(
        runtime,
        key,
        visible_models,
        &source_metadata.context_windows,
        &source_metadata.image_models,
        &source_metadata.reasoning_catalog_templates,
        live_manifests.iter().chain(stale.iter()),
    )
}

#[cfg(test)]
pub(in crate::gateway) fn build_codex_models_response(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    visible_models: &[String],
    source_context_windows: &BTreeMap<String, u64>,
    upstream: Option<&Value>,
) -> Option<Value> {
    build_codex_models_response_from_manifests(
        runtime,
        key,
        visible_models,
        source_context_windows,
        &Default::default(),
        &Default::default(),
        upstream,
    )
}

#[cfg(test)]
pub(in crate::gateway) fn build_codex_models_response_with_source_reasoning(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    visible_models: &[String],
    source_context_windows: &BTreeMap<String, u64>,
    source_reasoning_templates: &BTreeMap<String, Map<String, Value>>,
    upstream: Option<&Value>,
) -> Option<Value> {
    build_codex_models_response_from_manifests(
        runtime,
        key,
        visible_models,
        source_context_windows,
        &Default::default(),
        source_reasoning_templates,
        upstream,
    )
}

#[cfg(test)]
pub(in crate::gateway) fn build_codex_models_response_with_source_capabilities(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    visible_models: &[String],
    source_context_windows: &BTreeMap<String, u64>,
    source_image_models: &BTreeSet<String>,
    source_reasoning_templates: &BTreeMap<String, Map<String, Value>>,
    upstream: Option<&Value>,
) -> Option<Value> {
    build_codex_models_response_from_manifests(
        runtime,
        key,
        visible_models,
        source_context_windows,
        source_image_models,
        source_reasoning_templates,
        upstream,
    )
}

fn build_codex_models_response_from_manifests<'a>(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    visible_models: &[String],
    source_context_windows: &BTreeMap<String, u64>,
    source_image_models: &BTreeSet<String>,
    source_reasoning_templates: &BTreeMap<String, Map<String, Value>>,
    upstreams: impl IntoIterator<Item = &'a Value>,
) -> Option<Value> {
    let upstream_models = upstreams
        .into_iter()
        .filter_map(upstream_codex_models)
        .flat_map(|models| models.iter())
        .collect::<Vec<_>>();
    let mut visible = visible_models
        .iter()
        .filter_map(|display_id| {
            runtime.resolve_model(key, display_id).map(|upstream_id| {
                (
                    upstream_id.to_ascii_lowercase(),
                    (upstream_id, display_id.clone()),
                )
            })
        })
        .collect::<Vec<_>>();
    if visible.is_empty() {
        return None;
    }
    let picker_positions = canonicalize_model_ids(
        visible
            .iter()
            .map(|(_, (_, display_id))| display_id.as_str()),
    )
    .into_iter()
    .enumerate()
    .map(|(position, id)| (id.to_ascii_lowercase(), position))
    .collect::<HashMap<_, _>>();
    visible.sort_by_key(|(_, (_, display_id))| {
        picker_positions
            .get(&display_id.to_ascii_lowercase())
            .copied()
            .unwrap_or(usize::MAX)
    });

    // Source models remain provider-agnostic in the runtime. The picker is the
    // presentation boundary: it groups familiar model IDs while the upstream
    // catalog only supplies capability templates for those same IDs.
    let upstream_by_model = upstream_models
        .iter()
        .copied()
        .filter_map(|model| {
            let object = model.as_object()?;
            let slug = object.get("slug")?.as_str()?.trim();
            if !is_valid_model_id(slug) {
                return None;
            }
            let normalized = slug.to_ascii_lowercase();
            if !codex_model_is_picker_eligible(slug)
                || object.get("supported_in_api") == Some(&Value::Bool(false))
                || object
                    .get("visibility")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case("hide"))
            {
                return None;
            }
            visible
                .iter()
                .any(|(upstream_id, _)| upstream_id == &normalized)
                .then_some((normalized, object.clone()))
        })
        .fold(HashMap::new(), |mut entries, (normalized, object)| {
            entries.entry(normalized).or_insert(object);
            entries
        });

    let template = upstream_models
        .iter()
        .copied()
        .find(|model| codex_catalog_entry_is_compatible(model))
        .and_then(Value::as_object);
    let mut models = Vec::with_capacity(visible.len());
    for (index, (normalized, (upstream_id, display_id))) in visible.into_iter().enumerate() {
        if !codex_model_is_picker_eligible(&upstream_id) {
            continue;
        }
        let priority = crate::CODEX_CATALOG_PRIORITY_BASE.saturating_add(index as u64);
        let native_account_model = runtime.codex_model_has_chatgpt_account(key, &upstream_id);
        let source_context_window = source_context_windows
            .get(&upstream_id.to_ascii_lowercase())
            .copied();
        let mut model = if native_account_model {
            let native_model_id = upstream_by_model
                .get(&normalized)
                .and_then(|entry| entry.get("slug"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|slug| is_valid_model_id(slug))
                .unwrap_or(&display_id);
            upstream_by_model
                .get(&normalized)
                .and_then(|entry| {
                    normalize_native_codex_catalog_entry(entry, native_model_id, priority, None)
                })
                .unwrap_or_else(|| {
                    // A missing/invalid account manifest must not borrow
                    // another account's native capabilities. Keep only the
                    // exact configured/upstream ID until a fresh manifest is
                    // available.
                    let mut fallback =
                        routed_codex_catalog_entry(None, native_model_id, priority, None);
                    fallback["slug"] = Value::String(native_model_id.to_string());
                    fallback
                })
        } else {
            source_reasoning_templates
                .get(&normalized)
                .and_then(|capabilities| {
                    normalize_upstream_codex_catalog_entry(
                        capabilities,
                        &display_id,
                        priority,
                        source_context_window,
                    )
                })
                .unwrap_or_else(|| {
                    routed_codex_catalog_entry(
                        template,
                        &display_id,
                        priority,
                        source_context_window,
                    )
                })
        };
        let uses_responses_lite = native_account_model
            && upstream_by_model
                .get(&normalized)
                .and_then(|entry| entry.get("use_responses_lite"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
        runtime.set_codex_model_uses_responses_lite(&upstream_id, uses_responses_lite);
        if !native_account_model {
            if let Some(context_window) = source_context_window {
                model["context_window"] = context_window.into();
                model["max_context_window"] = context_window.into();
                model
                    .as_object_mut()
                    .expect("normalized catalog entry is an object")
                    .remove("auto_compact_token_limit");
                model["effective_context_window_percent"] = 95.into();
            }
            if source_image_models.contains(&normalized) {
                model["input_modalities"] = json!(["text", "image"]);
            }
            apply_source_reasoning_allowed_levels(
                &mut model,
                &runtime.model_reasoning_allowed_levels(&upstream_id),
            );
        }
        models.push(model);
    }

    normalize_codex_catalog_priorities(&mut models);
    if models.is_empty() {
        None
    } else {
        Some(json!({ "models": models }))
    }
}

fn apply_source_reasoning_allowed_levels(model: &mut Value, allowed_levels: &[String]) {
    if allowed_levels.is_empty() {
        let automatic_default = model
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .and_then(|levels| {
                levels.iter().find_map(|level| {
                    level
                        .get("effort")
                        .and_then(Value::as_str)
                        .filter(|effort| effort.eq_ignore_ascii_case("medium"))
                })
            })
            .map(str::to_owned);
        if let Some(effort) = automatic_default {
            model["default_reasoning_level"] = Value::String(effort);
        } else {
            model
                .as_object_mut()
                .expect("normalized catalog entry is an object")
                .remove("default_reasoning_level");
        }
        return;
    }
    let allowed = allowed_levels
        .iter()
        .map(|level| level.trim().to_ascii_lowercase())
        .filter(|level| !level.is_empty())
        .collect::<BTreeSet<_>>();
    let detected_levels = model
        .get("supported_reasoning_levels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let levels = detected_levels
        .into_iter()
        .filter(|level| {
            level
                .get("effort")
                .and_then(Value::as_str)
                .is_some_and(|effort| allowed.contains(&effort.to_ascii_lowercase()))
        })
        .collect::<Vec<_>>();
    let (has_levels, default_reasoning_level) = {
        let default_reasoning_level = match levels.as_slice() {
            [] => None,
            [level] => level
                .get("effort")
                .and_then(Value::as_str)
                .map(str::to_owned),
            _ => levels.iter().find_map(|level| {
                level
                    .get("effort")
                    .and_then(Value::as_str)
                    .filter(|effort| effort.eq_ignore_ascii_case("medium"))
                    .map(str::to_owned)
            }),
        };
        (!levels.is_empty(), default_reasoning_level)
    };
    model["supported_reasoning_levels"] = Value::Array(levels);
    if !has_levels {
        model["supported_reasoning_levels"] = Value::Array(Vec::new());
        model
            .as_object_mut()
            .expect("normalized catalog entry is an object")
            .remove("default_reasoning_level");
    } else if let Some(effort) = default_reasoning_level {
        model["default_reasoning_level"] = Value::String(effort);
    } else {
        model
            .as_object_mut()
            .expect("normalized catalog entry is an object")
            .remove("default_reasoning_level");
    }
}

fn upstream_codex_models(payload: &Value) -> Option<&Vec<Value>> {
    payload
        .get("models")
        .and_then(Value::as_array)
        .filter(|models| models.len() <= 4_096)
}

fn allowed_openai_model_protocols(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
) -> Vec<WireApi> {
    let mut protocols = Vec::new();
    if runtime.allows_client_wire_api(key, ClientWireApi::Responses) {
        protocols.push(WireApi::Responses);
    }
    if runtime.allows_client_wire_api(key, ClientWireApi::ChatCompletions) {
        protocols.push(WireApi::ChatCompletions);
    }
    protocols
}

fn allowed_codex_model_protocols(runtime: &GatewayRuntime, key: &AuthenticatedKey) -> Vec<WireApi> {
    if runtime.allows_client_wire_api(key, ClientWireApi::Responses) {
        vec![WireApi::Responses]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn confirmed_reasoning_modes_prefer_medium_over_provider_ultra_default() {
        let mut model = json!({
            "default_reasoning_level": "ultra",
            "supported_reasoning_levels": [
                {"effort": "low"},
                {"effort": "medium"},
                {"effort": "ultra"}
            ]
        });

        apply_source_reasoning_allowed_levels(
            &mut model,
            &["low".to_string(), "medium".to_string(), "ultra".to_string()],
        );

        assert_eq!(model["default_reasoning_level"], "medium");
    }

    #[test]
    fn confirmed_reasoning_modes_do_not_keep_provider_ultra_default_without_medium() {
        let mut model = json!({
            "default_reasoning_level": "ultra",
            "supported_reasoning_levels": [
                {"effort": "low"},
                {"effort": "high"},
                {"effort": "ultra"}
            ]
        });

        apply_source_reasoning_allowed_levels(
            &mut model,
            &["low".to_string(), "high".to_string(), "ultra".to_string()],
        );

        assert!(model.get("default_reasoning_level").is_none());
    }

    #[test]
    fn unconfirmed_reasoning_modes_are_not_synthesized() {
        let mut model = json!({
            "default_reasoning_level": "low",
            "supported_reasoning_levels": [
                {"effort": "low", "description": "Provider low"}
            ]
        });

        apply_source_reasoning_allowed_levels(
            &mut model,
            &["low".to_string(), "xhigh".to_string(), "max".to_string()],
        );

        assert_eq!(
            model["supported_reasoning_levels"],
            json!([{"effort": "low", "description": "Provider low"}])
        );
        assert_eq!(model["default_reasoning_level"], "low");
    }

    #[test]
    fn automatic_reasoning_does_not_keep_provider_ultra_default_without_medium() {
        let mut model = json!({
            "default_reasoning_level": "ultra",
            "supported_reasoning_levels": [
                {"effort": "low"},
                {"effort": "high"},
                {"effort": "ultra"}
            ]
        });

        apply_source_reasoning_allowed_levels(&mut model, &[]);

        assert!(model.get("default_reasoning_level").is_none());
    }
}
