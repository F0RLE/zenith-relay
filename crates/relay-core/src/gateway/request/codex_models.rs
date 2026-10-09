use super::super::auth::{invalid_host, unauthorized, valid_local_host};
use super::super::errors::api_error;
use super::super::now_ms;
use crate::catalog::{
    apply_codex_ultra_from_official_model, normalize_codex_catalog_priorities,
    normalize_native_codex_catalog_entry, set_codex_service_tiers,
};
use crate::error_codes;
use crate::protocol::ClientWireApi;
use crate::providers::chatgpt::{configured_codex_client_version, valid_codex_client_version};
use crate::runtime::{AuthenticatedKey, AuthorizationIdentityPolicy};
use crate::{
    codex_model_is_picker_eligible_for, is_valid_model_id, routed_codex_catalog_entry,
    GatewayRuntime, WireApi,
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

const MAX_CODEX_MODELS_BODY_BYTES: usize = crate::transport::MAX_MODEL_CATALOG_BODY_BYTES;
const CODEX_MODELS_FETCH_CONCURRENCY: usize = 4;
const CODEX_MODELS_FETCH_BUDGET: Duration = Duration::from_secs(12);

mod project;
#[cfg(test)]
use project::apply_model_reasoning_allowed_levels;
#[cfg(test)]
pub(in crate::gateway) use project::build_codex_models_response;
use project::{
    allowed_codex_model_protocols, allowed_openai_model_protocols,
    build_codex_models_response_from_manifests, upstream_codex_models,
};

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
            .map(|(_, query_value)| query_value.into_owned())
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
        "data": models.into_iter().map(|model_id| json!({
            "id": model_id,
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
            manifest_result = fetches.next() => match manifest_result {
                Some(manifest_result) => completed.push(manifest_result),
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
            .send_authorized_request(
                candidate_id,
                request,
                crate::runtime::AuthorizationDispatch {
                    client_version: Some(client_version.as_str()),
                    identity_policy: AuthorizationIdentityPolicy::RelayCodex,
                    turn_scope: None,
                    budget: None,
                    lease: None,
                },
            )
            .await
        else {
            continue;
        };
        if !response.response.status().is_success() {
            continue;
        }
        let Ok(manifest_body) =
            crate::transport::collect_limited(response.response, MAX_CODEX_MODELS_BODY_BYTES).await
        else {
            continue;
        };
        let Ok(manifest_document) = serde_json::from_slice::<Value>(&manifest_body) else {
            continue;
        };
        if upstream_codex_models(&manifest_document).is_some() {
            return Some(manifest_document);
        }
    }
    None
}

#[cfg(test)]
mod tests;
