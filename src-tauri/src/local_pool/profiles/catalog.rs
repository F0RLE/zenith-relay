use crate::{
    launcher::is_codex_running,
    local_pool::{
        accounts::{LimitedBodyError, collect_limited},
        error::{CommandError, ErrorCode, ErrorDiagnostics, LocalPoolError, Result as LocalResult},
        models::{LocalGatewayKeyRecord, ProviderSourceRecord},
        profiles::codex,
        state::DesktopState,
    },
    platform::default_codex_home,
};
use std::{future::Future, time::Duration};
use url::Url;
use zenith_relay_core::{
    SourceAdapter, WireApi, providers::chatgpt::configured_codex_client_version, url_has_userinfo,
};

const ZENITH_API_HOST: &str = "api.zenithmarket.dev";
// Keep this bound aligned with the shared model-discovery transport limit and
// the profile catalog parser. Full ChatGPT inventories may exceed 512 KiB.
const MAX_CODEX_MODEL_CATALOG_BYTES: usize = 4 * 1024 * 1024;

pub(in crate::local_pool) async fn fetch_local_codex_model_catalog(
    base_url: &str,
    secret: &str,
) -> Result<String, CommandError> {
    fetch_codex_model_catalog(base_url, secret, false).await
}

async fn fetch_codex_model_catalog(
    base_url: &str,
    secret: &str,
    allow_empty: bool,
) -> Result<String, CommandError> {
    let mut base = Url::parse(base_url)
        .map_err(|_| LocalPoolError::new(ErrorCode::InvalidState, "pool API address is invalid"))?;
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    let mut url = base.join("models").map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "pool model address is invalid")
    })?;
    let client_version = configured_codex_client_version();
    url.query_pairs_mut()
        .append_pair("client_version", &client_version);
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| {
            LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                "pool model request could not be initialized",
            )
        })?;
    let response = client
        .get(url)
        .bearer_auth(secret)
        .send()
        .await
        .map_err(|_| {
            LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                "pool model catalog is unavailable",
            )
        })?;
    if !response.status().is_success() {
        return Err(LocalPoolError::new(
            ErrorCode::GatewayUnavailable,
            "pool model catalog request was rejected",
        )
        .into());
    }
    let catalog_response_body = collect_limited(response, MAX_CODEX_MODEL_CATALOG_BYTES)
        .await
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                match error {
                    LimitedBodyError::TooLarge => "pool model catalog exceeds 4 MiB",
                    LimitedBodyError::Transport => "pool model catalog could not be read",
                },
            )
        })?;
    let catalog: serde_json::Value =
        serde_json::from_slice(&catalog_response_body).map_err(|_| {
            LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                "pool returned an invalid model catalog",
            )
        })?;
    let models = catalog
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                "pool returned an invalid model catalog",
            )
        })?;
    if models.is_empty() && !allow_empty {
        return Err(LocalPoolError::new(
            ErrorCode::PoolModelsUnavailable,
            "pool has no available ChatGPT-compatible models; refresh model access for enabled pool connections; quota availability alone does not grant model access",
        )
        .with_diagnostic(ErrorDiagnostics {
            reason: Some("empty_model_catalog".into()),
            route: Some("/v1/models".into()),
            ..Default::default()
        })
        .into());
    }
    serde_json::to_string(&catalog)
        .map_err(|_| LocalPoolError::new(ErrorCode::InvalidState, "model catalog failed").into())
}

