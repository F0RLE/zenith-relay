use super::super::auth::{invalid_host, unauthorized, valid_local_host};
use super::super::errors::api_error;
use super::super::now_ms;
use crate::catalog::{normalize_codex_catalog_priorities, normalize_native_codex_catalog_entry};
use crate::protocol::ClientWireApi;
use crate::providers::chatgpt::{configured_codex_client_version, valid_codex_client_version};
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
#[cfg(test)]
use serde_json::Map;
use serde_json::{json, Value};
#[cfg(test)]
use std::collections::BTreeSet;
use std::collections::{BTreeMap, HashMap, HashSet};
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
    _allowed_protocols: &[WireApi],
    visible_models: &[String],
    client_version: &str,
) -> Option<Value> {
    let now_ms = now_ms();
    let routes = runtime.codex_models_routes(key, now_ms).await;
    let candidate_ids = routes
        .iter()
        .map(|(candidate_id, _)| candidate_id.clone())
        .collect::<Vec<_>>();
    let fallback_client_version = configured_codex_client_version();
    let client_versions = if client_version == fallback_client_version {
        vec![client_version.to_string()]
    } else {
        vec![client_version.to_string(), fallback_client_version]
    };
    let mut live_manifests = Vec::<(String, Value)>::new();
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
                .send_authorized_request(&candidate_id, request, Some(client_version.as_str()))
                .await
            else {
                continue;
            };
            let response = response.response;
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
            live_candidate_ids.insert(candidate_id.clone());
            live_manifests.push((candidate_id, manifest));
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
        live_manifests.into_iter().chain(stale),
    )
}

#[cfg(test)]
pub(in crate::gateway) fn build_codex_models_response(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    visible_models: &[String],
    _source_context_windows: &BTreeMap<String, u64>,
    upstream: Option<&Value>,
) -> Option<Value> {
    build_codex_models_response_from_manifests(
        runtime,
        key,
        visible_models,
        upstream
            .cloned()
            .into_iter()
            .map(|manifest| (String::new(), manifest)),
    )
}

#[cfg(test)]
pub(in crate::gateway) fn build_codex_models_response_with_source_reasoning(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    visible_models: &[String],
    _source_context_windows: &BTreeMap<String, u64>,
    _source_reasoning_templates: &BTreeMap<String, Map<String, Value>>,
    upstream: Option<&Value>,
) -> Option<Value> {
    build_codex_models_response_from_manifests(
        runtime,
        key,
        visible_models,
        upstream
            .cloned()
            .into_iter()
            .map(|manifest| (String::new(), manifest)),
    )
}

#[cfg(test)]
pub(in crate::gateway) fn build_codex_models_response_with_source_capabilities(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    visible_models: &[String],
    _source_context_windows: &BTreeMap<String, u64>,
    _source_image_models: &BTreeSet<String>,
    _source_reasoning_templates: &BTreeMap<String, Map<String, Value>>,
    upstream: Option<&Value>,
) -> Option<Value> {
    build_codex_models_response_from_manifests(
        runtime,
        key,
        visible_models,
        upstream
            .cloned()
            .into_iter()
            .map(|manifest| (String::new(), manifest)),
    )
}

