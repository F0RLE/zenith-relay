use super::super::{ImportItemError, ItemResult, DEFAULT_OPENAI_SOURCE_URL};
use crate::local_pool::models::ProviderSourceRecord;
use crate::local_pool::state::DesktopState;
use crate::local_pool::store::secret_store;
use sha2::{Digest, Sha256};
use url::Url;
use zenith_relay_core::accounts::ParsedImportItem;
use zenith_relay_core::error_codes;
use zenith_relay_core::WireApi;

pub(crate) fn imported_source_base_url(import_item: &ParsedImportItem) -> ItemResult<String> {
    if import_item.base_url_supplied && import_item.base_url.is_none() {
        return Err(ImportItemError::new(
            error_codes::SOURCE_BASE_URL_INVALID,
            "source base URL is invalid",
        ));
    }
    canonical_source_base_url(
        import_item
            .base_url
            .as_deref()
            .unwrap_or(DEFAULT_OPENAI_SOURCE_URL),
    )
}

pub(crate) fn imported_source_wire_api(
    import_item: &ParsedImportItem,
    existing: Option<&ProviderSourceRecord>,
) -> ItemResult<WireApi> {
    if import_item.protocol_supplied && import_item.protocol.is_none() {
        return Err(ImportItemError::new(
            error_codes::SOURCE_PROTOCOL_INVALID,
            "source protocol is invalid",
        ));
    }
    match import_item.protocol.as_deref() {
        Some("responses") => Ok(WireApi::Responses),
        Some("chat_completions") => Ok(WireApi::ChatCompletions),
        None => Ok(existing.map_or(WireApi::Responses, |source| source.wire_api)),
        _ => Err(ImportItemError::new(
            error_codes::SOURCE_PROTOCOL_INVALID,
            "source protocol is invalid",
        )),
    }
}

pub(crate) fn canonical_source_base_url(source_url: &str) -> ItemResult<String> {
    let mut url = Url::parse(source_url.trim()).map_err(|_| {
        ImportItemError::new(
            error_codes::SOURCE_BASE_URL_INVALID,
            "source base URL is invalid",
        )
    })?;
    let normalized_path = url.path().trim_end_matches('/').to_string();
    url.set_path(if normalized_path.is_empty() {
        "/"
    } else {
        &normalized_path
    });
    Ok(url.to_string().trim_end_matches('/').to_string())
}

pub(crate) fn source_identity_key(base_url: &str, api_key: &str) -> ItemResult<String> {
    let base_url = canonical_source_base_url(base_url)?;
    let secret_hash = hex::encode(Sha256::digest(api_key.as_bytes()));
    Ok(hex::encode(Sha256::digest(
        format!("source\0{base_url}\0{secret_hash}").as_bytes(),
    )))
}

pub(crate) fn find_existing_source(
    state: &DesktopState,
    base_url: &str,
    api_key: &str,
) -> ItemResult<Option<ProviderSourceRecord>> {
    let target = source_identity_key(base_url, api_key)?;
    let sources = state
        .store()
        .map_err(|_| {
            ImportItemError::new(
                error_codes::SOURCE_STORE_FAILED,
                "source store is unavailable",
            )
        })?
        .sources()
        .to_vec();
    let mut matching = Vec::new();
    for source in sources {
        let Some(secret) = secret_store::load(&source.secret_ref).map_err(|_| {
            ImportItemError::new(
                error_codes::SOURCE_SECRET_STORE_FAILED,
                "source secret store is unavailable",
            )
        })?
        else {
            continue;
        };
        if source_identity_key(&source.base_url, &secret)? == target {
            matching.push(source);
        }
    }
    match matching.len() {
        0 => Ok(None),
        1 => Ok(matching.pop()),
        _ => Err(ImportItemError::recovery(
            "multiple local sources have the same credential identity",
        )),
    }
}
