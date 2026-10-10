use super::observations::{SourceRead, SourceReadHints};
use super::{
    capabilities::catalog_capabilities, normalize_source_protocol_bindings, service_protocol,
    ModelEndpointCapability, ProviderSource, SourceAdapter, SourceConnector, SourceProtocolBinding,
    SourceProtocolBindingKey, SourceProtocolConfig,
};
use crate::scheduler::refresh::http::ManagementHttpScope;
use crate::{ApiModelPriceOverride, Result};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Duration;

mod fetch;
mod pricing;

pub(crate) use fetch::discover_models_with_client;
use pricing::detected_model_price;

pub async fn discover_source_models(source: &ProviderSource) -> Result<Vec<String>> {
    discover_source_models_for_protocol_bindings(source, &[]).await
}

/// Discovers a source catalog using each explicitly configured binding.
/// Authentication and the discovery endpoint follow the binding's upstream
/// protocol. No request body or response format is adapted during discovery.
pub async fn discover_source_models_for_protocol_bindings(
    source: &ProviderSource,
    protocol_bindings: &[SourceProtocolBinding],
) -> Result<Vec<String>> {
    discover_source_models_and_protocol_bindings(source, protocol_bindings)
        .await
        .map(|discovery| discovery.models)
}

/// The result of binding-aware model discovery.
///
/// `models` is the de-duplicated union in upstream response order. Each
/// `protocol_bindings` entry normally contains the models advertised for that
/// binding after its explicit allow-list is applied. A successful automatic
/// source-wide binding is preserved with an empty `model_ids`; `models` still
/// holds its current discovered catalog. A successful `/models` response is
/// catalog evidence, not a completion capability probe. Runtime routes are
/// resolved automatically from the catalog and protocol hints. A valid empty
/// response is still success and is represented by a binding with
/// an empty `model_ids` list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceDiscovery {
    pub models: Vec<String>,
    pub protocol_bindings: Vec<SourceProtocolBinding>,
    /// A provider may expose its OpenAI-compatible catalog below `/v1` even
    /// when the user entered only the host root. This is set only after the
    /// root request returned 404 and the `/v1` retry succeeded.
    pub resolved_base_url: Option<String>,
    /// Complete token prices declared by the source model catalog. These are
    /// refreshed with discovery and never replace a user-configured override.
    pub detected_model_prices: BTreeMap<String, ApiModelPriceOverride>,
    pub capabilities: Vec<ModelEndpointCapability>,
}

impl SourceDiscovery {
    /// Copies catalog evidence onto the stored source fields.
    /// A resolved base URL replaces the endpoint. When discovery did not
    /// resolve one, the current endpoint stays unchanged.
    pub fn apply_catalog(
        &self,
        base_url: &mut String,
        models: &mut Vec<String>,
        protocol_bindings: &mut Vec<SourceProtocolBinding>,
        protocol_config: &mut SourceProtocolConfig,
        detected_model_prices: &mut BTreeMap<String, ApiModelPriceOverride>,
    ) {
        if let Some(resolved) = &self.resolved_base_url {
            *base_url = resolved.clone();
        }
        *models = self.models.clone();
        *protocol_bindings = self.protocol_bindings.clone();
        protocol_config.merge_catalog(self.capabilities.clone());
        *detected_model_prices = self.detected_model_prices.clone();
    }
}

/// Stored fields replaced by [`SourceDiscovery::apply_catalog`].
/// Operator policy, secrets, and membership stay outside this comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceCatalogEvidence<'a> {
    pub base_url: &'a str,
    pub models: &'a [String],
    pub protocol_bindings: &'a [SourceProtocolBinding],
    pub protocol_config: &'a SourceProtocolConfig,
    pub detected_model_prices: &'a BTreeMap<String, ApiModelPriceOverride>,
}

/// A host record can expose the catalog fields written by discovery.
pub trait SourceCatalogRecord {
    fn catalog_evidence(&self) -> SourceCatalogEvidence<'_>;
}

impl SourceCatalogRecord for SourceCatalogEvidence<'_> {
    fn catalog_evidence(&self) -> SourceCatalogEvidence<'_> {
        *self
    }
}

/// Whether discovery replaced any stored catalog field.
pub fn source_catalog_changed<T: SourceCatalogRecord + ?Sized>(
    previous_catalog: &T,
    updated_source: &T,
) -> bool {
    previous_catalog.catalog_evidence() != updated_source.catalog_evidence()
}

/// Catalog refresh is read-only at the provider: it never sends a generation.
pub async fn discover_source_with_protocol_config(
    source: &ProviderSource,
    bindings: &[SourceProtocolBinding],
    config: &SourceProtocolConfig,
) -> Result<SourceDiscovery> {
    discover_source_with_protocol_config_with_scope(
        source,
        bindings,
        config,
        ManagementHttpScope::default(),
    )
    .await
}

pub async fn discover_source_with_protocol_config_with_scope(
    source: &ProviderSource,
    bindings: &[SourceProtocolBinding],
    config: &SourceProtocolConfig,
    scope: ManagementHttpScope,
) -> Result<SourceDiscovery> {
    read_source_models_with_scope(source, bindings, config, scope)
        .await
        .read_value
}

