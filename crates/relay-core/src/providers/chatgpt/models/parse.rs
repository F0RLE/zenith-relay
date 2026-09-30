use super::super::{valid_access_token, valid_codex_client_version};
use super::{
    ModelDiscoveryFailure, ModelDiscoveryFailureCode, MAX_ACCOUNT_ID_BYTES, MAX_MODELS,
    MAX_MODEL_SLUG_BYTES,
};
use serde::Deserialize;
use std::collections::HashSet;
use url::Url;

use crate::url_has_userinfo;

#[derive(Deserialize)]
struct ModelsResponse {
    models: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    slug: String,
}

pub(super) fn parse_models(body: &[u8]) -> Result<Vec<String>, ModelDiscoveryFailure> {
    let response: ModelsResponse = serde_json::from_slice(body)
        .map_err(|_| ModelDiscoveryFailure::new(ModelDiscoveryFailureCode::InvalidResponse))?;
    if response.models.len() > MAX_MODELS {
        return Err(ModelDiscoveryFailure::new(
            ModelDiscoveryFailureCode::InvalidResponse,
        ));
    }
    let mut seen = HashSet::new();
    Ok(response
        .models
        .into_iter()
        // This endpoint is the account's authoritative model inventory.
        // `supported_in_api` describes the upstream's current API capability,
        // not whether the account owns the model. Retaining the model lets
        // Relay expose newly enabled capabilities without a hardcoded list.
        .filter_map(|model| {
            let slug = model.slug.trim();
            (!slug.is_empty()
                && slug.len() <= MAX_MODEL_SLUG_BYTES
                && !slug.chars().any(char::is_control)
                && seen.insert(slug.to_string()))
            .then(|| slug.to_string())
        })
        .collect())
}

pub(super) fn validate_endpoint(endpoint: &Url) -> Result<(), ModelDiscoveryFailure> {
    let loopback_http = endpoint.scheme() == "http"
        && endpoint
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"));
    if (endpoint.scheme() != "https" && !loopback_http)
        || endpoint.host_str().is_none()
        || url_has_userinfo(endpoint)
        || endpoint.fragment().is_some()
    {
        Err(ModelDiscoveryFailure::new(
            ModelDiscoveryFailureCode::InvalidEndpoint,
        ))
    } else {
        Ok(())
    }
}

pub(super) fn validate_access_token(access_token: &str) -> Result<(), ModelDiscoveryFailure> {
    if !valid_access_token(access_token) {
        Err(ModelDiscoveryFailure::new(
            ModelDiscoveryFailureCode::InvalidAccessToken,
        ))
    } else {
        Ok(())
    }
}

pub(super) fn validate_account_id(account_id: &str) -> Result<(), ModelDiscoveryFailure> {
    if account_id.is_empty()
        || account_id.len() > MAX_ACCOUNT_ID_BYTES
        || account_id.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(ModelDiscoveryFailure::new(
            ModelDiscoveryFailureCode::InvalidAccountId,
        ))
    } else {
        Ok(())
    }
}

pub(super) fn validate_client_version(client_version: &str) -> Result<(), ModelDiscoveryFailure> {
    if valid_codex_client_version(client_version) {
        Ok(())
    } else {
        Err(ModelDiscoveryFailure::new(
            ModelDiscoveryFailureCode::InvalidClientVersion,
        ))
    }
}
