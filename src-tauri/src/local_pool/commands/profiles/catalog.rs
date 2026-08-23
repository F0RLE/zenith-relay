use crate::{
    codex_config::load_api_key_for_launch,
    launcher::is_codex_running,
    local_pool::{
        accounts::{collect_limited, LimitedBodyError},
        error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
        models::{LocalGatewayKeyRecord, ProviderSourceRecord},
        profiles::codex,
        state::DesktopState,
        store::secret_store,
    },
    platform::default_codex_home,
};
use std::time::Duration;
use url::Url;
use zenith_relay_core::{providers::chatgpt::CODEX_MODELS_CLIENT_VERSION, SourceAdapter, WireApi};

const ZENITH_API_HOST: &str = "api.zenithmarket.dev";
const MAX_CODEX_MODEL_CATALOG_BYTES: usize = 512 * 1024;

pub(super) async fn fetch_codex_model_catalog(
    base_url: &str,
    secret: &str,
) -> Result<String, CommandError> {
    let mut base = Url::parse(base_url)
        .map_err(|_| LocalPoolError::new(ErrorCode::InvalidState, "pool API address is invalid"))?;
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    let mut url = base.join("models").map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "pool model address is invalid")
    })?;
    url.query_pairs_mut()
        .append_pair("client_version", CODEX_MODELS_CLIENT_VERSION);
    let client = reqwest::Client::builder()
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
    let body = collect_limited(response, MAX_CODEX_MODEL_CATALOG_BYTES)
        .await
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                match error {
                    LimitedBodyError::TooLarge => "pool model catalog is too large",
                    LimitedBodyError::Transport => "pool model catalog could not be read",
                },
            )
        })?;
    let catalog: serde_json::Value = serde_json::from_slice(&body).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::GatewayUnavailable,
            "pool returned an invalid model catalog",
        )
    })?;
    if catalog
        .get("models")
        .and_then(serde_json::Value::as_array)
        .is_none_or(Vec::is_empty)
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "pool has no Codex-compatible models",
        )
        .into());
    }
    serde_json::to_string(&catalog)
        .map_err(|_| LocalPoolError::new(ErrorCode::InvalidState, "model catalog failed").into())
}

pub(super) async fn fetch_direct_source_model_manifest(
    base_url: &str,
    secret: &str,
) -> Result<serde_json::Value, CommandError> {
    let mut base = Url::parse(base_url).map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "source API address is invalid")
    })?;
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    let url = base.join("models").map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "source model address is invalid")
    })?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| {
            LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                "source model request could not be initialized",
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
                "source model catalog is unavailable",
            )
        })?;
    if !response.status().is_success() {
        return Err(LocalPoolError::new(
            ErrorCode::GatewayUnavailable,
            "source model catalog request was rejected",
        )
        .into());
    }
    let body = collect_limited(response, MAX_CODEX_MODEL_CATALOG_BYTES)
        .await
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                match error {
                    LimitedBodyError::TooLarge => "source model catalog is too large",
                    LimitedBodyError::Transport => "source model catalog could not be read",
                },
            )
        })?;
    let manifest: serde_json::Value = serde_json::from_slice(&body).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::GatewayUnavailable,
            "source returned an invalid model catalog",
        )
    })?;
    let has_models = manifest
        .get("models")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|models| !models.is_empty());
    let has_data = manifest
        .get("data")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|models| !models.is_empty());
    if !has_models && !has_data {
        return Err(LocalPoolError::new(
            ErrorCode::GatewayUnavailable,
            "source returned no usable model metadata",
        )
        .into());
    }
    Ok(manifest)
}

pub(super) enum CodexCatalogRefreshTarget {
    LocalGateway(String),
    DirectSource(Box<ProviderSourceRecord>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::local_pool) enum CodexCatalogRefreshStatus {
    Updated,
    Skipped,
    Deferred,
}

pub(super) fn active_catalog_refresh_target(
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
    if is_codex_running() {
        return Ok(CodexCatalogRefreshStatus::Deferred);
    }
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
            let secret = super::super::pool::ensure_local_gateway_key_secret(&key)?;
            fetch_codex_model_catalog(&format!("http://{address}/v1"), &secret)
                .await
                .map_err(|error| LocalPoolError::new(error.code, error.message))?
        }
        CodexCatalogRefreshTarget::DirectSource(source) => {
            let models = direct_source_response_models(source.as_ref())?;
            let api_key = load_direct_source_api_key(
                &source.base_url,
                &source.secret_ref,
                secret_store::load,
                load_api_key_for_launch,
                secret_store::save,
            )?;
            let manifest = Some(
                fetch_direct_source_model_manifest(&source.base_url, &api_key)
                    .await
                    .map_err(|error| LocalPoolError::new(error.code, error.message))?,
            );
            let Some(catalog) = codex::direct_source_model_catalog_with_manifest(
                &profile_dir,
                &models,
                manifest.as_ref(),
            )?
            else {
                return Ok(CodexCatalogRefreshStatus::Skipped);
            };
            catalog
        }
    };
    codex::refresh_managed_model_catalog(&profile_dir, &backup_root, &catalog)?;
    Ok(CodexCatalogRefreshStatus::Updated)
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

pub(super) fn validate_direct_source(source: &ProviderSourceRecord) -> LocalResult<Vec<String>> {
    if !source.enabled {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source must be enabled before launching ChatGPT",
        ));
    }
    direct_source_response_models(source)
}

pub(super) fn load_direct_source_api_key(
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
        && url.username().is_empty()
        && url.password().is_none()
}
