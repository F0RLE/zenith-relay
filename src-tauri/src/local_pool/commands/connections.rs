use super::{core_error, restart_after_secret_change, sync_records_or_rollback};
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
    models::{LocalPoolSnapshot, ProviderSourceRecord},
    state::DesktopState,
    store::secret_store,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, time::Duration};
use tauri::State;
use url::Url;
use uuid::Uuid;
use zenith_relay_core::{
    discover_source_models, source_points_to_gateway, ProviderSource, WireApi,
};

type CommandResult<T> = std::result::Result<T, CommandError>;
const OPENROUTER_CREDITS_URL: &str = "https://openrouter.ai/api/v1/credits";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStats {
    source_id: String,
    provider: String,
    supported: bool,
    balance: Option<String>,
    spent: Option<String>,
    requests: Option<i64>,
    requests_display: Option<String>,
    total_tokens: Option<i64>,
    total_tokens_display: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSourceInput {
    name: String,
    base_url: String,
    api_key: String,
    #[serde(default = "responses_wire_api")]
    wire_api: WireApi,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    draining: bool,
    #[serde(default)]
    allowed_models: Vec<String>,
    #[serde(default)]
    excluded_models: Vec<String>,
    #[serde(default)]
    priority: i32,
    #[serde(default = "default_weight")]
    weight: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSourceInput {
    source_id: String,
    name: String,
    base_url: String,
    wire_api: WireApi,
    models: Vec<String>,
    #[serde(default)]
    in_pool: Option<bool>,
    #[serde(default)]
    draining: bool,
    #[serde(default)]
    allowed_models: Vec<String>,
    #[serde(default)]
    excluded_models: Vec<String>,
    priority: i32,
    weight: u32,
}

#[tauri::command]
pub async fn create_local_source(
    input: CreateSourceInput,
    state: State<'_, DesktopState>,
) -> CommandResult<ProviderSourceRecord> {
    let _mutation = state.setup_guard().await;
    ensure_supported_wire_api(input.wire_api)?;
    let id = format!("source_{}", Uuid::new_v4().simple());
    let secret_ref = format!("source:{id}");
    let mut runtime_source = ProviderSource {
        id: id.clone(),
        name: input.name.trim().to_string(),
        base_url: input.base_url.trim().to_string(),
        api_key: input.api_key.trim().to_string(),
        wire_api: input.wire_api,
        models: input.models,
    };
    runtime_source.validate().map_err(core_error)?;
    ensure_not_gateway_self_source(&state, &runtime_source.base_url)?;
    runtime_source.models = discover_source_models(&runtime_source)
        .await
        .map_err(core_error)?;
    if runtime_source.models.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "source did not expose any configured models",
        )
        .into());
    }

    let mut record = ProviderSourceRecord {
        id,
        name: runtime_source.name,
        enabled: true,
        in_pool: false,
        draining: input.draining,
        base_url: runtime_source.base_url,
        secret_ref: secret_ref.clone(),
        wire_api: runtime_source.wire_api,
        models: runtime_source.models,
        allowed_models: input.allowed_models,
        excluded_models: input.excluded_models,
        priority: input.priority,
        weight: input.weight,
        last_used_at: None,
        last_test_at: Some(Utc::now().to_rfc3339()),
        last_test_status: Some("ok".into()),
        last_error: None,
    };
    record.normalize();
    let (old_sources, old_keys) = current_records(&state)?;
    secret_store::save(&secret_ref, &runtime_source.api_key)?;
    if let Err(error) = state.store()?.upsert_source(record.clone()) {
        cleanup_created_secret(&secret_ref, &error)?;
        return Err(error.into());
    }
    if let Err(error) = sync_records_or_rollback(&state, old_sources, old_keys).await {
        let source_was_rolled_back = state.store()?.source(&record.id).is_none();
        if source_was_rolled_back {
            cleanup_created_secret(&secret_ref, &error)?;
        }
        return Err(error.into());
    }
    Ok(record)
}

