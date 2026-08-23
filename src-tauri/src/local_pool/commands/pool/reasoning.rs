use crate::local_pool::{
    accounts::collect_limited,
    error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
    models::{LocalPoolSnapshot, ProviderSourceRecord},
    state::DesktopState,
    store::secret_store,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, time::Duration};
use tauri::State;
use zenith_relay_core::{
    AdapterRequestContext, ProviderSource, SourceConnector, SourceProtocolBinding, WireApi,
};

type CommandResult<T> = std::result::Result<T, CommandError>;
const MAX_REASONING_PROBE_BODY_BYTES: usize = 256 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelReasoningInput {
    pub(super) model_id: String,
    #[serde(default)]
    pub(super) allowed_levels: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProbeModelReasoningInput {
    pub(super) model_id: String,
    pub(super) level: String,
    #[serde(default)]
    pub(super) add_successful_to_settings: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReasoningProbeSourceResult {
    pub source_id: String,
    pub source_name: String,
    pub available: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReasoningProbeResult {
    pub model_id: String,
    pub level: String,
    pub source_count: usize,
    pub available_count: usize,
    pub applied_to_settings: bool,
    pub sources: Vec<ModelReasoningProbeSourceResult>,
}

pub(super) async fn set_local_model_reasoning(
    input: SetModelReasoningInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let canonical = super::canonical_pool_model(&state, &input.model_id)?;
    apply_local_model_reasoning(&state, canonical, input.allowed_levels, None).await
}

async fn apply_local_model_reasoning(
    state: &DesktopState,
    canonical: String,
    requested_levels: Vec<String>,
    expected_levels: Option<Vec<String>>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let policy_key = zenith_relay_core::reasoning_policy_key(&canonical);
    let mut normalized_allowed_levels =
        zenith_relay_core::normalize_model_reasoning_allowed_levels(BTreeMap::from([(
            policy_key.clone(),
            requested_levels,
        )]))
        .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    let allowed_levels = normalized_allowed_levels
        .remove(&policy_key)
        .unwrap_or_default();
    let old_gateway = state.store()?.gateway().clone();
    if let Some(expected_levels) = expected_levels.as_deref() {
        let current_levels = zenith_relay_core::reasoning_policy_levels(
            &old_gateway.model_reasoning_allowed_levels,
            &canonical,
        )
        .map(ToOwned::to_owned);
        if current_levels.as_deref() != Some(expected_levels) {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "reasoning settings changed while the probe was running",
            )
            .into());
        }
    }
    let mut gateway = old_gateway.clone();
    gateway
        .model_reasoning_allowed_levels
        .remove(&canonical.to_ascii_lowercase());
    gateway
        .model_reasoning_allowed_levels
        .insert(policy_key, allowed_levels);
    if gateway == old_gateway {
        return state.snapshot().await.map_err(Into::into);
    }
    state.store()?.replace_gateway(gateway.clone())?;
    if let Some(runtime) = state.gateway.runtime().await {
        if let Err(error) =
            runtime.set_model_reasoning_allowed_levels(gateway.model_reasoning_allowed_levels)
        {
            state.store()?.replace_gateway(old_gateway)?;
            return Err(LocalPoolError::new(ErrorCode::InvalidState, error.to_string()).into());
        }
    }
    let snapshot = state.snapshot().await?;
    drop(_mutation);
    Ok(snapshot)
}

/// Probe each eligible source with the same adapter path used for pool traffic.
pub(super) async fn probe_local_model_reasoning(
    input: ProbeModelReasoningInput,
    state: State<'_, DesktopState>,
) -> CommandResult<ModelReasoningProbeResult> {
    let canonical = super::canonical_pool_model(&state, &input.model_id)?;
    let level = normalized_reasoning_probe_level(&canonical, input.level)?;
    let sources = state.store()?.sources().to_vec();
    let mut results = Vec::new();
    let mut probes = tokio::task::JoinSet::new();

    for source in sources
        .into_iter()
        .filter(|source| source.in_pool && source.enabled && !source.draining)
    {
        let Some((binding, source_model)) = source_probe_binding(&source, &canonical) else {
            continue;
        };
        let source_id = source.id.clone();
        let source_name = source.name.clone();
        if !binding.supports_reasoning_effort(&level) {
            results.push(ModelReasoningProbeSourceResult {
                source_id,
                source_name,
                available: false,
            });
            continue;
        }
        let Ok(Some(api_key)) = secret_store::load(&source.secret_ref) else {
            results.push(ModelReasoningProbeSourceResult {
                source_id,
                source_name,
                available: false,
            });
            continue;
        };
        let probe_level = level.clone();
        probes.spawn(async move {
            probe_source_reasoning(source, api_key, binding, source_model, probe_level).await
        });
    }
    while let Some(result) = probes.join_next().await {
        if let Ok(result) = result {
            results.push(result);
        }
    }
    results.sort_by(|left, right| left.source_name.cmp(&right.source_name));
    if results.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::NotFound,
            "no eligible API source can probe this pool model",
        )
        .into());
    }
    let available_count = results.iter().filter(|result| result.available).count();
    let applied_to_settings = input.add_successful_to_settings && available_count > 0;
    if applied_to_settings {
        let gateway = state.store()?.gateway().clone();
        let previous_levels = zenith_relay_core::reasoning_policy_levels(
            &gateway.model_reasoning_allowed_levels,
            &canonical,
        )
        .map(ToOwned::to_owned)
        .unwrap_or_default();
        let mut allowed_levels = previous_levels.clone();
        allowed_levels.push(level.clone());
        apply_local_model_reasoning(
            &state,
            canonical.clone(),
            allowed_levels,
            Some(previous_levels),
        )
        .await?;
    }
    Ok(ModelReasoningProbeResult {
        model_id: canonical,
        level,
        source_count: results.len(),
        available_count,
        applied_to_settings,
        sources: results,
    })
}