pub(in crate::local_pool) enum CodexCatalogRefreshTarget {
    LocalGateway(String),
    DirectSource(Box<ProviderSourceRecord>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::local_pool) enum CodexCatalogRefreshStatus {
    Updated,
    Skipped,
    Deferred,
}

pub(super) async fn refresh_client_catalogs(
    opencode: impl Future<Output = LocalResult<()>>,
    codex: impl Future<Output = LocalResult<CodexCatalogRefreshStatus>>,
) -> LocalResult<CodexCatalogRefreshStatus> {
    // These profiles are independent: an invalid OpenCode file must not
    // prevent Codex from applying or scheduling its own catalog update.
    let opencode_result = opencode.await;
    let codex_result = codex.await;
    codex_result.and_then(|status| opencode_result.map(|()| status))
}

pub(in crate::local_pool) fn active_catalog_refresh_target(
    binding: &codex::ProfileBinding,
    keys: &[LocalGatewayKeyRecord],
    sources: &[ProviderSourceRecord],
) -> Option<CodexCatalogRefreshTarget> {
    if !binding.active || binding.credential_kind != codex::ProfileCredentialKind::LocalGateway {
        return None;
    }
    keys.iter()
        .find(|key| key.system && key.id == binding.credential_id)
        .map(|key| CodexCatalogRefreshTarget::LocalGateway(key.id.clone()))
        .or_else(|| {
            sources
                .iter()
                .find(|source| source.enabled && source.id == binding.credential_id)
                .and_then(|source| {
                    direct_source_response_models(source)
                        .ok()
                        .map(|_| CodexCatalogRefreshTarget::DirectSource(Box::new(source.clone())))
                })
        })
}

pub(in crate::local_pool) async fn refresh_active_codex_catalog(
    state: &DesktopState,
) -> LocalResult<CodexCatalogRefreshStatus> {
    let client_running = is_codex_running().map_err(LocalPoolError::io)?;
    let profile_dir = default_codex_home();
    let backup_root = state.profile_backup_root();
    let Some(binding) = codex::profile_bindings(&profile_dir, &backup_root)?
        .into_iter()
        .find(|binding| {
            binding.active && binding.credential_kind == codex::ProfileCredentialKind::LocalGateway
        })
    else {
        return Ok(CodexCatalogRefreshStatus::Skipped);
    };
    let target = {
        let store = state.store()?;
        active_catalog_refresh_target(&binding, store.keys(), store.sources())
    };
    let Some(target) = target else {
        return Ok(CodexCatalogRefreshStatus::Skipped);
    };
    let mut supports_websockets = None;
    let catalog = match target {
        CodexCatalogRefreshTarget::LocalGateway(key_id) => {
            let Some(address) = state.gateway.address().await else {
                return Ok(CodexCatalogRefreshStatus::Skipped);
            };
            let Some(key) = state
                .store()?
                .key(&key_id)
                .filter(|key| key.system)
                .cloned()
            else {
                return Ok(CodexCatalogRefreshStatus::Skipped);
            };
            let secret =
                crate::local_pool::store::gateway_keys::ensure_local_gateway_key_secret(&key)?;
            supports_websockets = Some(state.store()?.gateway().codex_websockets_enabled);
            fetch_codex_model_catalog(&format!("http://{address}/v1"), &secret, true)
                .await
                .map_err(|error| LocalPoolError::new(error.code, error.message))?
        }
        CodexCatalogRefreshTarget::DirectSource(source) => {
            let models = direct_source_response_models(source.as_ref())?;
            codex::direct_source_model_catalog_with_capabilities(
                &profile_dir,
                &models,
                &state.model_metadata_catalog(),
                &source
                    .protocol_config
                    .effective_capabilities(&source.base_url, &source.models),
            )?
            .unwrap_or_else(|| "{\"models\":[]}".into())
        }
    };
    let catalog = context_catalog(state, &catalog)?;
    // The managed catalog has its own ownership checks and atomic replacement.
    // Updating it does not require stopping the client or changing its profile.
    if client_running {
        codex::refresh_managed_model_catalog(&profile_dir, &backup_root, &catalog, Some(&binding))?;
        // The managed catalog is updated, but context policy and websocket
        // transport changes are deferred until the client is restarted.
        // Recording Deferred persists a warning so the UI can prompt for a restart.
        return Ok(CodexCatalogRefreshStatus::Deferred);
    }
    let previous_transport = supports_websockets
        .map(|enabled| {
            codex::set_local_gateway_websockets_with_previous(
                &profile_dir,
                &backup_root,
                enabled,
                Some(&binding.credential_id),
            )
        })
        .transpose()?
        .flatten();
    let context_change = match codex::sync_context_policy(&profile_dir, &backup_root) {
        Ok(change) => change,
        Err(error) => {
            if let Some(previous_transport_config) = previous_transport {
                codex::set_local_gateway_websockets_with_previous(
                    &profile_dir,
                    &backup_root,
                    previous_transport_config,
                    Some(&binding.credential_id),
                )?;
            }
            return Err(error);
        }
    };
    if let Err(error) =
        codex::refresh_managed_model_catalog(&profile_dir, &backup_root, &catalog, Some(&binding))
    {
        let context_rollback = context_change.rollback();
        if let Some(previous_transport_config) = previous_transport {
            codex::set_local_gateway_websockets_with_previous(
                &profile_dir,
                &backup_root,
                previous_transport_config,
                Some(&binding.credential_id),
            )?;
        }
        return Err(context_rollback.err().unwrap_or(error));
    }
    Ok(CodexCatalogRefreshStatus::Updated)
}

pub(in crate::local_pool) fn context_catalog(
    state: &DesktopState,
    catalog: &str,
) -> LocalResult<String> {
    use serde_json::{Value, json};
    let model_policies = state.store()?.gateway().model_context_policies.clone();
    let mut document: Value =
        serde_json::from_str(catalog).map_err(LocalPoolError::invalid_state)?;
    let metadata = state.model_metadata_catalog();
    let mut limits = serde_json::Map::new();
    if let Some(models) = document.get("models").and_then(Value::as_array) {
        for row in models {
            let Some(slug) = row.get("slug").and_then(Value::as_str) else {
                continue;
            };
            let model = zenith_relay_core::decode_codex_model_alias(slug)
                .unwrap_or_else(|| slug.to_owned());
            let key = zenith_relay_core::model_id_key(&model);
            let scoped = document
                .get("relay_context_limits")
                .and_then(|limits| limits.get(&key));
            limits.insert(
                key,
                json!(catalog_context_limits(row, &model, &metadata, scoped)?),
            );
        }
    }
    document["relay_model_context_policies"] = json!(model_policies);
    document["relay_context_limits"] = Value::Object(limits);
    serde_json::to_string(&document).map_err(LocalPoolError::invalid_state)
}

fn catalog_context_limits(
    row: &serde_json::Value,
    model: &str,
    metadata: &zenith_relay_core::model_metadata::ModelMetadataCatalog,
    scoped: Option<&serde_json::Value>,
) -> LocalResult<zenith_relay_core::ModelTokenLimits> {
    let scoped = scoped
        .cloned()
        .map(serde_json::from_value::<zenith_relay_core::ModelTokenLimits>)
        .transpose()
        .map_err(LocalPoolError::invalid_state)?
        .unwrap_or_default();
    let mut capabilities = metadata.capabilities_for(model);
    scoped.constrain(&mut capabilities);
    Ok(zenith_relay_core::ModelTokenLimits {
        context: row
            .get("max_context_window")
            .and_then(serde_json::Value::as_u64)
            .into_iter()
            .chain(scoped.context)
            .min(),
        input: capabilities.input_limit,
        output: capabilities.output_limit,
    })
}

pub(super) fn direct_source_response_models(
    source: &ProviderSourceRecord,
) -> LocalResult<Vec<String>> {
    let bindings = source
        .effective_protocol_bindings()
        .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    let Some(binding) = bindings.into_iter().find(|binding| {
        binding.wire_api == WireApi::Responses && binding.adapter == SourceAdapter::Native
    }) else {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "direct ChatGPT launch requires a native Responses API source",
        ));
    };
    if binding.model_ids.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source has no Responses API models",
        ));
    }
    Ok(binding.model_ids)
}