#[tauri::command]
pub async fn update_local_source(
    input: UpdateSourceInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let current = state
        .store()?
        .source(&input.source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    let mut updated = ProviderSourceRecord {
        id: current.id.clone(),
        name: input.name,
        enabled: current.enabled,
        in_pool: current.in_pool,
        draining: input.draining,
        base_url: input.base_url,
        secret_ref: current.secret_ref.clone(),
        wire_api: input.wire_api,
        models: input.models,
        allowed_models: input.allowed_models,
        excluded_models: input.excluded_models,
        priority: input.priority,
        weight: input.weight,
        last_used_at: current.last_used_at,
        last_test_at: current.last_test_at,
        last_test_status: current.last_test_status,
        last_error: current.last_error,
    };
    if let Some(in_pool) = input.in_pool {
        updated.in_pool = in_pool;
    }
    updated.normalize();
    validate_source_record(&state, &updated)?;
    let (old_sources, old_keys) = current_records(&state)?;
    state.store()?.upsert_source(updated)?;
    sync_records_or_rollback(&state, old_sources, old_keys).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_local_source_enabled(
    source_id: String,
    enabled: bool,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let mut source = state
        .store()?
        .source(&source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    if source.enabled == enabled {
        return state.snapshot().await.map_err(Into::into);
    }
    if enabled {
        validate_source_record(&state, &source)?;
    }
    let (old_sources, old_keys) = current_records(&state)?;
    source.enabled = enabled;
    state.store()?.upsert_source(source)?;
    sync_records_or_rollback(&state, old_sources, old_keys).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn delete_local_source(
    source_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let source = state
        .store()?
        .source(&source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    let old_secret = secret_store::load(&source.secret_ref)?;
    let (old_sources, old_keys) = current_records(&state)?;
    let sources = old_sources
        .iter()
        .filter(|candidate| candidate.id != source_id)
        .cloned()
        .collect::<Vec<_>>();
    let mut keys = old_keys.clone();
    prune_key_source_scopes(&mut keys, &sources);
    state.store()?.replace_records(sources, keys)?;
    sync_records_or_rollback(&state, old_sources.clone(), old_keys.clone()).await?;

    if let Err(cleanup) = secret_store::delete(&source.secret_ref) {
        if let Some(secret) = old_secret {
            secret_store::save(&source.secret_ref, &secret).map_err(|restore| {
                LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("{cleanup}; failed to restore source secret: {restore}"),
                )
            })?;
            let (deleted_sources, deleted_keys) = current_records(&state)?;
            let restore_records = { state.store()?.replace_records(old_sources, old_keys) };
            if let Err(restore) = restore_records {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("{cleanup}; failed to restore deleted source records: {restore}"),
                )
                .into());
            }
            if let Err(restore) =
                sync_records_or_rollback(&state, deleted_sources, deleted_keys).await
            {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("{cleanup}; failed to restore gateway after source cleanup: {restore}"),
                )
                .into());
            }
        }
        return Err(cleanup.into());
    }
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn rotate_local_source_key(
    source_id: String,
    api_key: String,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let source = state
        .store()?
        .source(&source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    ensure_supported_wire_api(source.wire_api)?;
    let api_key = api_key.trim().to_string();
    ProviderSource {
        id: source.id.clone(),
        name: source.name.clone(),
        base_url: source.base_url.clone(),
        api_key: api_key.clone(),
        wire_api: source.wire_api,
        models: source.models.clone(),
    }
    .validate()
    .map_err(core_error)?;
    ensure_not_gateway_self_source(&state, &source.base_url)?;
    let old_secret = secret_store::load(&source.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
    secret_store::save(&source.secret_ref, &api_key)?;
    restart_after_secret_change(&state, &source.secret_ref, &old_secret).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn test_local_source(
    source_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<ProviderSourceRecord> {
    let _mutation = state.setup_guard().await;
    let source = state
        .store()?
        .source(&source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    ensure_supported_wire_api(source.wire_api)?;
    let api_key = secret_store::load(&source.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
    let runtime_source = ProviderSource {
        id: source.id.clone(),
        name: source.name.clone(),
        base_url: source.base_url.clone(),
        api_key,
        wire_api: source.wire_api,
        models: source.models.clone(),
    };
    ensure_not_gateway_self_source(&state, &runtime_source.base_url)?;
    let models = match discover_source_models(&runtime_source).await {
        Ok(models) => models,
        Err(error) => {
            let error = core_error(error);
            let mut failed = source.clone();
            failed.last_test_at = Some(Utc::now().to_rfc3339());
            failed.last_test_status = Some("error".into());
            failed.last_error = Some(error.message.clone());
            state.store()?.upsert_source(failed)?;
            return Err(error.into());
        }
    };
    if models.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "source did not expose any configured models",
        )
        .into());
    }
    let runtime_changed = source.models != models;
    let mut updated = source;
    updated.models = models;
    updated.last_test_at = Some(Utc::now().to_rfc3339());
    updated.last_test_status = Some("ok".into());
    updated.last_error = None;
    updated.normalize();
    let (old_sources, old_keys) = current_records(&state)?;
    state.store()?.upsert_source(updated.clone())?;
    if runtime_changed {
        sync_records_or_rollback(&state, old_sources, old_keys).await?;
    }
    Ok(updated)
}

#[tauri::command]
pub async fn get_local_source_stats(
    source_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<SourceStats> {
    let source = state
        .store()?
        .source(&source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    let provider = source_stats_provider(&source.base_url);
    if provider == "unsupported" {
        return Ok(unsupported_source_stats(source_id));
    }
    let api_key = secret_store::load(&source.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
    match provider {
        "zenith" => {
            let stats = crate::fetch_key_stats(&api_key)
                .await
                .map_err(|message| LocalPoolError::new(ErrorCode::GatewayUnavailable, message))?;
            Ok(SourceStats {
                source_id,
                provider: "zenith".into(),
                supported: true,
                balance: Some(stats.balance),
                spent: Some(stats.spent),
                requests: Some(stats.requests),
                requests_display: Some(stats.requests_display),
                total_tokens: Some(stats.total_tokens),
                total_tokens_display: Some(stats.total_tokens_display),
            })
        }
        "openrouter" => fetch_openrouter_stats(source_id, &api_key).await,
        _ => unreachable!(),
    }
}

async fn fetch_openrouter_stats(source_id: String, api_key: &str) -> CommandResult<SourceStats> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| {
            LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                "OpenRouter stats request could not be initialized",
            )
        })?;
    let response = client
        .get(OPENROUTER_CREDITS_URL)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|_| {
            LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                "OpenRouter stats request failed",
            )
        })?;
    if !response.status().is_success() {
        return Err(LocalPoolError::new(
            ErrorCode::GatewayUnavailable,
            format!(
                "OpenRouter stats request failed ({})",
                response.status().as_u16()
            ),
        )
        .into());
    }
    let payload = response.json::<serde_json::Value>().await.map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "OpenRouter stats response is invalid",
        )
    })?;
    openrouter_stats_from_value(source_id, &payload).map_err(Into::into)
}

