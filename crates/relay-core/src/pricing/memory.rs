use super::{CatalogEntry, PricingCatalog};
use crate::gateway::CacheMemoryDiagnostics;
use std::mem::size_of;

impl PricingCatalog {
    /// Retained payload only: excludes BTree node slack, allocator headers and
    /// temporary source trees. Shared snapshot handles do not duplicate it.
    pub fn memory_diagnostics(&self) -> Vec<CacheMemoryDiagnostics> {
        let mut prices = 0;
        for (key, entry) in self.entries.iter() {
            prices += size_of::<(String, CatalogEntry)>()
                + key.capacity()
                + entry.model_id.capacity()
                + entry.provider.as_ref().map_or(0, String::capacity);
        }
        let (mut index_entries, mut indices) = self.lookup.memory_usage();
        for (alias, key) in self.unique.iter() {
            indices += size_of::<(String, String)>() + alias.capacity() + key.capacity();
        }
        for key in self.conflicts.iter() {
            indices += size_of::<String>() + key.capacity();
        }
        index_entries += self.unique.len() + self.conflicts.len();
        [
            ("pricing_entries", self.entries.len(), prices),
            ("pricing_indices", index_entries, indices),
        ]
        .into_iter()
        .map(|(key, entries, bytes)| CacheMemoryDiagnostics {
            key,
            entries: Some(entries),
            bytes: Some(bytes),
            limit_bytes: None,
            basis: "retained_payload",
        })
        .collect()
    }
}

#[cfg(test)]
mod benchmark_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_payload_is_shared_and_diagnostics_never_expose_labels() {
        let catalog = PricingCatalog::from_litellm_json(
            r#"{"synthetic-private-label":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002}}"#,
        )
        .unwrap();
        let copy = catalog.clone();
        assert!(std::sync::Arc::ptr_eq(&catalog.entries, &copy.entries));
        assert!(std::sync::Arc::ptr_eq(&catalog.unique, &copy.unique));
        assert!(std::sync::Arc::ptr_eq(&catalog.lookup, &copy.lookup));
        assert!(std::sync::Arc::ptr_eq(&catalog.conflicts, &copy.conflicts));
        let measured = catalog.memory_diagnostics();
        assert_eq!(measured[0].entries, Some(1));
        let (key, entry) = catalog.entries.first_key_value().unwrap();
        assert_eq!(
            measured[0].bytes,
            Some(
                size_of::<(String, CatalogEntry)>()
                    + key.capacity()
                    + entry.model_id.capacity()
                    + entry.provider.as_ref().map_or(0, String::capacity)
            )
        );
        assert!(measured.iter().all(|region| region.bytes.unwrap() > 0));
        assert!(
            !serde_json::to_string(&measured)
                .unwrap()
                .contains("synthetic-private-label")
        );
    }
}