pub(in crate::local_pool) fn validate_direct_source(
    source: &ProviderSourceRecord,
) -> LocalResult<Vec<String>> {
    if !source.enabled {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source must be enabled before launching ChatGPT",
        ));
    }
    direct_source_response_models(source)
}

pub(in crate::local_pool) fn load_direct_source_api_key(
    base_url: &str,
    secret_ref: &str,
    load: impl FnOnce(&str) -> LocalResult<Option<String>>,
    load_legacy_zenith_key: impl FnOnce() -> Option<String>,
    save: impl FnOnce(&str, &str) -> LocalResult<()>,
) -> LocalResult<String> {
    if let Some(api_key) = load(secret_ref)? {
        return Ok(api_key);
    }
    let api_key = is_zenith_api_base_url(base_url)
        .then(load_legacy_zenith_key)
        .flatten()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
    save(secret_ref, &api_key)?;
    Ok(api_key)
}

fn is_zenith_api_base_url(base_url: &str) -> bool {
    let Ok(url) = Url::parse(base_url) else {
        return false;
    };
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case(ZENITH_API_HOST))
        && url.port_or_known_default() == Some(443)
        && url.path().trim_end_matches('/') == "/v1"
        && url.query().is_none()
        && url.fragment().is_none()
        && !url_has_userinfo(&url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn context_policy_retains_route_limits_and_does_not_invent_native_windows() {
        use serde_json::json;
        let metadata =
            zenith_relay_core::model_metadata::ModelMetadataCatalog::from_models_dev_json(
                &json!({"synthetic": {"limit":{"context":100000,"input":90000,"output":10000}}})
                    .to_string(),
            )
            .unwrap();
        let row = json!({"max_context_window":2000});
        let scoped = json!({"context":2000,"input":1800,"output":200});
        let limits = catalog_context_limits(&row, "synthetic", &metadata, Some(&scoped)).unwrap();
        assert_eq!(limits.context, Some(2000));
        assert_eq!(limits.input, Some(1800));
        assert_eq!(limits.output, Some(200));
        let generous = json!({"context":1000000,"input":1000000,"output":1000000});
        let limits = catalog_context_limits(&row, "synthetic", &metadata, Some(&generous)).unwrap();
        assert_eq!(limits.context, Some(2000));
        assert_eq!(limits.input, Some(90000));
        assert_eq!(limits.output, Some(10000));
        assert_eq!(
            catalog_context_limits(&json!({}), "synthetic", &metadata, None)
                .unwrap()
                .context,
            None
        );
        assert!(
            catalog_context_limits(
                &row,
                "synthetic",
                &metadata,
                Some(&json!({"input":"invalid"}))
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn authoritative_empty_catalog_is_allowed_for_refresh_but_blocks_initial_attachment() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/v1/models",
            axum::routing::get(|| async { axum::Json(serde_json::json!({"models":[]})) }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let base_url = format!("http://{address}/v1");
        let refreshed = fetch_codex_model_catalog(&base_url, "synthetic-key", true)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&refreshed).unwrap()["models"],
            serde_json::json!([])
        );
        let error = fetch_local_codex_model_catalog(&base_url, "synthetic-key")
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::PoolModelsUnavailable);
        server.abort();
    }
    #[tokio::test]
    async fn opencode_failure_does_not_prevent_codex_catalog_refresh() {
        let refreshed = Cell::new(false);
        let catalog_refresh_result = refresh_client_catalogs(
            async {
                Err(LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "synthetic invalid config",
                ))
            },
            async {
                refreshed.set(true);
                Ok(CodexCatalogRefreshStatus::Updated)
            },
        )
        .await;
        assert!(refreshed.get());
        assert!(matches!(
            catalog_refresh_result.unwrap_err().code,
            ErrorCode::InvalidState
        ));
    }
}