pub async fn read_source_models(
    source: &ProviderSource,
    bindings: &[SourceProtocolBinding],
    config: &SourceProtocolConfig,
) -> SourceRead<Result<SourceDiscovery>> {
    read_source_models_with_scope(source, bindings, config, ManagementHttpScope::default()).await
}

pub async fn read_source_models_with_scope(
    source: &ProviderSource,
    bindings: &[SourceProtocolBinding],
    config: &SourceProtocolConfig,
    scope: ManagementHttpScope,
) -> SourceRead<Result<SourceDiscovery>> {
    let hints = SourceReadHints::default();
    let mut catalog_source = source.clone();
    catalog_source.wire_api = config
        .endpoint_hint
        .or_else(|| super::endpoint_url_protocol(&source.base_url))
        .or_else(|| service_protocol(&source.base_url))
        .unwrap_or(source.wire_api);
    // Stored bindings are discovery hints for providers that expose distinct
    // catalogs per physical protocol. Runtime routes are still recomputed from
    // the resulting catalog and capability evidence.
    let discovery_result = read_bindings(&catalog_source, bindings, &hints, &scope).await;
    SourceRead {
        read_value: discovery_result,
        retry_after_ms: hints.delay(),
    }
}

/// Discovers models independently for every configured binding.
///
/// Providers sometimes expose different model catalogs (and even different
/// credentials) on their Responses, Chat Completions, and Messages endpoints.
/// Discovery must therefore never reuse the first successful response for the
/// remaining bindings. A configured non-empty `model_ids` list is a strict
/// allow-list for that binding, except that a single legacy list equal to the
/// prior source catalog, or the native Responses remainder beside an explicit
/// Responses bridge, is recognized as an automatic source-wide route. An empty
/// single binding also uses the catalog returned under that binding's
/// authentication. The function succeeds when at least one binding returns a
/// valid catalog response, even when that catalog is empty; it fails only when
/// every binding request fails or is malformed.
pub async fn discover_source_models_and_protocol_bindings(
    source: &ProviderSource,
    protocol_bindings: &[SourceProtocolBinding],
) -> Result<SourceDiscovery> {
    read_bindings(
        source,
        protocol_bindings,
        &SourceReadHints::default(),
        &ManagementHttpScope::default(),
    )
    .await
}

async fn read_bindings(
    source: &ProviderSource,
    protocol_bindings: &[SourceProtocolBinding],
    hints: &SourceReadHints,
    scope: &ManagementHttpScope,
) -> Result<SourceDiscovery> {
    source.validate()?;
    let bindings = normalize_source_protocol_bindings(
        protocol_bindings.to_vec(),
        source.wire_api,
        &source.models,
    )?;
    // A source-wide route is an automatic catalog binding, not a snapshot
    // allow-list. The native Responses route can retain that role when the
    // remaining Responses models are explicitly assigned to a bridge.
    let automatic_catalog_routes =
        automatic_catalog_routes(protocol_bindings, &bindings, &source.models);
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut discovery = fetch::discover_protocol_bindings_with_client(
        &client,
        &SourceConnector::new(source, &bindings)?,
        &bindings,
        protocol_bindings,
        &automatic_catalog_routes,
        hints,
        scope,
    )
    .await?;
    if bindings.len() == 1 && automatic_catalog_routes.contains(&bindings[0].key()) {
        if let Some(binding) = discovery.protocol_bindings.first_mut() {
            binding.model_ids.clear();
        }
    }
    Ok(discovery)
}

fn automatic_catalog_routes(
    configured_bindings: &[SourceProtocolBinding],
    bindings: &[SourceProtocolBinding],
    source_models: &[String],
) -> BTreeSet<SourceProtocolBindingKey> {
    let mut automatic = BTreeSet::new();
    let Some(binding) = bindings.first() else {
        return automatic;
    };
    if configured_bindings.is_empty()
        || (configured_bindings.len() == 1
            && (configured_bindings[0].model_ids.is_empty()
                || (!source_models.is_empty()
                    && normalized_model_ids(&configured_bindings[0].model_ids)
                        == normalized_model_ids(source_models))))
    {
        automatic.insert(binding.key());
        return automatic;
    }

    let source_models = normalized_model_ids(source_models);
    for binding in bindings
        .iter()
        .filter(|binding| binding.adapter == SourceAdapter::Native)
    {
        let assigned_elsewhere = bindings
            .iter()
            .filter(|candidate| {
                candidate.wire_api == binding.wire_api && candidate.key() != binding.key()
            })
            .flat_map(|candidate| normalized_model_ids(&candidate.model_ids))
            .collect::<HashSet<_>>();
        let expected = source_models
            .difference(&assigned_elsewhere)
            .cloned()
            .collect::<HashSet<_>>();
        if !expected.is_empty() && normalized_model_ids(&binding.model_ids) == expected {
            automatic.insert(binding.key());
        }
    }
    automatic
}

fn normalized_model_ids(models: &[String]) -> HashSet<String> {
    crate::catalog::normalize_model_ids(models.to_vec())
        .into_iter()
        .map(|model| crate::model_id_key(&model))
        .collect()
}

#[cfg(test)]
mod tests;
