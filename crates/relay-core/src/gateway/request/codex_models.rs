use super::super::auth::{invalid_host, unauthorized, valid_local_host};
use super::super::errors::api_error;
use super::super::now_ms;
use crate::catalog::{
    normalize_codex_catalog_priorities, normalize_native_codex_catalog_entry,
    set_codex_service_tiers,
};
use crate::error_codes;
use crate::protocol::ClientWireApi;
use crate::providers::chatgpt::{configured_codex_client_version, valid_codex_client_version};
use crate::runtime::AuthenticatedKey;
use crate::{
    codex_model_is_picker_eligible, is_valid_model_id, routed_codex_catalog_entry, GatewayRuntime,
    WireApi,
};
use axum::body::Body;
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, Response, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::Json;
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

const MAX_CODEX_MODELS_BODY_BYTES: usize = 512 * 1024;
const CODEX_MODELS_FETCH_CONCURRENCY: usize = 4;
const CODEX_MODELS_FETCH_BUDGET: Duration = Duration::from_secs(12);

pub(in crate::gateway) async fn models(
    State(runtime): State<Arc<GatewayRuntime>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    if let Some(protocol) = crate::gateway::catalog::catalog_protocol(&headers) {
        return crate::gateway::catalog::native_catalog(&runtime, &headers, protocol, None);
    }
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
                error_codes::INVALID_REQUEST,
            );
        }
        if let Some(catalog) =
            codex_models_response(runtime.as_ref(), &key, &models, client_version).await
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
    visible_models: &[String],
    client_version: &str,
) -> Option<Value> {
    let now_ms = now_ms();
    let manifests = codex_account_model_manifests(runtime, key, client_version, now_ms).await;
    build_codex_models_response_from_manifests(runtime, key, visible_models, manifests)
}

