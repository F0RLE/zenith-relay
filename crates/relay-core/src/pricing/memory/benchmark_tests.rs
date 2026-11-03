use super::*;
use crate::{
    catalog_io, model_metadata::ModelMetadataCatalogLoader, pricing::PricingCatalogLoader,
};

fn reference_catalog_directory() -> std::path::PathBuf {
    // Explicit same-user selection for the ignored read-only public-catalog
    // probe, not a directory supplied by an application request or provider.
    std::path::PathBuf::from(
        std::env::var_os("RELAY_MEMORY_CATALOG_DIR")
            .expect("explicit public catalog directory required"),
    )
}

#[test]
#[ignore = "read-only local public catalog probe; requires RELAY_MEMORY_CATALOG_DIR"]
fn profile_local_catalog_load() {
    let root = reference_catalog_directory();
    let started = std::time::Instant::now();
    let pricing = PricingCatalogLoader::open(root.join("litellm-prices.json")).unwrap();
    let models = ModelMetadataCatalogLoader::open(root.join("models-dev.json")).unwrap();
    let pricing_snapshot = pricing.snapshot();
    let model_snapshot = models.snapshot();
    assert!(!pricing_snapshot.entries.is_empty());
    assert!(models.source_memory_diagnostics().unwrap().0 > 0);
    eprintln!(
        "{}",
        serde_json::json!({
            "loadMillis": started.elapsed().as_millis(),
            "pricingEntries": pricing_snapshot.entries.len(),
            "pricingHash": catalog_io::json_fingerprint(&pricing_snapshot.entries).unwrap().0,
            "modelMergedRevision": model_snapshot.revision,
            "priceRecordBytes": size_of::<CatalogEntry>(),
            "pricingRegions": pricing_snapshot.memory_diagnostics(),
            "pricingSourceJsonBytes": pricing.source_memory_diagnostics().unwrap().1,
            "modelRegions": model_snapshot.memory_diagnostics(),
        })
    );
    // Leave a stable retained-data sampling window; no refresh/network,
    // writes, accounts, credentials or history are touched by this probe.
    std::thread::sleep(std::time::Duration::from_secs(1));
}
