use super::*;

#[test]
#[ignore = "read-only local reference catalog accounting; requires RELAY_MEMORY_CATALOG_FILE"]
fn profile_local_catalog_payloads() {
    let path = std::env::var_os("RELAY_MEMORY_CATALOG_FILE")
        .expect("explicit public reference catalog path required");
    assert!(std::path::Path::new(&path).is_file());
    // open() only reads the reference cache; refresh/network and writes are
    // deliberately not invoked. Never read accounts, keys or conversations.
    let loader = ModelMetadataCatalogLoader::open(std::path::PathBuf::from(path)).unwrap();
    let catalog = loader.snapshot();
    assert!(!catalog.entries.is_empty());
    let mut shared = PayloadBytes::default();
    let mut repeated_bytes = 0;
    for metadata in catalog.entries.values() {
        for origin in metadata.field_provenance.values() {
            for revision in std::iter::once(&origin.revision)
                .chain(origin.conflicts.iter().map(|conflict| &conflict.revision))
            {
                repeated_bytes += revision.as_ref().map_or(0, |value| value.len());
                shared.revision(revision);
            }
        }
    }
    eprintln!(
        "{}",
        serde_json::json!({
            "regions": catalog.memory_diagnostics(),
            "normalizedCatalogHash": crate::catalog_io::json_fingerprint(&catalog.entries).unwrap().0,
            "sourceJsonBytes": loader.source_memory_diagnostics().map(|value| value.1),
            "duplicateRevisionPayloadBytesRemoved": repeated_bytes.saturating_sub(shared.bytes),
            "sharedRevisionPayloadBytes": shared.bytes,
        })
    );
}