async fn codex_account_model_manifests(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    client_version: &str,
    now_ms: u64,
) -> Vec<(String, Value)> {
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
    // Account transport manifests are independent. Preserve their ranked order,
    // but do not make connecting a pool wait for every account timeout in turn.
    let fetches = stream::iter(routes.into_iter().enumerate().map(
        |(index, (candidate_id, url))| {
            let versions = &client_versions;
            async move {
                let manifest =
                    fetch_codex_account_manifest(runtime, &candidate_id, url, versions).await;
                (index, candidate_id, manifest)
            }
        },
    ))
    .buffer_unordered(CODEX_MODELS_FETCH_CONCURRENCY);
    tokio::pin!(fetches);
    let deadline = tokio::time::sleep(CODEX_MODELS_FETCH_BUDGET);
    tokio::pin!(deadline);
    let mut completed = Vec::new();
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            result = fetches.next() => match result {
                Some(result) => completed.push(result),
                None => break,
            },
        }
    }
    completed.sort_by_key(|(index, _, _)| *index);
    let mut live_manifests = Vec::<(String, Value)>::new();
    let mut live_candidate_ids = HashSet::new();
    for (_, candidate_id, manifest) in completed {
        if let Some(manifest) = manifest {
            runtime.clear_candidate_capability_blocks(&candidate_id);
            runtime.remember_codex_model_manifest(&candidate_id, manifest.clone(), now_ms);
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
    live_manifests.into_iter().chain(stale).collect()
}

async fn fetch_codex_account_manifest(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    mut url: url::Url,
    client_versions: &[String],
) -> Option<Value> {
    for client_version in client_versions {
        url.query_pairs_mut()
            .clear()
            .append_pair("client_version", client_version);
        let request = runtime
            .request_client(candidate_id)
            .get(url.clone())
            .timeout(Duration::from_secs(10));
        let Ok(response) = runtime
            .send_authorized_request(candidate_id, request, Some(client_version.as_str()), None)
            .await
        else {
            continue;
        };
        if !response.response.status().is_success() {
            continue;
        }
        let Ok(body) =
            crate::transport::collect_limited(response.response, MAX_CODEX_MODELS_BODY_BYTES).await
        else {
            continue;
        };
        let Ok(upstream) = serde_json::from_slice::<Value>(&body) else {
            continue;
        };
        if upstream_codex_models(&upstream).is_some() {
            return Some(upstream);
        }
    }
    None
}

#[cfg(test)]
pub(in crate::gateway) fn build_codex_models_response(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    visible_models: &[String],
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
    // account catalog supplies transport templates for those same IDs.
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
        let capabilities = runtime.model_capabilities(&upstream_id);
        let native_entries = upstream_by_model
            .get(&normalized)
            .into_iter()
            .flatten()
            .filter(|(candidate_id, _)| {
                candidate_id.is_empty()
                    || native_account_ids
                        .iter()
                        .any(|account_id| account_id == candidate_id)
            });
        // Only the exact owning account's card can supply native transport.
        // Model identity is independent: a missing card must not rename a GPT
        // model or copy another account/model's transport controls.
        let native_catalog_model = has_native_account_route
            .then(|| {
                native_entries.clone().find_map(|(_, entry)| {
                    // Ignore participant semantic fields before validation too:
                    // malformed reasoning/image metadata must not discard the
                    // account's otherwise valid transport template.
                    let mut entry = entry.clone();
                    capabilities.apply_to_codex(&mut entry);
                    entry["display_name"] = json!(runtime.codex_model_display_name(&upstream_id));
                    entry.as_object().and_then(|entry| {
                        normalize_native_codex_catalog_entry(entry, &upstream_id, priority, None)
                    })
                })
            })
            .flatten();
        let mut model = native_catalog_model
            .unwrap_or_else(|| routed_codex_catalog_entry(None, &display_id, priority, None));
        model["display_name"] = json!(runtime.codex_model_display_name(&upstream_id));
        // Account models and unqualified GPT IDs retain their public spelling,
        // including an explicitly configured key prefix. Qualified provider
        // IDs keep reversible aliases; a similar leaf is not the same model.
        // The GPT family rule changes only picker identity, never inventory,
        // route eligibility, or capability evidence.
        if has_native_account_route
            || (normalized.starts_with("gpt-") && !upstream_id.contains('/'))
        {
            model["slug"] = Value::String(display_id.clone());
        }
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
        capabilities.apply_to_codex(&mut model);
        let supported = runtime.client_reasoning_levels(key, &upstream_id, WireApi::Responses);
        let catalog_default = model["default_reasoning_level"]
            .as_str()
            .filter(|default| supported.iter().any(|level| level == default))
            .map(str::to_owned);
        apply_model_reasoning_allowed_levels(&mut model, Some(&supported));
        if let Some(default) = catalog_default {
            model["default_reasoning_level"] = json!(default);
        }
        if let Some(allowed) = runtime.model_reasoning_policy_levels(&upstream_id) {
            let allowed = allowed
                .into_iter()
                .filter(|level| supported.contains(level))
                .collect::<Vec<_>>();
            apply_model_reasoning_allowed_levels(&mut model, Some(&allowed));
        }
        if runtime.model_has_translated_ultra_route(key, &display_id) {
            add_translated_ultra_after_max(&mut model);
        }
        set_codex_service_tiers(
            &mut model,
            runtime.model_supported_service_tiers(&upstream_id),
        );
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
        // No override: preserve the projected reference modes.
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
        CandidateHealth, CandidateQuota, DefaultServiceTier, GatewayRuntimeOptions,
        LocalGatewayKey, RuntimeMixedLocalKey, WireApi,
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
        native_catalog_test_runtime_with_accounts(
            model_prefix,
            model_metadata_catalog,
            &["native-account"],
            &["gpt-native"],
        )
    }

    fn native_catalog_test_runtime_with_accounts(
        model_prefix: Option<&str>,
        model_metadata_catalog: Option<crate::model_metadata::ModelMetadataCatalogHandle>,
        account_ids: &[&str],
        models: &[&str],
    ) -> GatewayRuntime {
        GatewayRuntime::from_mixed_pool_allow_unroutable(
            Vec::new(),
            account_ids
                .iter()
                .map(|account_id| RuntimeChatGptAccount {
                    id: (*account_id).into(),
                    source_id: "chatgpt".into(),
                    chatgpt_account_id: "chatgpt-account".into(),
                    responses_url: "https://example.test/v1/responses".into(),
                    models: models.iter().map(|model| (*model).into()).collect(),
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
                })
                .collect(),
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
    fn relay_speed_policy_overrides_missing_empty_and_conflicting_account_fields() {
        use crate::gateway::request::normalization::ServiceTierPolicy;

        let models = ["gpt-future", "gpt-another-synthetic"];
        let runtime =
            native_catalog_test_runtime_with_accounts(None, None, &["native-account"], &models);
        let key = runtime.authenticate_secret("secret").unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
        for fields in [
            json!({}),
            json!({"service_tiers": [], "additional_speed_tiers": []}),
            json!({"service_tiers": null, "additional_speed_tiers": "invalid"}),
            json!({"service_tiers": [{"id":"unrelated"}], "default_service_tier": "unrelated"}),
        ] {
            let sparse = json!({"models": models.iter().map(|id| {
                let mut row = fields.clone();
                row["slug"] = json!(id);
                row
            }).collect::<Vec<_>>()});
            runtime.remember_codex_model_manifest("native-account", sparse.clone(), now_ms());
            let response = build_codex_models_response_from_manifests(
                &runtime,
                &key,
                &visible,
                [("native-account".into(), sparse)],
            )
            .unwrap();
            assert_eq!(response["models"].as_array().unwrap().len(), models.len());
            for model in response["models"].as_array().unwrap() {
                let id = model["slug"].as_str().unwrap();
                assert!(models.contains(&id));
                assert_eq!(model["service_tiers"][0]["id"], "priority");
                assert_eq!(model["service_tiers"][1]["id"], "ultrafast");
                assert_eq!(
                    model["additional_speed_tiers"],
                    json!(["fast", "ultrafast"])
                );
                assert!(model.get("default_service_tier").is_none());
                let mut request = json!({"model": id, "service_tier": "ultrafast"});
                let policy = ServiceTierPolicy::pool_owned(&request);
                let selected = policy.select_for_model(&runtime, id);
                assert_eq!(selected, DefaultServiceTier::Ultrafast);
                policy.prepare_for_candidate(&mut request, selected, WireApi::Responses);
                assert_eq!(request["service_tier"], "ultrafast");
            }
        }
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
    fn native_account_catalog_uses_reference_capabilities_despite_conflicting_account_fields() {
        use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};

        let catalog = ModelMetadataCatalog::from_models_dev_json(
            r#"{
                "gpt-native": {
                    "name": "External catalog title",
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
            ("input_modalities".into(), json!(["text"])),
            ("output_modalities".into(), json!(["text"])),
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

        let response = build_codex_models_response(&runtime, &key, &visible, Some(&upstream))
            .expect("native catalog");
        let model = &response["models"][0];
        assert_eq!(model["slug"], "gpt-native");
        assert_eq!(model["display_name"], "External catalog title");
        assert_eq!(model["input_modalities"], json!(["text"]));
        assert_eq!(model["output_modalities"], json!(["text"]));
        assert_eq!(model["supports_parallel_tool_calls"], true);
        assert_eq!(model["supports_search_tool"], false);
        assert_eq!(
            model["supported_reasoning_levels"],
            json!([{"effort": "low", "description": "low"}])
        );
        assert_eq!(model["default_reasoning_level"], "low");

        // A missing account card uses this exact model's external name and
        // capabilities; it does not retain the native-only features above.
        let fallback = build_codex_models_response(&runtime, &key, &visible, None).unwrap();
        assert_eq!(
            fallback["models"][0]["display_name"],
            "External catalog title"
        );
        assert_eq!(fallback["models"][0]["slug"], "gpt-native");
        assert_eq!(fallback["models"][0]["supports_search_tool"], false);
        assert_eq!(fallback["models"][0]["default_reasoning_level"], "low");

        // A partial native card can supply capabilities without a title.
        // The normalizer's generated title must not hide a real catalog name.
        let mut unnamed = upstream.clone();
        unnamed["models"][0]
            .as_object_mut()
            .unwrap()
            .remove("display_name");
        let response =
            build_codex_models_response(&runtime, &key, &visible, Some(&unnamed)).unwrap();
        assert_eq!(
            response["models"][0]["display_name"],
            "External catalog title"
        );
        assert_eq!(response["models"][0]["supports_search_tool"], false);
        assert_eq!(response["models"][0]["default_reasoning_level"], "low");
    }

    #[test]
    fn native_catalog_follows_inventory_replacement_without_a_model_name_allowlist() {
        // Synthetic future identities deliberately include a non-GPT model.
        let old = "gpt-123-retired";
        let replacements = ["gpt-124-future", "next-family-synthetic"];
        let cards = [old, replacements[0], replacements[1]]
            .into_iter()
            .map(|id| {
                json!({
                    "slug": id,
                    "display_name": format!("Upstream title for {id}"),
                    "supported_reasoning_levels": [{"effort": "high", "description": "High"}],
                    "supports_parallel_tool_calls": true
                })
            })
            .collect::<Vec<_>>();
        // A retained manifest must not resurrect a model removed from the pool.
        for inventory in [&[old][..], &replacements[..]] {
            let runtime = native_catalog_test_runtime_with_accounts(
                None,
                None,
                &["native-account"],
                inventory,
            );
            let key = runtime
                .authenticate(Some(&axum::http::HeaderValue::from_static("Bearer secret")))
                .unwrap();
            let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
            let response = build_codex_models_response_from_manifests(
                &runtime,
                &key,
                &visible,
                [("native-account".into(), json!({"models": cards}))],
            )
            .unwrap();
            let models = response["models"].as_array().unwrap();
            assert_eq!(models.len(), inventory.len());
            assert_eq!(
                models
                    .iter()
                    .map(|row| row["slug"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                visible
                    .iter()
                    .map(String::as_str)
                    .filter(|id| codex_model_is_picker_eligible(id))
                    .collect::<Vec<_>>()
            );
            for (index, model) in models.iter().enumerate() {
                let id = model["slug"].as_str().unwrap();
                assert!(inventory.contains(&id));
                assert_eq!(model["display_name"], crate::codex_model_display_name(id));
                assert_eq!(model["supports_parallel_tool_calls"], true);
                assert_eq!(
                    model["priority"],
                    crate::CODEX_CATALOG_PRIORITY_BASE + index as u64
                );
                assert!(crate::codex_catalog_entry_is_compatible(model));
            }
        }
    }

    #[test]
    fn native_card_selection_skips_invalid_owners_without_borrowing_foreign_cards() {
        let runtime = native_catalog_test_runtime_with_accounts(
            None,
            None,
            &["native-account", "second-account"],
            &["gpt-native"],
        );
        let key = runtime
            .authenticate(Some(&axum::http::HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
        let mut native = routed_codex_catalog_entry(None, "gpt-native", 1_000, None);
        native["slug"] = json!("gpt-native");
        native["display_name"] = json!("Native account name");
        native["supports_parallel_tool_calls"] = json!(true);
        native["supported_reasoning_levels"] = json!([{"effort": "high", "description": "High"}]);
        let mut invalid = native.clone();
        invalid["display_name"] = json!("First account name");
        invalid["use_responses_lite"] = json!("invalid");
        let mut foreign = native.clone();
        foreign["display_name"] = json!("Foreign account name");
        let manifests = [
            ("unrelated-account".into(), json!({"models": [foreign]})),
            ("native-account".into(), json!({"models": [invalid]})),
            ("second-account".into(), json!({"models": [native]})),
        ];
        for count in [2, 3] {
            let response = build_codex_models_response_from_manifests(
                &runtime,
                &key,
                &visible,
                manifests[..count].iter().cloned(),
            )
            .unwrap();
            let models = response["models"].as_array().unwrap();
            assert_eq!(models.len(), 1);
            let model = &models[0];
            assert_eq!(model["slug"], "gpt-native");
            assert_eq!(model["display_name"], "GPT Native");
            assert_eq!(model["supports_parallel_tool_calls"], true);
            assert_eq!(
                model["supported_reasoning_levels"]
                    .as_array()
                    .unwrap()
                    .len(),
                0
            );
            assert_eq!(model["priority"], crate::CODEX_CATALOG_PRIORITY_BASE);
            assert!(crate::codex_catalog_entry_is_compatible(model));
        }
    }

    #[test]
    fn missing_native_card_keeps_identity_without_inheriting_capabilities() {
        for prefix in [None, Some("local")] {
            let runtime = native_catalog_test_runtime(prefix, None);
            let key = runtime
                .authenticate(Some(&axum::http::HeaderValue::from_static("Bearer secret")))
                .unwrap();
            let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
            let display_id = prefix.map_or_else(
                || "gpt-native".to_string(),
                |prefix| format!("{prefix}/gpt-native"),
            );
            let mut foreign = routed_codex_catalog_entry(None, "gpt-native", 1_000, None);
            foreign["slug"] = json!("gpt-native");
            foreign["display_name"] = json!("Foreign name must not leak");
            foreign["supports_parallel_tool_calls"] = json!(true);
            foreign["use_responses_lite"] = json!(true);
            foreign["supported_reasoning_levels"] = json!([{"effort": "ultra"}]);
            foreign["future_native_capability"] = json!(true);
            for manifests in [
                Vec::new(),
                vec![(
                    "unrelated-account".into(),
                    json!({"models": [foreign.clone()]}),
                )],
            ] {
                let response =
                    build_codex_models_response_from_manifests(&runtime, &key, &visible, manifests)
                        .expect("native catalog");
                let model = &response["models"][0];
                assert_eq!(model["slug"], display_id);
                assert_eq!(model["display_name"], "GPT Native");
                assert_eq!(model["comp_hash"], crate::CODEX_RELAY_CATALOG_HASH);
                assert_eq!(model["supports_parallel_tool_calls"], true);
                assert_eq!(model["supported_reasoning_levels"], json!([]));
                for field in [
                    "use_responses_lite",
                    "context_window",
                    "future_native_capability",
                ] {
                    assert!(model.get(field).is_none(), "unexpected capability: {field}");
                }
                assert!(crate::codex_catalog_entry_is_compatible(model));
            }
            let alias = crate::codex_model_alias(&display_id);
            for requested in [&display_id, &alias] {
                assert_eq!(
                    runtime.resolve_configured_account_model(&key, requested),
                    Some("gpt-native".into()),
                );
            }
        }
    }

    #[test]
    fn native_card_preserves_the_key_model_prefix() {
        let runtime = native_catalog_test_runtime(Some("local"), None);
        let key = runtime
            .authenticate(Some(&axum::http::HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
        let mut native = routed_codex_catalog_entry(None, "gpt-native", 1_000, None);
        native["slug"] = json!("gpt-native");
        native["supports_parallel_tool_calls"] = json!(true);
        let response = build_codex_models_response_from_manifests(
            &runtime,
            &key,
            &visible,
            [("native-account".into(), json!({"models": [native]}))],
        )
        .unwrap();
        let model = &response["models"][0];
        assert_eq!(model["slug"], "local/gpt-native");
        assert_eq!(model["supports_parallel_tool_calls"], true);
        assert_eq!(
            runtime.resolve_configured_account_model(&key, model["slug"].as_str().unwrap()),
            Some("gpt-native".into()),
        );
    }
}
