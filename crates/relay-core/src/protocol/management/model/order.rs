use super::{AccountSummary, ModelSummary, SourceSummary};
use crate::model_metadata::ModelMetadataCatalog;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Applies the operator's explicit presentation order without dropping a
/// newly discovered upstream model. Unknown or stale saved IDs are ignored;
/// models absent from the saved list follow the shared catalog policy.
pub fn apply_model_display_order(models: &mut [ModelSummary], saved_order: &[String]) {
    apply_model_display_order_with_catalog(models, saved_order, &ModelMetadataCatalog::empty());
}

/// Applies saved presentation order while placing new models through the
/// catalog's provider/family/version ordering. This changes presentation only;
/// routing and eligibility continue to use the live pool evidence.
pub fn apply_model_display_order_with_catalog(
    models: &mut [ModelSummary],
    saved_order: &[String],
    catalog: &ModelMetadataCatalog,
) {
    let order = catalog.merge_display_order(models.iter().map(|model| &model.id), saved_order);
    let positions = order
        .iter()
        .enumerate()
        .map(|(position, model)| (crate::model_id_key(model), position))
        .collect::<BTreeMap<_, _>>();
    models.sort_by_key(|model| {
        positions
            .get(&crate::model_id_key(&model.id))
            .copied()
            .unwrap_or(usize::MAX)
    });
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogIdentity {
    pub catalog_provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_order: Option<usize>,
}

/// Resolve IDs present in member inventory or exact saved rules/prices.
/// Wildcard rules are not models. Keep this independent from the pool's
/// filtered operational model summaries.
pub fn member_model_catalog(
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
    saved_order: &[String],
    catalog: &ModelMetadataCatalog,
) -> BTreeMap<String, ModelCatalogIdentity> {
    let source_models = sources.iter().flat_map(|source| {
        source
            .models
            .iter()
            .chain(
                source
                    .protocol_bindings
                    .iter()
                    .flat_map(|binding| &binding.model_ids),
            )
            .chain(
                source
                    .allowed_models
                    .iter()
                    .chain(&source.excluded_models)
                    .filter(|model| !model.contains('*')),
            )
            .chain(source.model_price_overrides.keys())
            .chain(source.detected_model_prices.keys())
    });
    let account_models = accounts.iter().flat_map(|account| {
        account.models.iter().chain(
            account
                .allowed_models
                .iter()
                .chain(&account.excluded_models)
                .filter(|model| !model.contains('*')),
        )
    });
    catalog
        .merge_display_order(source_models.chain(account_models), saved_order)
        .into_iter()
        .enumerate()
        .map(|(position, model_id)| {
            let identity = catalog.display_identity_for(&model_id);
            (
                crate::model_id_key(&model_id),
                ModelCatalogIdentity {
                    catalog_provider: identity.provider.unwrap_or_else(|| "other".into()),
                    catalog_family: (!identity.family.is_empty()).then_some(identity.family),
                    catalog_order: Some(position),
                },
            )
        })
        .collect()
}

/// Order complete member inventories for the editors, including excluded models
/// and members outside the pool. Do not use the filtered public catalog here.
/// This changes only snapshot presentation, never discovery or routing rules.
pub fn apply_member_model_display_order(
    sources: &mut [SourceSummary],
    accounts: &mut [AccountSummary],
    saved_order: &[String],
    catalog: &ModelMetadataCatalog,
) {
    for source in sources {
        source.models = catalog.merge_display_order(
            source.models.iter().chain(
                source
                    .protocol_bindings
                    .iter()
                    .flat_map(|binding| &binding.model_ids),
            ),
            saved_order,
        );
    }
    for account in accounts {
        account.models = catalog.merge_display_order(account.models.iter(), saved_order);
    }
}

pub fn apply_model_metadata(models: &mut [ModelSummary], catalog: &ModelMetadataCatalog) {
    for model in models {
        // Snapshots can be refreshed in place by callers. Clear the complete
        // presentation projection before applying the new catalog so a model
        // removed from metadata cannot retain fields from an older snapshot.
        let metadata = catalog.resolve(&model.id);
        model.codex_display_name = catalog.codex_display_name(&model.id);
        model.catalog_provider = catalog.display_provider_for(&model.id);
        model.catalog_source_model_id = metadata.map(|metadata| metadata.source_model_id.clone());
        model.catalog_canonical_model_id =
            metadata.and_then(|metadata| metadata.canonical_model_id.clone());
        model.catalog_family = metadata.and_then(|metadata| metadata.family.clone());
        model.catalog_name = metadata.and_then(|metadata| metadata.name.clone());
        model.catalog_release_date = metadata.and_then(|metadata| metadata.release_date.clone());
        model.catalog_last_updated = metadata.and_then(|metadata| metadata.last_updated.clone());
        model.catalog_status = metadata.and_then(|metadata| metadata.status.clone());
        let capabilities = catalog.capabilities_for(&model.id);
        model.catalog_capabilities = Some(capabilities.clone());
        model.catalog_reasoning = capabilities.reasoning;
        model.catalog_reasoning_method = capabilities.reasoning_method;
        model.catalog_reasoning_effort_levels = capabilities.reasoning_effort_levels;
        model.catalog_default_reasoning_effort = capabilities.default_reasoning_effort;
        model.catalog_reasoning_budget_min_tokens = capabilities.reasoning_budget_min_tokens;
        model.catalog_reasoning_budget_max_tokens = capabilities.reasoning_budget_max_tokens;
        model.catalog_reasoning_budget_default_tokens =
            capabilities.reasoning_budget_default_tokens;
        model.catalog_tool_call = capabilities.tool_call;
        model.catalog_structured_output = capabilities.structured_output;
        model.catalog_attachment = capabilities.attachment;
        model.catalog_open_weights = capabilities.open_weights;
        model.catalog_input_modalities = capabilities.input_modalities;
        model.catalog_output_modalities = capabilities.output_modalities;
        model.catalog_context_limit = capabilities.context_limit;
        model.catalog_input_limit = capabilities.input_limit;
        model.catalog_output_limit = capabilities.output_limit;
    }
}
