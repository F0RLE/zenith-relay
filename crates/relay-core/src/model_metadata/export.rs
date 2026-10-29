use super::{
    MetadataSourceStatus, ModelDisplayIdentity, ModelMetadata, ModelMetadataCatalog,
    ModelMetadataError, model_order_revision,
};
use crate::catalog_io::{self, CatalogIoError};
use serde::Serialize;
use std::{collections::BTreeMap, path::Path};

pub const MODEL_CATALOG_SCHEMA: &str =
    include_str!("../../../../catalog/model-catalog.schema.json");

/// Public reference data only. Participant inventory, routes, saved display
/// overrides, prices, credentials, and usage are not inputs to this export.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicModelCatalog {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub schema_version: u32,
    pub order_revision: String,
    pub metadata_revision: Option<String>,
    pub fetched_at_ms: Option<u64>,
    pub stale: bool,
    pub sources: BTreeMap<String, MetadataSourceStatus>,
    pub models: Vec<PublicModelCatalogEntry>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicModelCatalogEntry {
    pub id: String,
    pub position: usize,
    pub identity: ModelDisplayIdentity,
    pub metadata: ModelMetadata,
}

impl ModelMetadataCatalog {
    pub fn export_reference_catalog(&self) -> PublicModelCatalog {
        let models = self
            .order_model_ids(self.entries.keys())
            .into_iter()
            .enumerate()
            .map(|(position, id)| PublicModelCatalogEntry {
                identity: self.display_identity_for(&id),
                metadata: self.entries[&id].clone(),
                id,
                position,
            })
            .collect();
        PublicModelCatalog {
            schema: "model-catalog.schema.json".into(),
            schema_version: 1,
            order_revision: model_order_revision().into(),
            metadata_revision: self.revision.clone(),
            fetched_at_ms: self.fetched_at_ms,
            stale: self.stale,
            sources: self.sources.clone(),
            models,
        }
    }

    /// Replace the generated file atomically. A missing public reference is
    /// an error, rather than a successful empty catalog.
    pub fn write_reference_catalog(&self, path: &Path) -> Result<bool, ModelMetadataError> {
        if self.entries.is_empty() {
            return Err(ModelMetadataError::InvalidCatalog);
        }
        let exported = serde_json::to_value(self.export_reference_catalog())
            .map_err(|_| ModelMetadataError::InvalidCatalog)?;
        let schema = serde_json::from_str(MODEL_CATALOG_SCHEMA)
            .map_err(|_| ModelMetadataError::InvalidCatalog)?;
        jsonschema::options()
            .offline()
            .build(&schema)
            .map_err(|_| ModelMetadataError::InvalidCatalog)?
            .validate(&exported)
            .map_err(|_| ModelMetadataError::InvalidCatalog)?;
        catalog_io::write_json_if_changed(path, &exported, 64 * 1024 * 1024).map_err(|error| {
            match error {
                CatalogIoError::TooLarge => ModelMetadataError::CacheTooLarge,
                CatalogIoError::InvalidJson => ModelMetadataError::InvalidCatalog,
                _ => ModelMetadataError::Io,
            }
        })
    }
}