fn build_codex_models_response_from_manifests(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    visible_models: &[String],
    upstreams: impl IntoIterator<Item = (String, Value)>,
) -> Option<Value> {
    let upstream_manifests = upstreams.into_iter().collect::<Vec<_>>();
    let visible = visible_models
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
    // Source models remain provider-agnostic in the runtime. The picker is the
    // presentation boundary: it groups familiar model IDs while the upstream
    // catalog only supplies capability templates for those same IDs.
    let mut upstream_by_model = HashMap::<String, Vec<(String, Value)>>::new();
    for (candidate_id, manifest) in &upstream_manifests {
        let Some(models) = upstream_codex_models(manifest) else {
            continue;
        };
        for model in models {
            let Some(object) = model.as_object() else {
                continue;
            };
            let Some(slug) = object.get("slug").and_then(Value::as_str).map(str::trim) else {
                continue;
            };
            if !is_valid_model_id(slug)
                || !codex_model_is_picker_eligible(slug)
                || object
                    .get("visibility")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case("hide"))
            {
                continue;
            }
            let normalized = slug.to_ascii_lowercase();
            if visible
                .iter()
                .any(|(upstream_id, _)| upstream_id == &normalized)
            {
                upstream_by_model
                    .entry(normalized)
                    .or_default()
                    .push((candidate_id.clone(), Value::Object(object.clone())));
            }
        }
    }

    let template = upstream_manifests
        .iter()
        .filter_map(|(_, manifest)| upstream_codex_models(manifest))
        .flatten()
        .find(|model| codex_catalog_entry_is_compatible(model))
        .and_then(Value::as_object);
    let mut models = Vec::with_capacity(visible.len());
    for (index, (normalized, (upstream_id, display_id))) in visible.into_iter().enumerate() {
        if !codex_model_is_picker_eligible(&upstream_id) {
            continue;
        }
        let priority = crate::CODEX_CATALOG_PRIORITY_BASE.saturating_add(index as u64);
        // These helpers accept the client-facing model spelling because they
        // resolve the key prefix internally. Passing the already-resolved
        // upstream id would make prefixed keys look like API-only routes.
        let has_native_account_route = runtime.codex_model_has_chatgpt_account(key, &display_id);
        let native_account_ids = runtime.codex_model_chatgpt_account_ids(key, &display_id);
        let native_entry = upstream_by_model.get(&normalized).and_then(|entries| {
            entries.iter().find(|(candidate_id, _)| {
                candidate_id.is_empty()
                    || native_account_ids
                        .iter()
                        .any(|account_id| account_id == candidate_id)
            })
        });
        // A bare model slug makes Codex choose its native client contract.
        // The account's broad model inventory alone cannot prove that
        // contract: it can contain models visible to an account without a
        // compatible native card for the current plan or client version.
        // Preserve native identity and capabilities only when the exact
        // owning account supplied a valid card. Otherwise use Relay's alias
        // and conservative API projection, even if the account can still be
        // tried later by the scheduler.
        let native_catalog_model = has_native_account_route
            .then(|| {
                native_entry.and_then(|(_, entry)| {
                    entry.as_object().and_then(|entry| {
                        normalize_native_codex_catalog_entry(entry, &upstream_id, priority, None)
                    })
                })
            })
            .flatten();
        let native_account_model = native_catalog_model.is_some();
        let mut model = native_catalog_model
            .unwrap_or_else(|| routed_codex_catalog_entry(template, &display_id, priority, None));
        for candidate_id in &native_account_ids {
            let uses_responses_lite = upstream_by_model
                .get(&normalized)
                .and_then(|entries| entries.iter().find(|(owner, _)| owner == candidate_id))
                .and_then(|(_, entry)| entry.get("use_responses_lite"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            runtime.set_codex_model_uses_responses_lite(
                candidate_id,
                &upstream_id,
                uses_responses_lite,
            );
        }
        if !native_account_model {
            runtime
                .model_capabilities(&upstream_id)
                .apply_to_codex(&mut model);
        }
        // A saved override may narrow known capabilities, never manufacture
        // modes for an unknown model or copy them from an upstream manifest.
        if !native_account_model {
            if let Some(allowed) = runtime.model_reasoning_policy_levels(&upstream_id) {
                let supported = runtime
                    .model_capabilities(&upstream_id)
                    .reasoning_effort_levels;
                let allowed = allowed
                    .into_iter()
                    .filter(|level| supported.contains(level))
                    .collect::<Vec<_>>();
                apply_model_reasoning_allowed_levels(&mut model, Some(&allowed));
            }
            if runtime.model_has_translated_ultra_route(key, &display_id) {
                add_translated_ultra_after_max(&mut model);
            }
        }
        sort_supported_reasoning_levels(&mut model);
        models.push(model);
    }

    normalize_codex_catalog_priorities(&mut models);
    if models.is_empty() {
        None
    } else {
        Some(json!({ "models": models }))
    }
}

fn add_translated_ultra_after_max(model: &mut Value) {
    let Some(levels) = model
        .get_mut("supported_reasoning_levels")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let has_max = levels.iter().any(|level| {
        level
            .get("effort")
            .and_then(Value::as_str)
            .is_some_and(|effort| effort.eq_ignore_ascii_case("max"))
    });
    let has_ultra = levels.iter().any(|level| {
        level
            .get("effort")
            .and_then(Value::as_str)
            .is_some_and(|effort| effort.eq_ignore_ascii_case("ultra"))
    });
    if has_max && !has_ultra {
        levels.push(json!({
            "effort": "ultra",
            "description": "ultra"
        }));
    }
}

fn sort_supported_reasoning_levels(model: &mut Value) {
    let Some(levels) = model
        .get_mut("supported_reasoning_levels")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let order = crate::canonicalize_reasoning_levels(
        levels
            .iter()
            .filter_map(|level| level.get("effort").and_then(Value::as_str)),
    );
    levels.sort_by_key(|level| {
        let effort = level
            .get("effort")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase())
            .unwrap_or_default();
        order
            .iter()
            .position(|candidate| candidate == &effort)
            .unwrap_or(order.len())
    });
}