fn openrouter_stats_from_value(
    source_id: String,
    payload: &serde_json::Value,
) -> LocalResult<SourceStats> {
    let data = payload.get("data").unwrap_or(payload);
    let total_credits =
        number_field(data, &["total_credits", "totalCredits"]).ok_or_else(|| {
            LocalPoolError::new(ErrorCode::InvalidState, "OpenRouter credits are missing")
        })?;
    let total_usage = number_field(data, &["total_usage", "totalUsage"]).ok_or_else(|| {
        LocalPoolError::new(ErrorCode::InvalidState, "OpenRouter usage is missing")
    })?;
    Ok(SourceStats {
        source_id,
        provider: "openrouter".into(),
        supported: true,
        balance: format_usd(total_credits - total_usage),
        spent: format_usd(total_usage),
        requests: None,
        requests_display: None,
        total_tokens: None,
        total_tokens_display: None,
    })
}

fn source_stats_provider(base_url: &str) -> &'static str {
    let Ok(url) = Url::parse(base_url) else {
        return "unsupported";
    };
    match url.host_str().map(str::to_ascii_lowercase).as_deref() {
        Some("api.zenithmarket.dev") => "zenith",
        Some("openrouter.ai") => "openrouter",
        _ => "unsupported",
    }
}

fn unsupported_source_stats(source_id: String) -> SourceStats {
    SourceStats {
        source_id,
        provider: "unsupported".into(),
        supported: false,
        balance: None,
        spent: None,
        requests: None,
        requests_display: None,
        total_tokens: None,
        total_tokens_display: None,
    }
}

fn number_field(value: &serde_json::Value, names: &[&str]) -> Option<f64> {
    names.iter().find_map(|name| value.get(name)?.as_f64())
}

fn format_usd(value: f64) -> Option<String> {
    if !value.is_finite() {
        return None;
    }
    let microusd = (value * 1_000_000.0).round();
    if microusd < i64::MIN as f64 || microusd > i64::MAX as f64 {
        return None;
    }
    Some(crate::format_money_microusd(microusd as i64))
}

