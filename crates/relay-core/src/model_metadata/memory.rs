//! Retained catalog payload accounting, not an allocator or process heap profile.
//! Container node slack, allocator headers and temporary refresh trees are excluded.
use super::*;
use std::{collections::HashSet, mem::size_of};

#[derive(Default)]
struct PayloadBytes {
    bytes: usize,
    shared: HashSet<*const u8>,
}

impl PayloadBytes {
    fn string(&mut self, value: &str) {
        self.bytes += value.len();
    }

    fn owned(&mut self, value: &String) {
        self.bytes += value.capacity();
    }

    #[expect(
        clippy::ptr_arg,
        reason = "Heap accounting needs owned capacity; static borrowed labels allocate nothing"
    )]
    fn label(&mut self, value: &Cow<'static, str>) {
        self.bytes += match value {
            Cow::Owned(value) => value.capacity(),
            Cow::Borrowed(_) => 0,
        };
    }

    fn optional(&mut self, value: &Option<String>) {
        if let Some(value) = value {
            self.owned(value);
        }
    }

    fn strings(&mut self, values: &Vec<String>) {
        self.bytes += values.capacity() * size_of::<String>();
        for value in values {
            self.owned(value);
        }
    }

    fn revision(&mut self, value: &Option<Arc<str>>) {
        if let Some(value) = value {
            if self.shared.insert(value.as_ptr()) {
                self.string(value);
            }
        }
    }

    fn json(&mut self, value: &Value) {
        match value {
            Value::String(value) => self.owned(value),
            Value::Array(values) => {
                self.bytes += values.capacity() * size_of::<Value>();
                for value in values {
                    self.json(value);
                }
            }
            Value::Object(values) => {
                self.bytes += values.len() * size_of::<(String, Value)>();
                for (key, value) in values {
                    self.owned(key);
                    self.json(value);
                }
            }
            _ => {}
        }
    }
}

impl ModelMetadataCatalog {
    /// Snapshot entries/indices are Arc-owned. Count each region once, not once
    /// per handle; provenance is separate from model payloads to avoid overlap.
    pub fn memory_diagnostics(&self) -> Vec<crate::gateway::CacheMemoryDiagnostics> {
        let mut models = PayloadBytes::default();
        let mut provenance = PayloadBytes::default();
        let mut fields = 0;
        for (key, metadata) in self.entries.iter() {
            models.bytes += size_of::<(String, ModelMetadata)>();
            models.owned(key);
            models.owned(&metadata.source_model_id);
            models.owned(&metadata.provider);
            models.optional(&metadata.canonical_model_id);
            models.strings(&metadata.aliases);
            models.bytes += metadata.protocol_preferences.capacity() * size_of::<crate::WireApi>();
            for value in [
                &metadata.family,
                &metadata.name,
                &metadata.release_date,
                &metadata.last_updated,
                &metadata.status,
                &metadata.knowledge,
            ] {
                models.optional(value);
            }
            let caps = &metadata.capabilities;
            models.optional(&caps.reasoning_source);
            models.optional(&caps.default_reasoning_effort);
            for values in [
                &caps.reasoning_effort_levels,
                &caps.reasoning_modes,
                &caps.input_modalities,
                &caps.output_modalities,
            ] {
                models.strings(values);
            }
            models.bytes += caps.service_tiers.capacity() * size_of::<crate::DefaultServiceTier>();
            fields += metadata.field_provenance.len();
            for (field, origin) in &metadata.field_provenance {
                provenance.bytes += size_of::<(Cow<'static, str>, MetadataFieldProvenance)>();
                provenance.label(field);
                provenance.label(&origin.source);
                provenance.revision(&origin.revision);
                provenance.bytes +=
                    origin.conflicts.capacity() * size_of::<MetadataFieldConflict>();
                for conflict in &origin.conflicts {
                    provenance.label(&conflict.source);
                    provenance.revision(&conflict.revision);
                    provenance.json(&conflict.value);
                }
            }
        }
        let mut indices = PayloadBytes::default();
        for (key, value) in self.leaf_matches.iter() {
            indices.bytes += size_of::<(String, String)>();
            indices.owned(key);
            indices.owned(value);
        }
        for key in self.ambiguous_leaves.iter() {
            indices.bytes += size_of::<String>();
            indices.owned(key);
        }
        for index in [&self.alias_matches, &self.family_providers] {
            for (key, value) in index.iter() {
                indices.bytes += size_of::<(String, Option<String>)>();
                indices.owned(key);
                indices.optional(value);
            }
        }
        [
            ("model_metadata_models", self.entries.len(), models.bytes),
            ("model_metadata_provenance", fields, provenance.bytes),
            (
                "model_metadata_indices",
                self.leaf_matches.len()
                    + self.ambiguous_leaves.len()
                    + self.alias_matches.len()
                    + self.family_providers.len(),
                indices.bytes,
            ),
        ]
        .into_iter()
        .map(
            |(key, entries, bytes)| crate::gateway::CacheMemoryDiagnostics {
                key,
                entries: Some(entries),
                bytes: Some(bytes),
                limit_bytes: None,
                basis: "retained_payload",
            },
        )
        .collect()
    }
}

#[cfg(test)]
mod benchmark_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_diagnostics_count_owned_label_capacity_not_static_labels() {
        let mut measured = PayloadBytes::default();
        measured.label(&Cow::Borrowed("models_dev"));
        assert_eq!(measured.bytes, 0);
        let mut future_source = String::with_capacity(128);
        future_source.push_str("future-source");
        let capacity = future_source.capacity();
        measured.label(&Cow::Owned(future_source));
        assert_eq!(measured.bytes, capacity);
    }

    #[test]
    fn memory_diagnostics_count_shared_revisions_once_and_keep_contents_private() {
        let mut catalog = ModelMetadataCatalog::from_models_dev_json(
            r#"{"synthetic":{"models":{"example":{"name":"private-model-label"}}}}"#,
        )
        .unwrap();
        catalog.sources.insert(
            "synthetic".into(),
            MetadataSourceStatus {
                revision: Some("revision".repeat(8)),
                fetched_at_ms: None,
                stale: false,
            },
        );
        let metadata = Arc::make_mut(&mut catalog.entries)
            .values_mut()
            .next()
            .unwrap();
        for field in ["name", "family"] {
            metadata.field_provenance.insert(
                field.into(),
                MetadataFieldProvenance {
                    source: "synthetic".into(),
                    revision: None,
                    conflicts: vec![],
                },
            );
        }
        catalog.bind_field_revisions();
        let metadata = catalog.entries.values().next().unwrap();
        let a = metadata.field_provenance["name"].revision.as_ref().unwrap();
        let b = metadata.field_provenance["family"]
            .revision
            .as_ref()
            .unwrap();
        assert!(Arc::ptr_eq(a, b));
        assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::json!("revision".repeat(8))
        );
        let mut measured = PayloadBytes::default();
        measured.revision(&Some(Arc::clone(a)));
        measured.revision(&Some(Arc::clone(b)));
        assert_eq!(measured.bytes, a.len());
        let snapshot = catalog.memory_diagnostics();
        assert_eq!(snapshot.len(), 3);
        assert!(snapshot.iter().all(|region| region.bytes.unwrap() > 0));
        assert!(!serde_json::to_string(&snapshot)
            .unwrap()
            .contains("private-model-label"));
    }
}