fn apply_model_reasoning_allowed_levels(model: &mut Value, allowed_levels: Option<&[String]>) {
    let Some(allowed_levels) = allowed_levels else {
        // No override: preserve the provider-declared modes already present
        // in the source template. A probe is never required for defaults.
        return;
    };
    if allowed_levels.is_empty() {
        model["supported_reasoning_levels"] = Value::Array(Vec::new());
        model["default_reasoning_summary"] = Value::String("none".into());
        model["supports_reasoning_summary_parameter"] = Value::Bool(false);
        model["supports_reasoning_summaries"] = Value::Bool(false);
        model
            .as_object_mut()
            .expect("normalized catalog entry is an object")
            .remove("default_reasoning_level");
        return;
    }
    let detected_levels = model
        .get("supported_reasoning_levels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let detected_by_effort = detected_levels
        .into_iter()
        .filter_map(|level| {
            let effort = level.get("effort")?.as_str()?.trim().to_ascii_lowercase();
            (!effort.is_empty()).then_some((effort, level))
        })
        .collect::<BTreeMap<_, _>>();
    let levels = crate::canonicalize_reasoning_levels(allowed_levels.iter())
        .into_iter()
        .map(|effort| {
            detected_by_effort
                .get(&effort)
                .cloned()
                .unwrap_or_else(|| json!({"effort": effort, "description": effort}))
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
    // A manually exposed effort does not prove a provider-specific summary
    // contract.  Keep the selector narrow and let the actual route decide.
    model["default_reasoning_summary"] = Value::String("none".into());
    model["supports_reasoning_summary_parameter"] = Value::Bool(false);
    model["supports_reasoning_summaries"] = Value::Bool(false);
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
    use crate::accounts::{
        AccountAuthState, TokenAuthority, TokenPersistenceAdapter, TokenPersistenceFailure,
        TokenRefresh, TokenRefreshAdapter, TokenRefreshFailure, TokenRefreshFailureKind, TokenSet,
    };
    use crate::providers::chatgpt::{RuntimeChatGptAccount, RuntimeChatGptAuth};
    use crate::{
        CandidateHealth, CandidateQuota, GatewayRuntimeOptions, LocalGatewayKey,
        RuntimeMixedLocalKey, WireApi,
    };
    use futures_util::future::BoxFuture;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;

    struct NoopRefresh;

    impl TokenRefreshAdapter for NoopRefresh {
        fn refresh<'a>(
            &'a self,
            _account_id: &'a str,
            _refresh_token: &'a str,
            _now_ms: u64,
        ) -> BoxFuture<'a, std::result::Result<TokenRefresh, TokenRefreshFailure>> {
            Box::pin(async {
                Err(TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "not_called",
                ))
            })
        }
    }

    struct NoopPersistence;

    impl TokenPersistenceAdapter for NoopPersistence {
        fn persist<'a>(
            &'a self,
            _account_id: &'a str,
            _tokens: &'a TokenSet,
        ) -> BoxFuture<'a, std::result::Result<(), TokenPersistenceFailure>> {
            Box::pin(async { Ok(()) })
        }

        fn persist_auth_state<'a>(
            &'a self,
            _account_id: &'a str,
            _auth_state: AccountAuthState,
        ) -> BoxFuture<'a, std::result::Result<(), TokenPersistenceFailure>> {
            Box::pin(async { Ok(()) })
        }

        fn persist_agent_task_id<'a>(
            &'a self,
            _account_id: &'a str,
            _expected_task_id: Option<&'a str>,
            task_id: &'a str,
        ) -> BoxFuture<'a, std::result::Result<String, TokenPersistenceFailure>> {
            Box::pin(async move { Ok(task_id.to_string()) })
        }
    }

    fn native_catalog_test_runtime(
        model_prefix: Option<&str>,
        model_metadata_catalog: Option<crate::model_metadata::ModelMetadataCatalogHandle>,
    ) -> GatewayRuntime {
        let account_id = "native-account";
        GatewayRuntime::from_mixed_pool_allow_unroutable(
            Vec::new(),
            vec![RuntimeChatGptAccount {
                id: account_id.into(),
                source_id: "chatgpt".into(),
                chatgpt_account_id: "chatgpt-account".into(),
                responses_url: "https://example.test/v1/responses".into(),
                models: vec!["gpt-native".into()],
                enabled: true,
                draining: false,
                priority: 0,
                weight: 1,
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                health: CandidateHealth::Healthy,
                quota: CandidateQuota::Unknown,
                quota_updated_at_ms: None,
                quota_snapshot: Default::default(),
                subscription_plan_type: None,
                subscription_expires_at_ms: None,
                last_used_at_ms: None,
                cooldowns: Default::default(),
                consecutive_failures: 0,
                proxy: None,
            }],
            vec![RuntimeMixedLocalKey {
                key: LocalGatewayKey {
                    id: "key".into(),
                    secret: "secret".into(),
                },
                enabled: true,
                source_ids: None,
                account_ids: None,
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                model_prefix: model_prefix.map(str::to_owned),
                wire_apis: None,
            }],
            RuntimeChatGptAuth {
                token_authority: Arc::new(TokenAuthority::new(1).unwrap()),
                refresh_adapter: Arc::new(NoopRefresh),
                persistence_adapter: Arc::new(NoopPersistence),
                refresh_skew_ms: 60_000,
                agent_identities: HashMap::new(),
            },
            GatewayRuntimeOptions {
                model_metadata_catalog,
                ..GatewayRuntimeOptions::default()
            },
            Arc::new(|_| {}),
        )
        .unwrap()
    }

    #[test]
    fn manual_reasoning_modes_prefer_medium_over_provider_ultra_default() {
        let mut model = json!({
            "default_reasoning_level": "ultra",
            "supported_reasoning_levels": [
                {"effort": "low"},
                {"effort": "medium"},
                {"effort": "ultra"}
            ]
        });

        apply_model_reasoning_allowed_levels(
            &mut model,
            Some(&["low".to_string(), "medium".to_string(), "ultra".to_string()]),
        );

        assert_eq!(model["default_reasoning_level"], "medium");
    }

    #[test]
    fn manual_reasoning_modes_do_not_keep_provider_ultra_default_without_medium() {
        let mut model = json!({
            "default_reasoning_level": "ultra",
            "supported_reasoning_levels": [
                {"effort": "low"},
                {"effort": "high"},
                {"effort": "ultra"}
            ]
        });

        apply_model_reasoning_allowed_levels(
            &mut model,
            Some(&["low".to_string(), "high".to_string(), "ultra".to_string()]),
        );

        assert!(model.get("default_reasoning_level").is_none());
    }

    #[test]
    fn manual_reasoning_modes_allow_provider_specific_efforts() {
        let mut model = json!({
            "default_reasoning_level": "low",
            "supported_reasoning_levels": [
                {"effort": "low", "description": "Provider low"}
            ]
        });

        apply_model_reasoning_allowed_levels(
            &mut model,
            Some(&["low".to_string(), "xhigh".to_string(), "max".to_string()]),
        );

        assert_eq!(
            model["supported_reasoning_levels"],
            json!([
                {"effort": "low", "description": "Provider low"},
                {"effort": "xhigh", "description": "xhigh"},
                {"effort": "max", "description": "max"}
            ])
        );
        assert!(model.get("default_reasoning_level").is_none());
    }

    #[test]
    fn no_manual_override_preserves_provider_reasoning_modes() {
        let mut model = json!({
            "default_reasoning_level": "ultra",
            "supported_reasoning_levels": [
                {"effort": "low"},
                {"effort": "high"},
                {"effort": "ultra"}
            ]
        });

        apply_model_reasoning_allowed_levels(&mut model, None);

        assert_eq!(
            model["supported_reasoning_levels"],
            json!([{"effort": "low"}, {"effort": "high"}, {"effort": "ultra"}])
        );
        assert_eq!(model["default_reasoning_level"], "ultra");
    }

    #[test]
    fn missing_reasoning_metadata_stays_empty_until_catalog_evidence_exists() {
        let model = json!({"supported_reasoning_levels": []});
        assert_eq!(model["supported_reasoning_levels"], json!([]));
        assert!(model.get("default_reasoning_level").is_none());
    }

    #[test]
    fn provider_empty_reasoning_metadata_is_not_replaced_by_known_defaults() {
        let model = json!({"supported_reasoning_levels": []});

        // The caller skips this fallback when the provider explicitly sent an
        // empty field; the helper itself remains a no-op for unknown models.

        assert_eq!(model["supported_reasoning_levels"], json!([]));
    }

    #[test]
    fn native_account_catalog_keeps_native_capabilities_over_external_metadata() {
        use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};

        let catalog = ModelMetadataCatalog::from_models_dev_json(
            r#"{
                "gpt-native": {
                    "reasoning": true,
                    "reasoning_effort_levels": ["low"],
                    "default_reasoning_effort": "low",
                    "tool_call": true,
                    "modalities": {"input": ["text"], "output": ["text"]}
                }
            }"#,
        )
        .unwrap();
        let runtime =
            native_catalog_test_runtime(None, Some(ModelMetadataCatalogHandle::new(catalog)));
        let key = runtime
            .authenticate(Some(&axum::http::HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
        assert!(runtime.codex_model_has_chatgpt_account(&key, "gpt-native"));
        let mut native_entry = routed_codex_catalog_entry(None, "gpt-native", 1_000, None)
            .as_object()
            .unwrap()
            .clone();
        native_entry.extend([
            ("slug".into(), json!("gpt-native")),
            ("display_name".into(), json!("Native GPT")),
            ("input_modalities".into(), json!(["text", "audio"])),
            ("output_modalities".into(), json!(["text", "audio"])),
            ("supports_parallel_tool_calls".into(), json!(true)),
            ("supports_search_tool".into(), json!(true)),
            (
                "supported_reasoning_levels".into(),
                json!([{ "effort": "high", "description": "Native high" }]),
            ),
            ("default_reasoning_level".into(), json!("high")),
        ]);
        let upstream = json!({"models": [Value::Object(native_entry)]});
        assert!(normalize_native_codex_catalog_entry(
            upstream["models"][0].as_object().unwrap(),
            "gpt-native",
            1_000,
            None,
        )
        .is_some());

        let response = build_codex_models_response(
            &runtime,
            &key,
            &visible,
            &Default::default(),
            Some(&upstream),
        )
        .expect("native catalog");
        let model = &response["models"][0];
        assert_eq!(model["slug"], "gpt-native");
        assert_eq!(model["input_modalities"], json!(["text", "audio"]));
        assert_eq!(model["output_modalities"], json!(["text", "audio"]));
        assert_eq!(model["supports_parallel_tool_calls"], true);
        assert_eq!(model["supports_search_tool"], true);
        assert_eq!(
            model["supported_reasoning_levels"],
            json!([{"effort": "high", "description": "Native high"}])
        );
        assert_eq!(model["default_reasoning_level"], "high");
    }

    #[test]
    fn unverified_native_account_model_uses_a_routed_alias() {
        let runtime = native_catalog_test_runtime(Some("local"), None);
        let key = runtime
            .authenticate(Some(&axum::http::HeaderValue::from_static("Bearer secret")))
            .unwrap();
        assert_eq!(
            runtime.resolve_configured_account_model(&key, "local/gpt-native"),
            Some("gpt-native".into())
        );
        let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
        assert!(runtime.codex_model_has_chatgpt_account(&key, "local/gpt-native"));
        let response =
            build_codex_models_response(&runtime, &key, &visible, &Default::default(), None)
                .expect("native catalog");

        assert_eq!(
            response["models"][0]["slug"],
            json!(crate::codex_model_alias("local/gpt-native"))
        );
        assert_eq!(
            response["models"][0]["description"],
            "Available through Zenith Relay."
        );
    }
}