fn validate_source_record(state: &DesktopState, source: &ProviderSourceRecord) -> LocalResult<()> {
    ensure_supported_wire_api(source.wire_api)?;
    let api_key = secret_store::load(&source.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
    if source.models.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "source must expose at least one model",
        ));
    }
    let runtime_source = ProviderSource {
        id: source.id.clone(),
        name: source.name.clone(),
        base_url: source.base_url.clone(),
        api_key,
        wire_api: source.wire_api,
        models: source.models.clone(),
    };
    runtime_source.validate().map_err(core_error)?;
    ensure_not_gateway_self_source(state, &runtime_source.base_url)
}

fn ensure_not_gateway_self_source(state: &DesktopState, base_url: &str) -> LocalResult<()> {
    let gateway = state.store()?.gateway().clone();
    let gateway_base_url = format!("http://{}:{}/v1", gateway.client_host, gateway.port);
    if source_points_to_gateway(base_url, &gateway_base_url) {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source base URL must not point back to this Relay gateway",
        ));
    }
    Ok(())
}

fn ensure_supported_wire_api(wire_api: WireApi) -> LocalResult<()> {
    if matches!(wire_api, WireApi::Messages) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "messages wire API is not supported by the local runtime",
        ));
    }
    Ok(())
}

fn current_records(
    state: &DesktopState,
) -> LocalResult<(
    Vec<ProviderSourceRecord>,
    Vec<crate::local_pool::models::LocalGatewayKeyRecord>,
)> {
    let store = state.store()?;
    Ok((store.sources().to_vec(), store.keys().to_vec()))
}

fn cleanup_created_secret(secret_ref: &str, cause: &LocalPoolError) -> LocalResult<()> {
    secret_store::delete(secret_ref).map_err(|cleanup| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "{}; secret cleanup failed: {}",
                cause.message, cleanup.message
            ),
        )
    })
}

fn responses_wire_api() -> WireApi {
    WireApi::Responses
}

fn default_weight() -> u32 {
    1
}

fn prune_key_source_scopes(
    keys: &mut [crate::local_pool::models::LocalGatewayKeyRecord],
    sources: &[ProviderSourceRecord],
) {
    let valid_ids = sources
        .iter()
        .map(|source| source.id.as_str())
        .collect::<HashSet<_>>();
    for key in keys {
        if let Some(source_ids) = &mut key.source_ids {
            source_ids.retain(|id| valid_ids.contains(id.as_str()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::models::LocalGatewayKeyRecord;

    #[test]
    fn deleting_source_keeps_explicit_empty_scope_unavailable() {
        let mut keys = [LocalGatewayKeyRecord {
            id: "key_1".into(),
            label: "Scoped".into(),
            enabled: true,
            system: false,
            secret_ref: "key:key_1".into(),
            source_ids: Some(vec!["source_1".into()]),
            account_ids: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
            created_at: "2026-07-10T00:00:00Z".into(),
            last_used_at: None,
        }];
        prune_key_source_scopes(&mut keys, &[]);
        assert_eq!(keys[0].source_ids, Some(Vec::new()));
    }

    #[test]
    fn messages_wire_api_is_rejected_at_the_desktop_boundary() {
        assert!(ensure_supported_wire_api(WireApi::Responses).is_ok());
        assert!(ensure_supported_wire_api(WireApi::ChatCompletions).is_ok());
        assert!(matches!(
            ensure_supported_wire_api(WireApi::Messages)
                .unwrap_err()
                .code,
            ErrorCode::InvalidState
        ));
    }

    #[test]
    fn stats_provider_requires_an_exact_known_host() {
        assert_eq!(
            source_stats_provider("https://api.zenithmarket.dev/v1"),
            "zenith"
        );
        assert_eq!(
            source_stats_provider("https://openrouter.ai/api/v1"),
            "openrouter"
        );
        assert_eq!(
            source_stats_provider("https://openrouter.ai.evil.test/api/v1"),
            "unsupported"
        );
        assert_eq!(source_stats_provider("not a URL"), "unsupported");
    }

    #[test]
    fn openrouter_credits_are_normalized_without_inventing_usage_fields() {
        let stats = openrouter_stats_from_value(
            "source_1".into(),
            &serde_json::json!({"data": {"total_credits": 25.5, "total_usage": 4.25}}),
        )
        .unwrap();
        assert_eq!(stats.provider, "openrouter");
        assert_eq!(stats.balance.as_deref(), Some("$21.250000"));
        assert_eq!(stats.spent.as_deref(), Some("$4.250000"));
        assert_eq!(stats.requests, None);
        assert_eq!(stats.total_tokens, None);
    }
}