fn normalized_reasoning_probe_level(model: &str, level: String) -> LocalResult<String> {
    let mut normalized = zenith_relay_core::normalize_model_reasoning_allowed_levels(
        BTreeMap::from([(model.to_string(), vec![level])]),
    )
    .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    normalized
        .remove(&model.to_ascii_lowercase())
        .and_then(|levels| levels.into_iter().next())
        .ok_or_else(|| {
            LocalPoolError::new(ErrorCode::InvalidState, "reasoning probe level is invalid")
        })
}

fn source_probe_binding(
    source: &ProviderSourceRecord,
    model: &str,
) -> Option<(SourceProtocolBinding, String)> {
    source
        .effective_protocol_bindings()
        .ok()?
        .into_iter()
        .filter(|binding| binding.wire_api == WireApi::Responses)
        .find_map(|binding| {
            binding
                .model_ids
                .iter()
                .find(|candidate| candidate.eq_ignore_ascii_case(model))
                .cloned()
                .map(|source_model| (binding, source_model))
        })
}

async fn probe_source_reasoning(
    source: ProviderSourceRecord,
    api_key: String,
    binding: SourceProtocolBinding,
    source_model: String,
    level: String,
) -> ModelReasoningProbeSourceResult {
    let source_id = source.id.clone();
    let source_name = source.name.clone();
    let available = async {
        let connector = SourceConnector::new(
            &ProviderSource {
                id: source.id,
                name: source.name,
                base_url: source.base_url,
                api_key,
                wire_api: source.wire_api,
                models: source.models,
            },
            std::slice::from_ref(&binding),
        )
        .ok()?;
        let request = json!({
            "model": source_model.clone(),
            "input": "Reply with OK.",
            "max_output_tokens": 1,
            "reasoning": { "effort": level },
        });
        let prepared = binding
            .adapter
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Responses,
                request: &request,
                model: &source_model,
                stream: false,
                reasoning_mode: binding.reasoning_mode,
                cache_write_ttl: binding.cache_write_ttl,
                previous: None,
                response_scope: "reasoning-probe",
                response_id_seed: "reasoning-probe",
            })
            .ok()?;
        let url = connector.endpoint(binding.key(), &source_model, false)?;
        let (authorization_name, authorization) = connector.authorization_for_binding(&binding);
        let mut headers = connector.protocol_headers_for_binding(&binding);
        headers.insert(authorization_name, authorization);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()?;
        let response = client
            .post(url)
            .headers(headers)
            .json(prepared.upstream_body())
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let body = collect_limited(response, MAX_REASONING_PROBE_BODY_BYTES)
            .await
            .ok()?;
        let payload = serde_json::from_slice::<Value>(&body).ok()?;
        reasoning_probe_response_confirms(&payload, &level).then_some(())
    }
    .await
    .is_some();
    ModelReasoningProbeSourceResult {
        source_id,
        source_name,
        available,
    }
}

pub(crate) fn reasoning_probe_response_confirms(response: &Value, requested_level: &str) -> bool {
    let response = response
        .get("response")
        .filter(|value| value.is_object())
        .unwrap_or(response);
    if !response.is_object() || response.get("error").is_some() {
        return false;
    }
    if response
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| matches!(status, "failed" | "cancelled" | "incomplete" | "error"))
    {
        return false;
    }
    let has_completion_shape = response
        .get("output")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
        || response
            .get("choices")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
        || response
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
        || response
            .get("output_text")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty());
    if !has_completion_shape {
        return false;
    }
    let effective_level = [
        response.pointer("/output_config/effort"),
        response.pointer("/reasoning/effort"),
        response.get("reasoning_effort"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .find_map(zenith_relay_core::normalize_reasoning_effort);
    if effective_level.as_deref() == Some(requested_level) {
        return true;
    }
    [
        response.pointer("/usage/output_tokens_details/reasoning_tokens"),
        response.pointer("/usage/reasoning_tokens"),
    ]
    .into_iter()
    .flatten()
    .any(|tokens| tokens.as_u64().is_some_and(|tokens| tokens > 0))
}
